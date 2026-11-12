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
        reporter.warning(format_args!(
            "{} is not on PATH: add it to run the installed programs by name",
            bin.display()
        ));
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
