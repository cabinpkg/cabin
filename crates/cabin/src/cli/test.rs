//! Glue layer for `cabin test`.
//!
//! `cabin test` builds the selected `test` targets through
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

    /// Run only the named `test` target; may be repeated.
    ///
    /// Each name must match a `test` target declared by a
    /// selected package; a name that does not is an error.
    /// Every match across the selected packages runs, so
    /// workspace members may share a test name.
    #[arg(long = "test", value_name = "NAME")]
    pub test: Vec<String>,

    /// Exit successfully when the selected packages declare no
    /// `test` targets. By default, an empty selection errors
    /// so CI does not silently pass when tests have not been
    /// declared yet.
    #[arg(long)]
    pub allow_no_tests: bool,
}

/// Run `cabin test`: build the selected `test` targets,
/// invoke each linked executable in deterministic order, and
/// print a summary. Exits non-zero on any test failure.
pub(crate) fn test(args: &TestArgs, reporter: crate::cli::term_verbosity::Reporter) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;

    // First-pass load with no registry / patches so we can
    // resolve config + workspace selection before re-loading
    // with the test-aware policy.
    let offline = crate::cli::config::effective_offline(args.offline)?;
    // `cabin test` activates `[dev-dependencies]` for the
    // selected test runners; ports referenced from any
    // workspace member's dev-deps must therefore participate in
    // discovery so the second-pass loader can resolve them.
    let test_selection = build_workspace_selection(&args.workspace_selection);
    let (prepared_ports, initial_graph) = crate::cli::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        args.cache_dir.as_deref(),
        offline,
        args.frozen,
        true,
        &test_selection,
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

    let registry: Vec<RegistryPackageSource> = if has_versioned {
        let Some(index_source) = resolved_index_source.as_ref() else {
            bail!(
                "versioned dependencies require --index-path, --index-url, or a `[registry]` config setting"
            );
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
            selection: workspace_selection_for_pipeline,
            selection_request: &initial_request,
            patched_names: &patched_names,
            active_patches: &active_patches,
            source_replacements: &effective_config.source_replacements,
            no_patches: args.no_patches,
            dev_for: &dev_for,
        })?;
        pipeline.registry_sources()
    } else {
        Vec::new()
    };

    // For `cabin test`, the strict set must include every package
    // reachable from the selected test runners *with their
    // dev-dependencies activated* — otherwise a transitive
    // path-dep that only becomes an active graph edge through a
    // dev edge would be missing from the strict set, and its
    // broken port edge would silently drop instead of surfacing
    // the typed `PortDependencyNotPrepared` / `PortDirectoryMissing`
    // diagnostic. `initial_graph` was loaded with
    // `include_dev_for: &BTreeSet::new()`, so its closure misses
    // dev-activated edges. Re-load a permissive dev-aware
    // skeleton with the resolver's full registry + active patches
    // + prepared ports so the closure walk reaches every active
    // edge the upcoming strict load will validate.
    let patched_sources = active_patches.workspace_sources();
    let dev_aware_skeleton = load_workspace_with_options(
        &manifest_path,
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &patched_sources,
            ports: &port_sources,
            registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&BTreeSet::new()),
            include_dev_for: &dev_for,
            port_policy: cabin_workspace::PortPolicy::TolerateExcept(&BTreeSet::new()),
        },
    )?;
    let dev_aware_selection = cabin_workspace::resolve_package_selection(
        &dev_aware_skeleton,
        &build_workspace_selection(&args.workspace_selection),
    )?;
    let mut strict_packages: BTreeSet<String> =
        dev_aware_selection.closure_package_names(&dev_aware_skeleton);
    strict_packages.extend(patched_names.iter().cloned());
    strict_packages.extend(registry.iter().map(|r| r.name.as_str().to_owned()));
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
    cabin_build::validate_toolchain_for_backend(
        &toolchain,
        &detection_report,
        cabin_build::graph_has_c_sources(&graph, &selected_closure),
    )?;
    let ninja = cabin_toolchain::locate_ninja()?;

    let manifest_compiler_wrapper = workspace_compiler_wrapper_settings(&graph);
    let cli_compiler_wrapper = compiler_wrapper_override_from_args(&args.toolchain)?;

    let profile_selection =
        profile_selection_from_flags(args.profile.as_deref(), args.release, &effective_config)?;
    let manifest_profiles = workspace_profile_definitions(&graph);
    let profile = cabin_core::resolve_profile(&profile_selection, &manifest_profiles)
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    // The MSVC backend cannot consume pkg-config's GNU-style flags;
    // reject a test build that would need them before probing. Scoped to
    // the selected closure.
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
    let feature_resolution =
        compute_feature_resolution(&graph, &resolved_selection, &selection_request)?;
    let prep =
        crate::cli::build_prep::resolve_build_prep(crate::cli::build_prep::BuildConfigInputs {
            graph: &graph,
            host_platform: &host_platform,
            toolchain: &toolchain,
            cli_compiler_wrapper,
            manifest_compiler_wrapper: &manifest_compiler_wrapper,
            effective_config: &effective_config,
            profile: &profile,
            dev_for: &dev_for,
            feature_resolution: &feature_resolution,
            reporter,
        })?;

    // Build every test target in the selected packages, narrowed
    // to the requested names when `--test` is given (`--target`
    // stays reserved for a platform/toolchain target). The
    // deselected count feeds the summary's `filtered out` field.
    let all_test_selectors: Vec<ManifestTargetSelector> =
        select_targets_of_kind(&graph, Some(&resolved_selection.packages), TargetKind::Test);
    let total_test_targets = all_test_selectors.len();
    let test_selectors: Vec<ManifestTargetSelector> = if args.test.is_empty() {
        all_test_selectors
    } else {
        select_named_test_targets(
            &graph,
            &resolved_selection.packages,
            &all_test_selectors,
            &args.test,
        )?
    };
    let filtered_out = total_test_targets - test_selectors.len();

    if test_selectors.is_empty() {
        if args.allow_no_tests {
            println!("cabin test: no test targets found");
            return Ok(());
        }
        bail!(
            "no test targets found in the selected packages; declare a `test` target or pass `--allow-no-tests`"
        );
    }

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
        build_dir: build_dir.clone(),
        profile: profile.clone(),
        selected: Some(test_selectors),
        configuration: root_configuration.as_ref(),
        selected_packages: Some(&resolved_selection.packages),
        compiler_wrapper: prep.compiler_wrapper.as_ref(),
        dialect: cabin_build::Dialect::from_compiler_kind(detection_report.cxx.identity.kind),
    })?;

    // `cabin test` builds with Ninja's default parallelism (no
    // `-j`) and prints no `Finished` banner — the test summary is
    // its completion signal — so the returned build duration is
    // unused here.
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
        jobs: None,
        reporter,
    })?;

    // Build → run hand-off. The plan builder reads `test`
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

    // Status lines mirror `cargo test`: a blank line, the
    // `running N tests` header, one `test <pkg>:<target> ... ok`
    // line per executable as it finishes (emitted live by the
    // streaming sink), and a blank-line-separated epilogue with
    // the optional `failures:` recap plus the summary line.
    let mut sink = cabin_test::StreamingSink {
        stdout: std::io::stdout().lock(),
        stderr: std::io::stderr().lock(),
    };
    println!(
        "\nrunning {} test{}",
        test_plan.len(),
        plural(test_plan.len())
    );
    let summary = cabin_test::run_tests(&test_plan, &mut sink)?;
    let _ = writeln!(
        sink.stdout,
        "{}",
        cabin_test::render_epilogue(&summary, filtered_out)
    );
    if !summary.all_passed() {
        bail!(
            "test failures: {} of {} test executables failed",
            summary.failed(),
            summary.total()
        );
    }
    Ok(())
}

/// Resolve the `--test <NAME>` selection: narrow `all` (the full
/// `test`-target enumeration for the selected packages) to the
/// requested names. Every requested name must match a `test`
/// target declared by a selected package; every match across
/// those packages is kept, so two workspace members may run a
/// same-named test in one invocation. Diagnostics mirror
/// `cabin run --bin`: an unknown name and a name that only
/// matches another target kind get distinct messages.
fn select_named_test_targets(
    graph: &cabin_workspace::PackageGraph,
    selected_packages: &[usize],
    all: &[ManifestTargetSelector],
    names: &[String],
) -> Result<Vec<ManifestTargetSelector>> {
    // A `BTreeSet` dedupes repeated `--test` names and keeps the
    // validation order deterministic.
    let requested: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    for &name in &requested {
        if all.iter().any(|sel| sel.name == name) {
            continue;
        }
        // Walk every selected package before deciding which
        // diagnostic to emit: the kind-mismatch message must only
        // fire when the name exists somewhere with another kind.
        let other_kind = selected_packages
            .iter()
            .flat_map(|&idx| graph.packages[idx].package.targets.iter())
            .find(|t| t.name.as_str() == name)
            .map(|t| t.kind);
        if let Some(kind) = other_kind {
            bail!(
                "--test `{name}` matched a target of kind `{}`; expected `test`",
                kind.as_str()
            );
        }
        bail!("--test `{name}` was not found in the selected packages");
    }
    Ok(all
        .iter()
        .filter(|sel| requested.contains(sel.name.as_str()))
        .cloned()
        .collect())
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
    use camino::Utf8PathBuf;

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
            kind: TargetKind::Test,
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
                is_port: false,
            }],
        }
    }

    /// Two-package graph for `--test` selection tests: `alpha`
    /// declares a library plus two tests (one name shared with
    /// `beta`); `beta` declares the shared test plus an
    /// executable.
    fn named_selection_graph() -> PackageGraph {
        let target = |name: &str, kind: TargetKind| Target {
            name: TargetName::new(name).unwrap(),
            kind,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
        };
        let alpha = Package::new(
            PackageName::new("alpha").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            vec![
                target("alpha", TargetKind::Library),
                target("alpha_test", TargetKind::Test),
                target("shared_test", TargetKind::Test),
            ],
            Vec::new(),
        )
        .unwrap();
        let beta = Package::new(
            PackageName::new("beta").unwrap(),
            semver::Version::parse("0.1.0").unwrap(),
            vec![
                target("shared_test", TargetKind::Test),
                target("beta_tool", TargetKind::Executable),
            ],
            Vec::new(),
        )
        .unwrap();
        let member = |package: Package, dir: &str| WorkspacePackage {
            package,
            manifest_path: PathBuf::from(format!("{dir}/cabin.toml")),
            manifest_dir: PathBuf::from(dir),
            deps: Vec::new(),
            kind: PackageKind::Local,
            is_port: false,
        };
        PackageGraph {
            root_manifest_path: PathBuf::from("ws/cabin.toml"),
            root_dir: PathBuf::from("ws"),
            is_workspace_root: true,
            root_package: None,
            root_settings: Default::default(),
            primary_packages: vec![0, 1],
            default_members: vec![0, 1],
            excluded_members: Vec::new(),
            packages: vec![member(alpha, "ws/alpha"), member(beta, "ws/beta")],
        }
    }

    /// Enumerate the selection's `test` targets the way the
    /// command does, then apply the `--test` name filter.
    fn named_selection(
        graph: &PackageGraph,
        selected: &[usize],
        names: &[String],
    ) -> Result<Vec<ManifestTargetSelector>> {
        let all = select_targets_of_kind(graph, Some(selected), TargetKind::Test);
        select_named_test_targets(graph, selected, &all, names)
    }

    #[test]
    fn named_selection_keeps_every_match_across_packages() {
        let graph = named_selection_graph();
        let selectors = named_selection(&graph, &[0, 1], &["shared_test".to_owned()]).unwrap();
        let got: Vec<(Option<&str>, &str)> = selectors
            .iter()
            .map(|s| (s.package.as_deref(), s.name.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (Some("alpha"), "shared_test"),
                (Some("beta"), "shared_test")
            ]
        );
    }

    #[test]
    fn named_selection_filters_to_requested_names_and_dedupes() {
        let graph = named_selection_graph();
        let selectors = named_selection(
            &graph,
            &[0, 1],
            &["alpha_test".to_owned(), "alpha_test".to_owned()],
        )
        .unwrap();
        assert_eq!(selectors.len(), 1);
        assert_eq!(selectors[0].package.as_deref(), Some("alpha"));
        assert_eq!(selectors[0].name, "alpha_test");
    }

    #[test]
    fn named_selection_unknown_name_errors() {
        let graph = named_selection_graph();
        let err = named_selection(&graph, &[0, 1], &["missing_test".to_owned()])
            .expect_err("unknown name must error");
        assert_eq!(
            err.to_string(),
            "--test `missing_test` was not found in the selected packages"
        );
    }

    #[test]
    fn named_selection_kind_mismatch_errors() {
        let graph = named_selection_graph();
        let err = named_selection(&graph, &[0, 1], &["beta_tool".to_owned()])
            .expect_err("non-test target kind must error");
        assert_eq!(
            err.to_string(),
            "--test `beta_tool` matched a target of kind `executable`; expected `test`"
        );
    }

    #[test]
    fn named_selection_respects_package_selection() {
        let graph = named_selection_graph();
        // `alpha_test` exists in the graph but not in the selected
        // package set; the lookup must not see it.
        let err = named_selection(&graph, &[1], &["alpha_test".to_owned()])
            .expect_err("name outside the selected packages must error");
        assert_eq!(
            err.to_string(),
            "--test `alpha_test` was not found in the selected packages"
        );
    }

    #[test]
    fn populate_test_env_overlay_errors_when_package_missing_from_graph() {
        let graph = test_graph();
        let build_graph = BuildGraph {
            actions: Vec::new(),
            dialect: cabin_build::Dialect::GnuLike,
            default_outputs: vec![Utf8PathBuf::from("build/dev/packages/demo/demo_test")],
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
