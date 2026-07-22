//! Shared build-configuration preamble for the commands that resolve
//! a [`cabin_core::BuildConfiguration`] per package: `build`, `run`,
//! `test`, and `cabin explain build-config`.
//!
//! Each of those commands derives, from an already-resolved toolchain
//! and profile, the per-package build flags and the compiler
//! wrapper, then folds them into a [`cabin_core::ToolchainSummary`].
//! [`resolve_build_prep`] is the single home for that fail-hard
//! sequence so a change to how those inputs are assembled lands in one
//! place.  The caller keeps its own `resolve_build_configurations` call
//! (it needs the per-command package selection, which is computed at
//! the call site) and threads [`BuildPrep`] into it.
//!
//! Scope: [`resolve_build_prep`] owns only the part *after* the
//! caller has resolved (and, for the building commands, detected /
//! validated) the toolchain and chosen the profile.
//! [`prepare_workspace`] + [`plan_prepared`] extend the pipeline for
//! the three building commands (`build` / `run` / `test`): they own
//! the whole shared preamble - workspace resolution, the artifact
//! pipeline, toolchain, profile, features, planning, and the
//! post-plan standards gates - parameterized only on
//! [`DevActivation`]; command-specific target selection and
//! execution stay at the call sites.  `cabin metadata` is
//! intentionally not a caller: it uses the fail-*soft* wrapper path
//! (`resolve_compiler_wrapper`) and must keep it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use cabin_workspace::PackageGraph;

use crate::cli::term_verbosity::Reporter;

/// Inputs to [`resolve_build_prep`].  The caller supplies the
/// already-resolved toolchain and profile; the helper owns the flag /
/// wrapper / summary derivation.
pub(crate) struct BuildConfigInputs<'a> {
    pub graph: &'a cabin_workspace::PackageGraph,
    pub host_platform: &'a cabin_core::TargetPlatform,
    pub toolchain: &'a cabin_core::ResolvedToolchain,
    /// Toolchain detection report.  `Some` for the building commands
    /// (fail-hard detection already ran); `None` only when a
    /// fail-soft caller could not detect - compiler cfg conditions
    /// then evaluate as `unknown`.
    pub detection: Option<&'a cabin_core::ToolchainDetectionReport>,
    /// `--compiler-wrapper` / `--no-compiler-wrapper` override, already
    /// parsed from the command's toolchain args.
    pub cli_compiler_wrapper: Option<cabin_core::CompilerWrapperRequest>,
    pub manifest_compiler_wrapper: Option<&'a cabin_core::CompilerWrapperRequest>,
    pub effective_config: &'a cabin_config::EffectiveConfig,
    pub profile: &'a cabin_core::ResolvedProfile,
    pub dev_for: &'a BTreeSet<String>,
    /// Resolved features for the selected closure.  Gates each
    /// package's `[target.'cfg(feature = "...")'.profile]` flag
    /// layers; must be computed before this preamble so the build
    /// flags observe the selected feature set.
    pub feature_resolution: &'a cabin_feature::FeatureResolution,
    pub reporter: Reporter,
}

/// The resolved per-package flags, compiler wrapper, and the
/// toolchain summary they fold into.  The caller feeds
/// `toolchain_summary` + `build_flags` into
/// `resolve_build_configurations`, and threads `build_flags` +
/// `compiler_wrapper` into the planner.
pub(crate) struct BuildPrep {
    pub build_flags: HashMap<usize, cabin_core::ResolvedProfileFlags>,
    /// Standard-flag conflict candidates per package, detected on
    /// the pre-augmentation manifest flags.  Threaded into
    /// `PlanRequest` so the planner can record violations for the
    /// compiles each candidate's scope covers.
    pub standard_flag_conflicts: HashMap<usize, Vec<cabin_core::StandardFlagConflict>>,
    pub compiler_wrapper: Option<cabin_core::ResolvedCompilerWrapper>,
    pub toolchain_summary: cabin_core::ToolchainSummary,
}

/// Resolve per-package build flags and the compiler wrapper, and
/// fold them into a [`cabin_core::ToolchainSummary`].
///
/// Flag augmentation may emit reporter warnings; it runs at the same
/// point the inlined preamble ran (before the caller's package
/// selection / `resolve_build_configurations`), so the surfaced output
/// is unchanged.  Wrapper resolution is silent on success and fatal on
/// failure (a misbehaving wrapper never silently bypasses caching).
#[allow(clippy::needless_pass_by_value)] // consumed: `cli_compiler_wrapper` is moved into the wrapper resolver
pub(crate) fn resolve_build_prep(inputs: BuildConfigInputs) -> Result<BuildPrep> {
    let (build_flags, standard_flag_conflicts) = resolve_per_package_build_flags(
        inputs.graph,
        inputs.profile,
        inputs.host_platform,
        inputs.feature_resolution,
        inputs.detection,
    );
    let build_flags = augment_build_flags(
        inputs.graph,
        inputs.host_platform,
        inputs.dev_for,
        build_flags,
        inputs.reporter,
    )?;
    let compiler_wrapper = resolve_compiler_wrapper_layered(
        inputs.cli_compiler_wrapper,
        inputs.manifest_compiler_wrapper,
        inputs.effective_config,
    )?;
    let toolchain_summary = cabin_core::ToolchainSummary::from_resolved_parts(
        inputs.toolchain,
        compiler_wrapper.as_ref(),
    );
    Ok(BuildPrep {
        build_flags,
        standard_flag_conflicts,
        compiler_wrapper,
        toolchain_summary,
    })
}

/// Which packages' `[dev-dependencies]` a command activates.
#[derive(Clone, Copy)]
pub(crate) enum DevActivation {
    /// `cabin build` / `cabin run`: dev deps stay declaration-only,
    /// matching the dev-dep activation rule.
    Disabled,
    /// `cabin test`: activate dev deps for the *selected* primary
    /// packages so their `[dev-dependencies]` reach the resolver /
    /// fetch pipeline.  Dev-deps never propagate transitively.
    SelectedPrimaries,
}

/// The CLI inputs `BuildArgs` / `RunArgs` / `TestArgs` share
/// verbatim, borrowed for one [`prepare_workspace`] call.
/// Command-specific knobs (`--jobs`, `--bin`, `--test`,
/// `--allow-no-tests`, trailing program args) stay on the caller.
pub(crate) struct WorkspacePipelineArgs<'a> {
    pub manifest_path: Option<&'a Path>,
    pub offline: bool,
    pub cache_dir: Option<&'a Path>,
    pub build_dir: Option<&'a Path>,
    pub locked: bool,
    pub frozen: bool,
    pub no_patches: bool,
    pub features: &'a [String],
    pub all_features: bool,
    pub no_default_features: bool,
    pub index_path: Option<&'a Path>,
    pub index_url: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub release: bool,
    pub workspace_selection: &'a super::WorkspaceSelectionArgs,
    pub toolchain: &'a super::ToolchainSelectionArgs,
    pub dev: DevActivation,
}

/// Everything the shared preamble produces that the command tails
/// (target selection, planning, Ninja invocation, execution)
/// consume.
pub(crate) struct PreparedWorkspace {
    pub manifest_path: PathBuf,
    pub effective_config: cabin_config::EffectiveConfig,
    pub graph: cabin_workspace::PackageGraph,
    pub resolved_selection: cabin_workspace::ResolvedSelection,
    pub selection_request: cabin_core::SelectionRequest,
    pub feature_resolution: cabin_feature::FeatureResolution,
    pub enabled_features: HashMap<usize, BTreeSet<String>>,
    pub profile: cabin_core::ResolvedProfile,
    pub toolchain: cabin_core::ResolvedToolchain,
    pub detection_report: cabin_core::ToolchainDetectionReport,
    pub language_standards: HashMap<usize, cabin_core::ResolvedLanguageStandards>,
    pub approx_standards: cabin_build::RequestedStandards,
    pub prep: BuildPrep,
    pub build_dir: PathBuf,
    pub ninja: PathBuf,
    pub lockfile_pinned: BTreeSet<(String, String)>,
    pub dev_for: BTreeSet<String>,
}

/// The shared `build` / `run` / `test` preamble: workspace
/// resolution, the artifact pipeline, the strict re-load, toolchain
/// resolution + detection + validation, profile choice, feature
/// resolution, and [`resolve_build_prep`] - everything up to (but
/// not including) command-specific target selection and planning.
///
/// The only per-command policy is [`DevActivation`]: with dev deps
/// activated the pipeline adds the two extra dev-aware loads
/// `cabin test` needs (the pre-resolution probe graph and the
/// strict-set skeleton); with them disabled those loads collapse to
/// the initial graph and the pipeline is exactly the `build` / `run`
/// preamble.
pub(crate) fn prepare_workspace(
    args: &WorkspacePipelineArgs<'_>,
    reporter: Reporter,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<PreparedWorkspace> {
    let manifest_path = super::resolve_invocation_manifest(args.manifest_path)?;

    // First-pass load: needed to detect versioned dependencies
    // before we know whether we have to fetch anything.  This load
    // also surfaces manifest / workspace errors before we touch
    // the index.  Under dev activation, ports referenced from any
    // workspace member's dev-deps must participate in discovery so
    // the second-pass loader can resolve them.
    let offline = super::config::effective_offline(args.offline)?;
    let workspace_selection = super::build_workspace_selection(args.workspace_selection);
    let include_dev = matches!(args.dev, DevActivation::SelectedPrimaries);
    let super::port::WorkspacePrep {
        port_sources,
        effective_config,
        // Patched names are excluded from the closure / artifact
        // pipeline because they ship from a local working copy.
        active_patches,
        graph: initial_graph,
        ..
    } = super::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        args.cache_dir,
        offline,
        args.frozen,
        include_dev,
        &workspace_selection,
        args.no_patches,
        Some(reporter),
    )?;
    let patched_names = active_patches.owned_patched_names();
    let resolved_index_source =
        super::config::resolve_index_source(args.index_path, args.index_url, &effective_config)?;
    super::config::enforce_offline_index_source(offline, resolved_index_source.as_ref())?;
    let resolved_cache_dir = super::config::resolve_cache_dir(args.cache_dir, &effective_config);

    // only the *selected closure* drives the index
    // requirement.  An unrelated workspace member's versioned dep
    // must not force the user to pass `--index-path` when
    // `cabin build -p selected` is run on a C/C++-only selection.
    let initial_resolved_selection =
        cabin_workspace::resolve_package_selection(&initial_graph, &workspace_selection)?;
    let initial_request =
        super::build_selection_request(args.features, args.all_features, args.no_default_features);
    let dev_for: BTreeSet<String> = match args.dev {
        DevActivation::Disabled => BTreeSet::new(),
        DevActivation::SelectedPrimaries => initial_resolved_selection
            .packages
            .iter()
            .map(|i| initial_graph.packages[*i].package.name.as_str().to_owned())
            .collect(),
    };
    let patched_sources = active_patches.workspace_sources();

    // `initial_graph` is loaded without dev edges, so a closure
    // walk over it cannot reach packages that only become
    // reachable through a dev edge - and their *normal* versioned
    // deps would silently miss the resolver.  Under dev
    // activation, re-load a pre-resolution dev-aware skeleton (no
    // registry yet, tolerant policies) and drive the versioned-dep
    // detection and the artifact pipeline from it, so e.g. a dev
    // path dep's own registry dependency resolves and fetches like
    // any other.
    let dev_probe: Option<(
        cabin_workspace::PackageGraph,
        cabin_workspace::ResolvedSelection,
    )> = if include_dev {
        let dev_probe_graph = cabin_workspace::load_workspace_with_options(
            &manifest_path,
            &cabin_workspace::WorkspaceLoadOptions {
                registry: &[],
                patches: &patched_sources,
                ports: &port_sources,
                registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&BTreeSet::new()),
                include_dev_for: &dev_for,
                port_policy: cabin_workspace::PortPolicy::TolerateExcept(&BTreeSet::new()),
            },
        )?;
        let probe_selection =
            cabin_workspace::resolve_package_selection(&dev_probe_graph, &workspace_selection)?;
        Some((dev_probe_graph, probe_selection))
    } else {
        None
    };
    let (probe_graph, probe_selection) = match &dev_probe {
        Some((graph, selection)) => (graph, selection),
        None => (&initial_graph, &initial_resolved_selection),
    };

    let initial_features = super::compute_feature_resolution(
        probe_graph,
        probe_selection,
        &initial_request,
        &dev_for,
    )?;
    let patched_root_deps_preview =
        super::collect_patched_versioned_deps(&active_patches, &patched_names)?;
    let has_versioned = !patched_root_deps_preview.is_empty()
        || super::closure_has_versioned_deps_excluding_patches(
            probe_graph,
            probe_selection,
            &initial_features,
            &patched_names,
            &dev_for,
        );

    let (registry, lockfile_pinned): (
        Vec<super::RegistryPackageSource>,
        BTreeSet<(String, String)>,
    ) = if has_versioned {
        let Some(index_source) = resolved_index_source.as_ref() else {
            bail!(super::VERSIONED_DEPS_REQUIRE_INDEX);
        };
        let inputs = super::config::resolve_pipeline_inputs(
            index_source,
            &effective_config,
            args.cache_dir,
            resolved_cache_dir.as_ref(),
            offline,
            args.locked,
            args.frozen,
            args.no_patches,
            false,
        )?;
        let pipeline = super::run_artifact_pipeline(&super::ArtifactPipelineRequest {
            manifest_path: &manifest_path,
            initial_graph: probe_graph,
            index_source: &inputs.index_source,
            policy: inputs.policy,
            cache_dir: &inputs.cache_dir,
            reporter,
            selection: super::build_workspace_selection(args.workspace_selection),
            selection_request: &initial_request,
            patched_names: &patched_names,
            active_patches: &active_patches,
            source_replacements: &effective_config.source_replacements,
            incompatible_standards: super::config::resolve_incompatible_standards(
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

    // Re-load the workspace, this time stitching in the resolved
    // registry packages plus active patches.  When both lists are
    // empty this is identical to the first-pass load.
    //
    // `strict_packages` controls which packages require their
    // versioned / port deps to be satisfied.  The set is the
    // selection's closure plus every package the resolver fetched
    // into `registry`.  The closure alone misses any package
    // reached only after resolution - in particular, transitive
    // registry packages a patched manifest pulled in via a version
    // dep that did not exist on the upstream package.  Without the
    // registry extension those packages would parent a
    // missing-registry / missing-port edge under the scoped policy
    // and silently drop it, leaving the build to fail later with a
    // less actionable diagnostic.  `patched_names` is folded in
    // defensively too - closure already reaches the patched
    // manifests now, but the explicit add keeps the strict set
    // correct if anything in the chicken-and-egg loading order
    // ever shifts.
    //
    // Under dev activation the strict set must additionally cover
    // every package reachable from the selected test runners *with
    // their dev-dependencies activated* - otherwise a transitive
    // path-dep that only becomes an active graph edge through a
    // dev edge would be missing from the strict set, and its
    // broken port edge would silently drop instead of surfacing
    // the typed `PortDependencyNotPrepared` / `PortDirectoryMissing`
    // diagnostic.  The pre-resolution probe graph carries dev
    // edges but not the resolver's registry, so the closure walks
    // a permissive dev-aware skeleton loaded with the full
    // registry + active patches + prepared ports instead.
    let mut strict_packages: BTreeSet<String> = if include_dev {
        let dev_aware_skeleton = cabin_workspace::load_workspace_with_options(
            &manifest_path,
            &cabin_workspace::WorkspaceLoadOptions {
                registry: &registry,
                patches: &patched_sources,
                ports: &port_sources,
                registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&BTreeSet::new()),
                include_dev_for: &dev_for,
                port_policy: cabin_workspace::PortPolicy::TolerateExcept(&BTreeSet::new()),
            },
        )?;
        let dev_aware_selection =
            cabin_workspace::resolve_package_selection(&dev_aware_skeleton, &workspace_selection)?;
        dev_aware_selection.closure_package_names(&dev_aware_skeleton)
    } else {
        initial_resolved_selection.closure_package_names(&initial_graph)
    };
    strict_packages.extend(patched_names.iter().cloned());
    strict_packages.extend(registry.iter().map(|r| r.name.as_str().to_owned()));
    let graph = cabin_workspace::load_workspace_with_options(
        &manifest_path,
        &cabin_workspace::WorkspaceLoadOptions {
            registry: &registry,
            patches: &patched_sources,
            ports: &port_sources,
            registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&strict_packages),
            include_dev_for: &dev_for,
            port_policy: cabin_workspace::PortPolicy::TolerateExcept(&strict_packages),
        },
    )?;

    // Resolve the build directory.  Precedence:
    // `--build-dir` > `CABIN_BUILD_DIR` env var
    // > `[paths] build-dir` config setting > built-in default.
    let (build_dir_input, _build_dir_source) =
        super::config::resolve_build_dir_with_env(args.build_dir, &effective_config);
    let build_dir = super::absolutise(&build_dir_input)
        .with_context(|| format!("failed to resolve build dir {}", build_dir_input.display()))?;

    let host_platform = cabin_core::TargetPlatform::current();
    let toolchain_selection = super::toolchain_selection_from_args(args.toolchain)?;
    let toolchain = super::resolve_toolchain_layered(
        &graph,
        &toolchain_selection,
        &effective_config,
        &host_platform,
    )?;
    // Detect compiler / archiver identity and validate that the
    // backend's required capabilities (GCC-style flags, depfile
    // emission, `-std=c++17`, ar-compatible archiving) are
    // available before any Ninja file is written.  Fail fast and
    // clear here rather than letting Ninja produce a confusing
    // error from a broken command line.
    let detection_report =
        cabin_toolchain::detect_toolchain(&toolchain, &cabin_toolchain::ProcessRunner)?;
    // Resolve the workspace package selection up-front.  The planner
    // consumes the selected indices through `PlanRequest::selected_packages`
    // so default-target enumeration narrows to the picked packages instead
    // of every primary - and the backend checks below scope to the
    // selected closure so an unselected member's C source or pkg-config
    // dependency never gates the command.
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;
    let selected_closure = resolved_selection.closure(&graph);
    // Package-level approximation used only for the MSVC
    // `/external:I` fallback decision; the authoritative toolchain
    // validation runs against the *planned* compiles right after
    // `plan()`, so an unbuilt sibling target's standard never gates
    // the command.  Dev-only targets participate exactly when dev
    // deps are activated (`cabin test` builds them; `cabin build` /
    // `run` do not).
    let language_standards = super::resolve_per_package_language_standards(&graph);
    let approx_standards = cabin_build::collect_requested_standards(
        &graph,
        &selected_closure,
        &language_standards,
        &dev_for,
    );
    cabin_build::validate_toolchain_for_backend(&toolchain, &detection_report)?;
    let ninja = cabin_toolchain::locate_ninja()?;

    let manifest_compiler_wrapper = super::workspace_compiler_wrapper_settings(&graph);
    let cli_compiler_wrapper = super::compiler_wrapper_override_from_args(args.toolchain)?;

    // Translate `--profile` / `--release` into a typed selection
    // (clap's `conflicts_with` already rejects the two-flag form).
    // The workspace root manifest's `[profile.<name>]` tables are
    // the only source of profile definitions; a `build.profile`
    // setting in any active config file slots between the CLI
    // flag and the built-in `dev` default.
    let profile_selection =
        super::profile_selection_from_flags(args.profile, args.release, &effective_config)?;
    let manifest_profiles = super::workspace_profile_definitions(&graph);
    let profile = cabin_core::resolve_profile(&profile_selection, &manifest_profiles)?;

    // The MSVC backend cannot consume pkg-config's GNU-style flags;
    // reject a command that would need them before probing.  Scoped
    // to the selected closure so an unrelated member's system
    // dependency does not block `-p other` under MSVC.
    super::system_deps::ensure_dialect_supports_system_deps(
        &graph,
        &host_platform,
        &dev_for,
        cabin_build::Dialect::from_compiler_kind(detection_report.cxx.identity.kind),
        &selected_closure,
    )?;
    // Resolve features for the selected closure *before* deriving
    // build flags: `[target.'cfg(feature = "...")'.profile]` layers
    // are gated on the enabled-feature set, so feature resolution
    // must precede `resolve_build_prep`.
    let selection_request =
        super::build_selection_request(args.features, args.all_features, args.no_default_features);
    let feature_resolution = super::compute_feature_resolution(
        &graph,
        &resolved_selection,
        &selection_request,
        &dev_for,
    )?;

    // Per-package build flags + the (fail-hard) compiler wrapper,
    // folded into a toolchain summary.
    let prep = resolve_build_prep(BuildConfigInputs {
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
    let enabled_features = super::enabled_features_by_package(&feature_resolution);

    Ok(PreparedWorkspace {
        manifest_path,
        effective_config,
        graph,
        resolved_selection,
        selection_request,
        feature_resolution,
        enabled_features,
        profile,
        toolchain,
        detection_report,
        language_standards,
        approx_standards,
        prep,
        build_dir,
        ninja,
        lockfile_pinned,
        dev_for,
    })
}

/// Plan the prepared workspace and run the shared post-plan
/// standards gates.  `selected` narrows the plan to explicit
/// manifest targets (`cabin run`'s picked executable, `cabin
/// test`'s test selectors); `None` plans the default enumeration.
/// `check` rewrites the planned graph into `cabin check`'s
/// syntax-only form before the gates run.
pub(crate) fn plan_prepared(
    prepared: &PreparedWorkspace,
    selected: Option<Vec<cabin_build::ManifestTargetSelector>>,
    check: bool,
    color: cabin_core::ColorChoice,
) -> Result<cabin_build::BuildGraph> {
    // Validation only: the planner takes no configuration input, but
    // an invalid per-package configuration selection must still fail
    // the command before any Ninja file is written.
    super::resolve_build_configurations(
        &prepared.graph,
        &prepared.selection_request,
        &prepared.resolved_selection.packages,
        &prepared.profile,
        &prepared.prep.toolchain_summary,
        &prepared.prep.build_flags,
    )?;
    let plan_graph = super::plan(&super::PlanRequest {
        graph: &prepared.graph,
        toolchain: &prepared.toolchain,
        build_flags: &prepared.prep.build_flags,
        language_standards: &prepared.language_standards,
        standard_flag_conflicts: &prepared.prep.standard_flag_conflicts,
        build_dir: prepared.build_dir.clone(),
        profile: prepared.profile.clone(),
        selected,
        selected_packages: Some(&prepared.resolved_selection.packages),
        compiler_wrapper: prepared.prep.compiler_wrapper.as_ref(),
        dialect: cabin_build::Dialect::from_compiler_kind(
            prepared.detection_report.cxx.identity.kind,
        ),
        msvc_external_includes: cabin_build::msvc_external_includes_supported(
            &prepared.detection_report,
            prepared.approx_standards.has_c_sources(),
        ),
        enabled_features: Some(&prepared.enabled_features),
        standard_compat: true,
    })?;
    // `cabin check` reuses the build graph but rewrites it into a
    // syntax-only check (no codegen, no link) scoped to the selected
    // workspace packages' own translation units.
    let plan_graph = if check {
        let packages_root = prepared
            .build_dir
            .join(prepared.profile.name.as_str())
            .join("packages");
        // Fold `path_components` so the scoping roots stay
        // byte-identical to the planner's `packages/<scope>/<name>`
        // output dirs, which the check-graph filter compares against.
        let selected_pkg_dirs: Vec<PathBuf> = prepared
            .resolved_selection
            .packages
            .iter()
            .map(|&idx| {
                prepared.graph.packages[idx]
                    .package
                    .name
                    .path_components()
                    .fold(packages_root.clone(), |dir, c| dir.join(c))
            })
            .collect();
        cabin_build::into_check_graph(plan_graph, &selected_pkg_dirs)
    } else {
        plan_graph
    };
    // Standard-compat violations render before the hard build-time
    // enforcement below and gate the command themselves.
    super::standard_compat::report(
        &plan_graph.standard_compat_violations,
        color,
        &prepared.lockfile_pinned,
    )?;
    // Validate the plan-dependent toolchain contract against exactly
    // the compiles the final graph runs - after the check rewrite
    // (which drops dependency compiles) and before any Ninja file is
    // written.
    cabin_build::validate_planned_standards(&plan_graph)?;
    cabin_build::validate_toolchain_standards(
        &prepared.toolchain,
        &prepared.detection_report,
        &cabin_build::requested_standards_of(&plan_graph),
    )?;
    Ok(plan_graph)
}

/// Render the optimization / debuginfo descriptor that follows
/// the profile name in the `Finished` status line, matching
/// cargo's own banner:
///
/// - `unoptimized + debuginfo` for `dev` and any other `O0` +
///   debug build,
/// - `optimized` for `release` and other non-zero opt levels,
/// - `optimized + debuginfo` when both flags are on.
pub(crate) fn profile_descriptor(profile: &cabin_core::ResolvedProfile) -> String {
    let opt = if matches!(profile.opt_level, cabin_core::OptLevel::O0) {
        "unoptimized"
    } else {
        "optimized"
    };
    if profile.debug {
        format!("{opt} + debuginfo")
    } else {
        opt.to_owned()
    }
}

/// Translate `cabin build`'s `--profile` / `--release` flags into
/// a typed [`cabin_core::ProfileSelection`].
///
/// `--release` is preserved as a compatibility alias for
/// `--profile release`. clap's `conflicts_with` already rejects
/// the both-set combination so this helper only sees one of the
/// three possible inputs.
/// Shared profile-selection precedence: explicit `--profile NAME`
/// wins, then the legacy `--release` alias, then any config-
/// supplied default, then the built-in `dev` profile.  Used by
/// `cabin build` and `cabin test`.
pub(crate) fn profile_selection_from_flags(
    profile: Option<&str>,
    release: bool,
    config: &cabin_config::EffectiveConfig,
) -> Result<cabin_core::ProfileSelection> {
    if let Some(name) = profile {
        let pname = cabin_core::ProfileName::new(name.to_owned())?;
        return Ok(cabin_core::ProfileSelection::from_name(pname));
    }
    if release {
        return Ok(cabin_core::ProfileSelection::release_alias());
    }
    if let Some((selection, _source)) = crate::cli::config::config_profile_selection(config)? {
        return Ok(selection);
    }
    Ok(cabin_core::ProfileSelection::default_dev())
}

/// `cabin metadata` accepts a `--profile` flag but no `--release`
/// alias (metadata is read-only and doesn't need the legacy spelling).
/// Falls back to a config-supplied default when the user did not
/// pass `--profile`; otherwise the built-in `dev` profile applies.
pub(crate) fn profile_selection_for_metadata(
    name: Option<&str>,
    config: &cabin_config::EffectiveConfig,
) -> Result<cabin_core::ProfileSelection> {
    profile_selection_from_flags(name, false, config)
}

/// Look up the profile-definition table that should drive
/// resolution.  Profiles are workspace-wide: only the entry-point
/// manifest's `[profile.*]` tables count, so we read them off the
/// graph's root package (workspace root or single-package root).
pub(crate) fn workspace_profile_definitions(
    graph: &PackageGraph,
) -> BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition> {
    graph.root_settings.profiles.clone()
}

/// Workspace-root manifest's `[toolchain]` plus any
/// `[target.'cfg(...)'.toolchain]` overrides.  Workspace member
/// manifests cannot declare a `[toolchain]` table - the workspace
/// loader rejects them - so reading off the root is sufficient.
pub(crate) fn workspace_toolchain_settings(graph: &PackageGraph) -> cabin_core::ToolchainSettings {
    graph.root_settings.toolchain.clone()
}

/// Translate `cabin build`'s / `cabin metadata`'s tool-selection
/// CLI flags into a typed [`cabin_core::ToolchainSelection`].
pub(crate) fn toolchain_selection_from_args(
    args: &super::ToolchainSelectionArgs,
) -> Result<cabin_core::ToolchainSelection> {
    let mut sel = cabin_core::ToolchainSelection::default();
    if let Some(raw) = &args.cc {
        sel = sel.with_cli(cabin_core::ToolKind::CCompiler, parse_cli_tool(raw)?);
    }
    if let Some(raw) = &args.cxx {
        sel = sel.with_cli(cabin_core::ToolKind::CxxCompiler, parse_cli_tool(raw)?);
    }
    if let Some(raw) = &args.ar {
        sel = sel.with_cli(cabin_core::ToolKind::Archiver, parse_cli_tool(raw)?);
    }
    Ok(sel)
}

fn parse_cli_tool(raw: &str) -> Result<cabin_core::ToolSpec> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("tool argument must be a non-empty path or command name");
    }
    Ok(cabin_core::ToolSpec::parse(trimmed.to_owned()))
}

/// Resolve a toolchain by layering manifest settings, the
/// optional `[toolchain]` config layer, and process-discovered
/// defaults on top of `selection` (already-parsed CLI overrides
/// or `ToolchainSelection::default()`).
pub(crate) fn resolve_toolchain_layered(
    graph: &PackageGraph,
    selection: &cabin_core::ToolchainSelection,
    effective_config: &cabin_config::EffectiveConfig,
    host_platform: &cabin_core::TargetPlatform,
) -> Result<cabin_core::ResolvedToolchain> {
    let manifest_toolchain_settings = workspace_toolchain_settings(graph);
    let config_toolchain_layer = crate::cli::config::toolchain_layer(effective_config);
    let mut toolchain_inputs = cabin_toolchain::ResolveInputs::from_process(
        selection,
        &manifest_toolchain_settings,
        host_platform,
    );
    if let Some(layer) = config_toolchain_layer.as_ref() {
        toolchain_inputs = toolchain_inputs.with_config(layer);
    }
    Ok(cabin_toolchain::resolve_toolchain(&toolchain_inputs)?)
}

/// Translate the `--compiler-wrapper` / `--no-compiler-wrapper`
/// CLI flag pair into a typed
/// [`cabin_core::CompilerWrapperRequest`] override.  Clap already
/// rejects passing both flags simultaneously; this helper only
/// validates the value passed to `--compiler-wrapper`.
pub(crate) fn compiler_wrapper_override_from_args(
    args: &super::ToolchainSelectionArgs,
) -> Result<Option<cabin_core::CompilerWrapperRequest>> {
    if args.no_compiler_wrapper {
        return Ok(Some(cabin_core::CompilerWrapperRequest::Disabled));
    }
    let Some(raw) = args.compiler_wrapper.as_deref() else {
        return Ok(None);
    };
    let parsed = cabin_core::CompilerWrapperRequest::parse(raw)
        .with_context(|| format!("invalid --compiler-wrapper value `{raw}`"))?;
    Ok(Some(parsed))
}

/// Resolve the compiler wrapper by layering the CLI
/// override (`--compiler-wrapper` / `--no-compiler-wrapper`), the
/// manifest's `[build]` setting, the optional config
/// `[build] compiler-wrapper` layer, and process-detected
/// version metadata.  Returns the typed resolution on success;
/// callers that want fail-soft behavior (e.g. `cabin metadata`)
/// call `resolve_compiler_wrapper` directly.
pub(crate) fn resolve_compiler_wrapper_layered(
    cli_override: Option<cabin_core::CompilerWrapperRequest>,
    manifest_request: Option<&cabin_core::CompilerWrapperRequest>,
    effective_config: &cabin_config::EffectiveConfig,
) -> Result<Option<cabin_core::ResolvedCompilerWrapper>> {
    let mut wrapper_inputs =
        cabin_toolchain::WrapperInputs::from_process(cli_override, manifest_request);
    if let Some(layer) = crate::cli::config::wrapper_layer(effective_config) {
        wrapper_inputs = wrapper_inputs.with_config(layer);
    }
    cabin_toolchain::resolve_compiler_wrapper(
        &wrapper_inputs,
        Some(&cabin_toolchain::ProcessRunner),
    )
    .map_err(|err| anyhow::anyhow!(err.to_string()))
}

/// Workspace-root manifest's compiler-wrapper settings.  Mirrors
/// [`workspace_toolchain_settings`] - the workspace loader rejects
/// non-empty declarations on member manifests so reading the root
/// is sufficient.
pub(crate) fn workspace_compiler_wrapper_settings(
    graph: &PackageGraph,
) -> Option<cabin_core::CompilerWrapperRequest> {
    graph.root_settings.compiler_wrapper.clone()
}

/// Compute per-package effective language standards for every
/// package in the graph (pure manifest data; no toolchain input).
/// Keyed by package index, mirroring `resolve_per_package_build_flags`.
pub(crate) fn resolve_per_package_language_standards(
    graph: &PackageGraph,
) -> HashMap<usize, cabin_core::ResolvedLanguageStandards> {
    graph
        .packages
        .iter()
        .enumerate()
        .map(|(idx, pkg)| {
            (
                idx,
                cabin_core::resolve_language_standards(&pkg.package.language),
            )
        })
        .collect()
}

/// Compute per-package `ResolvedProfileFlags` for every package in
/// the graph.  The result is keyed by package index so callers
/// (planner, metadata view) can read them without rerunning the
/// merge per package.
pub(crate) fn resolve_per_package_build_flags(
    graph: &PackageGraph,
    profile: &cabin_core::ResolvedProfile,
    host_platform: &cabin_core::TargetPlatform,
    feature_resolution: &cabin_feature::FeatureResolution,
    detection: Option<&cabin_core::ToolchainDetectionReport>,
) -> (
    HashMap<usize, cabin_core::ResolvedProfileFlags>,
    HashMap<usize, Vec<cabin_core::StandardFlagConflict>>,
) {
    // Detected compiler identities gate `[target.'cfg(cc/cxx = ...)'.profile]`
    // layers.  `None` (fail-soft commands where detection failed) evaluates
    // those layers as family `unknown` with no version.
    let (cc_identity, cxx_identity) = match detection {
        Some(report) => (
            report.cc.as_ref().map(|tool| &tool.identity),
            Some(&report.cxx.identity),
        ),
        None => (None, None),
    };
    let mut out = HashMap::with_capacity(graph.packages.len());
    let mut conflicts: HashMap<usize, Vec<cabin_core::StandardFlagConflict>> = HashMap::new();
    for (idx, pkg) in graph.packages.iter().enumerate() {
        // A registry/downloaded dependency's own `[profile]` build flags are
        // untrusted: only local packages (the workspace root, its members, and
        // `path` dependencies) may contribute raw compiler/linker flags.
        // `resolve_build_flags` drops the dependency's cflags/cxxflags/ldflags
        // when this is false, so a malicious dependency cannot smuggle a
        // code-executing compiler flag (e.g. `-fplugin=`) onto its build line.
        let package_trusted = matches!(pkg.kind, cabin_workspace::PackageKind::Local);
        // The package's resolved enabled features gate its
        // `[target.'cfg(feature = "...")'.profile]` flag layers. cabin-core
        // stays feature-vocabulary-only (it must not depend on cabin-feature),
        // so the cli pulls the name set out of the resolution and hands core
        // a bare `&BTreeSet<String>`.
        let package_features = feature_resolution.for_package(idx);
        let ctx = cabin_core::ConditionContext::with_features(
            host_platform,
            &package_features.enabled_features,
        )
        .with_compilers(cc_identity, cxx_identity);
        let resolved = cabin_core::resolve_build_flags(
            &pkg.package.build,
            Some(profile),
            &graph.root_settings.profiles,
            &ctx,
            package_trusted,
        );
        // The documented escape-hatch conflict *candidates*: a
        // first-class standard declaration plus an explicit
        // `-std=` / `/std:` in the same package's manifest-derived
        // flags.  Detected before the system-dep / env augmentation
        // layers so CFLAGS / CXXFLAGS and pkg-config output stay
        // exempt; the build planner surfaces a candidate only when
        // a compile its scope covers is planned.
        let pkg_conflicts = cabin_core::find_standard_flag_conflicts(
            pkg.package.name.as_str(),
            &pkg.package.language,
            &pkg.package.targets,
            &resolved,
        );
        if !pkg_conflicts.is_empty() {
            conflicts.insert(idx, pkg_conflicts);
        }
        out.insert(idx, resolved);
    }
    (out, conflicts)
}

/// Apply the documented post-profile build-flag layers - `pkg-config`
/// probes for active system dependencies, then `CPPFLAGS` / `CFLAGS`
/// / `CXXFLAGS` / `LDFLAGS` from the process environment - in the
/// order both layers must run for the resulting
/// `BuildConfiguration::fingerprint` to stay stable across commands.
/// Reports from both layers are intentionally discarded; callers that
/// need them invoke the individual `crate::cli::system_deps` /
/// `crate::cli::env_flags` helpers directly.
pub(crate) fn augment_build_flags(
    graph: &PackageGraph,
    host_platform: &cabin_core::TargetPlatform,
    dev_for: &BTreeSet<String>,
    build_flags: HashMap<usize, cabin_core::ResolvedProfileFlags>,
    reporter: Reporter,
) -> Result<HashMap<usize, cabin_core::ResolvedProfileFlags>> {
    let (build_flags, _system_dep_reports) =
        crate::cli::system_deps::augment_build_flags_with_system_deps(
            graph,
            host_platform,
            dev_for,
            build_flags,
            reporter,
        )?;
    let (build_flags, _env_build_flags) = crate::cli::env_flags::augment_build_flags_with_env(
        graph,
        build_flags,
        |k| std::env::var_os(k),
        reporter,
    )?;
    Ok(build_flags)
}

/// Resolve a `BuildConfiguration` for every package in the graph.
/// CLI feature selection requests apply to primary packages only -
/// non-primary packages (transitive path / registry deps) fall back
/// to their declared defaults until per-dependency feature requests
/// land.
pub(crate) fn resolve_build_configurations(
    graph: &PackageGraph,
    request: &cabin_core::SelectionRequest,
    selected: &[usize],
    profile: &cabin_core::ResolvedProfile,
    toolchain: &cabin_core::ToolchainSummary,
    build_flags: &HashMap<usize, cabin_core::ResolvedProfileFlags>,
) -> Result<HashMap<usize, cabin_core::BuildConfiguration>> {
    let selected_set: HashSet<usize> = selected.iter().copied().collect();
    let mut out: HashMap<usize, cabin_core::BuildConfiguration> = HashMap::new();
    for (idx, pkg) in graph.packages.iter().enumerate() {
        // CLI feature requests apply only to *selected* packages.
        // Non-selected packages - including workspace siblings the
        // user did not pick - fall back to their declared defaults
        // so an unrelated package's missing feature does not fail
        // an unrelated build.
        let pkg_request = if selected_set.contains(&idx) {
            request.clone()
        } else {
            cabin_core::SelectionRequest::default()
        };
        let pkg_flags = build_flags.get(&idx).cloned().unwrap_or_default();
        let cfg = cabin_core::BuildConfiguration::resolve(cabin_core::BuildConfigurationInput {
            package: pkg.package.name.as_str(),
            features: &pkg.package.features,
            request: &pkg_request,
            profile: profile.clone(),
            toolchain: toolchain.clone(),
            build_flags: pkg_flags,
            language: cabin_core::LanguageStandardsSummary::from_package(&pkg.package),
        })
        .with_context(|| {
            format!(
                "invalid configuration selection for package `{}`",
                pkg.package.name.as_str()
            )
        })?;
        out.insert(idx, cfg);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::PackageName;

    #[test]
    fn registry_dependency_build_flags_are_dropped_but_local_kept() {
        use cabin_core::{Package, Target};
        use cabin_workspace::{PackageKind, WorkspacePackage};
        use std::path::PathBuf;

        fn dep_with_command_flags(name: &str, kind: PackageKind) -> WorkspacePackage {
            let mut package = Package::new(
                PackageName::new(name).unwrap(),
                semver::Version::parse("0.1.0").unwrap(),
                Vec::<Target>::new(),
                Vec::new(),
            )
            .unwrap();
            package.build.general.cflags = vec!["-fplugin=evil.so".into()];
            package.build.general.cxxflags = vec!["-B.".into()];
            package.build.general.ldflags = vec!["-fuse-ld=/tmp/evil".into()];
            WorkspacePackage {
                package,
                manifest_dir: PathBuf::from("/tmp"),
                manifest_path: PathBuf::from("/tmp/cabin.toml"),
                kind,
                deps: Vec::new(),
                is_port: false,
            }
        }

        let graph = PackageGraph {
            root_manifest_path: PathBuf::from("/tmp/cabin.toml"),
            root_dir: PathBuf::from("/tmp"),
            is_workspace_root: false,
            root_package: Some(0),
            root_settings: Default::default(),
            primary_packages: vec![0],
            default_members: vec![0],
            excluded_members: Vec::new(),
            packages: vec![
                dep_with_command_flags("local_dep", PackageKind::Local),
                dep_with_command_flags("registry_dep", PackageKind::Registry),
            ],
        };

        let host = cabin_core::TargetPlatform::current();
        let profile = cabin_core::resolve_profile(
            &cabin_core::ProfileSelection::default_dev(),
            &graph.root_settings.profiles,
        )
        .unwrap();
        let (resolved, _conflicts) = resolve_per_package_build_flags(
            &graph,
            &profile,
            &host,
            &cabin_feature::FeatureResolution::default(),
            None,
        );

        // A local package (workspace member / path dependency) is trusted:
        // its declared compiler and linker flags are preserved.
        let local = resolved.get(&0).expect("local package flags");
        assert_eq!(local.cflags, vec!["-fplugin=evil.so".to_owned()]);
        assert_eq!(local.cxxflags, vec!["-B.".to_owned()]);
        assert_eq!(local.ldflags, vec!["-fuse-ld=/tmp/evil".to_owned()]);

        // A registry dependency is untrusted: its compiler and linker flags
        // are dropped so it cannot execute code at build time.
        let registry = resolved.get(&1).expect("registry package flags");
        assert!(registry.cflags.is_empty());
        assert!(registry.cxxflags.is_empty());
        assert!(registry.ldflags.is_empty());
    }
}
