//! Glue layer for `cabin test`.
//!
//! `cabin test` builds the selected `cpp_test` targets through
//! the same pipeline as `cabin build` (workspace load → artifact
//! pipeline → planner → Ninja → invoke ninja), then hands the
//! resulting [`cabin_build::BuildGraph`] to
//! [`cabin_test::run_tests`] which spawns each test executable
//! and reports a deterministic summary.
//!
//! This module owns only the orchestration. Test planning and
//! test execution live in the dedicated `cabin-test` crate;
//! workspace loading, dependency resolution, build planning, and
//! Ninja generation live in their respective crates. The CLI
//! layer threads typed values between them.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use cabin_build::{ManifestTargetSelector, PlanRequest, plan, select_targets_of_kind};
use cabin_core::TargetKind;
use cabin_workspace::{
    RegistryPackageSource, WorkspaceLoadOptions, collect_patched_versioned_deps, load_workspace,
    load_workspace_with_options,
};

use crate::cli::{
    ArtifactPipelineRequest, ToolchainSelectionArgs, WorkspaceSelectionArgs, absolutise,
    build_selection_request, build_workspace_selection, cache_dir_for,
    closure_has_versioned_deps_excluding_patches, compiler_wrapper_override_from_args,
    compute_feature_resolution, lock_mode_for_flags, profile_selection_from_flags,
    resolve_build_configurations, resolve_invocation_manifest, resolve_per_package_build_flags,
    run_artifact_pipeline, toolchain_selection_from_args, workspace_compiler_wrapper_settings,
    workspace_profile_definitions,
};
use crate::plural;

/// `cabin test` arguments. Subset of `BuildArgs` plus a few
/// test-specific knobs. Mutually exclusive flags are enforced by
/// `clap`.
#[derive(Debug, Args)]
pub(crate) struct TestArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Directory for build outputs (build.ninja, object files,
    /// linked test executables). Defaults to `build`.
    #[arg(long, value_name = "PATH")]
    pub build_dir: Option<PathBuf>,

    /// Build with optimizations.
    ///
    /// Compatibility alias for `--profile release`; cannot be
    /// used together with `--profile`.
    #[arg(short = 'r', long, conflicts_with = "profile")]
    pub release: bool,

    /// Build profile (`dev`, `release`, or any custom profile
    /// declared in `[profile.<name>]`). Defaults to `dev` —
    /// the same default as `cabin build` so test runs match the
    /// developer's working profile.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Path to a directory containing the local JSON package
    /// index. Required when the test build closure has any
    /// versioned dependency and `--index-url` is not given.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Sparse HTTP index URL.
    #[arg(long, value_name = "URL")]
    pub index_url: Option<String>,

    /// Override the default artifact cache directory.
    #[arg(long, value_name = "PATH")]
    pub cache_dir: Option<PathBuf>,

    /// Require an existing, current `cabin.lock`.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects state-writing side
    /// effects.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access. Combine with `cabin vendor` to run
    /// `cabin test` against a self-contained local index.
    #[arg(long)]
    pub offline: bool,

    /// Enable named features for the selected packages.
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Enable every feature declared by selected packages.
    #[arg(long)]
    pub all_features: bool,

    /// Disable each selected package's default features.
    #[arg(long)]
    pub no_default_features: bool,

    /// Workspace package-selection flags.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Toolchain-selection flags.
    #[command(flatten)]
    pub toolchain: ToolchainSelectionArgs,

    /// Disable every active patch and source-replacement entry
    /// for this invocation.
    #[arg(long)]
    pub no_patches: bool,

    /// Exit successfully when the selected packages declare no
    /// `cpp_test` targets. By default, an empty selection errors
    /// so CI does not silently pass when tests have not been
    /// declared yet.
    #[arg(long)]
    pub allow_no_tests: bool,
}

/// Run `cabin test`: build the selected `cpp_test` targets,
/// invoke each linked executable in deterministic order, and
/// print a summary. Exits non-zero on any test failure.
pub(crate) fn test(args: &TestArgs, reporter: crate::term_verbosity_glue::Reporter) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;

    // First-pass load with no registry / patches so we can
    // resolve config + workspace selection before re-loading
    // with the test-aware policy.
    let initial_graph = load_workspace(&manifest_path)?;
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

    // Activate dev-deps for the *selected* primary packages so
    // their `[dev-dependencies]` reach the resolver / fetch
    // pipeline. Dev-deps never propagate transitively.
    let dev_for: BTreeSet<String> = initial_resolved_selection
        .packages
        .iter()
        .map(|i| initial_graph.packages[*i].package.name.as_str().to_owned())
        .collect();

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
    let offline = crate::config_glue::effective_offline(args.offline)?;
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
            None => cache_dir_for(&manifest_path, args.cache_dir.as_deref())?,
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

    // Re-load with registry + patches + dev-dep activation so
    // the planner sees every test-only dep edge.
    let strict_packages: BTreeSet<String> = initial_resolved_selection
        .closure(&initial_graph)
        .into_iter()
        .map(|i| initial_graph.packages[i].package.name.as_str().to_owned())
        .collect();
    let patched_sources = active_patches.workspace_sources();
    let graph = load_workspace_with_options(
        &manifest_path,
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &patched_sources,
            strict_packages: &strict_packages,
            include_dev_for: &dev_for,
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
    let (build_flags, _system_dep_reports) =
        crate::system_deps_glue::augment_build_flags_with_system_deps(
            &graph,
            &host_platform,
            &dev_for,
            build_flags,
            reporter,
        )?;
    let (build_flags, _env_build_flags) = crate::env_flags_glue::augment_build_flags_with_env(
        &graph,
        build_flags,
        |k| std::env::var_os(k),
        reporter,
    )?;

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

    // Build every `cpp_test` in the selected packages.
    // Single-test selection is reserved for a future explicit-kind
    // flag (`--target` is reserved for a platform/toolchain target).
    let test_selectors: Vec<ManifestTargetSelector> = select_targets_of_kind(
        &graph,
        Some(&resolved_selection.packages),
        TargetKind::CppTest,
    );

    if test_selectors.is_empty() {
        if args.allow_no_tests {
            println!("cabin test: no test targets found");
            return Ok(());
        }
        bail!(
            "no test targets found in the selected packages; declare a `cpp_test` target or pass `--allow-no-tests`"
        );
    }

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
    let _feature_resolution =
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
        selected: Some(test_selectors),
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

    // Implementation-detail status is verbose-only: under `-v`
    // the user sees which files Cabin wrote and how Ninja was
    // invoked, alongside Ninja's own raw banner.
    reporter.verbose(format_args!("cabin: wrote {}", ninja_file.display()));
    reporter.verbose(format_args!("cabin: wrote {}", ccmd_file.display()));
    reporter.verbose(format_args!(
        "cabin: invoking {} -C {}",
        ninja.display(),
        profile_build_root.display()
    ));
    let mut ninja_cmd = std::process::Command::new(&ninja);
    // Route Ninja through the shared runner so `cabin test`'s
    // build phase prints the same cargo-style `Compiling …`
    // banner `cabin build` emits — and so the verbose passthrough
    // and the default-mode filtering stay in one place.
    let status = crate::cli::run_ninja(
        ninja_cmd.arg("-C").arg(&profile_build_root),
        reporter,
        &graph,
    )
    .with_context(|| format!("failed to invoke ninja at {}", ninja.display()))?;
    if !status.success() {
        bail!("ninja exited with {status}");
    }

    // Build → run hand-off. The plan builder reads `cpp_test`
    // targets out of the graph and aligns them with the
    // `default_outputs` the planner emitted, so empty
    // `default_outputs` produce a clear error rather than a
    // silent no-op.
    let mut test_plan =
        cabin_test::plan_tests(&graph, &plan_graph, Some(&resolved_selection.packages));
    populate_test_env_overlay(&mut test_plan, &graph, &profile, &build_dir)?;
    if test_plan.is_empty() {
        if args.allow_no_tests {
            println!("cabin test: no test targets found");
            return Ok(());
        }
        bail!("no test targets were produced by the build graph; pass `--allow-no-tests` to skip");
    }

    let mut sink = cabin_test::StreamingSink {
        stdout: std::io::stdout().lock(),
        stderr: std::io::stderr().lock(),
    };
    println!(
        "running {} test{}",
        test_plan.len(),
        plural(test_plan.len())
    );
    for executable in &test_plan {
        // Per-test "running" line goes out before output streams
        // so multi-test runs are easy to scan.
        let _ = writeln!(
            sink.stdout,
            "{}",
            cabin_test::render_running_line(executable)
        );
    }
    let summary = cabin_test::run_tests(&test_plan, &mut sink)?;
    for result in &summary.results {
        let _ = writeln!(sink.stdout, "{}", cabin_test::render_result_line(result));
    }
    let _ = writeln!(sink.stdout, "{}", cabin_test::render_summary_line(&summary));
    if !summary.all_passed() {
        bail!(
            "test failures: {} of {} test executables failed",
            summary.failed(),
            summary.total()
        );
    }
    Ok(())
}

/// Walk every executable in `plan` and attach the typed
/// `CABIN_*` package-execution overlay produced by
/// [`cabin_env::package_env`]. The overlay is layered on top of
/// the inherited environment at runtime; PATH and friends remain
/// intact so test executables can still find shared system
/// tools. The only fallible step is mapping each executable back
/// to its workspace package.
fn populate_test_env_overlay(
    plan: &mut cabin_test::TestPlan,
    graph: &cabin_workspace::PackageGraph,
    profile: &cabin_core::ResolvedProfile,
    build_dir: &std::path::Path,
) -> Result<()> {
    let mut failure = None;
    plan.for_each_executable_mut(|exe| {
        if failure.is_some() {
            return;
        }
        let Some(idx) = graph.index_of(exe.package.as_str()) else {
            failure = Some(anyhow::anyhow!(
                "failed to build test env for `{}:{}`: package is not present in the workspace graph",
                exe.package,
                exe.target
            ));
            return;
        };
        let pkg = &graph.packages[idx];
        exe.env = cabin_env::package_env(&cabin_env::PackageEnvInputs {
            manifest_dir: pkg.manifest_dir.as_path(),
            manifest_path: pkg.manifest_path.as_path(),
            package_name: pkg.package.name.as_str(),
            package_version: &pkg.package.version.to_string(),
            profile: profile.name.as_str(),
            build_dir,
        });
    });
    if let Some(err) = failure {
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use cabin_build::BuildGraph;
    use cabin_core::{
        Package, PackageName, ProfileDefinition, ProfileName, ProfileSelection, Target, TargetKind,
        TargetName, resolve_profile,
    };
    use cabin_workspace::{PackageGraph, PackageKind, WorkspacePackage};

    fn dev_profile() -> cabin_core::ResolvedProfile {
        resolve_profile(
            &ProfileSelection::default_dev(),
            &BTreeMap::<ProfileName, ProfileDefinition>::new(),
        )
        .expect("built-in dev profile resolves")
    }

    fn test_graph() -> PackageGraph {
        let target = Target {
            name: TargetName::new("demo_test").unwrap(),
            kind: TargetKind::CppTest,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
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
            }],
        }
    }

    #[test]
    fn populate_test_env_overlay_errors_when_package_missing_from_graph() {
        let graph = test_graph();
        let build_graph = BuildGraph {
            actions: Vec::new(),
            default_outputs: vec![PathBuf::from("build/dev/packages/demo/demo_test")],
            compile_commands: Vec::new(),
        };
        let mut plan = cabin_test::plan_tests(&graph, &build_graph, Some(&[0]));
        assert_eq!(plan.len(), 1);
        // Detach the executable from any workspace package so the
        // only remaining fallible step (graph lookup) trips.
        plan.for_each_executable_mut(|exe| exe.package.clear());

        let err = populate_test_env_overlay(&mut plan, &graph, &dev_profile(), Path::new("build"))
            .expect_err("an executable with no owning package must be surfaced");

        assert!(
            err.to_string().contains("failed to build test env"),
            "unexpected error: {err}"
        );
    }
}
