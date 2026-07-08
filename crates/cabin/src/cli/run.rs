//! Glue layer for `cabin run`.
//!
//! `cabin run` is a thin wrapper over the same build pipeline
//! `cabin build` runs (workspace load → patches → artifact
//! pipeline → planner → Ninja).  After Ninja produces the
//! linked executable, this module locates the file that the
//! planner emitted for the selected target, populates a
//! deterministic `CABIN_*` environment, and execs the binary
//! with the user's stdio attached.  Arguments after `--` are
//! forwarded verbatim.
//!
//! The typed `CABIN_*` env overlay is built by
//! `cabin_env::package_env`; this module only orchestrates.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;
use clap::Args;

use cabin_build::{ManifestTargetSelector, PlanRequest, plan};
use cabin_core::{Package, TargetKind};
use cabin_workspace::{
    RegistryPackageSource, WorkspaceLoadOptions, collect_patched_versioned_deps,
    load_workspace_with_options,
};

use crate::cli::{
    ArtifactPipelineRequest, ToolchainSelectionArgs, WorkspaceSelectionArgs, absolutise,
    build_selection_request, build_workspace_selection,
    closure_has_versioned_deps_excluding_patches, compiler_wrapper_override_from_args,
    compute_feature_resolution, profile_selection_from_flags, resolve_build_configurations,
    resolve_invocation_manifest, run_artifact_pipeline, toolchain_selection_from_args,
    workspace_compiler_wrapper_settings, workspace_profile_definitions,
};

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Build output directory.  Same precedence rules as
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
    /// declared in `[profile.<name>]`).  Defaults to `dev`.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Build and run the named `executable` target.
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

    /// Arguments forwarded to the executed program.  Everything
    /// after `--` is passed verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Run `cabin run`: build and execute a selected binary target.
///
/// Returns an `ExitCode` so the spawned program's exit status
/// becomes Cabin's own exit status.  Failing to start the
/// program (or any pipeline error) surfaces as an
/// [`anyhow::Error`] and Cabin exits non-zero from the
/// top-level dispatcher.
pub(crate) fn run(
    args: &RunArgs,
    reporter: crate::cli::term_verbosity::Reporter,
    color: cabin_core::ColorChoice,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<ExitCode> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;

    let offline = crate::cli::config::effective_offline(args.offline)?;
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let (prepared_ports, initial_graph) = crate::cli::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        args.cache_dir.as_deref(),
        offline,
        args.frozen,
        false,
        &workspace_selection,
        args.no_patches,
    )?;
    crate::cli::port::report_downloaded_ports(reporter, &prepared_ports);
    let port_sources: Vec<cabin_workspace::PortPackageSource> = prepared_ports
        .iter()
        .map(crate::cli::port::workspace_source)
        .collect();
    let effective_config = crate::cli::config::load_effective_config(&initial_graph)?;
    let active_patches =
        crate::cli::patch::load_active_patches(&initial_graph, &effective_config, args.no_patches)?;
    let patched_names = active_patches.owned_patched_names();

    let initial_resolved_selection =
        cabin_workspace::resolve_package_selection(&initial_graph, &workspace_selection)?;

    let initial_request =
        build_selection_request(&args.features, args.all_features, args.no_default_features);
    let dev_for: BTreeSet<String> = BTreeSet::new();
    let initial_features = compute_feature_resolution(
        &initial_graph,
        &initial_resolved_selection,
        &initial_request,
        &dev_for,
    )?;

    let resolved_index_source = crate::cli::config::resolve_index_source(
        args.index_path.as_deref(),
        args.index_url.as_deref(),
        &effective_config,
    )?;
    crate::cli::config::enforce_offline_index_source(offline, resolved_index_source.as_ref())?;
    let resolved_cache_dir =
        crate::cli::config::resolve_cache_dir(args.cache_dir.as_deref(), &effective_config);

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

    let (registry, lockfile_pinned): (Vec<RegistryPackageSource>, BTreeSet<(String, String)>) =
        if has_versioned {
            let Some(index_source) = resolved_index_source.as_ref() else {
                bail!(crate::cli::VERSIONED_DEPS_REQUIRE_INDEX);
            };
            let inputs = crate::cli::config::resolve_pipeline_inputs(
                index_source,
                &effective_config,
                args.cache_dir.as_deref(),
                resolved_cache_dir.as_ref(),
                offline,
                args.locked,
                args.frozen,
                args.no_patches,
                false,
            )?;
            let pipeline = run_artifact_pipeline(&ArtifactPipelineRequest {
                manifest_path: &manifest_path,
                initial_graph: &initial_graph,
                index_path: inputs.index_path.as_deref(),
                index_url: inputs.index_url.as_deref(),
                mode: inputs.mode,
                allow_write: inputs.allow_write,
                frozen: args.frozen,
                cache_dir: &inputs.cache_dir,
                reporter,
                selection: workspace_selection,
                selection_request: &initial_request,
                patched_names: &patched_names,
                active_patches: &active_patches,
                source_replacements: &effective_config.source_replacements,
                incompatible_standards: crate::cli::config::resolve_incompatible_standards(
                    &effective_config,
                )?,
                no_patches: args.no_patches,
                dev_for: &dev_for,
                experimental_features,
            })?;
            (pipeline.registry_sources(), pipeline.lockfile_pinned)
        } else {
            (Vec::new(), BTreeSet::new())
        };

    // Mirror `cabin build`: the strict set is the selection's
    // closure on `initial_graph` plus every patched name plus
    // every resolver-fetched registry package.  Registry packages
    // a patched manifest introduces via a new version dep are not
    // in `initial_graph` (the initial load runs with `registry:
    // &[]`), so the closure misses them; without this extension
    // their missing-registry / missing-port edges silently drop
    // under the scoped policy and the build fails later with a
    // less actionable link error.
    let mut strict_packages: BTreeSet<String> =
        initial_resolved_selection.closure_package_names(&initial_graph);
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

    let (build_dir_input, _build_dir_source) = crate::cli::config::resolve_build_dir_with_env(
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
    // Resolve the selection up-front so the backend checks below scope to
    // the selected closure rather than the whole loaded workspace.
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;
    let selected_closure = resolved_selection.closure(&graph);
    // Package-level approximation used only for the MSVC
    // `/external:I` fallback decision; the authoritative toolchain
    // validation runs against the *planned* compiles right after
    // `plan()` - `cabin run --bin <name>` plans one executable, so
    // an unbuilt sibling target's standard never gates the run.
    let language_standards = crate::cli::resolve_per_package_language_standards(&graph);
    let approx_standards = cabin_build::collect_requested_standards(
        &graph,
        &selected_closure,
        &language_standards,
        &BTreeSet::new(),
    );
    cabin_build::validate_toolchain_for_backend(&toolchain, &detection_report)?;
    let ninja = cabin_toolchain::locate_ninja()?;

    let manifest_compiler_wrapper = workspace_compiler_wrapper_settings(&graph);
    let cli_compiler_wrapper = compiler_wrapper_override_from_args(&args.toolchain)?;

    let profile_selection =
        profile_selection_from_flags(args.profile.as_deref(), args.release, &effective_config)?;
    let manifest_profiles = workspace_profile_definitions(&graph);
    let profile = cabin_core::resolve_profile(&profile_selection, &manifest_profiles)?;
    // The MSVC backend cannot consume pkg-config's GNU-style flags;
    // reject a run that would need them before probing.  Scoped to the
    // selected closure.
    crate::cli::system_deps::ensure_dialect_supports_system_deps(
        &graph,
        &host_platform,
        &dev_for,
        cabin_build::Dialect::from_compiler_kind(detection_report.cxx.identity.kind),
        &selected_closure,
    )?;
    // Resolve features before deriving build flags so
    // `[target.'cfg(feature = "...")'.profile]` layers are gated on
    // the selected feature set.
    let selection_request =
        build_selection_request(&args.features, args.all_features, args.no_default_features);
    let feature_resolution = compute_feature_resolution(
        &graph,
        &resolved_selection,
        &selection_request,
        &BTreeSet::new(),
    )?;
    let prep =
        crate::cli::build_prep::resolve_build_prep(crate::cli::build_prep::BuildConfigInputs {
            graph: &graph,
            host_platform: &host_platform,
            toolchain: &toolchain,
            detection: Some(&detection_report),
            cli_compiler_wrapper,
            manifest_compiler_wrapper: manifest_compiler_wrapper.as_ref(),
            effective_config: &effective_config,
            profile: &profile,
            dev_for: &dev_for,
            feature_resolution: &feature_resolution,
            reporter,
        })?;

    // Pick the run target. `--bin` narrows the search to a
    // named `executable`; otherwise we look for a single
    // buildable `executable` in the selected closure
    // (feature-gated executables are skipped, like the default
    // build enumeration).
    let enabled_features = crate::cli::enabled_features_by_package(&feature_resolution);
    let run_target = pick_run_target(
        &graph,
        &resolved_selection.packages,
        args.bin.as_deref(),
        &enabled_features,
    )?;

    let configurations = resolve_build_configurations(
        &graph,
        &selection_request,
        &resolved_selection.packages,
        &profile,
        &prep.toolchain_summary,
        &prep.build_flags,
    )?;

    let root_configuration = graph
        .root_package
        .and_then(|i| configurations.get(&i))
        .cloned();
    let plan_graph = plan(&PlanRequest {
        graph: &graph,
        toolchain: &toolchain,
        build_flags: &prep.build_flags,
        language_standards: &language_standards,
        standard_flag_conflicts: &prep.standard_flag_conflicts,
        build_dir: build_dir.clone(),
        profile: profile.clone(),
        selected: Some(vec![ManifestTargetSelector {
            package: Some(run_target.package_name.clone()),
            name: run_target.target_name.clone(),
        }]),
        configuration: root_configuration.as_ref(),
        selected_packages: Some(&resolved_selection.packages),
        compiler_wrapper: prep.compiler_wrapper.as_ref(),
        dialect: cabin_build::Dialect::from_compiler_kind(detection_report.cxx.identity.kind),
        msvc_external_includes: cabin_build::msvc_external_includes_supported(
            &detection_report,
            approx_standards.has_c_sources(),
        ),
        enabled_features: Some(&enabled_features),
        standard_compat: true,
    })?;
    crate::cli::standard_compat::report(
        &plan_graph.standard_compat_violations,
        color,
        &lockfile_pinned,
    )?;
    cabin_build::validate_planned_standards(&plan_graph)?;
    cabin_build::validate_toolchain_standards(
        &toolchain,
        &detection_report,
        &cabin_build::requested_standards_of(&plan_graph),
    )?;

    let jobs = crate::cli::config::resolve_build_jobs(args.jobs, &effective_config)?;
    let elapsed =
        crate::cli::ninja::invoke_ninja_and_report(&crate::cli::ninja::NinjaInvocationRequest {
            build_dir: &build_dir,
            profile: &profile,
            plan_graph: &plan_graph,
            graph: &graph,
            toolchain: &toolchain,
            cxx_kind: detection_report.cxx.identity.kind,
            feature_resolution: &feature_resolution,
            dev_for: &dev_for,
            ninja: &ninja,
            jobs,
            reporter,
        })?;

    // Cargo-style `Finished` banner so `cabin run` shows the
    // same build summary as `cabin build` before handing off to
    // the executed program.
    reporter.status(
        "Finished",
        format_args!(
            "`{}` profile [{}] target(s) in {:.2}s",
            profile.name.as_str(),
            crate::cli::profile_descriptor(&profile),
            elapsed.as_secs_f64(),
        ),
    );

    let executable = locate_target_executable(&plan_graph.default_outputs, &run_target)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "build graph did not produce an executable for `{}:{}`",
                run_target.package_name,
                run_target.target_name,
            )
        })?;

    // Build the env.  We do not clear the user's environment -
    // the spawned program inherits PATH, LANG, etc. - but we
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

    // Cargo-style `Running` banner: the executable path shown
    // relative to the invoked manifest's directory (project
    // root) so the line reads like cargo's `Running
    // \`target/debug/foo\``.  Falls back to the absolute path
    // when `--build-dir` places the binary outside the project
    // tree.
    reporter.status(
        "Running",
        format_args!("`{}`", display_run_path(&executable, &manifest_path)),
    );

    // Working directory: mirror Cargo by inheriting the user's
    // current working directory.  Trailing args (`cabin run --
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
/// `ExitCode` cannot represent signal kills directly.  Codes
/// outside `u8` range (Windows reports full 32-bit statuses)
/// collapse to `1` rather than being masked: `256 & 0xff` would
/// report a failing program as success.
fn exit_code_for(status: std::process::ExitStatus) -> ExitCode {
    ExitCode::from(exit_code_byte(status.code()))
}

fn exit_code_byte(code: Option<i32>) -> u8 {
    match code {
        Some(code) => u8::try_from(code).unwrap_or(1),
        None => 1,
    }
}

/// Resolved run target.  The orchestration layer narrows to
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
    enabled_features: &std::collections::HashMap<usize, BTreeSet<String>>,
) -> Result<RunTarget> {
    let pool: Vec<usize> = if selected_packages.is_empty() {
        (0..graph.packages.len()).collect()
    } else {
        selected_packages.to_vec()
    };
    if let Some(name) = bin {
        // Explicit `--bin` requests stay hard errors on gated
        // targets: the planner reports the missing features.
        return find_target(graph, &pool, name, TargetKind::Executable, "--bin");
    }
    // Default: pick a single executable in the selected
    // packages.  Enumeration skips feature-gated executables
    // (like the default build selection); ambiguous selections
    // produce a diagnostic listing every candidate so users can
    // decide.
    let empty_features = BTreeSet::new();
    let mut candidates: Vec<RunTarget> = Vec::new();
    let mut gated: Vec<RunTarget> = Vec::new();
    for &idx in &pool {
        let pkg = &graph.packages[idx];
        let enabled = enabled_features.get(&idx).unwrap_or(&empty_features);
        for target in &pkg.package.targets {
            if target.kind == TargetKind::Executable {
                let run_target = make_run_target(
                    &pkg.package,
                    &pkg.manifest_path,
                    &pkg.manifest_dir,
                    target.name.as_str(),
                );
                if target.missing_required_features(enabled).is_empty() {
                    candidates.push(run_target);
                } else {
                    gated.push(run_target);
                }
            }
        }
    }
    if candidates.is_empty() {
        // Every executable is feature-gated: hand the first one to
        // the planner so the failure names the missing features
        // instead of claiming no executable exists.
        if let Some(first) = gated.into_iter().next() {
            return Ok(first);
        }
        bail!("no `executable` target found in the selected packages; declare one to run it");
    }
    if candidates.len() > 1 {
        let listed: Vec<String> = candidates
            .iter()
            .map(|t| format!("{}:{}", t.package_name, t.target_name))
            .collect();
        bail!(
            "multiple `executable` targets found in the selected packages; pass `--bin <name>` to disambiguate. Candidates: {}",
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
    // mismatch: a `library` named `foo` in pkg A must not
    // mask an `executable` named `foo` in pkg B because
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
                &pkg.manifest_path,
                &pkg.manifest_dir,
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

/// Render the executable path for the `Running` banner so it
/// reads like cargo's `Running target/debug/foo` line - the
/// path relative to the invoked manifest's directory (the
/// "project root" the user sees).  If the executable lives
/// outside that tree (an out-of-tree `--build-dir`), fall back
/// to the absolute path so the line still points at the file
/// being executed.
fn display_run_path(executable: &Path, manifest_path: &Path) -> String {
    manifest_path
        .parent()
        .and_then(|base| executable.strip_prefix(base).ok())
        .map_or_else(
            || executable.display().to_string(),
            |rel| rel.display().to_string(),
        )
}

/// Walk the planner's `default_outputs` looking for the
/// executable produced for `target`.  The planner names every
/// `executable` output
/// `<build_dir>/<profile>/packages/<pkg>/<target>` (no extension
/// on POSIX; `.exe` on Windows).  We scan rather than re-deriving
/// the path so the planner stays the single source of truth.
fn locate_target_executable(
    default_outputs: &[Utf8PathBuf],
    target: &RunTarget,
) -> Option<PathBuf> {
    // The build graph carries UTF-8 paths; the located executable is
    // demoted to a native `PathBuf` here because the caller spawns it
    // through `std::process::Command`.
    let needle_tail: PathBuf = [
        "packages",
        target.package_name.as_str(),
        target.target_name.as_str(),
    ]
    .iter()
    .collect();
    default_outputs
        .iter()
        .find(|p| p.as_std_path().ends_with(&needle_tail))
        .map(|p| p.as_std_path().to_path_buf())
        .or_else(|| {
            // Windows build output appends `.exe`; the
            // unsuffixed needle does not match.  Try matching
            // the parent directory and last component
            // separately.
            let parent_tail: PathBuf = ["packages", target.package_name.as_str()].iter().collect();
            default_outputs.iter().find_map(|p| {
                let std = p.as_std_path();
                let same_parent = std.parent().is_some_and(|pp| pp.ends_with(&parent_tail));
                let stem = std.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if same_parent && stem == target.target_name {
                    Some(std.to_path_buf())
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

    #[test]
    fn exit_code_byte_maps_out_of_range_codes_to_failure() {
        assert_eq!(exit_code_byte(Some(0)), 0);
        assert_eq!(exit_code_byte(Some(3)), 3);
        assert_eq!(exit_code_byte(Some(255)), 255);
        // Windows statuses exceed u8; masking with `& 0xff` would
        // turn 256 into "success".
        assert_eq!(exit_code_byte(Some(256)), 1);
        assert_eq!(exit_code_byte(Some(-1)), 1);
        // Signal-terminated child: no code at all.
        assert_eq!(exit_code_byte(None), 1);
    }

    fn target(name: &str, kind: TargetKind) -> Target {
        Target {
            name: TargetName::new(name).unwrap(),
            kind,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            required_features: Vec::new(),
            language: Default::default(),
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
            is_port: false,
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
        // Regression: pkg[0] declares a `library` named "shared"
        // and pkg[1] declares an `executable` named "shared".
        // `find_target` must not bail on pkg[0]'s wrong-kind match
        // before reaching pkg[1].
        let graph = two_pkg_graph(vec![
            workspace_package("lib_pkg", vec![target("shared", TargetKind::Library)]),
            workspace_package("exe_pkg", vec![target("shared", TargetKind::Executable)]),
        ]);
        let chosen = find_target(&graph, &[0, 1], "shared", TargetKind::Executable, "--bin")
            .expect("an executable candidate exists in pkg[1]");
        assert_eq!(chosen.package_name, "exe_pkg");
        assert_eq!(chosen.target_name, "shared");
    }

    #[test]
    fn find_target_reports_kind_mismatch_when_no_executable_candidate_exists() {
        let graph = two_pkg_graph(vec![workspace_package(
            "lib_pkg",
            vec![target("shared", TargetKind::Library)],
        )]);
        let err = find_target(&graph, &[0], "shared", TargetKind::Executable, "--bin")
            .expect_err("a library-only match must produce a kind-mismatch error");
        let msg = err.to_string();
        assert!(
            msg.contains("matched a target of kind") && msg.contains("library"),
            "expected kind-mismatch wording, got: {msg}",
        );
    }

    #[test]
    fn find_target_reports_not_found_when_name_missing() {
        let graph = two_pkg_graph(vec![
            workspace_package("a", vec![target("foo", TargetKind::Executable)]),
            workspace_package("b", vec![target("bar", TargetKind::Executable)]),
        ]);
        let err = find_target(&graph, &[0, 1], "missing", TargetKind::Executable, "--bin")
            .expect_err("absent target name must produce a not-found error");
        assert!(
            err.to_string().contains("not found"),
            "expected not-found wording, got: {err}",
        );
    }
}
