//! Orchestration for `cabin tidy`.
//!
//! Translates the CLI flag bundle into the typed inputs the
//! shared crates accept and routes their outcomes back to the
//! reporter.  Source discovery is reused verbatim from
//! `cabin-source-discovery`; build planning lives in `cabin-build`
//! and `cabin-ninja` writes the compile database; clang-tidy
//! invocation lives in `cabin-tidy`.  No source-walking,
//! compile-database generation, or `run-clang-tidy` command-line
//! construction lives in this file.
//!
//! `cabin tidy` is the only Cabin command that needs a
//! `compile_commands.json` to do its job, so this module is also
//! the only place that calls `cabin_build::plan` outside the
//! build / run / test pipeline.  The planner is run *without*
//! invoking Ninja: clang-tidy reads the JSON compilation database
//! directly and a build is unnecessary for analysis.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Args;

use cabin_build::{ManifestTargetSelector, PlanRequest, plan, select_targets_of_kind};
use cabin_core::{DependencySource, SelectionRequest, TargetKind, TargetPlatform, Verbosity};
use cabin_source_discovery::{SourceDiscoveryRequest, discover_sources};
use cabin_tidy::{
    ExitStatusKind, TidyMode, TidyReport, TidyRequest, TidyVerbosity, resolve_tidy_executable,
    run_tidy,
};
use cabin_workspace::PackageGraph;

use crate::cli::source_tooling::{
    absolutize, describe_packages, display_workspace_relative, nested_package_excludes,
    package_selection_from_flags,
};
use crate::cli::term_verbosity::Reporter;
use crate::plural;

/// `cabin tidy` argument bundle.
///
/// Field doc-comments are picked up by clap and rendered in
/// `cabin tidy --help`; keep them user-focused.
#[derive(Debug, Args)]
pub(crate) struct TidyArgs {
    /// Path to the cabin.toml manifest.  Same precedence rules
    /// as `cabin build`: when omitted, Cabin walks upward from
    /// the current directory to find the nearest manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Build output directory for the generated compile database.
    /// Same precedence rules as `cabin build`: `--build-dir` >
    /// `CABIN_BUILD_DIR` > `[paths] build-dir` config setting >
    /// built-in default `build`.
    #[arg(long, value_name = "PATH")]
    pub build_dir: Option<PathBuf>,

    /// Apply the fixes clang-tidy suggests.  Off by default;
    /// Cabin never rewrites your sources unless this flag is
    /// passed explicitly.
    #[arg(long)]
    pub fix: bool,

    /// Exclude one file or directory from the analysis.  May be
    /// repeated.  Paths are resolved against the current working
    /// directory.
    #[arg(long, value_name = "PATH")]
    pub exclude: Vec<PathBuf>,

    /// Disable VCS ignore handling so files that are normally
    /// hidden by `.gitignore` are also analyzed.  Cabin's
    /// built-in build / cache / vendor exclusions still apply.
    #[arg(long)]
    pub no_ignore_vcs: bool,

    /// Analyze every workspace member.  Cannot be combined with
    /// `--package` or `--default-members`.
    #[arg(long, conflicts_with_all = &["package", "default_members"])]
    pub workspace: bool,

    /// Analyze the named workspace package.  Repeat the flag to
    /// select multiple packages.  Errors when a name is not a
    /// workspace member.
    #[arg(long = "package", short = 'p', value_name = "PACKAGE")]
    pub package: Vec<String>,

    /// Analyze `[workspace.default-members]`.  Errors when the
    /// workspace declares no default-members.
    #[arg(long, conflicts_with_all = &["workspace", "package"])]
    pub default_members: bool,

    /// Number of parallel `clang-tidy` instances to run.  Same
    /// precedence chain as `cabin build`: this flag wins over
    /// `CABIN_BUILD_JOBS`, then the `[build] jobs` config
    /// setting, then the backend's own default.  In `--fix`
    /// mode Cabin clamps the effective value to `1` so
    /// concurrent rewrites cannot race.
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<cabin_core::BuildJobs>,
}

/// Entry point invoked by the top-level dispatcher.
pub(crate) fn tidy(args: &TidyArgs, reporter: Reporter) -> Result<ExitCode> {
    let manifest_path = crate::cli::resolve_invocation_manifest(args.manifest_path.as_deref())?;
    // `cabin tidy` runs static analysis over local sources:
    // never auto-download foundation ports.  The cache
    // short-circuit serves an already-prepared workspace.
    let workspace_selection =
        package_selection_from_flags(args.workspace, &args.package, args.default_members);
    let (_port_sources, graph) = crate::cli::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        None,
        true,
        false,
        false,
        &workspace_selection,
        false,
    )?;
    let effective_config = crate::cli::config::load_effective_config(&graph)?;

    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;
    let selection_request = SelectionRequest::default();
    let feature_resolution =
        crate::cli::compute_feature_resolution(&graph, &resolved_selection, &selection_request)?;

    // `cabin tidy` does not run the artifact pipeline or load
    // extracted registry manifests, so selected packages with
    // versioned dependencies cannot be planned accurately here.
    // Scope this check to the selected closure so an unrelated
    // workspace member does not block `cabin tidy -p <name>`.
    if let Some(name) = first_selected_versioned_dependency_package_name(
        &graph,
        &resolved_selection,
        &feature_resolution,
    ) {
        bail!(
            "package `{name}` declares versioned registry dependencies; `cabin tidy` does not run the artifact pipeline, so registry-backed selections are not supported"
        );
    }

    // Match `cabin build` / `cabin fmt`'s build-directory
    // resolution so the walker excludes whatever
    // directory `cabin build` would have written into and so the
    // compile database lands at the same path the user already
    // sees in their tree.
    let (build_dir_input, _) = crate::cli::config::resolve_build_dir_with_env(
        args.build_dir.as_deref(),
        &effective_config,
    );
    let build_dir = absolutize(&graph.root_dir, &build_dir_input);

    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let absolute_excludes: Vec<PathBuf> =
        args.exclude.iter().map(|p| absolutize(&cwd, p)).collect();

    let executable = resolve_tidy_executable(|key| std::env::var_os(key));
    let tidy_verbosity = match reporter.verbosity() {
        Verbosity::Quiet | Verbosity::Normal => TidyVerbosity::Normal,
        Verbosity::Verbose | Verbosity::VeryVerbose => TidyVerbosity::Verbose,
    };

    let mode = if args.fix {
        TidyMode::Fix
    } else {
        TidyMode::Check
    };

    // Resolve jobs through the same precedence chain `cabin
    // build`/`run`/`tidy` honor: CLI wins over CABIN_BUILD_JOBS,
    // then the [build] jobs config setting, then the backend's
    // own default.  In `--fix` mode the effective value is
    // clamped to 1: two clang-tidy instances applying overlapping
    // rewrites can race, and the safest behavior is to serialize
    // them.  When the user explicitly asked for a higher count we
    // surface the override in verbose mode rather than silently
    // dropping the request.
    let requested_jobs = crate::cli::config::resolve_build_jobs(args.jobs, &effective_config)?;
    let effective_jobs = if matches!(mode, TidyMode::Fix) {
        if requested_jobs.is_some_and(|j| j.get() > 1) {
            reporter.verbose(format_args!(
                "cabin: --fix forces tidy parallelism to 1 (requested -j{})",
                requested_jobs.expect("checked above").get(),
            ));
        }
        Some(cabin_core::BuildJobs::new(1).expect("1 is non-zero"))
    } else {
        requested_jobs
    };

    // Short-circuit before asking the planner to do anything: a
    // workspace whose selected packages declare no C/C++ targets
    // has nothing to analyze, and `cabin_build::plan` would
    // otherwise bail with `EmptySelectedPackages`.  Mirrors the
    // "no files to check" path used by other source tools.
    let selected_indices: BTreeSet<usize> = resolved_selection.packages.iter().copied().collect();
    if !any_cpp_targets(&graph, &selected_indices) {
        reporter.status("Checked", format_args!("no C/C++ source files"));
        return Ok(ExitCode::SUCCESS);
    }

    // Build the per-target compile-command list by running the
    // planner with the dev profile and a process-resolved
    // toolchain.
    let host_platform = cabin_core::TargetPlatform::current();
    let toolchain_selection = cabin_core::ToolchainSelection::default();
    let toolchain = crate::cli::resolve_toolchain_layered(
        &graph,
        &toolchain_selection,
        &effective_config,
        &host_platform,
    )?;

    let manifest_profiles = crate::cli::workspace_profile_definitions(&graph);
    let profile_selection = cabin_core::ProfileSelection::default_dev();
    let profile = cabin_core::resolve_profile(&profile_selection, &manifest_profiles)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    // Detect before resolving flags so `cfg(cc/cxx = ...)` profile
    // layers observe the same identities the build commands use, and
    // spell the compile database in the *resolved* compiler's dialect,
    // matching `cabin build` - otherwise a user-selected GNU toolchain
    // on Windows would get MSVC-flagged commands clang-tidy cannot
    // consume. `cabin tidy` drives clang-tidy, not the compiler, so
    // detection stays fail-soft: on failure the dialect falls back to
    // the host default and compiler cfg conditions evaluate as
    // `unknown`.
    let detection_report =
        cabin_toolchain::detect_toolchain(&toolchain, &cabin_toolchain::ProcessRunner).ok();
    let dialect = detection_report
        .as_ref()
        .map_or_else(cabin_build::Dialect::host_default, |report| {
            cabin_build::Dialect::from_compiler_kind(report.cxx.identity.kind)
        });

    let language_standards = crate::cli::resolve_per_package_language_standards(&graph);
    let (build_flags, standard_flag_conflicts) = crate::cli::resolve_per_package_build_flags(
        &graph,
        &profile,
        &host_platform,
        &feature_resolution,
        detection_report.as_ref(),
    );
    // `cabin tidy` does not opt into dev-dep activation;
    // dev-kind system deps stay declaration-only here.
    let dev_for: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // The MSVC backend cannot consume pkg-config's GNU-style flags, so
    // reject before `augment_build_flags` probes pkg-config and merges
    // them into a compile database clang-tidy would then read.  Scoped to
    // the selected closure exactly as `cabin build` is: a path
    // dependency's system-dep flags propagate into the selected packages'
    // compile commands, so the closure is the set that can corrupt the
    // database, while an unrelated member's dependency never gates
    // `cabin tidy -p other`.  The check fires on the same dialect tidy
    // plans with, including the fail-soft host-default fallback above -
    // if tidy commits to MSVC, MSVC's constraint applies to the database
    // it is about to emit.
    let selected_closure = resolved_selection.closure(&graph);
    crate::cli::system_deps::ensure_dialect_supports_system_deps(
        &graph,
        &host_platform,
        &dev_for,
        dialect,
        &selected_closure,
    )?;
    let build_flags =
        crate::cli::augment_build_flags(&graph, &host_platform, &dev_for, build_flags, reporter)?;

    // Build configurations are required by the planner so the
    // per-package `BuildConfiguration` exists for every selected
    // package.  We use empty selection requests (no CLI
    // features); tidy is workspace-wide analysis, so
    // per-feature configuration is not the bar.
    let toolchain_summary = cabin_core::ToolchainSummary::from_resolved_parts(&toolchain, None);
    let configurations = crate::cli::resolve_build_configurations(
        &graph,
        &selection_request,
        &resolved_selection.packages,
        &profile,
        &toolchain_summary,
        &build_flags,
    )?;
    let root_configuration = graph
        .root_package
        .and_then(|i| configurations.get(&i))
        .cloned();

    // The planner's default selection only emits compile commands
    // for default-buildable kinds (library, header-only, executable),
    // which silently excludes `*_test` / `*_example` sources.
    // Tidy is asymmetric to fmt without those kinds, so enumerate
    // every C/C++ kind explicitly here - both the `cpp_*` family
    // and the `c_*` family.
    let tidy_selectors: Vec<ManifestTargetSelector> = TargetKind::all()
        .iter()
        .copied()
        .flat_map(|kind| select_targets_of_kind(&graph, Some(&resolved_selection.packages), kind))
        .collect();

    let plan_graph = plan(&PlanRequest {
        graph: &graph,
        toolchain: &toolchain,
        build_flags: &build_flags,
        language_standards: &language_standards,
        standard_flag_conflicts: &standard_flag_conflicts,
        build_dir: build_dir.clone(),
        profile: profile.clone(),
        selected: Some(tidy_selectors),
        configuration: root_configuration.as_ref(),
        selected_packages: Some(&resolved_selection.packages),
        compiler_wrapper: None,
        dialect,
        // Mirrors the fail-soft dialect fallback above: without a
        // detection report tidy cannot know the `cl` version, so it
        // conservatively spells dependency includes as plain `/I`.
        msvc_external_includes: detection_report.as_ref().is_some_and(|report| {
            cabin_build::msvc_external_includes_supported(
                report,
                cabin_build::collect_requested_standards(
                    &graph,
                    &selected_closure,
                    &language_standards,
                    &dev_for,
                )
                .has_c_sources(),
            )
        }),
    })?;
    // `cabin tidy` skips the fail-hard toolchain validation, so it
    // must surface planner-recorded MSVC standard violations itself -
    // a violating compile is omitted from the compile database and
    // must never be dropped silently.
    cabin_build::validate_planned_standards(&plan_graph)?;

    // Use the per-profile build root so the compile database
    // lands at the same path `cabin build` produces.  This is
    // what the user already sees in their tree and what
    // `clang-tidy -p <dir>` expects.
    let profile_build_root = build_dir.join(profile.name.as_str());
    std::fs::create_dir_all(&profile_build_root).with_context(|| {
        format!(
            "failed to create build directory {}",
            profile_build_root.display()
        )
    })?;
    let compile_db_path = profile_build_root.join("compile_commands.json");
    cabin_ninja::write_compile_commands(&compile_db_path, &plan_graph)?;

    // Filter discovered sources to those that have an entry in
    // the compile database. `clang-tidy` cannot analyze a file
    // without a compile command, and `run-clang-tidy` interprets
    // bare filenames as regex patterns matched against the
    // database, so passing files with no entry would produce
    // confusing "no matches" warnings on stderr.
    let mut excluded_directories = nested_package_excludes(&graph, &selected_indices);
    excluded_directories.push(build_dir);
    let roots: Vec<PathBuf> = resolved_selection
        .packages
        .iter()
        .map(|&idx| graph.packages[idx].manifest_dir.clone())
        .collect();
    let request = SourceDiscoveryRequest {
        roots,
        excluded_paths: absolute_excludes,
        excluded_directories,
        respect_vcs_ignore: !args.no_ignore_vcs,
    };
    let discovered = discover_sources(&request)
        .map_err(|err| anyhow::anyhow!("source discovery failed: {err}"))?;
    let compile_db_files: BTreeSet<PathBuf> = plan_graph
        .compile_commands
        .iter()
        .map(|cc| canonicalize_or_self(cc.file.as_std_path()))
        .collect();
    let files: Vec<PathBuf> = discovered
        .into_iter()
        .map(|f| canonicalize_or_self(&f.absolute_path))
        .filter(|p| compile_db_files.contains(p))
        .collect();

    let mut selected_names: Vec<String> = resolved_selection
        .packages
        .iter()
        .map(|&idx| graph.packages[idx].package.name.as_str().to_owned())
        .collect();
    selected_names.sort();

    if files.is_empty() {
        reporter.status(
            "Checked",
            format_args!(
                "no C/C++ source files in {}",
                describe_packages(&selected_names)
            ),
        );
        return Ok(ExitCode::SUCCESS);
    }

    reporter.verbose(format_args!(
        "cabin: running clang-tidy for {}",
        describe_packages(&selected_names),
    ));
    reporter.verbose(format_args!(
        "cabin: tidying {} file{} across {}",
        files.len(),
        plural(files.len()),
        describe_packages(&selected_names),
    ));
    reporter.verbose(format_args!(
        "cabin: compile database = {}",
        compile_db_path.display(),
    ));
    if let Some(jobs) = effective_jobs {
        reporter.verbose(format_args!("cabin: jobs = {}", jobs.get()));
    }
    reporter.very_verbose(format_args!(
        "cabin: running `{} -p {}{}{} <{} file{}>`",
        executable.to_string_lossy(),
        profile_build_root.display(),
        match mode {
            TidyMode::Fix => " -fix",
            TidyMode::Check => "",
        },
        match (tidy_verbosity, effective_jobs) {
            (TidyVerbosity::Normal, Some(j)) => format!(" -quiet -j {}", j.get()),
            (TidyVerbosity::Normal, None) => " -quiet".to_owned(),
            (TidyVerbosity::Verbose, Some(j)) => format!(" -j {}", j.get()),
            (TidyVerbosity::Verbose, None) => String::new(),
        },
        files.len(),
        plural(files.len()),
    ));
    for file in &files {
        reporter.very_verbose(format_args!(
            "  {}",
            display_workspace_relative(&graph.root_dir, file),
        ));
    }

    let tidy_request = TidyRequest {
        executable,
        compile_database_dir: profile_build_root,
        files,
        mode,
        jobs: effective_jobs,
        verbosity: tidy_verbosity,
    };

    match run_tidy(&tidy_request) {
        Ok(TidyReport::Tidied { files_processed }) => {
            reporter.status(
                "Checked",
                format_args!("{} file{}", files_processed, plural(files_processed)),
            );
            Ok(ExitCode::SUCCESS)
        }
        Ok(TidyReport::NoFiles) => {
            // Pre-filtered above; the runner's empty-list
            // short-circuit is the source of truth.
            Ok(ExitCode::SUCCESS)
        }
        Ok(TidyReport::TidyFailed {
            status,
            files_processed,
        }) => {
            reporter.status(
                "Failed",
                format_args!(
                    "clang-tidy on {} ({}, {} file{})",
                    describe_packages(&selected_names),
                    describe_status(&status),
                    files_processed,
                    plural(files_processed),
                ),
            );
            Ok(ExitCode::FAILURE)
        }
        Err(err) => bail!(err.to_string()),
    }
}

/// Best-effort path canonicalization.  When the file does not
/// exist or canonicalization otherwise fails, returns the input
/// unchanged so the caller still has a usable absolute path.
/// Source-discovery and the planner both produce absolute paths
/// already; this only normalizes symlink resolution so the
/// per-side comparison sees the same bytes.
fn canonicalize_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether the union of the selected packages contains at least
/// one C/C++ target.  The planner's enumeration would otherwise
/// fail with `EmptySelectedPackages` for workspaces that hold
/// only Rust libraries or pure manifest entries.
fn any_cpp_targets(graph: &PackageGraph, selected: &BTreeSet<usize>) -> bool {
    selected.iter().any(|&idx| {
        graph.packages[idx]
            .package
            .targets
            .iter()
            .any(|t| TargetKind::all().contains(&t.kind))
    })
}

/// Find the first selected-closure package that declares an active
/// versioned registry dependency.  Used to surface an actionable
/// error instead of letting `cabin_build::plan` fail with a
/// confusing downstream message.
fn first_selected_versioned_dependency_package_name(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    features: &cabin_feature::FeatureResolution,
) -> Option<String> {
    let closure = selection.closure(graph);
    let host_platform = TargetPlatform::current();
    let mut hits: BTreeSet<&str> = BTreeSet::new();
    for idx in closure {
        let pkg = &graph.packages[idx];
        for dep in &pkg.package.dependencies {
            if dep.kind.is_resolved_by_default()
                && dep.matches_platform(&host_platform)
                && (!dep.optional || features.is_optional_dep_enabled(idx, dep.name.as_str()))
                && matches!(dep.source, DependencySource::Version(_))
            {
                hits.insert(pkg.package.name.as_str());
            }
        }
    }
    hits.into_iter().next().map(str::to_owned)
}

fn describe_status(status: &ExitStatusKind) -> String {
    match status {
        ExitStatusKind::Code(c) => format!("exit code {c}"),
        ExitStatusKind::Signal(s) => format!("signal {s}"),
        ExitStatusKind::Unknown => "unknown status".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cabin_core::{Package, PackageName, Target, TargetName};
    use cabin_workspace::{PackageKind, WorkspacePackage};

    fn graph_with_single_target(kind: TargetKind) -> PackageGraph {
        let target = Target {
            name: TargetName::new("only").unwrap(),
            kind,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            language: Default::default(),
        };
        let package = Package::new(
            PackageName::new("demo").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            vec![target],
            Vec::new(),
        )
        .unwrap();
        PackageGraph {
            root_manifest_path: PathBuf::from("demo/cabin.toml"),
            root_dir: PathBuf::from("demo"),
            is_workspace_root: false,
            root_package: Some(0),
            root_settings: Default::default(),
            primary_packages: vec![0],
            default_members: vec![0],
            excluded_members: Vec::new(),
            packages: vec![WorkspacePackage {
                package,
                manifest_path: PathBuf::from("demo/cabin.toml"),
                manifest_dir: PathBuf::from("demo"),
                deps: Vec::new(),
                kind: PackageKind::Local,
                is_port: false,
            }],
        }
    }

    #[test]
    fn any_cpp_targets_detects_test_only_package() {
        let graph = graph_with_single_target(TargetKind::Test);
        let selected: BTreeSet<usize> = [0].into_iter().collect();
        assert!(
            any_cpp_targets(&graph, &selected),
            "test must count as a C/C++ target for `cabin tidy`",
        );
    }

    #[test]
    fn any_cpp_targets_detects_example_only_package() {
        let graph = graph_with_single_target(TargetKind::Example);
        let selected: BTreeSet<usize> = [0].into_iter().collect();
        assert!(
            any_cpp_targets(&graph, &selected),
            "example must count as a C/C++ target for `cabin tidy`",
        );
    }

    #[test]
    fn any_cpp_targets_still_detects_library() {
        let graph = graph_with_single_target(TargetKind::Library);
        let selected: BTreeSet<usize> = [0].into_iter().collect();
        assert!(any_cpp_targets(&graph, &selected));
    }
}
