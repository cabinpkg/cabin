//! Shared build-configuration preamble for the commands that resolve
//! a [`cabin_core::BuildConfiguration`] per package: `build`, `check`,
//! `run`, `test`, and `cabin explain build-config`.
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
//! the building commands (`build` / `check` / `run` / `test`): they own
//! the whole shared preamble - workspace resolution, the artifact
//! pipeline, toolchain, profile, features, planning, and the
//! post-plan standards gates - parameterized on [`DevActivation`] and
//! whether the build context is prepared; command-specific target
//! selection and execution stay at the call sites.  `cabin metadata` is
//! intentionally not a caller of [`resolve_build_prep`]: it resolves
//! the wrapper fail-*soft* and must keep it.  [`resolve_inspection_prep`]
//! is the read-only commands' fail-soft profile / toolchain / detection
//! step, shared by `cabin metadata` and `cabin explain build-config`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use cabin_workspace::PackageGraph;

use crate::cli::term_verbosity::Reporter;

/// Inputs to [`resolve_build_prep`].  The caller supplies the
/// already-resolved toolchain and profile; the helper owns the flag /
/// wrapper / summary derivation.
pub(crate) struct BuildConfigInputs<'a> {
    pub graph: &'a cabin_workspace::PackageGraph,
    pub platform: &'a cabin_core::TargetPlatform,
    /// The `--target` triple, folded into the configuration fingerprint.
    pub target: Option<&'a str>,
    /// The sysroot as the user wrote it, folded into the configuration
    /// fingerprint; `None` for none or a detected SDK.
    pub sysroot: Option<&'a str>,
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
/// Flag augmentation may emit reporter warnings.  Wrapper resolution
/// is silent on success and fatal on failure (a misbehaving wrapper
/// never silently bypasses caching).
#[allow(clippy::needless_pass_by_value)] // consumed: `cli_compiler_wrapper` is moved into the wrapper resolver
pub(crate) fn resolve_build_prep(inputs: BuildConfigInputs) -> Result<BuildPrep> {
    let (build_flags, standard_flag_conflicts) = resolve_per_package_build_flags(
        inputs.graph,
        inputs.profile,
        inputs.platform,
        inputs.feature_resolution,
        inputs.detection,
    );
    let build_flags = augment_build_flags(
        inputs.graph,
        inputs.platform,
        inputs.dev_for,
        build_flags,
        inputs.reporter,
    )?;
    let compiler_wrapper = resolve_compiler_wrapper_layered(
        inputs.cli_compiler_wrapper,
        inputs.manifest_compiler_wrapper,
        inputs.effective_config,
    )?;
    let mut toolchain_summary = cabin_core::ToolchainSummary::from_resolved_parts(
        inputs.toolchain,
        compiler_wrapper.as_ref(),
    );
    toolchain_summary.target = inputs.target.map(str::to_owned);
    toolchain_summary.sysroot = inputs.sysroot.map(str::to_owned);
    Ok(BuildPrep {
        build_flags,
        standard_flag_conflicts,
        compiler_wrapper,
        toolchain_summary,
    })
}

/// The profile and toolchain `cabin metadata` and `cabin explain
/// build-config` evaluate configurations against.
pub(crate) struct InspectionPrep {
    pub manifest_profiles: BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
    pub profile: cabin_core::ResolvedProfile,
    pub toolchain: cabin_core::ResolvedToolchain,
    /// `None` when detection failed: the read-only commands are
    /// fail-soft so a misbehaving compiler does not block inspecting
    /// the rest of the workspace, and compiler cfg conditions then
    /// evaluate as `unknown` with no version.
    pub detection: Option<cabin_core::ToolchainDetectionReport>,
    pub sysroot: Option<SysrootChoice>,
}

/// Resolve the profile (`--profile`, no `--release` alias on the
/// read-only commands, then config, then `dev`) and the layered
/// toolchain, then detect it, downgrading a detection failure to a
/// warning.  Compiler-wrapper resolution stays with the callers:
/// `cabin metadata` tolerates its failure, `cabin explain
/// build-config` does not.
pub(crate) fn resolve_inspection_prep(
    graph: &PackageGraph,
    profile: Option<&str>,
    toolchain_args: &super::ToolchainSelectionArgs,
    platform: &cabin_core::TargetPlatform,
    effective_config: &cabin_config::EffectiveConfig,
    reporter: Reporter,
) -> Result<InspectionPrep> {
    let manifest_profiles = workspace_profile_definitions(graph);
    let profile_selection = profile_selection_from_flags(profile, false, effective_config)?;
    let profile = cabin_core::resolve_profile(&profile_selection, &manifest_profiles)?;
    let toolchain_selection = toolchain_selection_from_args(toolchain_args)?;
    let toolchain =
        resolve_toolchain_layered(graph, &toolchain_selection, effective_config, platform)?;
    let (detection, apple_sdk) =
        match detect_toolchain(&toolchain, toolchain_args, effective_config, platform) {
            Ok((report, apple_sdk)) => (Some(report), apple_sdk),
            Err(err) => {
                reporter.warning(format_args!("toolchain detection failed: {err}"));
                (None, None)
            }
        };
    let sysroot = select_sysroot(
        toolchain_args,
        effective_config,
        platform,
        detection.as_ref(),
        apple_sdk,
    )?;
    Ok(InspectionPrep {
        manifest_profiles,
        profile,
        toolchain,
        detection,
        sysroot,
    })
}

/// The sysroot a command builds with, and where it came from.
pub(crate) struct SysrootChoice {
    pub sysroot: cabin_core::Sysroot,
    /// `cli`, the config file's source, or `detected` for an Apple
    /// SDK Cabin found itself.
    pub source: &'static str,
    /// The choice as the user wrote it, which the configuration
    /// fingerprint hashes like a tool spec; `None` for a detected SDK,
    /// a local fact like a resolved tool path.
    pub written: Option<String>,
}

/// Detect `toolchain`, and the Apple SDK a sysroot would be discovered
/// as unless one is chosen explicitly.
fn detect_toolchain(
    toolchain: &cabin_core::ResolvedToolchain,
    toolchain_args: &super::ToolchainSelectionArgs,
    effective_config: &cabin_config::EffectiveConfig,
    platform: &cabin_core::TargetPlatform,
) -> Result<
    (
        cabin_core::ToolchainDetectionReport,
        Option<cabin_core::Sysroot>,
    ),
    cabin_toolchain::ToolchainDetectionFailure,
> {
    if explicit_sysroot(toolchain_args, effective_config).is_some() {
        let report = cabin_toolchain::detect_toolchain(toolchain, &cabin_toolchain::ProcessRunner)?;
        return Ok((report, None));
    }
    cabin_toolchain::detect_toolchain_and_apple_sdk(
        toolchain,
        platform,
        std::env::var_os("SDKROOT").as_deref(),
        &cabin_toolchain::ProcessRunner,
    )
}

/// `--sysroot`, else the config `[sysroot]` entry for `--target`: the
/// path, its source, and the choice as the user wrote it.
fn explicit_sysroot(
    toolchain_args: &super::ToolchainSelectionArgs,
    effective_config: &cabin_config::EffectiveConfig,
) -> Option<(PathBuf, &'static str, String)> {
    match &toolchain_args.sysroot {
        Some(path) => Some((path.clone(), "cli", path.to_string_lossy().into_owned())),
        None => toolchain_args
            .target
            .as_ref()
            .and_then(|target| effective_config.sysroots.get(&target.target))
            .map(|setting| {
                let source = super::config::config_value_source(setting.source).as_key();
                let written = setting.value.to_string();
                (setting.absolute().into_std_path_buf(), source, written)
            }),
    }
}

/// `--sysroot`, else the config `[sysroot]` entry for `--target`, else
/// the Apple SDK Apple Clang would use, `apple_sdk` from
/// [`detect_toolchain`] (contracts: explicit choices over discovery).
/// Without a detection report, an explicit choice goes unchecked.
pub(crate) fn select_sysroot(
    toolchain_args: &super::ToolchainSelectionArgs,
    effective_config: &cabin_config::EffectiveConfig,
    platform: &cabin_core::TargetPlatform,
    detection: Option<&cabin_core::ToolchainDetectionReport>,
    apple_sdk: Option<cabin_core::Sysroot>,
) -> Result<Option<SysrootChoice>> {
    if let Some((path, source, written)) = explicit_sysroot(toolchain_args, effective_config) {
        // Ninja runs from the profile directory, and a relative
        // `CABIN_CONFIG` leaves a config entry relative too.
        let path = super::absolutise(&path)
            .with_context(|| format!("failed to resolve sysroot {}", path.display()))?;
        let path = camino::Utf8PathBuf::from_path_buf(path)
            .map_err(|path| anyhow::anyhow!("sysroot {} is not valid UTF-8", path.display()))?;
        let sysroot = cabin_toolchain::explicit_sysroot(path, platform, detection)?;
        return Ok(Some(SysrootChoice {
            sysroot,
            source,
            written: Some(written),
        }));
    }
    Ok(apple_sdk.map(|sysroot| SysrootChoice {
        sysroot,
        source: "detected",
        written: None,
    }))
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
    /// Whether the command builds the build context; `cabin check`
    /// checks only the selected packages' own sources.
    pub build_context: bool,
}

/// Everything the shared preamble produces that the command tails
/// (target selection, planning, Ninja invocation, execution)
/// consume.
pub(crate) struct PreparedWorkspace {
    pub manifest_path: PathBuf,
    pub effective_config: cabin_config::EffectiveConfig,
    pub graph: cabin_workspace::PackageGraph,
    pub resolved_selection: cabin_workspace::ResolvedSelection,
    pub feature_resolution: cabin_feature::FeatureResolution,
    pub enabled_features: HashMap<usize, BTreeSet<String>>,
    pub profile: cabin_core::ResolvedProfile,
    pub toolchain: cabin_core::ResolvedToolchain,
    pub detection_report: cabin_core::ToolchainDetectionReport,
    pub language_standards: HashMap<usize, cabin_core::ResolvedLanguageStandards>,
    pub approx_standards: cabin_build::RequestedStandards,
    pub prep: BuildPrep,
    /// The [`cabin_build::layout::platform_dir`] this build writes under.
    pub build_dir: PathBuf,
    /// The build directory itself: where the lock and the usage catalog
    /// live.
    pub build_root: PathBuf,
    pub ninja: PathBuf,
    pub lockfile_pinned: BTreeSet<(String, String)>,
    pub dev_for: BTreeSet<String>,
    /// The platform `--target` selects, or the build machine.
    pub platform: cabin_core::TargetPlatform,
    /// See [`cabin_build::PlanRequest::clang_target`].
    pub clang_target: Option<String>,
    /// See [`cabin_build::PlanRequest::sysroot`].  The build context
    /// builds with the compilers' default.
    pub sysroot: Option<cabin_core::Sysroot>,
    /// Whether `sysroot` was configured (`--sysroot`, `[sysroot]`)
    /// rather than detected.
    pub configured_sysroot: bool,
    /// The `[runner]` config entry for the `--target` triple: the
    /// program and arguments `cabin run` / `cabin test` put before
    /// each executable they run; empty runs it directly.
    pub runner: Vec<String>,
    /// The build context, when the target context's packages declare
    /// build dependencies.
    pub build_context: Option<BuildContextPrep>,
}

impl PreparedWorkspace {
    /// The build context's toolchain when it is its own (`--target`).
    pub(crate) fn build_machine_detection(&self) -> Option<&cabin_core::ToolchainDetectionReport> {
        self.build_context
            .as_ref()
            .filter(|context| context.cross)
            .map(|context| &context.detection_report)
    }
}

/// The build context's plan inputs: tools for the build machine,
/// with features and flags of their own (contracts section 1).
pub(crate) struct BuildContextPrep {
    /// Packages a build edge reaches; the plan builds their default
    /// targets and what those link.
    pub roots: Vec<usize>,
    pub included: BTreeSet<usize>,
    /// Whether `--target` was given: the build machine then has a
    /// toolchain of its own.
    pub cross: bool,
    pub enabled_features: HashMap<usize, BTreeSet<String>>,
    pub build_flags: HashMap<usize, cabin_core::ResolvedProfileFlags>,
    pub standard_flag_conflicts: HashMap<usize, Vec<cabin_core::StandardFlagConflict>>,
    /// The build machine.
    pub platform: cabin_core::TargetPlatform,
    /// The build machine's toolchain: the target context's unless
    /// `cross`, else [`build_machine_toolchain`]'s.
    pub toolchain: cabin_core::ResolvedToolchain,
    pub detection_report: cabin_core::ToolchainDetectionReport,
}

/// The shared `build` / `run` / `test` preamble: workspace
/// resolution, the artifact pipeline, the strict re-load, toolchain
/// resolution + detection + validation, profile choice, feature
/// resolution, and [`resolve_build_prep`] - everything up to (but
/// not including) command-specific target selection and planning.
///
/// The per-command policy is [`DevActivation`] and
/// [`WorkspacePipelineArgs::build_context`].  With dev deps
/// activated the pipeline adds the two extra dev-aware loads
/// `cabin test` needs (the pre-resolution probe graph and the
/// strict-set skeleton); with them disabled those loads collapse to
/// the initial graph and the pipeline is exactly the `build` / `run`
/// preamble.
#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn prepare_workspace(
    args: &WorkspacePipelineArgs<'_>,
    reporter: Reporter,
) -> Result<PreparedWorkspace> {
    let manifest_path = super::resolve_invocation_manifest(args.manifest_path)?;
    let platform = args.toolchain.platform();

    // First-pass load: needed to detect versioned dependencies
    // before we know whether we have to fetch anything.  This load
    // also surfaces manifest / workspace errors before we touch
    // the index.
    let offline = super::config::effective_offline(args.offline)?;
    let workspace_selection = super::build_workspace_selection(args.workspace_selection);
    let include_dev = matches!(args.dev, DevActivation::SelectedPrimaries);
    let super::workspace_prep::WorkspacePrep {
        effective_config,
        // Patched names are excluded from the closure / artifact
        // pipeline because they ship from a local working copy.
        active_patches,
        graph: initial_graph,
    } = super::workspace_prep::load_workspace_and_patches(&manifest_path, args.no_patches)?;
    let patched_names = active_patches.owned_patched_names();
    let resolved_index_source =
        super::config::resolve_index_source(args.index_path, args.index_url, &effective_config)?;
    super::config::enforce_offline_index_source(offline, resolved_index_source.as_ref())?;
    let resolved_cache_dir = super::config::resolve_cache_dir(args.cache_dir, &effective_config);

    // Only the *selected closure* drives the index requirement: an
    // unrelated workspace member's versioned dep must not make
    // `cabin build -p <selected>` need an index when the selection
    // itself has no versioned deps.
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
    let dev_probe: Option<cabin_workspace::PackageGraph> = if include_dev {
        Some(cabin_workspace::load_workspace_with_options(
            &manifest_path,
            &cabin_workspace::WorkspaceLoadOptions {
                registry: &[],
                patches: &patched_sources,
                system: &[],
                registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&BTreeSet::new()),
                include_dev_for: &dev_for,
            },
        )?)
    } else {
        None
    };
    let probe_graph = dev_probe.as_ref().unwrap_or(&initial_graph);

    // The seed checks local-vs-local links claims - activated patch
    // forks' included - here too: the `!has_versioned` arm below
    // never reaches the artifact pipeline, so a local-only
    // workspace would otherwise skip the uniqueness check entirely.
    let seed = super::seed::ResolutionSeed::compute(&super::seed::SeedInputs {
        graph: probe_graph,
        selection: &workspace_selection,
        request: &initial_request,
        patched_names: &patched_names,
        active_patches: &active_patches,
        dev_for: &dev_for,
    })?;
    let has_versioned = seed.has_versioned_deps();

    let (registry, lockfile_pinned, activated_patches): (
        Vec<super::RegistryPackageSource>,
        BTreeSet<(String, String)>,
        BTreeSet<String>,
    ) = if has_versioned {
        let inputs = super::config::resolve_pipeline_inputs(
            resolved_index_source.as_ref(),
            &effective_config,
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
            index_user_chosen: inputs.index_user_chosen,
            policy: inputs.policy,
            cache_dir: &inputs.cache_dir,
            reporter,
            seed: &seed,
            patched_names: &patched_names,
            active_patches: &active_patches,
            source_replacements: &effective_config.source_replacements,
            incompatible_standards: super::config::resolve_incompatible_standards(
                &effective_config,
            )?,
            no_patches: args.no_patches,
            dev_for: &dev_for,
        })?;
        (
            pipeline.registry_sources(),
            pipeline.lockfile_pinned,
            pipeline.activated_patches,
        )
    } else {
        super::enforce_locked_patch_state(
            &manifest_path,
            &super::LockPolicy::from_flags(args.locked, args.frozen),
            &active_patches,
            &effective_config.source_replacements,
            args.no_patches,
        )?;
        (
            Vec::new(),
            BTreeSet::new(),
            seed.patched_deps.activated.clone(),
        )
    };

    // Re-load the workspace, this time stitching in the resolved
    // registry packages plus active patches.
    //
    // `strict_packages` controls which packages require their
    // versioned deps to be satisfied.  The set is the
    // selection's closure plus every package the resolver fetched
    // into `registry`.  The closure alone misses any package
    // reached only after resolution - in particular, transitive
    // registry packages a patched manifest pulled in via a version
    // dep that did not exist on the upstream package.  Without the
    // registry extension those packages would parent a
    // missing-registry edge under the scoped policy
    // and silently drop it, leaving the build to fail later with a
    // less actionable diagnostic.  Activated patched names are
    // folded in explicitly: a transitively-discovered patch is
    // reached through registry metadata, not the local closure
    // walk.  Dormant patches stay out - their versioned deps are
    // deliberately never resolved, so requiring them would fail
    // the build over a patch nothing uses.
    //
    // Under dev activation the strict set must additionally cover
    // every package reachable from the selected test runners *with
    // their dev-dependencies activated* - otherwise a transitive
    // path-dep that only becomes an active graph edge through a
    // dev edge would be missing from the strict set, and its
    // broken registry edge would silently drop instead of
    // surfacing the typed diagnostic.  The pre-resolution probe
    // graph carries dev edges but not the resolver's registry, so
    // the closure walks a permissive dev-aware skeleton loaded with
    // the full registry + active patches instead.
    let mut strict_packages: BTreeSet<String> = if include_dev {
        let dev_aware_skeleton = cabin_workspace::load_workspace_with_options(
            &manifest_path,
            &cabin_workspace::WorkspaceLoadOptions {
                registry: &registry,
                patches: &patched_sources,
                system: &[],
                registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&BTreeSet::new()),
                include_dev_for: &dev_for,
            },
        )?;
        let dev_aware_selection =
            cabin_workspace::resolve_package_selection(&dev_aware_skeleton, &workspace_selection)?;
        dev_aware_selection.closure_package_names(&dev_aware_skeleton)
    } else {
        initial_resolved_selection.closure_package_names(&initial_graph)
    };
    // The closure also follows optional path edges no feature enables;
    // resolution fetched none of those packages' registry dependencies.
    let seed_included = seed.included();
    for (idx, pkg) in probe_graph.packages.iter().enumerate() {
        if !seed_included.contains(&idx) {
            strict_packages.remove(pkg.package.name.as_str());
        }
    }
    strict_packages.extend(activated_patches.iter().cloned());
    strict_packages.extend(registry.iter().map(|r| r.name.as_str().to_owned()));
    let graph = cabin_workspace::load_workspace_with_options(
        &manifest_path,
        &cabin_workspace::WorkspaceLoadOptions {
            registry: &registry,
            patches: &patched_sources,
            system: &[],
            registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&strict_packages),
            include_dev_for: &dev_for,
        },
    )?;

    // Resolve the workspace package selection up-front.  The planner
    // consumes the selected indices through `PlanRequest::selected_packages`
    // so default-target enumeration narrows to the picked packages instead
    // of every primary - and the backend checks below scope to the
    // selected closure so an unselected member's C source or pkg-config
    // dependency never gates the command.
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;
    // Resolve features for the selected closure before toolchain
    // detection and build-flag derivation: the links check below
    // must not hide behind an environment-dependent failure, and
    // `[target.'cfg(feature = "...")'.profile]` flag layers are
    // gated on the enabled-feature set.
    let selection_request =
        super::build_selection_request(args.features, args.all_features, args.no_default_features);
    let feature_resolution = super::compute_feature_resolution(
        &graph,
        &resolved_selection,
        &selection_request,
        Some(&platform),
        &dev_for,
    )?;
    let build_machine = cabin_core::TargetPlatform::current();
    let build_features = cabin_feature::resolve_build_context_features(
        &graph,
        &feature_resolution,
        Some(&platform),
        Some(&build_machine),
    )?;
    // The reload skips an optional registry dependency resolution
    // left out, so one these features enable would otherwise build
    // without its edge.
    if let Some(name) = feature_resolution
        .versioned_requests
        .keys()
        .chain(build_features.versioned_requests.keys())
        .next()
    {
        bail!(
            "feature resolution enables dependency `{name}`, which dependency resolution did not select"
        );
    }
    // The exact links-uniqueness check.  The final graph is the
    // only place every claimant exists as a loaded manifest -
    // fetched registry packages and activated forks alike -
    // and this feature resolution applies the dependents' real
    // edge requests, which those earlier checks only approximate
    // conservatively (see `activated_fork_claims`).  The artifact
    // pipeline has already written the lockfile by now; that is
    // fine - the recorded resolution itself is valid, only this
    // command's feature-exact link plan is not.  Each context is its
    // own link/runtime domain.
    for features in [&feature_resolution, &build_features] {
        super::seed::enforce_local_links_uniqueness(&super::seed::local_links_claims(
            &graph, features,
        ))?;
    }

    let build_root = super::config::resolve_build_dir(args.build_dir, &effective_config)?;
    let build_dir = cabin_build::layout::platform_dir(&build_root, args.toolchain.target.as_ref());

    let toolchain_selection = super::toolchain_selection_from_args(args.toolchain)?;
    let toolchain = super::resolve_toolchain_layered(
        &graph,
        &toolchain_selection,
        &effective_config,
        &platform,
    )?;
    // Detect compiler / archiver identity and validate that the tools
    // can run the backend's command shapes for their dialect (MSVC, or
    // GCC-style flags with depfiles and an `ar`-compatible archiver)
    // before any Ninja file is written.  Fail fast and clear here
    // rather than letting Ninja produce a confusing error from a
    // broken command line.
    let (detection_report, apple_sdk) =
        detect_toolchain(&toolchain, args.toolchain, &effective_config, &platform)?;
    let compiler_targets = cabin_toolchain::check_compiler_targets(
        &detection_report,
        &platform,
        args.toolchain.target.is_some(),
        &cabin_toolchain::ProcessRunner,
    )?;
    for warning in &compiler_targets.warnings {
        reporter.warning(format_args!("{warning}"));
    }
    let sysroot_choice = select_sysroot(
        args.toolchain,
        &effective_config,
        &platform,
        Some(&detection_report),
        apple_sdk,
    )?;
    let written_sysroot = sysroot_choice
        .as_ref()
        .and_then(|choice| choice.written.clone());
    let configured_sysroot = written_sysroot.is_some();
    let sysroot = sysroot_choice.map(|choice| choice.sysroot);
    // Feature resolution's reach: the closure through edges active on
    // this platform, now that the graph also holds every other platform's.
    let selected_closure = &feature_resolution.included;
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
        selected_closure,
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
        &platform,
        &dev_for,
        cabin_build::Dialect::from_compiler_kind(detection_report.cxx.identity.kind),
        selected_closure,
    )?;
    let prep = resolve_build_prep(BuildConfigInputs {
        graph: &graph,
        platform: &platform,
        target: args.toolchain.target.as_ref().map(|t| t.target.as_str()),
        sysroot: written_sysroot.as_deref(),
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
    let build_context = if !args.build_context || build_features.included.is_empty() {
        None
    } else {
        let cross = args.toolchain.target.is_some();
        let (context_toolchain, context_detection) = if cross {
            build_machine_toolchain(&graph, &build_machine, &platform, &detection_report)?
        } else {
            (toolchain.clone(), detection_report.clone())
        };
        Some(build_context_prep(BuildContextInputs {
            graph: &graph,
            target_features: &feature_resolution,
            features: &build_features,
            platform: &platform,
            build_machine,
            cross,
            profile: &profile,
            toolchain: context_toolchain,
            detection: context_detection,
            reporter,
        })?)
    };

    let mut runner = args
        .toolchain
        .target
        .as_ref()
        .and_then(|target| effective_config.runners.get(&target.target))
        .map(|runner| runner.value.clone())
        .unwrap_or_default();
    // A relative program path means the current directory, as a
    // `[toolchain]` path does; `cabin test` starts each test elsewhere.
    if let Some(program) = runner.first_mut()
        && matches!(
            cabin_core::ToolSpec::parse(program.as_str()),
            cabin_core::ToolSpec::Path(path) if path.is_relative()
        )
    {
        *program = std::path::absolute(&*program)
            .context("failed to determine current directory")?
            .into_os_string()
            .into_string()
            .map_err(|_| anyhow::anyhow!("the current directory is not valid UTF-8"))?;
    }
    Ok(PreparedWorkspace {
        manifest_path,
        effective_config,
        graph,
        resolved_selection,
        feature_resolution,
        enabled_features,
        profile,
        toolchain,
        detection_report,
        language_standards,
        approx_standards,
        prep,
        build_dir,
        build_root,
        ninja,
        lockfile_pinned,
        dev_for,
        platform,
        clang_target: compiler_targets.clang_target,
        sysroot,
        configured_sysroot,
        runner,
        build_context,
    })
}

/// What [`build_context_prep`] reads from the target context's
/// preparation.
struct BuildContextInputs<'a> {
    graph: &'a PackageGraph,
    target_features: &'a cabin_feature::FeatureResolution,
    features: &'a cabin_feature::FeatureResolution,
    platform: &'a cabin_core::TargetPlatform,
    build_machine: cabin_core::TargetPlatform,
    cross: bool,
    profile: &'a cabin_core::ResolvedProfile,
    toolchain: cabin_core::ResolvedToolchain,
    detection: cabin_core::ToolchainDetectionReport,
    reporter: Reporter,
}

/// Prepare the build context's plan inputs: its roots, features, and
/// flags, with `cfg(...)` evaluated for the build machine.
fn build_context_prep(inputs: BuildContextInputs<'_>) -> Result<BuildContextPrep> {
    let graph = inputs.graph;
    let (build_flags, standard_flag_conflicts) = resolve_per_package_build_flags(
        graph,
        inputs.profile,
        &inputs.build_machine,
        inputs.features,
        Some(&inputs.detection),
    );
    // Environment flags describe the target platform; they reach the
    // build context only when it is the same build.
    let build_flags = if inputs.cross {
        build_flags
    } else {
        crate::cli::env_flags::augment_build_flags_with_env(
            graph,
            build_flags,
            |k| std::env::var_os(k),
            inputs.reporter,
        )?
    };
    let mut roots = BTreeSet::new();
    for (features, platform) in [
        (inputs.target_features, inputs.platform),
        (inputs.features, &inputs.build_machine),
    ] {
        for &idx in &features.included {
            for edge in &graph.packages[idx].deps {
                if edge.kind == cabin_core::DependencyKind::Build && edge.matches_platform(platform)
                {
                    roots.insert(edge.index);
                }
            }
        }
    }
    Ok(BuildContextPrep {
        roots: roots.into_iter().collect(),
        included: inputs.features.included.clone(),
        cross: inputs.cross,
        enabled_features: super::enabled_features_by_package(inputs.features),
        build_flags,
        standard_flag_conflicts,
        platform: inputs.build_machine,
        toolchain: inputs.toolchain,
        detection_report: inputs.detection,
    })
}

/// Resolve and detect the build machine's toolchain for a `--target`
/// build's build context.  `HOST_CC`, `HOST_CXX`, and `HOST_AR` stand
/// in for `CC`, `CXX`, and `AR`, and the manifest's `[toolchain]`
/// tables apply as evaluated for the build machine; the CLI flags, the
/// environment's `CC`, `CXX`, and `AR`, and config describe the target.
fn build_machine_toolchain(
    graph: &PackageGraph,
    build_machine: &cabin_core::TargetPlatform,
    platform: &cabin_core::TargetPlatform,
    target: &cabin_core::ToolchainDetectionReport,
) -> Result<(
    cabin_core::ResolvedToolchain,
    cabin_core::ToolchainDetectionReport,
)> {
    cabin_toolchain::check_build_machine_beside(target, platform, build_machine)?;
    let manifest = workspace_toolchain_settings(graph);
    let selection = cabin_core::ToolchainSelection::default();
    let mut inputs =
        cabin_toolchain::ResolveInputs::from_process(&selection, &manifest, build_machine);
    inputs.env = Box::new(|var| {
        std::env::var_os(match var {
            "CC" => "HOST_CC",
            "CXX" => "HOST_CXX",
            "AR" => "HOST_AR",
            other => other,
        })
    });
    let toolchain = cabin_toolchain::resolve_toolchain(&inputs).context(
        "failed to resolve the build machine's toolchain for `[build-dependencies]` (set HOST_CC, HOST_CXX, and HOST_AR to choose it)",
    )?;
    let report = cabin_toolchain::detect_toolchain(&toolchain, &cabin_toolchain::ProcessRunner)
        .context("failed to detect the build machine's toolchain for `[build-dependencies]`")?;
    cabin_toolchain::check_build_machine_compilers(
        &report,
        target,
        build_machine,
        &cabin_toolchain::ProcessRunner,
    )?;
    cabin_build::validate_toolchain_for_backend(&toolchain, &report)
        .context("the build machine's toolchain for `[build-dependencies]` cannot build")?;
    Ok((toolchain, report))
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
    let plan_graph = super::plan(&super::PlanRequest {
        graph: &prepared.graph,
        toolchain: &prepared.toolchain,
        build_flags: &prepared.prep.build_flags,
        language_standards: &prepared.language_standards,
        standard_flag_conflicts: &prepared.prep.standard_flag_conflicts,
        system_targets: &HashMap::new(),
        build_dir: prepared.build_dir.clone(),
        platform: prepared.platform.clone(),
        profile: prepared.profile.clone(),
        selected,
        selected_packages: Some(&prepared.resolved_selection.packages),
        compiler_wrapper: prepared.prep.compiler_wrapper.as_ref(),
        dialect: cabin_build::Dialect::from_compiler_kind(
            prepared.detection_report.cxx.identity.kind,
        ),
        archiver_windows_rsp_quoting: prepared.detection_report.ar.identity.windows_rsp_quoting(),
        msvc_external_includes: cabin_build::msvc_external_includes_supported(
            &prepared.detection_report,
            prepared.approx_standards.has_c_sources(),
        ),
        clang_cl: cabin_build::clang_cl_languages(&prepared.detection_report),
        enabled_features: Some(&prepared.enabled_features),
        standard_compat: true,
        clang_target: prepared.clang_target.clone(),
        sysroot: prepared.sysroot.clone(),
        build_context: false,
    })?;
    // `cabin check` reuses the build graph but rewrites it into a
    // syntax-only check (no codegen, no link) scoped to the selected
    // workspace packages' own translation units.
    let plan_graph = if check {
        let profile_dir =
            cabin_build::layout::profile_dir(&prepared.build_dir, &prepared.profile.name);
        // The planner's own output dirs, which the check-graph filter
        // compares against.
        let selected_pkg_dirs: Vec<PathBuf> = prepared
            .resolved_selection
            .packages
            .iter()
            .map(|&idx| {
                cabin_build::layout::package_dir(
                    &profile_dir,
                    false,
                    &prepared.graph.packages[idx].package.name,
                )
            })
            .collect();
        cabin_build::into_check_graph(plan_graph, &selected_pkg_dirs)
    } else {
        plan_graph
    };
    let target_standards = cabin_build::requested_standards_of(&plan_graph);
    let mut plan_graph = plan_graph;
    let mut context_standards = None;
    if let Some(context) = &prepared.build_context {
        let context_graph = plan_build_context(prepared, context)?;
        context_standards = Some((context, cabin_build::requested_standards_of(&context_graph)));
        plan_graph.append(context_graph);
    }
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
    cabin_build::validate_output_paths(&plan_graph)?;
    cabin_build::validate_toolchain_standards(
        &prepared.toolchain,
        &prepared.detection_report,
        &target_standards,
    )?;
    if let Some((context, standards)) = context_standards {
        cabin_build::validate_toolchain_standards(
            &context.toolchain,
            &context.detection_report,
            &standards,
        )?;
    }
    Ok(plan_graph)
}

/// Plan the build context: the build dependencies' default targets,
/// built for the build machine with its toolchain.
fn plan_build_context(
    prepared: &PreparedWorkspace,
    context: &BuildContextPrep,
) -> Result<cabin_build::BuildGraph> {
    // shortcut: build-context units get no pkg-config probe (stage 06's
    // per-context providers), so a system dependency there fails here.
    super::system_deps::reject_build_context_system_deps(
        &prepared.graph,
        &context.platform,
        &context.included,
    )?;
    super::plan(&super::PlanRequest {
        graph: &prepared.graph,
        toolchain: &context.toolchain,
        build_flags: &context.build_flags,
        language_standards: &prepared.language_standards,
        standard_flag_conflicts: &context.standard_flag_conflicts,
        system_targets: &HashMap::new(),
        build_dir: prepared.build_dir.clone(),
        platform: context.platform.clone(),
        profile: prepared.profile.clone(),
        selected: None,
        selected_packages: Some(&context.roots),
        compiler_wrapper: prepared.prep.compiler_wrapper.as_ref(),
        dialect: cabin_build::Dialect::from_compiler_kind(
            context.detection_report.cxx.identity.kind,
        ),
        archiver_windows_rsp_quoting: context.detection_report.ar.identity.windows_rsp_quoting(),
        msvc_external_includes: cabin_build::msvc_external_includes_supported(
            &context.detection_report,
            cabin_build::collect_requested_standards(
                &prepared.graph,
                &context.included,
                &prepared.language_standards,
                &BTreeSet::new(),
            )
            .has_c_sources(),
        ),
        clang_cl: cabin_build::clang_cl_languages(&context.detection_report),
        enabled_features: Some(&context.enabled_features),
        standard_compat: true,
        clang_target: None,
        sysroot: None,
        build_context: true,
    })
    .context("cannot plan the `[build-dependencies]` for the build machine")
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

/// Translate `--profile` / `--release` into a typed
/// [`cabin_core::ProfileSelection`].  Precedence: `--profile NAME`,
/// then `--release` (a compatibility alias for `--profile release`;
/// clap's `conflicts_with` rejects passing both), then the config's
/// `build.profile`, then the built-in `dev` profile.
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
    platform: &cabin_core::TargetPlatform,
) -> Result<cabin_core::ResolvedToolchain> {
    let manifest_toolchain_settings = workspace_toolchain_settings(graph);
    let config_toolchain_layer = crate::cli::config::toolchain_layer(effective_config);
    let mut toolchain_inputs = cabin_toolchain::ResolveInputs::from_process(
        selection,
        &manifest_toolchain_settings,
        platform,
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
/// a fail-soft caller (`cabin metadata`) downgrades the error to a
/// warning.
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
    platform: &cabin_core::TargetPlatform,
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
            platform,
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
pub(crate) fn augment_build_flags(
    graph: &PackageGraph,
    platform: &cabin_core::TargetPlatform,
    dev_for: &BTreeSet<String>,
    build_flags: HashMap<usize, cabin_core::ResolvedProfileFlags>,
    reporter: Reporter,
) -> Result<HashMap<usize, cabin_core::ResolvedProfileFlags>> {
    let build_flags = crate::cli::system_deps::augment_build_flags_with_system_deps(
        graph,
        platform,
        dev_for,
        build_flags,
        reporter,
    )?;
    crate::cli::env_flags::augment_build_flags_with_env(
        graph,
        build_flags,
        |k| std::env::var_os(k),
        reporter,
    )
}

/// Resolve a `BuildConfiguration` for every package in the graph.
/// A package the selection includes carries the features `features`
/// enabled on it - the CLI selection on selected packages plus every
/// dependency edge's requests; one outside the selection, such as an
/// unselected workspace sibling, shows its declared defaults.
pub(crate) fn resolve_build_configurations(
    graph: &PackageGraph,
    features: &cabin_feature::FeatureResolution,
    profile: &cabin_core::ResolvedProfile,
    toolchain: &cabin_core::ToolchainSummary,
    build_flags: &HashMap<usize, cabin_core::ResolvedProfileFlags>,
) -> Result<HashMap<usize, cabin_core::BuildConfiguration>> {
    let mut out: HashMap<usize, cabin_core::BuildConfiguration> = HashMap::new();
    for (idx, pkg) in graph.packages.iter().enumerate() {
        let pkg_request = if features.included.contains(&idx) {
            cabin_core::SelectionRequest {
                features: features
                    .for_package(idx)
                    .enabled_features
                    .iter()
                    .filter(|name| name.as_str() != cabin_core::DEFAULT_FEATURE_KEY)
                    .cloned()
                    .collect(),
                all_features: false,
                no_default_features: true,
            }
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
            targets: &pkg.package.targets,
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
