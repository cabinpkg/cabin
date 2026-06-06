use super::{
    ArtifactPipelineRequest, BTreeSet, FetchArgs, LockMode, Reporter, ResolutionRequest,
    ResolveArgs, Result, UpdateArgs, WorkspaceSelectionArgsForUpdate, bail,
    build_selection_request, build_workspace_selection, cache_dir_for,
    closure_has_versioned_deps_excluding_patches, collect_patched_versioned_deps,
    compute_feature_resolution, emit_fetch_output, lock_mode_for_flags,
    resolve_invocation_manifest, run_artifact_pipeline, run_resolution,
};

pub(super) fn resolve(args: &ResolveArgs, reporter: Reporter) -> Result<()> {
    let mode = lock_mode_for_flags(args.locked, args.frozen);
    // Both --locked and --frozen forbid writing the lockfile. The
    // distinction becomes meaningful once a fetcher / cache exists for
    // `--frozen` to refuse to populate; today they behave the same.
    let allow_write = !(args.locked || args.frozen);
    if args.frozen && args.index_url.is_some() {
        bail!(
            "cannot use --index-url with --frozen: there is no persistent HTTP index metadata cache, so a frozen run would have to perform network fetches it is not allowed to perform"
        );
    }
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let selection_request =
        build_selection_request(&args.features, args.all_features, args.no_default_features);
    run_resolution(
        &ResolutionRequest {
            manifest_path: &manifest_path,
            index_path: args.index_path.as_deref(),
            index_url: args.index_url.as_deref(),
            format: args.format,
            mode,
            allow_write,
            frozen: args.frozen,
            update_package: None,
            selection: workspace_selection,
            selection_request,
            no_patches: args.no_patches,
            offline: args.offline,
        },
        reporter,
    )
}

pub(super) fn update(args: &UpdateArgs, reporter: Reporter) -> Result<()> {
    let mode = match &args.package {
        Some(name) => LockMode::UpdatePackage(name.clone()),
        None => LockMode::UpdateAll,
    };
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    // `cabin update` keeps its `--package <name>` flag for the
    // dep-targeted-update meaning. Workspace member scoping uses
    // the dedicated bundle without `-p`.
    let workspace_selection = build_update_workspace_selection(&args.workspace_selection);
    run_resolution(
        &ResolutionRequest {
            manifest_path: &manifest_path,
            index_path: args.index_path.as_deref(),
            index_url: args.index_url.as_deref(),
            format: args.format,
            mode,
            allow_write: true,
            frozen: false,
            update_package: args.package.as_deref(),
            selection: workspace_selection,
            selection_request: cabin_core::SelectionRequest::default(),
            no_patches: args.no_patches,
            offline: args.offline,
        },
        reporter,
    )
}

/// Convert `WorkspaceSelectionArgsForUpdate` (the
/// `cabin update`-specific bundle without `-p / --package`) into
/// the same `PackageSelection` shape every other workspace-aware
/// command consumes.
pub(super) fn build_update_workspace_selection(
    args: &WorkspaceSelectionArgsForUpdate,
) -> cabin_workspace::PackageSelection {
    use cabin_workspace::SelectionMode;
    let mode = if args.workspace {
        SelectionMode::WholeWorkspace
    } else if args.default_members {
        SelectionMode::DefaultMembers
    } else {
        SelectionMode::CurrentPackage
    };
    cabin_workspace::PackageSelection {
        mode,
        exclude: args.exclude.clone(),
    }
}

pub(super) fn fetch(args: &FetchArgs, reporter: Reporter) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let offline_pre = crate::cli::config::effective_offline(args.offline)?;
    let fetch_selection = build_workspace_selection(&args.workspace_selection);
    let (_port_sources, initial_graph) = crate::cli::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        args.cache_dir.as_deref(),
        offline_pre,
        args.frozen,
        false,
        &fetch_selection,
        args.no_patches,
    )?;
    let effective_config = crate::cli::config::load_effective_config(&initial_graph)?;
    let active_patches =
        crate::cli::patch::load_active_patches(&initial_graph, &effective_config, args.no_patches)?;
    let patched_names = active_patches.owned_patched_names();
    // validate the workspace selection up-front so a typo
    // like `--package missing` fails even when there are no
    // versioned deps to fetch.
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&initial_graph, &workspace_selection)?;
    // `cabin fetch` does not currently expose feature flags,
    // so feature resolution runs with the documented defaults
    // (each selected root's `default` feature, no extras). This
    // still excludes disabled optional dependencies from the
    // index-requirement check below — the user opts into them
    // via `cabin build --features ...` / `cabin resolve
    // --features ...`.
    let initial_features = compute_feature_resolution(
        &initial_graph,
        &resolved_selection,
        &cabin_core::SelectionRequest::default(),
    )?;

    // scope the index requirement to the selected
    // closure. Unrelated members' versioned deps no longer force a
    // user who passed `--package <selected>` to also pass
    // `--index-path`. Patched manifests contribute their own
    // versioned deps too, so a workspace whose only versioned
    // edge comes from `[patch]` still needs the index.
    let dev_for: BTreeSet<String> = BTreeSet::new();
    let patched_root_deps_preview =
        collect_patched_versioned_deps(&active_patches, &patched_names)?;
    if patched_root_deps_preview.is_empty()
        && !closure_has_versioned_deps_excluding_patches(
            &initial_graph,
            &resolved_selection,
            &initial_features,
            &patched_names,
            &dev_for,
        )
    {
        emit_fetch_output(
            &[],
            args.format,
            &cache_dir_for(&manifest_path, args.cache_dir.as_deref()).unwrap_or_default(),
            &manifest_path,
        )?;
        return Ok(());
    }

    let resolved_index_source = crate::cli::config::resolve_index_source(
        args.index_path.as_deref(),
        args.index_url.as_deref(),
        &effective_config,
    )?;
    let fetch_offline = crate::cli::config::effective_offline(args.offline)?;
    crate::cli::config::enforce_offline_index_source(
        fetch_offline,
        resolved_index_source.as_ref(),
    )?;
    let resolved_cache_dir =
        crate::cli::config::resolve_cache_dir(args.cache_dir.as_deref(), &effective_config);
    let Some(index_source) = resolved_index_source.as_ref() else {
        bail!(
            "versioned dependencies require --index-path, --index-url, or a `[registry]` config setting"
        );
    };
    let inputs = crate::cli::config::resolve_pipeline_inputs(
        index_source,
        &effective_config,
        &manifest_path,
        args.cache_dir.as_deref(),
        resolved_cache_dir.as_ref(),
        fetch_offline,
        args.locked,
        args.frozen,
        args.no_patches,
        false,
    )?;

    let fetch_request = cabin_core::SelectionRequest::default();
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
        selection_request: &fetch_request,
        patched_names: &patched_names,
        active_patches: &active_patches,
        source_replacements: &effective_config.source_replacements,
        no_patches: args.no_patches,
        dev_for: &dev_for,
    })?;

    emit_fetch_output(
        &pipeline.fetched,
        args.format,
        &inputs.cache_dir,
        &manifest_path,
    )?;
    Ok(())
}
