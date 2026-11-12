//! Glue layer for `cabin install` and `cabin uninstall`.
//!
//! `cabin install` builds one package's `executable` targets with the
//! pipeline `cabin build` runs - the same planner, the same confinement
//! of build scripts - in a temporary build directory of its own, and
//! hands the built files to `cabin-install`, which owns what an install
//! root holds and how it changes.  Two things differ from a build: only
//! the user's configuration files are read, and the lockfile is read
//! but never written.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use clap_complete::Shell;

use cabin_build::ManifestTargetSelector;
use cabin_core::TargetKind;
use cabin_install::{Dependency, Root, Source, Toolchain};
use cabin_workspace::PackageKind;

use crate::cli::build_prep::{
    DevActivation, PreparedWorkspace, WorkspacePipelineArgs, build_prepared, prepare_workspace,
};
use crate::cli::term_verbosity::Reporter;
use crate::cli::{ToolchainSelectionArgs, WorkspaceSelectionArgs};

#[derive(Debug, Args)]
pub(crate) struct InstallArgs {
    /// Install the package in this directory.
    #[arg(long, value_name = "DIR", required_unless_present = "list")]
    pub path: Option<PathBuf>,

    /// Install only the named `executable` target.  Repeat the flag
    /// to install several.
    #[arg(long = "bin", value_name = "NAME")]
    pub bin: Vec<String>,

    /// Install into `<DIR>/bin` instead of `.cabin/bin` in the home
    /// directory.
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,

    /// List the installed packages and their executables.
    #[arg(long, conflicts_with = "path")]
    pub list: bool,

    /// Build profile (`dev`, `release`, or any custom profile
    /// declared in `[profile.<name>]`).  Defaults to `release`.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Enable named features.
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Enable every declared feature.
    #[arg(long)]
    pub all_features: bool,

    /// Disable default features.
    #[arg(long)]
    pub no_default_features: bool,

    /// Take every dependency version from `cabin.lock`, and fail when
    /// it cannot supply one.
    #[arg(long)]
    pub locked: bool,

    /// Forbid network access.
    #[arg(long)]
    pub offline: bool,

    /// Replace files in the way: another package's executables, and
    /// files Cabin did not install.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct UninstallArgs {
    /// The installed package to remove.
    #[arg(value_name = "PACKAGE")]
    pub package: String,

    /// Remove only the named executable.  Repeat the flag to remove
    /// several.
    #[arg(long = "bin", value_name = "NAME")]
    pub bin: Vec<String>,

    /// Uninstall from `<DIR>/bin` instead of `.cabin/bin` in the home
    /// directory.
    #[arg(long, value_name = "DIR")]
    pub root: Option<PathBuf>,
}

pub(crate) fn install(
    args: &InstallArgs,
    reporter: Reporter,
    color: cabin_core::ColorChoice,
) -> Result<()> {
    let root = root(args.root.as_deref())?;
    if args.list {
        return list(&root);
    }
    let dir = args.path.as_deref().expect("clap requires --path");
    // Canonical, so the walk to a workspace root visits the
    // directory's real ancestors.
    let dir = cabin_fs::canonicalize(dir)
        .with_context(|| format!("cannot find the package directory {}", dir.display()))?;
    let package_manifest = dir.join("cabin.toml");
    let Some(package) = cabin_manifest::load_manifest(&package_manifest)?.package else {
        bail!(
            "{} declares no package to install",
            package_manifest.display()
        );
    };
    // A workspace member is built as its workspace builds it: with
    // what it inherits, and the workspace's lockfile.
    let manifest_path = cabin_workspace::discover_workspace_root(&dir)?
        .map_or(package_manifest, |root| root.manifest_path);
    // Dropped, and so deleted, after everything that holds a file in it.
    let build_dir = tempfile::Builder::new()
        .prefix("cabin-install-")
        .tempdir()
        .context("failed to create a temporary build directory")?;
    let prepared = prepare_workspace(
        &WorkspacePipelineArgs {
            manifest_path: Some(&manifest_path),
            offline: args.offline,
            cache_dir: None,
            build_dir: Some(build_dir.path()),
            locked: args.locked,
            frozen: false,
            no_patches: false,
            features: &args.features,
            all_features: args.all_features,
            no_default_features: args.no_default_features,
            index_path: None,
            index_url: None,
            profile: Some(args.profile.as_deref().unwrap_or("release")),
            release: false,
            workspace_selection: &WorkspaceSelectionArgs {
                package: vec![package.name.as_str().to_owned()],
                ..WorkspaceSelectionArgs::default()
            },
            toolchain: &ToolchainSelectionArgs::default(),
            dev: DevActivation::Disabled,
            build_context: true,
            install: true,
        },
        reporter,
    )?;
    let index = prepared.resolved_selection.packages[0];
    let package = &prepared.graph.packages[index];
    let name = package.package.name.as_str();
    let targets = executables(&prepared, index, &args.bin)?;
    let jobs = crate::cli::config::resolve_build_jobs(None, &prepared.effective_config)?;
    let (plan_graph, built) = build_prepared(
        &prepared,
        Some(
            targets
                .iter()
                .map(|target| ManifestTargetSelector {
                    package: Some(name.to_owned()),
                    name: target.clone(),
                })
                .collect(),
        ),
        false,
        color,
        jobs,
        reporter,
    )?;
    let dependencies = dependencies(&prepared, &plan_graph, index)?;
    report_resolution(&dependencies, &manifest_path, reporter);
    let mut files = Vec::new();
    for target in targets {
        let output = plan_graph
            .target_outputs
            .get(&(name.to_owned(), target.clone()))
            .with_context(|| format!("the build produced no executable for `{name}:{target}`"))?;
        let file_name = output.file_name().expect("an executable has a file name");
        files.push((file_name.to_owned(), output.as_std_path().to_owned()));
    }

    let identity = &prepared.detection_report.lead().identity;
    let record = cabin_install::Package {
        name: name.to_owned(),
        version: package.package.version.to_string(),
        source: path_source(&package.manifest_dir)?,
        features: prepared
            .enabled_features
            .get(&index)
            .map(|features| features.iter().cloned().collect())
            .unwrap_or_default(),
        profile: prepared.profile.name.as_str().to_owned(),
        toolchain: Toolchain {
            compiler: identity.kind.as_key().to_owned(),
            version: identity.version.as_ref().map(|version| version.raw.clone()),
        },
        dependencies,
        binaries: Vec::new(),
    };
    let shown = format!(
        "{name} v{} ({})",
        record.version,
        package.manifest_dir.display()
    );
    let done = match root.install(record, &files, args.force, wait_message(reporter, &root)) {
        Err(err @ cabin_install::Error::Collision { .. }) => {
            bail!("{err}\npass `--force` to replace what is in the way")
        }
        done => done?,
    };
    drop(built);

    let bin = root.bin();
    for (verb, names) in [
        ("Installing", &done.added),
        ("Replacing", &done.replaced),
        ("Removing", &done.removed),
    ] {
        for file in names {
            reporter.status(verb, format_args!("{}", bin.join(file).display()));
        }
    }
    for file in &done.left {
        reporter.warning(format_args!(
            "{} changed since Cabin installed it, so it stays; Cabin no longer tracks it",
            bin.join(file).display()
        ));
    }
    reporter.status("Installed", format_args!("{shown}"));
    if !on_path(&bin) {
        reporter.warning(format_args!("{}", path_hint(&bin)));
    }
    Ok(())
}

pub(crate) fn uninstall(args: &UninstallArgs, reporter: Reporter) -> Result<()> {
    let root = root(args.root.as_deref())?;
    // `--bin` names a target, as for `cabin install`: its file has the
    // suffix an executable has here.
    let bins: Vec<String> = args
        .bin
        .iter()
        .map(|name| format!("{name}{}", std::env::consts::EXE_SUFFIX))
        .collect();
    let removed = match root.uninstall(&args.package, &bins, wait_message(reporter, &root)) {
        Err(err @ cabin_install::Error::Modified(_)) => {
            bail!("{err}\nnothing was removed: delete or restore it, then run this again")
        }
        removed => removed?,
    };
    let bin = root.bin();
    for file in removed {
        reporter.status("Removing", format_args!("{}", bin.join(file).display()));
    }
    Ok(())
}

fn list(root: &Root) -> Result<()> {
    for package in root.packages()? {
        let source = match &package.source {
            Source::Path(path) => path.as_str(),
            Source::Registry { checksum, .. } => checksum,
            Source::System => "system",
        };
        // The record is a file: nothing in it reaches the terminal raw.
        let header = format!("{} v{} ({source}):", package.name, package.version);
        println!("{}", cabin_core::escape_control_chars(&header));
        for binary in &package.binaries {
            println!("    {}", cabin_core::escape_control_chars(&binary.name));
        }
    }
    Ok(())
}

/// `--root`, else `.cabin` in the user's home directory.
fn root(flag: Option<&Path>) -> Result<Root> {
    let dir = match flag {
        Some(dir) => crate::cli::absolutise(dir)?,
        None => std::env::home_dir()
            .context("no home directory to install into: pass --root")?
            .join(".cabin"),
    };
    Ok(Root::new(dir))
}

fn wait_message(reporter: Reporter, root: &Root) -> impl FnOnce() {
    let bin = root.bin();
    move || {
        reporter.status(
            "Blocking",
            format_args!(
                "waiting for another command to finish with {}",
                bin.display()
            ),
        );
    }
}

/// The targets to install: those `--bin` names, else every
/// `executable` of the package its enabled features allow.
fn executables(prepared: &PreparedWorkspace, index: usize, bins: &[String]) -> Result<Vec<String>> {
    let package = &prepared.graph.packages[index].package;
    let name = package.name.as_str();
    if !bins.is_empty() {
        for bin in bins {
            match package.targets.iter().find(|t| t.name.as_str() == bin) {
                Some(target) if target.kind == TargetKind::Executable => {}
                Some(target) => bail!(
                    "--bin `{bin}` is a `{}` target of package `{name}`; only an `executable` installs",
                    target.kind.as_str()
                ),
                None => bail!("--bin `{bin}` was not found in package `{name}`"),
            }
        }
        let mut bins = bins.to_vec();
        bins.sort();
        bins.dedup();
        return Ok(bins);
    }
    let none = std::collections::BTreeSet::new();
    let enabled = prepared.enabled_features.get(&index).unwrap_or(&none);
    let (ready, gated): (Vec<_>, Vec<_>) = package
        .targets
        .iter()
        .filter(|target| target.kind == TargetKind::Executable)
        .partition(|target| target.missing_required_features(enabled).is_empty());
    // With every executable behind a feature, the planner names the
    // features to enable.
    let chosen = if ready.is_empty() { gated } else { ready };
    if chosen.is_empty() {
        bail!("package `{name}` has no `executable` target to install");
    }
    Ok(chosen
        .into_iter()
        .map(|target| target.name.as_str().to_owned())
        .collect())
}

/// Say where the versions of the registry dependencies came from.
fn report_resolution(dependencies: &[Dependency], manifest_path: &Path, reporter: Reporter) {
    let locked: Vec<bool> = dependencies
        .iter()
        .filter_map(|dependency| match dependency.source {
            Source::Registry { locked, .. } => Some(locked),
            Source::Path(_) | Source::System => None,
        })
        .collect();
    if locked.is_empty() {
        return;
    }
    let lockfile = crate::cli::lockfile_path_for(manifest_path);
    if lockfile.is_file() {
        reporter.status(
            "Resolved",
            format_args!(
                "registry dependencies: {}, {} at the versions {} names",
                locked.len(),
                locked.iter().filter(|locked| **locked).count(),
                lockfile.display()
            ),
        );
    } else {
        reporter.status(
            "Resolved",
            format_args!(
                "registry dependencies: {}, without a lockfile",
                locked.len()
            ),
        );
    }
}

/// Every package but `index` that `plan` has a target of, in either
/// context: a dependency no enabled feature or this platform asks for
/// is not one.
fn dependencies(
    prepared: &PreparedWorkspace,
    plan: &cabin_build::BuildGraph,
    index: usize,
) -> Result<Vec<Dependency>> {
    let mut out = Vec::new();
    for (dependency, package) in prepared.graph.packages.iter().enumerate() {
        let name = package.package.name.as_str().to_owned();
        let planned =
            plan.planned_packages.contains(&name) || plan.build_context_packages.contains(&name);
        if dependency == index || !planned {
            continue;
        }
        let version = package.package.version.to_string();
        let source = match package.kind {
            PackageKind::Local => path_source(&package.manifest_dir)?,
            PackageKind::Registry => {
                match prepared.checksums.get(&(name.clone(), version.clone())) {
                    Some(checksum) => Source::Registry {
                        checksum: checksum.clone(),
                        locked: prepared
                            .lockfile_pinned
                            .contains(&(name.clone(), version.clone())),
                    },
                    None => Source::System,
                }
            }
        };
        out.push(Dependency {
            name,
            version,
            source,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn path_source(dir: &Path) -> Result<Source> {
    let dir = camino::Utf8PathBuf::try_from(dir.to_owned())
        .with_context(|| format!("{} is not valid UTF-8", dir.display()))?;
    Ok(Source::Path(dir))
}

fn on_path(bin: &Path) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir == bin))
}

/// How to put `bin` on `PATH`, for the warning that it is not there.
/// Cabin only says what to add: it never edits a startup file.
fn path_hint(bin: &Path) -> String {
    if cfg!(windows) {
        return windows_path_hint(bin);
    }
    let shell = std::env::var_os("SHELL").and_then(Shell::from_shell_path);
    // The line says `$HOME`, so the home it is under is that variable's
    // and not the account's.
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    unix_path_hint(bin, home.as_deref().map(Path::new), shell)
}

fn windows_path_hint(bin: &Path) -> String {
    let bin = bin.display();
    format!(
        r#"{bin} is not in your PATH.

To run installed programs from anywhere, add it to your user Path
variable: open "Edit environment variables for your account" from
the Start menu, edit Path, and add a new entry:

    {bin}

Then open a new terminal."#
    )
}

/// The line to add to the startup file of `shell`, the user's login
/// shell.  Any other shell, and a directory no line can name, get the
/// instruction without a line rather than a guess.
fn unix_path_hint(bin: &Path, home: Option<&Path>, shell: Option<Shell>) -> String {
    // Under the home directory the line says `$HOME`, as the user
    // would write it.  A home of `/` is over every directory.
    let under_home = home
        .filter(|home| home.parent().is_some())
        .and_then(|home| bin.strip_prefix(home).ok());
    let (shown, prefix, rest) = match under_home {
        Some(rest) => (format!("~/{}", rest.display()), "$HOME/", rest),
        None => (bin.display().to_string(), "", bin),
    };
    let line = bin
        .to_str()
        // `PATH` cannot hold a `:`, and a directory the reporter would
        // escape is not the one the line would then name.
        .filter(|dir| !dir.contains(':') && cabin_core::escape_control_chars(dir) == *dir)
        .and(rest.to_str())
        .zip(shell)
        .and_then(|(dir, shell)| {
            // Inside double quotes, so `$HOME` expands: a backslash
            // goes before each character the shell reads there.
            let quoted = |special: &str| {
                let mut quoted = String::from(prefix);
                for c in dir.chars() {
                    if special.contains(c) {
                        quoted.push('\\');
                    }
                    quoted.push(c);
                }
                quoted
            };
            let export = || format!(r#"export PATH="{}:$PATH""#, quoted(r#"\"$`"#));
            match shell {
                Shell::Zsh => Some(("~/.zshrc", export())),
                // A macOS terminal starts bash as a login shell, which
                // reads the first of these that exists and never
                // `~/.bashrc`.
                Shell::Bash if cfg!(target_os = "macos") => Some((
                    ["~/.bash_profile", "~/.bash_login", "~/.profile"]
                        .into_iter()
                        .find(|file| home.is_some_and(|home| home.join(&file[2..]).exists()))
                        .unwrap_or("~/.bash_profile"),
                    export(),
                )),
                Shell::Bash => Some(("~/.bashrc", export())),
                Shell::Fish => Some((
                    "~/.config/fish/config.fish",
                    format!(r#"fish_add_path "{}""#, quoted(r#"\"$"#)),
                )),
                _ => None,
            }
        });
    match line {
        Some((file, line)) => format!(
            r"{shown} is not in your PATH.

To run installed programs from anywhere, add the following
line to {file}:

    {line}

Then restart your shell."
        ),
        None => format!(
            r"{shown} is not in your PATH.

To run installed programs from anywhere, add that directory to
PATH in your shell's startup file."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix(bin: &str, shell: Option<Shell>) -> String {
        unix_path_hint(Path::new(bin), Some(Path::new("/home/me")), shell)
    }

    #[test]
    fn the_default_root_gets_a_line_that_says_home() {
        assert_eq!(
            unix("/home/me/.cabin/bin", Some(Shell::Zsh)),
            r#"~/.cabin/bin is not in your PATH.

To run installed programs from anywhere, add the following
line to ~/.zshrc:

    export PATH="$HOME/.cabin/bin:$PATH"

Then restart your shell."#
        );
    }

    /// Creating `~/.bash_profile` would stop a login bash reading the
    /// `~/.profile` it reads now.
    #[cfg(target_os = "macos")]
    #[test]
    fn bash_on_macos_gets_the_startup_file_it_already_reads() {
        let home = tempfile::tempdir().unwrap();
        let hint = || {
            unix_path_hint(
                &home.path().join("bin"),
                Some(home.path()),
                Some(Shell::Bash),
            )
        };
        std::fs::write(home.path().join(".profile"), "").unwrap();
        assert!(hint().contains("line to ~/.profile:"), "{}", hint());
        std::fs::write(home.path().join(".bash_profile"), "").unwrap();
        assert!(hint().contains("line to ~/.bash_profile:"), "{}", hint());
    }

    #[test]
    fn each_shell_gets_its_own_file_and_line() {
        let bash = unix("/home/me/.cabin/bin", Some(Shell::Bash));
        let file = if cfg!(target_os = "macos") {
            "line to ~/.bash_profile:"
        } else {
            "line to ~/.bashrc:"
        };
        assert!(bash.contains(file), "{bash}");
        assert!(
            bash.contains(r#"    export PATH="$HOME/.cabin/bin:$PATH""#),
            "{bash}"
        );

        let fish = unix("/home/me/.cabin/bin", Some(Shell::Fish));
        assert!(
            fish.contains("line to ~/.config/fish/config.fish:"),
            "{fish}"
        );
        assert!(
            fish.contains(r#"    fish_add_path "$HOME/.cabin/bin""#),
            "{fish}"
        );
    }

    /// A `--root` outside the home directory is named in full, with
    /// what the shell would read inside the quotes escaped: a backtick
    /// is not special to fish.
    #[test]
    fn a_custom_root_is_quoted_for_the_shell() {
        let bin = r#"/opt/my $tools "a`b\c/bin"#;
        let zsh = unix(bin, Some(Shell::Zsh));
        assert!(
            zsh.starts_with(&format!("{bin} is not in your PATH.")),
            "{zsh}"
        );
        assert!(
            zsh.contains(r#"    export PATH="/opt/my \$tools \"a\`b\\c/bin:$PATH""#),
            "{zsh}"
        );
        let fish = unix(bin, Some(Shell::Fish));
        assert!(
            fish.contains(r#"    fish_add_path "/opt/my \$tools \"a`b\\c/bin""#),
            "{fish}"
        );
    }

    #[test]
    fn an_unknown_shell_or_directory_gets_no_line() {
        let generic = r"/opt/tools/bin is not in your PATH.

To run installed programs from anywhere, add that directory to
PATH in your shell's startup file.";
        assert_eq!(unix("/opt/tools/bin", None), generic);
        let rootless = unix_path_hint(Path::new("/opt/tools/bin"), Some(Path::new("/")), None);
        assert_eq!(rootless, generic);
        assert_eq!(unix("/opt/tools/bin", Some(Shell::Elvish)), generic);
        for bin in ["/opt/a:b/bin", "/opt/a\nb/bin", "/opt/a\u{202e}b/bin"] {
            let hint = unix(bin, Some(Shell::Zsh));
            assert!(!hint.contains("export"), "{hint}");
        }
        // `$HOME` must not hide what the line cannot hold.
        let hint = unix_path_hint(
            Path::new("/home/a:b/.cabin/bin"),
            Some(Path::new("/home/a:b")),
            Some(Shell::Zsh),
        );
        assert!(!hint.contains("export"), "{hint}");
    }

    #[test]
    fn windows_is_told_about_the_user_path_variable() {
        let hint = windows_path_hint(Path::new(r"C:\Users\me\.cabin\bin"));
        assert!(
            hint.starts_with(r"C:\Users\me\.cabin\bin is not in your PATH."),
            "{hint}"
        );
        assert!(hint.contains("your user Path\nvariable"), "{hint}");
        assert!(
            hint.contains("\n    C:\\Users\\me\\.cabin\\bin\n"),
            "{hint}"
        );
        assert!(!hint.contains("export"), "{hint}");
    }
}
