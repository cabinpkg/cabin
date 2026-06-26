//! Orchestration for `cabin fmt`.
//!
//! Translates the CLI flag bundle into the typed inputs the
//! shared crates accept and routes their outcomes back to the
//! reporter.  Keeping this glue in a dedicated module preserves
//! the package rule that `cabin` stays thin: arg parsing
//! and reporter wiring live here, but no source-discovery
//! algorithms and no `clang-format` command-line construction
//! live in this file.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Args;

use cabin_fmt::{
    FormatMode, FormatReport, FormatRequest, resolve_formatter_executable, run_formatter,
};
use cabin_source_discovery::{SourceDiscoveryRequest, discover_sources};

use crate::cli::source_tooling::{
    absolutize, describe_packages, display_workspace_relative, nested_package_excludes,
    package_selection_from_flags,
};
use crate::cli::term_verbosity::Reporter;
use crate::plural;

/// `cabin fmt` argument bundle.
///
/// Field doc-comments are picked up by clap and rendered in
/// `cabin fmt --help`; keep them user-focused.
#[derive(Debug, Args)]
pub(crate) struct FmtArgs {
    /// Path to the cabin.toml manifest.  Same precedence rules
    /// as `cabin build`: when omitted, Cabin walks upward from
    /// the current directory to find the nearest manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Build output directory to exclude from source discovery.
    /// Same precedence rules as `cabin build`: `--build-dir` >
    /// `CABIN_BUILD_DIR` > `[paths] build-dir` config setting >
    /// built-in default `build`.
    #[arg(long, value_name = "PATH")]
    pub build_dir: Option<PathBuf>,

    /// Verify formatting without rewriting any file.  Exits
    /// non-zero when at least one file would be reformatted.
    #[arg(long)]
    pub check: bool,

    /// Exclude one file or directory from formatting.  May be
    /// repeated.  Paths are resolved against the current
    /// working directory.
    #[arg(long, value_name = "PATH")]
    pub exclude: Vec<PathBuf>,

    /// Disable VCS ignore handling so files that are normally
    /// hidden by `.gitignore` are also formatted.  Cabin's
    /// built-in build / cache / vendor exclusions still apply.
    #[arg(long)]
    pub no_ignore_vcs: bool,

    /// Format every workspace member.  Cannot be combined with
    /// `--package` or `--default-members`.
    #[arg(long, conflicts_with_all = &["package", "default_members"])]
    pub workspace: bool,

    /// Format the named workspace package.  Repeat the flag to
    /// select multiple packages.  Errors when a name is not a
    /// workspace member.
    #[arg(long = "package", short = 'p', value_name = "PACKAGE")]
    pub package: Vec<String>,

    /// Format `[workspace.default-members]`.  Errors when the
    /// workspace declares no default-members.
    #[arg(long, conflicts_with_all = &["workspace", "package"])]
    pub default_members: bool,
}

/// Entry point invoked by the top-level dispatcher.
pub(crate) fn fmt(args: &FmtArgs, reporter: Reporter) -> Result<ExitCode> {
    let manifest_path = crate::cli::resolve_invocation_manifest(args.manifest_path.as_deref())?;
    // `cabin fmt` rewrites local source files: it never reads
    // foundation-port contents and never reaches the network.
    // Skipping port edges lets a fresh checkout (or any CI lint
    // job) format without first downloading an uncached port.
    let graph = cabin_workspace::load_workspace_skip_ports(&manifest_path)?;
    let effective_config = crate::cli::config::load_effective_config(&graph)?;

    let workspace_selection =
        package_selection_from_flags(args.workspace, &args.package, args.default_members);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;

    // Effective build directory, honoring `--build-dir` /
    // `CABIN_BUILD_DIR` / `[paths] build-dir`.  We want the
    // walker to exclude exactly the directory `cabin build`
    // would have written into.
    let (build_dir_input, _) = crate::cli::config::resolve_build_dir_with_env(
        args.build_dir.as_deref(),
        &effective_config,
    );
    let build_dir = absolutize(&graph.root_dir, &build_dir_input);

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let absolute_excludes: Vec<PathBuf> =
        args.exclude.iter().map(|p| absolutize(&cwd, p)).collect();

    let selected_indices: BTreeSet<usize> = resolved_selection.packages.iter().copied().collect();
    let roots: Vec<PathBuf> = resolved_selection
        .packages
        .iter()
        .map(|&idx| graph.packages[idx].manifest_dir.clone())
        .collect();
    let nested_excludes = nested_package_excludes(&graph, &selected_indices);

    let mut excluded_directories: Vec<PathBuf> = nested_excludes;
    excluded_directories.push(build_dir);

    let request = SourceDiscoveryRequest {
        roots,
        excluded_paths: absolute_excludes,
        excluded_directories,
        respect_vcs_ignore: !args.no_ignore_vcs,
    };
    let discovered = discover_sources(&request)
        .map_err(|err| anyhow::anyhow!("source discovery failed: {err}"))?;
    let files: Vec<PathBuf> = discovered.into_iter().map(|f| f.absolute_path).collect();

    let executable = resolve_formatter_executable(|key| std::env::var_os(key));
    let mode = if args.check {
        FormatMode::Check
    } else {
        FormatMode::Write
    };

    let mut selected_names: Vec<String> = resolved_selection
        .packages
        .iter()
        .map(|&idx| graph.packages[idx].package.name.as_str().to_owned())
        .collect();
    selected_names.sort();

    if files.is_empty() {
        reporter.status(
            "Formatted",
            format_args!(
                "no C/C++ sources found in {}",
                describe_packages(&selected_names)
            ),
        );
        return Ok(ExitCode::SUCCESS);
    }

    reporter.verbose(format_args!(
        "cabin: formatting {} file{} across {}",
        files.len(),
        plural(files.len()),
        describe_packages(&selected_names),
    ));

    let mode_args = match mode {
        FormatMode::Write => "--style=file -i",
        FormatMode::Check => "--style=file --dry-run -Werror",
    };
    reporter.very_verbose(format_args!(
        "cabin: running `{} {} <{} file{}>`",
        executable.to_string_lossy(),
        mode_args,
        files.len(),
        plural(files.len()),
    ));
    for file in &files {
        reporter.very_verbose(format_args!(
            "  {}",
            display_workspace_relative(&graph.root_dir, file),
        ));
    }

    let request = FormatRequest {
        executable,
        files,
        mode,
    };

    match run_formatter(&request) {
        Ok(FormatReport::Wrote { files_processed }) => {
            reporter.status(
                "Formatted",
                format_args!("{} file{}", files_processed, plural(files_processed)),
            );
            Ok(ExitCode::SUCCESS)
        }
        Ok(FormatReport::Clean { files_inspected }) => {
            reporter.status(
                "Formatted",
                format_args!(
                    "{} file{} already up to date",
                    files_inspected,
                    plural(files_inspected),
                ),
            );
            Ok(ExitCode::SUCCESS)
        }
        Ok(FormatReport::NeedsFormatting {
            files_inspected,
            stderr,
        }) => {
            // Pass clang-format's `--dry-run -Werror` stderr
            // through verbatim so `cabin fmt --check` shows the
            // per-file diagnostic - same shape as `cargo fmt
            // --check`, which forwards rustfmt's diff.  An empty
            // stderr is skipped so a `CABIN_FMT` wrapper that
            // signals failure via exit code alone does not
            // produce a blank line; the status banner below is
            // the always-on Cabin-owned signal that something
            // failed so CI users see an actionable message even
            // when the wrapped formatter is silent.
            if !stderr.is_empty() {
                reporter.tool_stderr(&stderr);
            }
            reporter.status(
                "Failed",
                format_args!(
                    "`cabin fmt --check`: {} file{} would be reformatted (re-run without --check to apply)",
                    files_inspected,
                    plural(files_inspected),
                ),
            );
            Ok(ExitCode::FAILURE)
        }
        Err(err) => bail!(err.to_string()),
    }
}
