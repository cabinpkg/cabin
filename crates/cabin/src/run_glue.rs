//! Glue layer for `cabin run`.
//!
//! `cabin run` is a thin wrapper over the same build pipeline
//! `cabin build` runs (workspace load → patches → artifact
//! pipeline → planner → Ninja). After Ninja produces the
//! linked executable, this module locates the file that the
//! planner emitted for the selected target, populates a
//! deterministic `CABIN_*` environment, and execs the binary
//! with the user's stdio attached. Arguments after `--` are
//! forwarded verbatim.
//!
//! The typed `CABIN_*` env overlay is built by
//! `cabin_env::package_env`; this module only orchestrates.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Args;

use cabin_build::{ManifestTargetSelector, PlanRequest, plan};
use cabin_core::{Package, TargetKind};
use cabin_workspace::{
    RegistryPackageSource, WorkspaceLoadOptions, collect_patched_versioned_deps,
    load_workspace_with_options,
};

use crate::cli::{
    ArtifactPipelineRequest, ToolchainSelectionArgs, WorkspaceSelectionArgs, absolutise,
    augment_build_flags, build_selection_request, build_workspace_selection,
    closure_has_versioned_deps_excluding_patches, compiler_wrapper_override_from_args,
    compute_feature_resolution, lock_mode_for_flags, profile_selection_from_flags,
    resolve_build_configurations, resolve_invocation_manifest, resolve_per_package_build_flags,
    run_artifact_pipeline, toolchain_selection_from_args, workspace_compiler_wrapper_settings,
    workspace_profile_definitions,
};

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Build output directory. Same precedence rules as
    /// `cabin build`: `--build-dir` > `CABIN_BUILD_DIR` >
    /// `[paths] build-dir` config setting > built-in default
    /// `build`.
    #[arg(long, value_name = "PATH")]
    pub build_dir: Option<PathBuf>,

    /// Build with optimizations.
    ///
    /// Compatibility alias for `--profile release`; cannot be
    /// used together with `--profile`.
    #[arg(short = 'r', long, conflicts_with = "profile")]
    pub release: bool,

    /// Build profile (`dev`, `release`, or any custom profile
    /// declared in `[profile.<name>]`). Defaults to `dev`.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Build and run the named `cpp_executable` target.
    #[arg(long = "bin", value_name = "NAME")]
    pub bin: Option<String>,

    /// Path to a directory containing the local JSON package index.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL to read package metadata from.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Override the default artifact cache directory.
    #[arg(long, value_name = "PATH")]
    pub cache_dir: Option<PathBuf>,

    /// Require an existing, current `cabin.lock`.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects state-writing side effects.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access.
    #[arg(long)]
    pub offline: bool,

    /// Enable named features.
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Enable every declared feature.
    #[arg(long)]
    pub all_features: bool,

    /// Disable default features.
    #[arg(long)]
    pub no_default_features: bool,

    /// Workspace package-selection flags.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Toolchain-selection flags.
    #[command(flatten)]
    pub toolchain: ToolchainSelectionArgs,

    /// Disable every active patch / source-replacement entry.
    #[arg(long)]
    pub no_patches: bool,

    /// Number of parallel jobs to use for the build phase.
    ///
    /// Precedence: this flag > `CABIN_BUILD_JOBS` env var >
    /// `[build] jobs` config setting > backend default.  The
    /// value must be a positive integer; `0` is rejected.
    /// Cabin does not forward `--jobs` to the executed
    /// program; arguments after `--` (which may include their
    /// own `--jobs`) reach the program verbatim.
    #[arg(short = 'j', long = "jobs", value_name = "N")]
    pub jobs: Option<cabin_core::BuildJobs>,

    /// Arguments forwarded to the executed program. Everything
    /// after `--` is passed verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Run `cabin run`: build and execute a selected binary target.
///
/// Returns an `ExitCode` so the spawned program's exit status
/// becomes Cabin's own exit status. Failing to start the
/// program (or any pipeline error) surfaces as an
/// [`anyhow::Error`] and Cabin exits non-zero from the
/// top-level dispatcher.
pub(crate) fn run(
    args: &RunArgs,
    reporter: crate::term_verbosity_glue::Reporter,
) -> Result<ExitCode> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;

    let offline = crate::config_glue::effective_offline(args.offline)?;
    let run_selection = build_workspace_selection(&args.workspace_selection);
    let (prepared_ports, initial_graph) = crate::port_glue::prepare_ports_and_load_initial_graph(
        &manifest_path,
        args.cache_dir.as_deref(),
        offline,
        args.frozen,
        false,
        &run_selection,
        args.no_patches,
    )?;
    let port_sources: Vec<cabin_workspace::PortPackageSource> = prepared_ports
        .iter()
        .map(crate::port_glue::workspace_source)
        .collect();
    let effective_config = crate::config_glue::load_effective_config(&initial_graph)?;
    let active_patches =
        crate::patch_glue::load_active_patches(&initial_graph, &effective_config, args.no_patches)?;
    let patched_names = active_patches.owned_patched_names();

    let workspace_selection_for_pipeline = build_workspace_selection(&args.workspace_selection);
    let initial_resolved_selection = cabin_workspace::resolve_package_selection(
        &initial_graph,
        &workspace_selection_for_pipeline,
    )?;

    let initial_request =
        build_selection_request(&args.features, args.all_features, args.no_default_features);
    let dev_for: BTreeSet<String> = BTreeSet::new();
    let initial_features = compute_feature_resolution(
        &initial_graph,
        &initial_resolved_selection,
        &initial_request,
    )?;

    let resolved_index_source = crate::config_glue::resolve_index_source(
        args.index_path.as_deref(),
        args.index_url.as_deref(),
        &effective_config,
    )?;
    crate::config_glue::enforce_offline_index_source(offline, resolved_index_source.as_ref())?;
    let resolved_cache_dir =
        crate::config_glue::resolve_cache_dir(args.cache_dir.as_deref(), &effective_config);

    let patched_root_deps_preview =
        collect_patched_versioned_deps(&active_patches, &patched_names)?;
    let has_versioned = !patched_root_deps_preview.is_empty()
        || closure_has_versioned_deps_excluding_patches(
            &initial_graph,
            &initial_resolved_selection,
            &initial_features,
            &patched_names,
            &dev_for,
        );

    let registry: Vec<RegistryPackageSource> = if has_versioned {
        let Some(index_source) = resolved_index_source.as_ref() else {
            bail!(
                "versioned dependencies require --index-path, --index-url, or a `[registry]` config setting"
            );
        };
        let mode = lock_mode_for_flags(args.locked, args.frozen);
        let allow_write = !(args.locked || args.frozen);
        let cache_dir = match resolved_cache_dir.as_ref() {
            Some((path, _)) => path.clone(),
            None => crate::cli::cache_dir_for(&manifest_path, args.cache_dir.as_deref())?,
        };
        let initial_locator = match &index_source.kind {
            crate::config_glue::IndexSourceKind::Path(p) => {
                cabin_core::SourceLocator::IndexPath { path: p.clone() }
            }
            crate::config_glue::IndexSourceKind::Url(u) => {
                cabin_core::SourceLocator::IndexUrl { url: u.clone() }
            }
        };
        let resolved_locator = crate::patch_glue::apply_source_replacement(
            initial_locator,
            &effective_config,
            args.no_patches,
        )?;
        crate::config_glue::enforce_offline_post_replacement(offline, &resolved_locator)?;
        let (replaced_path, replaced_url) =
            crate::patch_glue::locator_to_index_inputs(&resolved_locator.resolved);
        let pipeline = run_artifact_pipeline(&ArtifactPipelineRequest {
            manifest_path: &manifest_path,
            initial_graph: &initial_graph,
            index_path: replaced_path.as_deref(),
            index_url: replaced_url.as_deref(),
            mode,
            allow_write,
            frozen: args.frozen,
            cache_dir: &cache_dir,
            reporter,
            selection: workspace_selection_for_pipeline,
            selection_request: &initial_request,
            patched_names: &patched_names,
            active_patches: &active_patches,
            source_replacements: &effective_config.source_replacements,
            no_patches: args.no_patches,
            dev_for: &dev_for,
        })?;
        pipeline
            .fetched
            .iter()
            .map(|p| RegistryPackageSource {
                name: p.name.clone(),
                version: p.version.clone(),
                manifest_path: p.source_dir.join("cabin.toml"),
            })
            .collect()
    } else {
        Vec::new()
    };

    // Mirror `cabin build`: the strict set is the selection's
    // closure on `initial_graph` plus every patched name plus
    // every resolver-fetched registry package. Registry packages
    // a patched manifest introduces via a new version dep are not
    // in `initial_graph` (the initial load runs with `registry:
    // &[]`), so the closure misses them; without this extension
    // their missing-registry / missing-port edges silently drop
    // under the scoped policy and the build fails later with a
    // less actionable link error.
    let mut strict_packages: BTreeSet<String> = initial_resolved_selection
        .closure(&initial_graph)
        .into_iter()
        .map(|i| initial_graph.packages[i].package.name.as_str().to_owned())
        .collect();
    strict_packages.extend(patched_names.iter().cloned());
    strict_packages.extend(registry.iter().map(|r| r.name.as_str().to_owned()));
    let patched_sources = active_patches.workspace_sources();
    let graph = load_workspace_with_options(
        &manifest_path,
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &patched_sources,
            ports: &port_sources,
            registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&strict_packages),
            include_dev_for: &dev_for,
            port_policy: cabin_workspace::PortPolicy::TolerateExcept(&strict_packages),
        },
    )?;

    let (build_dir_input, _build_dir_source) = crate::config_glue::resolve_build_dir_with_env(
        args.build_dir.as_deref(),
        &effective_config,
    );
    let build_dir = absolutise(&build_dir_input)
        .with_context(|| format!("failed to resolve build dir {}", build_dir_input.display()))?;

    let host_platform = cabin_core::TargetPlatform::current();
    let toolchain_selection = toolchain_selection_from_args(&args.toolchain)?;
    let toolchain = crate::cli::resolve_toolchain_layered(
        &graph,
        &toolchain_selection,
        &effective_config,
        &host_platform,
    )?;
    let detection_report =
        cabin_toolchain::detect_toolchain(&toolchain, &cabin_toolchain::ProcessRunner)?;
    cabin_build::validate_toolchain_for_backend(&toolchain, &detection_report)?;
    let ninja = cabin_toolchain::locate_ninja()?;

    let manifest_compiler_wrapper = workspace_compiler_wrapper_settings(&graph);
    let cli_compiler_wrapper = compiler_wrapper_override_from_args(&args.toolchain)?;

    let profile_selection =
        profile_selection_from_flags(args.profile.as_deref(), args.release, &effective_config)?;
    let manifest_profiles = workspace_profile_definitions(&graph);
    let profile = cabin_core::resolve_profile(&profile_selection, &manifest_profiles)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let profile_build = profile.build.as_ref();
    let build_flags = resolve_per_package_build_flags(&graph, profile_build, &host_platform);
    let build_flags = augment_build_flags(&graph, &host_platform, &dev_for, build_flags, reporter)?;

    let compiler_wrapper = crate::cli::resolve_compiler_wrapper_layered(
        cli_compiler_wrapper,
        &manifest_compiler_wrapper,
        &effective_config,
        &host_platform,
    )?;
    let toolchain_summary =
        cabin_core::ToolchainSummary::from_resolved_parts(&toolchain, compiler_wrapper.as_ref());

    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;

    // Pick the run target. `--bin` narrows the search to a
    // named `cpp_executable`; otherwise we look for a single
    // `cpp_executable` in the selected closure.
    let run_target = pick_run_target(&graph, &resolved_selection.packages, args.bin.as_deref())?;

    let selection_request =
        build_selection_request(&args.features, args.all_features, args.no_default_features);
    let configurations = resolve_build_configurations(
        &graph,
        &selection_request,
        &resolved_selection.packages,
        &profile,
        &toolchain_summary,
        &build_flags,
    )?;
    let feature_resolution =
        compute_feature_resolution(&graph, &resolved_selection, &selection_request)?;

    let root_configuration = graph
        .root_package
        .and_then(|i| configurations.get(&i))
        .cloned();
    let plan_graph = plan(&PlanRequest {
        graph: &graph,
        toolchain: &toolchain,
        build_flags: &build_flags,
        build_dir: build_dir.clone(),
        profile: profile.clone(),
        selected: Some(vec![ManifestTargetSelector {
            package: Some(run_target.package_name.clone()),
            name: run_target.target_name.clone(),
        }]),
        configuration: root_configuration.as_ref(),
        selected_packages: Some(&resolved_selection.packages),
        compiler_wrapper: compiler_wrapper.as_ref(),
    })?;

    let profile_build_root = build_dir.join(profile.name.as_str());
    std::fs::create_dir_all(&profile_build_root).with_context(|| {
        format!(
            "failed to create build directory {}",
            profile_build_root.display()
        )
    })?;

    let ninja_file = profile_build_root.join("build.ninja");
    cabin_ninja::write_build_ninja(&ninja_file, &plan_graph)?;
    let ccmd_file = profile_build_root.join("compile_commands.json");
    cabin_ninja::write_compile_commands(&ccmd_file, &plan_graph)?;

    let jobs = crate::config_glue::resolve_build_jobs(args.jobs, &effective_config)?;
    // Implementation-detail status is verbose-only: under `-v`
    // the user sees which files Cabin wrote and how Ninja was
    // invoked, alongside Ninja's own raw banner.
    reporter.verbose(format_args!("cabin: wrote {}", ninja_file.display()));
    reporter.verbose(format_args!("cabin: wrote {}", ccmd_file.display()));
    reporter.verbose(format_args!(
        "cabin: invoking {} {}-C {}",
        ninja.display(),
        crate::ninja_glue::ninja_jobs_echo(jobs),
        profile_build_root.display()
    ));
    let mut ninja_cmd = std::process::Command::new(&ninja);
    if let Some(jobs) = jobs {
        ninja_cmd.arg(jobs.as_ninja_arg());
    }
    // Route Ninja through the shared runner so `cabin run`'s
    // build phase prints the same cargo-style `Compiling …`
    // banner `cabin build` emits — and so the verbose passthrough
    // and the default-mode filtering stay in one place.
    let run = crate::ninja_glue::run_ninja(
        ninja_cmd.arg("-C").arg(&profile_build_root),
        reporter,
        &graph,
    )
    .with_context(|| format!("failed to invoke ninja at {}", ninja.display()))?;
    if !run.status.success() {
        crate::ninja_glue::emit_link_diagnostic_if_applicable(
            &run,
            &graph,
            &feature_resolution,
            &dev_for,
            reporter,
        );
        bail!("ninja exited with {}", run.status);
    }

    let executable = locate_target_executable(&plan_graph.default_outputs, &run_target)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "build graph did not produce an executable for `{}:{}`",
                run_target.package_name,
                run_target.target_name,
            )
        })?;

    // Build the env. We do not clear the user's environment —
    // the spawned program inherits PATH, LANG, etc. — but we
    // overlay the deterministic CABIN_* values so the program
    // sees consistent package metadata.
    let env_overlay = cabin_env::package_env(&cabin_env::PackageEnvInputs {
        manifest_dir: &run_target.manifest_dir,
        manifest_path: &run_target.manifest_path,
        package_name: &run_target.package_name,
        package_version: &run_target.package_version,
        profile: profile.name.as_str(),
        build_dir: &build_dir,
    });

    // Working directory: mirror Cargo by inheriting the user's
    // current working directory. Trailing args (`cabin run --
    // a b`) are forwarded to the spawned program verbatim; clap
    // strips the `--` separator before we see the vec.
    let mut command = std::process::Command::new(&executable);
    command.envs(env_overlay.iter().map(|(k, v)| (k.as_str(), v.as_os_str())));
    command.args(args.args.iter());
    let status = command.status().with_context(|| {
        format!(
            "failed to start `{}` ({}:{})",
            executable.display(),
            run_target.package_name,
            run_target.target_name
        )
    })?;
    Ok(exit_code_for(status))
}

/// Map the spawned program's exit status onto a `process::ExitCode`
/// so `cabin run`'s own exit code is the program's exit code.
/// Signal-terminated children produce exit code `1` because
/// `ExitCode` cannot represent signal kills directly.
fn exit_code_for(status: std::process::ExitStatus) -> ExitCode {
    match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1)),
        None => ExitCode::from(1),
    }
}

/// Resolved run target. The orchestration layer narrows to
/// exactly one of these before invoking the planner.
#[derive(Debug, Clone)]
struct RunTarget {
    package_name: String,
    package_version: String,
    target_name: String,
    manifest_dir: PathBuf,
    manifest_path: PathBuf,
}

fn pick_run_target(
    graph: &cabin_workspace::PackageGraph,
    selected_packages: &[usize],
    bin: Option<&str>,
) -> Result<RunTarget> {
    let pool: Vec<usize> = if selected_packages.is_empty() {
        (0..graph.packages.len()).collect()
    } else {
        selected_packages.to_vec()
    };
    if let Some(name) = bin {
        return find_target(graph, &pool, name, TargetKind::CppExecutable, "--bin");
    }
    // Default: pick a single cpp_executable in the selected
    // packages. Ambiguous selections produce a diagnostic
    // listing every candidate so users can decide.
    let mut candidates: Vec<RunTarget> = Vec::new();
    for &idx in &pool {
        let pkg = &graph.packages[idx];
        for target in &pkg.package.targets {
            if target.kind == TargetKind::CppExecutable {
                candidates.push(make_run_target(
                    &pkg.package,
                    &graph.packages[idx].manifest_path,
                    &graph.packages[idx].manifest_dir,
                    target.name.as_str(),
                ));
            }
        }
    }
    if candidates.is_empty() {
        bail!("no `cpp_executable` target found in the selected packages; declare one to run it");
    }
    if candidates.len() > 1 {
        let listed: Vec<String> = candidates
            .iter()
            .map(|t| format!("{}:{}", t.package_name, t.target_name))
            .collect();
        bail!(
            "multiple `cpp_executable` targets found in the selected packages; pass `--bin <name>` to disambiguate. Candidates: {}",
            listed.join(", ")
        );
    }
    Ok(candidates.into_iter().next().expect("len==1 above"))
}

fn find_target(
    graph: &cabin_workspace::PackageGraph,
    pool: &[usize],
    name: &str,
    expected_kind: TargetKind,
    flag: &str,
) -> Result<RunTarget> {
    // Walk every selected package before reporting a kind
    // mismatch: a `cpp_library` named `foo` in pkg A must not
    // mask a `cpp_executable` named `foo` in pkg B simply because
    // A is iterated first.
    let mut candidates: Vec<RunTarget> = Vec::new();
    let mut other_kind: Option<TargetKind> = None;
    for &idx in pool {
        let pkg = &graph.packages[idx];
        for target in &pkg.package.targets {
            if target.name.as_str() != name {
                continue;
            }
            if target.kind != expected_kind {
                other_kind.get_or_insert(target.kind);
                continue;
            }
            candidates.push(make_run_target(
                &pkg.package,
                &graph.packages[idx].manifest_path,
                &graph.packages[idx].manifest_dir,
                target.name.as_str(),
            ));
        }
    }
    if candidates.is_empty() {
        if let Some(kind) = other_kind {
            bail!(
                "{flag} `{name}` matched a target of kind `{}`; expected `{}`",
                kind.as_str(),
                expected_kind.as_str()
            );
        }
        bail!("{flag} `{name}` was not found in the selected packages");
    }
    if candidates.len() > 1 {
        let owners: Vec<String> = candidates.iter().map(|t| t.package_name.clone()).collect();
        bail!(
            "{flag} `{name}` is ambiguous; declared by packages: {}",
            owners.join(", ")
        );
    }
    Ok(candidates.into_iter().next().expect("len==1 above"))
}

fn make_run_target(
    package: &Package,
    manifest_path: &Path,
    manifest_dir: &Path,
    target_name: &str,
) -> RunTarget {
    RunTarget {
        package_name: package.name.as_str().to_owned(),
        package_version: package.version.to_string(),
        target_name: target_name.to_owned(),
        manifest_dir: manifest_dir.to_path_buf(),
        manifest_path: manifest_path.to_path_buf(),
    }
}

/// Walk the planner's `default_outputs` looking for the
/// executable produced for `target`. The planner names every
/// `cpp_executable` output
/// `<build_dir>/<profile>/packages/<pkg>/<target>` (no extension
/// on POSIX; `.exe` on Windows). We scan rather than re-deriving
/// the path so the planner stays the single source of truth.
fn locate_target_executable(default_outputs: &[PathBuf], target: &RunTarget) -> Option<PathBuf> {
    let needle_tail: PathBuf = [
        "packages",
        target.package_name.as_str(),
        target.target_name.as_str(),
    ]
    .iter()
    .collect();
    default_outputs
        .iter()
        .find(|p| p.ends_with(&needle_tail))
        .cloned()
        .or_else(|| {
            // Windows build output appends `.exe`; the
            // unsuffixed needle does not match. Try matching
            // the parent directory and last component
            // separately.
            let parent_tail: PathBuf = ["packages", target.package_name.as_str()].iter().collect();
            default_outputs.iter().find_map(|p| {
                let same_parent = p
                    .parent()
                    .map(|pp| pp.ends_with(&parent_tail))
                    .unwrap_or(false);
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if same_parent && stem == target.target_name {
                    Some(p.clone())
                } else {
                    None
                }
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use cabin_core::{PackageName, Target, TargetName};
    use cabin_workspace::{PackageKind, WorkspacePackage};

    fn target(name: &str, kind: TargetKind) -> Target {
        Target {
            name: TargetName::new(name).unwrap(),
            kind,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
        }
    }

    fn workspace_package(name: &str, targets: Vec<Target>) -> WorkspacePackage {
        let package = Package::new(
            PackageName::new(name).unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            targets,
            Vec::new(),
        )
        .unwrap();
        WorkspacePackage {
            package,
            manifest_path: PathBuf::from(format!("{name}/cabin.toml")),
            manifest_dir: PathBuf::from(name),
            deps: Vec::new(),
            kind: PackageKind::Local,
        }
    }

    fn two_pkg_graph(packages: Vec<WorkspacePackage>) -> cabin_workspace::PackageGraph {
        cabin_workspace::PackageGraph {
            root_manifest_path: PathBuf::from("ws/cabin.toml"),
            root_dir: PathBuf::from("ws"),
            is_workspace_root: true,
            root_package: None,
            root_settings: Default::default(),
            primary_packages: (0..packages.len()).collect(),
            default_members: (0..packages.len()).collect(),
            excluded_members: Vec::new(),
            packages,
        }
    }

    #[test]
    fn find_target_returns_executable_even_when_earlier_package_has_same_name_library() {
        // Regression: pkg[0] declares a `cpp_library` named "shared"
        // and pkg[1] declares a `cpp_executable` named "shared".
        // `find_target` must not bail on pkg[0]'s wrong-kind match
        // before reaching pkg[1].
        let graph = two_pkg_graph(vec![
            workspace_package("lib_pkg", vec![target("shared", TargetKind::CppLibrary)]),
            workspace_package("exe_pkg", vec![target("shared", TargetKind::CppExecutable)]),
        ]);
        let chosen = find_target(
            &graph,
            &[0, 1],
            "shared",
            TargetKind::CppExecutable,
            "--bin",
        )
        .expect("an executable candidate exists in pkg[1]");
        assert_eq!(chosen.package_name, "exe_pkg");
        assert_eq!(chosen.target_name, "shared");
    }

    #[test]
    fn find_target_reports_kind_mismatch_when_no_executable_candidate_exists() {
        let graph = two_pkg_graph(vec![workspace_package(
            "lib_pkg",
            vec![target("shared", TargetKind::CppLibrary)],
        )]);
        let err = find_target(&graph, &[0], "shared", TargetKind::CppExecutable, "--bin")
            .expect_err("a library-only match must produce a kind-mismatch error");
        let msg = err.to_string();
        assert!(
            msg.contains("matched a target of kind") && msg.contains("cpp_library"),
            "expected kind-mismatch wording, got: {msg}",
        );
    }

    #[test]
    fn find_target_reports_not_found_when_name_missing() {
        let graph = two_pkg_graph(vec![
            workspace_package("a", vec![target("foo", TargetKind::CppExecutable)]),
            workspace_package("b", vec![target("bar", TargetKind::CppExecutable)]),
        ]);
        let err = find_target(
            &graph,
            &[0, 1],
            "missing",
            TargetKind::CppExecutable,
            "--bin",
        )
        .expect_err("absent target name must produce a not-found error");
        assert!(
            err.to_string().contains("not found"),
            "expected not-found wording, got: {err}",
        );
    }
}
