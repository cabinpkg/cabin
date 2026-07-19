use super::{
    ArtifactPipelineRequest, BTreeSet, BuildArgs, Context, PathBuf, PlanRequest,
    RegistryPackageSource, Reporter, Result, absolutise, bail, build_selection_request,
    build_workspace_selection, closure_has_versioned_deps_excluding_patches,
    collect_patched_versioned_deps, compiler_wrapper_override_from_args,
    compute_feature_resolution, plan, profile_descriptor, profile_selection_for_build,
    resolve_build_configurations, resolve_invocation_manifest, resolve_toolchain_layered,
    run_artifact_pipeline, toolchain_selection_from_args, workspace_compiler_wrapper_settings,
    workspace_profile_definitions,
};

/// Whether [`build`] produces real artifacts (`cabin build`) or only
/// syntax-checks the selected workspace sources (`cabin check`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BuildMode {
    Build,
    Check,
}

pub(super) fn build(
    args: &BuildArgs,
    reporter: Reporter,
    mode: BuildMode,
    color: cabin_core::ColorChoice,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;

    // First-pass load: needed to detect versioned dependencies
    // before we know whether we have to fetch anything.  This load
    // also surfaces manifest / workspace errors before we touch
    // the index.
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
    // Resolve patch policy before we look at the index.  Patched
    // names are excluded from the closure / artifact pipeline
    // because they ship from a local working copy.
    let active_patches =
        crate::cli::patch::load_active_patches(&initial_graph, &effective_config, args.no_patches)?;
    let patched_names = active_patches.owned_patched_names();
    let resolved_index_source = crate::cli::config::resolve_index_source(
        args.index_path.as_deref(),
        args.index_url.as_deref(),
        &effective_config,
    )?;
    crate::cli::config::enforce_offline_index_source(offline, resolved_index_source.as_ref())?;
    let resolved_cache_dir =
        crate::cli::config::resolve_cache_dir(args.cache_dir.as_deref(), &effective_config);

    // only the *selected closure* drives the index
    // requirement.  An unrelated workspace member's versioned dep
    // must not force the user to pass `--index-path` when
    // `cabin build -p selected` is run on a C/C++-only selection.
    let initial_resolved_selection =
        cabin_workspace::resolve_package_selection(&initial_graph, &workspace_selection)?;
    let initial_request =
        build_selection_request(&args.features, args.all_features, args.no_default_features);
    let initial_features = compute_feature_resolution(
        &initial_graph,
        &initial_resolved_selection,
        &initial_request,
        &BTreeSet::new(),
    )?;
    let dev_for: BTreeSet<String> = BTreeSet::new();
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
                index_source: &inputs.index_source,
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

    // Re-load the workspace, this time stitching in the resolved
    // registry packages plus active patches.  When both lists are
    // empty this is identical to the first-pass load.
    //
    // `strict_packages` controls which packages require their
    // versioned / port deps to be satisfied.  The set is the
    // selection's closure on `initial_graph` plus every package
    // that the resolver fetched into `registry`.  The closure
    // alone misses any package reached only after resolution -
    // in particular, transitive registry packages a patched
    // manifest pulled in via a version dep that did not exist on
    // the upstream package.  Without the registry extension those
    // packages would parent a missing-registry / missing-port
    // edge under the scoped policy and silently drop it, leaving
    // the build to fail later with a less actionable diagnostic.
    // `patched_names` is folded in defensively too - closure
    // already reaches the patched manifests now, but the explicit
    // add keeps the strict set correct if anything in the
    // chicken-and-egg loading order ever shifts.
    let mut strict_packages: BTreeSet<String> =
        initial_resolved_selection.closure_package_names(&initial_graph);
    strict_packages.extend(patched_names.iter().cloned());
    strict_packages.extend(registry.iter().map(|r| r.name.as_str().to_owned()));
    let patched_sources = active_patches.workspace_sources();
    let graph = cabin_workspace::load_workspace_with_options(
        &manifest_path,
        &cabin_workspace::WorkspaceLoadOptions {
            registry: &registry,
            patches: &patched_sources,
            ports: &port_sources,
            registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&strict_packages),
            include_dev_for: &BTreeSet::new(),
            port_policy: cabin_workspace::PortPolicy::TolerateExcept(&strict_packages),
        },
    )?;

    // Resolve the build directory.  Precedence:
    // `--build-dir` > `CABIN_BUILD_DIR` env var
    // > `[paths] build-dir` config setting > built-in default.
    let (build_dir_input, _build_dir_source) = crate::cli::config::resolve_build_dir_with_env(
        args.build_dir.as_deref(),
        &effective_config,
    );
    let build_dir = absolutise(&build_dir_input)
        .with_context(|| format!("failed to resolve build dir {}", build_dir_input.display()))?;

    let host_platform = cabin_core::TargetPlatform::current();
    let toolchain_selection = toolchain_selection_from_args(&args.toolchain)?;
    let toolchain = resolve_toolchain_layered(
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
        cabin_toolchain::detect_toolchain(&toolchain, &cabin_toolchain::ProcessRunner)
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    // Resolve the workspace package selection up-front.  The planner
    // consumes the selected indices through `PlanRequest::selected_packages`
    // so default-target enumeration narrows to the picked packages instead
    // of every primary - and the backend checks below scope to the
    // selected closure so an unselected member's C source or pkg-config
    // dependency never gates `cabin build -p other`.
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;
    let selected_closure = resolved_selection.closure(&graph);
    // Package-level approximation used only for the MSVC
    // `/external:I` fallback decision below; the authoritative
    // toolchain validation runs against the *planned* compiles
    // right after `plan()`, so an unbuilt sibling target's standard
    // never gates the build. `cabin build` does not activate
    // dev-only targets, matching the dev-dep activation rule.
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

    // Translate `--profile` / `--release` into a typed selection
    // (clap's `conflicts_with` already rejects the two-flag form).
    // The workspace root manifest's `[profile.<name>]` tables are
    // the only source of profile definitions; a `build.profile`
    // setting in any active config file slots between the CLI
    // flag and the built-in `dev` default.
    let profile_selection = profile_selection_for_build(args, &effective_config)?;
    let manifest_profiles = workspace_profile_definitions(&graph);
    let profile = cabin_core::resolve_profile(&profile_selection, &manifest_profiles)?;

    // Per-package resolved build flags.  Each package's own
    // `[profile]` / `[target.'cfg(...)'.profile]` plus the active
    // profile's `[profile.<name>]` block compose into a
    // `ResolvedProfileFlags`.  Computed up-front so the planner
    // and metadata view see the same values.
    // `cabin build` does not opt into dev-dep activation; dev-kind
    // system deps stay declaration-only here so the probe step
    // matches the Cabin-package activation rule.
    let dev_for: BTreeSet<String> = BTreeSet::new();
    // The MSVC backend cannot consume pkg-config's GNU-style flags;
    // reject a build that would need them before probing.  Scoped to the
    // selected closure so an unrelated member's system dependency does not
    // block `cabin build -p other` under MSVC.
    crate::cli::system_deps::ensure_dialect_supports_system_deps(
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
        build_selection_request(&args.features, args.all_features, args.no_default_features);
    let feature_resolution = compute_feature_resolution(
        &graph,
        &resolved_selection,
        &selection_request,
        &BTreeSet::new(),
    )?;

    // Per-package build flags + the (fail-hard) compiler
    // wrapper, folded into a toolchain summary.  Shared with
    // `run` / `test` / `explain build-config` via `build_prep`.
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
    let enabled_features = crate::cli::enabled_features_by_package(&feature_resolution);
    let plan_graph = plan(&PlanRequest {
        graph: &graph,
        toolchain: &toolchain,
        build_flags: &prep.build_flags,
        language_standards: &language_standards,
        standard_flag_conflicts: &prep.standard_flag_conflicts,
        build_dir: build_dir.clone(),
        profile: profile.clone(),
        selected: None,
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
    // `cabin check` reuses the build graph but rewrites it into a
    // syntax-only check (no codegen, no link) scoped to the selected
    // workspace packages' own translation units.
    let plan_graph = if matches!(mode, BuildMode::Check) {
        let packages_root = build_dir.join(profile.name.as_str()).join("packages");
        // Fold `path_components` so the scoping roots stay
        // byte-identical to the planner's `packages/<scope>/<name>`
        // output dirs, which the check-graph filter compares against.
        let selected_pkg_dirs: Vec<PathBuf> = resolved_selection
            .packages
            .iter()
            .map(|&idx| {
                graph.packages[idx]
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
    crate::cli::standard_compat::report(
        &plan_graph.standard_compat_violations,
        color,
        &lockfile_pinned,
    )?;
    // Validate the plan-dependent toolchain contract against exactly
    // the compiles the final graph runs - after the check rewrite
    // (which drops dependency compiles) and before any Ninja file is
    // written.
    cabin_build::validate_planned_standards(&plan_graph)?;
    cabin_build::validate_toolchain_standards(
        &toolchain,
        &detection_report,
        &cabin_build::requested_standards_of(&plan_graph),
    )?;

    // Profile-aware Ninja root: `build/<profile>/build.ninja`
    // and `build/<profile>/compile_commands.json`.  Keeps dev /
    // release / custom builds from overwriting each other and
    // matches the per-package output tree the planner emits.
    // Build-specific verbose context (the shared "wrote …" /
    // "invoking …" lines are emitted by `invoke_ninja_and_report`).
    reporter.verbose(format_args!("cabin: profile = {}", profile.name.as_str()));
    reporter.verbose(format_args!("cabin: build dir = {}", build_dir.display()));
    reporter.verbose(format_args!("cabin: c++ compiler = {}", toolchain.cxx.path));
    if let Some(cc) = &toolchain.cc {
        reporter.very_verbose(format_args!("cabin: c compiler = {}", cc.path));
    }
    reporter.very_verbose(format_args!("cabin: archiver = {}", toolchain.ar.path));

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

    // Cargo-style `Finished` summary: profile name, the resolved
    // optimization / debuginfo descriptor, and the wall-clock
    // duration the Ninja invocation took.
    reporter.status(
        "Finished",
        format_args!(
            "`{}` profile [{}] target(s) in {:.2}s",
            profile.name.as_str(),
            profile_descriptor(&profile),
            elapsed.as_secs_f64(),
        ),
    );

    Ok(())
}
