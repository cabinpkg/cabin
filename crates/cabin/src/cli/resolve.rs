use std::path::{Path, PathBuf};

use cabin_core::PackageName;
use cabin_lockfile::Lockfile;
use cabin_resolver::{LockedVersion, ResolveInput, ResolveOutput, ResolvedPackage, ResolvedSource};

use super::{
    ArtifactPipelineRequest, BTreeSet, Context, FetchArgs, LockMode, Reporter, ResolveArgs,
    ResolveFormat, Result, UpdateArgs, WorkspaceSelectionArgsForUpdate, absolutise, bail,
    build_selection_request, build_workspace_selection, cache_dir_for,
    closure_has_versioned_deps_excluding_patches, collect_closure_versioned_deps_excluding_patches,
    collect_patched_versioned_deps, compute_feature_resolution, emit_fetch_output, load_http_index,
    load_local_index, lock_mode_for_flags, lockfile_from_resolution, lockfile_path_for,
    merge_versioned_deps, resolve_invocation_manifest, run_artifact_pipeline,
    selected_resolution_packages,
};

pub(super) fn resolve(
    args: &ResolveArgs,
    reporter: Reporter,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<()> {
    let mode = lock_mode_for_flags(args.locked, args.frozen);
    // Both --locked and --frozen forbid writing the lockfile.  The
    // distinction becomes meaningful once a fetcher / cache exists for
    // `--frozen` to refuse to populate; today they behave the same.
    let allow_write = !(args.locked || args.frozen);
    if args.frozen && args.index_url.is_some() {
        bail!(crate::cli::FROZEN_INDEX_URL_ERR);
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
            experimental_features,
        },
        reporter,
    )
}

pub(super) fn update(
    args: &UpdateArgs,
    reporter: Reporter,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<()> {
    let mode = match &args.package {
        Some(name) => LockMode::UpdatePackage(name.clone()),
        None => LockMode::UpdateAll,
    };
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    // `cabin update` keeps its `--package <name>` flag for the
    // dep-targeted-update meaning.  Workspace member scoping uses
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
            experimental_features,
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

pub(super) fn fetch(
    args: &FetchArgs,
    reporter: Reporter,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let offline = crate::cli::config::effective_offline(args.offline)?;
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let (_port_sources, initial_graph) = crate::cli::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        args.cache_dir.as_deref(),
        offline,
        args.frozen,
        false,
        &workspace_selection,
        args.no_patches,
    )?;
    let effective_config = crate::cli::config::load_effective_config(&initial_graph)?;
    let active_patches =
        crate::cli::patch::load_active_patches(&initial_graph, &effective_config, args.no_patches)?;
    let patched_names = active_patches.owned_patched_names();
    // validate the workspace selection up-front so a typo
    // like `--package missing` fails even when there are no
    // versioned deps to fetch.
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&initial_graph, &workspace_selection)?;
    // `cabin fetch` does not currently expose feature flags,
    // so feature resolution runs with the documented defaults
    // (each selected root's `default` feature, no extras).  This
    // still excludes disabled optional dependencies from the
    // index-requirement check below - the user opts into them
    // via `cabin build --features ...` / `cabin resolve
    // --features ...`.
    let initial_features = compute_feature_resolution(
        &initial_graph,
        &resolved_selection,
        &cabin_core::SelectionRequest::default(),
        &BTreeSet::new(),
    )?;

    // scope the index requirement to the selected
    // closure.  Unrelated members' versioned deps no longer force a
    // user who passed `--package <selected>` to also pass
    // `--index-path`.  Patched manifests contribute their own
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
            &cache_dir_for(args.cache_dir.as_deref()).unwrap_or_default(),
            &manifest_path,
        )?;
        return Ok(());
    }

    let resolved_index_source = crate::cli::config::resolve_index_source(
        args.index_path.as_deref(),
        args.index_url.as_deref(),
        &effective_config,
    )?;
    crate::cli::config::enforce_offline_index_source(offline, resolved_index_source.as_ref())?;
    let resolved_cache_dir =
        crate::cli::config::resolve_cache_dir(args.cache_dir.as_deref(), &effective_config);
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
        incompatible_standards: crate::cli::config::resolve_incompatible_standards(
            &effective_config,
        )?,
        no_patches: args.no_patches,
        dev_for: &dev_for,
        experimental_features,
    })?;

    emit_fetch_output(
        &pipeline.fetched,
        args.format,
        &inputs.cache_dir,
        &manifest_path,
    )?;
    Ok(())
}

struct ResolutionRequest<'a> {
    manifest_path: &'a Path,
    index_path: Option<&'a Path>,
    index_url: Option<&'a str>,
    format: ResolveFormat,
    mode: LockMode,
    allow_write: bool,
    /// Whether the original invocation was `cabin resolve --frozen`.
    /// `LockMode::Locked` intentionally covers both `--locked` and
    /// `--frozen`, so keep this bit to enforce frozen-only network
    /// restrictions after config and source replacement are applied.
    frozen: bool,
    /// Used only by `cabin update --package <name>` to validate that the
    /// named package exists in the manifest's dependency
    /// graph.
    update_package: Option<&'a str>,
    /// Workspace selection that contributes versioned deps
    /// to the resolution.
    selection: cabin_workspace::PackageSelection,
    /// Feature flags from the CLI.  Drives optional-dependency
    /// inclusion.
    selection_request: cabin_core::SelectionRequest,
    /// Whether `--no-patches` was supplied for this command.
    no_patches: bool,
    /// Whether `--offline` was supplied for this command.
    offline: bool,
    /// Experimental `-Z` features enabled for this invocation.
    /// Consulted by index loading, which gates the remote-registry
    /// `config.json` fields on `-Z remote-registry`.
    experimental_features: &'a cabin_core::ExperimentalFeatures,
}

fn run_resolution(request: &ResolutionRequest<'_>, reporter: Reporter) -> Result<()> {
    let manifest_path = absolutise(request.manifest_path)
        .with_context(|| format!("failed to resolve {}", request.manifest_path.display()))?;
    let offline = crate::cli::config::effective_offline(request.offline)?;
    let (_port_sources, graph) = crate::cli::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        None,
        offline,
        request.frozen,
        false,
        &request.selection,
        request.no_patches,
    )?;
    // CLI flags win; otherwise consult the merged effective
    // config for a `[registry]` default.  The orchestration layer
    // owns the final reconciliation; cabin-resolver / cabin-index
    // see only a concrete index source.
    let effective_config = crate::cli::config::load_effective_config(&graph)?;
    let active_patches =
        crate::cli::patch::load_active_patches(&graph, &effective_config, request.no_patches)?;
    let patched_names = active_patches.owned_patched_names();
    let resolved_index_source = crate::cli::config::resolve_index_source(
        request.index_path,
        request.index_url,
        &effective_config,
    )?;
    let resolution_offline = crate::cli::config::effective_offline(request.offline)?;
    crate::cli::config::enforce_offline_index_source(
        resolution_offline,
        resolved_index_source.as_ref(),
    )?;
    let (config_index_path, config_index_url): (Option<PathBuf>, Option<String>) =
        match resolved_index_source.as_ref() {
            Some(source) => {
                let initial = crate::cli::config::index_source_kind_to_locator(&source.kind);
                let resolved = crate::cli::patch::apply_source_replacement(
                    initial,
                    &effective_config,
                    request.no_patches,
                )?;
                crate::cli::config::enforce_offline_post_replacement(
                    resolution_offline,
                    &resolved,
                )?;
                crate::cli::patch::locator_to_index_inputs(&resolved.resolved)
            }
            None => (None, None),
        };
    let effective_index_path = config_index_path.as_deref();
    let effective_index_url = config_index_url.as_deref();
    if request.frozen && effective_index_url.is_some() {
        bail!(crate::cli::FROZEN_INDEX_URL_ERR);
    }

    // gather versioned deps from the selected primary
    // packages, including non-root workspace members.  Pure-workspace roots
    // (no `[package]`) work too - they take a synthetic identity.
    let resolved_selection = selected_resolution_packages(&graph, &request.selection)?;
    let features = compute_feature_resolution(
        &graph,
        &resolved_selection,
        &request.selection_request,
        &BTreeSet::new(),
    )?;
    let dev_for: BTreeSet<String> = BTreeSet::new();
    let mut root_deps = collect_closure_versioned_deps_excluding_patches(
        &graph,
        &resolved_selection,
        &features,
        &patched_names,
        &dev_for,
    )?;
    // Patched manifests live outside the workspace graph, so
    // their own versioned deps never reached the closure walk.
    // Fold them in so `cabin resolve` (and `--package` validation
    // below) sees the same root set the artifact pipeline does.
    let patched_root_deps = collect_patched_versioned_deps(&active_patches, &patched_names)?;
    merge_versioned_deps(&mut root_deps, patched_root_deps)?;
    let (root_name, root_version) = match graph.root_package {
        Some(idx) => (
            graph.packages[idx].package.name.clone(),
            graph.packages[idx].package.version.clone(),
        ),
        None => cabin_workspace::synthetic_root_identity(&graph),
    };

    let lockfile_path = lockfile_path_for(&manifest_path);

    // validate `--package` (the dep-targeted-update
    // flag on `cabin update`) before short-circuiting on an
    // empty resolution.  Otherwise an unknown name like
    // `cabin update --package missing` silently succeeds when
    // the workspace happens to have no versioned deps.
    if let Some(name) = request.update_package
        && !root_deps.contains_key(
            &PackageName::new(name)
                .map_err(|err| anyhow::anyhow!("invalid --package value {name:?}: {err}"))?,
        )
    {
        // `cabin update --package <name>` targets a *direct*
        // versioned dependency only.  The matching set is the
        // resolver's input - any name declared under
        // `[dependencies]` (the
        // kinds that participate in ordinary resolution).
        // Refreshing a transitive locked package requires
        // re-running `cabin update` without `--package`, or
        // scoping with `--workspace` / `--default-members`.
        // `root_deps` was gathered from every *selected* package
        // (plus active patches), so the message names the actual
        // lookup scope rather than the workspace root.
        let scope = match resolved_selection.packages.as_slice() {
            [idx] => format!("`{}`", graph.packages[*idx].package.name.as_str()),
            _ => "any selected package".to_owned(),
        };
        bail!(
            "package {name:?} is not a direct versioned dependency of {scope}; `cabin update --package` only refreshes direct dependencies declared under `[dependencies]`",
        );
    }

    // Read the lockfile up-front so the patch / source-replacement
    // staleness check below can apply even when the active patch
    // set covers every versioned dep (and the resolver itself has
    // nothing to do).
    let existing_lockfile: Option<Lockfile> = if lockfile_path.is_file() {
        Some(
            cabin_lockfile::read_lockfile(&lockfile_path)
                .with_context(|| format!("failed to read {}", lockfile_path.display()))?,
        )
    } else {
        None
    };

    // Patch / source-replacement state recorded into the new
    // lockfile and compared against the existing lockfile under
    // `--locked`.  Computed early so the no-versioned-deps fast
    // path below can still enforce the staleness check: if the
    // user added or removed a patch since the lockfile was
    // written, `--locked` must refuse, even though the resolver
    // itself would otherwise have nothing to do.
    let active_patch_records = crate::cli::patch::lockfile_patches(&active_patches);
    let active_replacement_records = crate::cli::patch::lockfile_source_replacements(
        &effective_config.source_replacements,
        request.no_patches,
    );
    if matches!(request.mode, LockMode::Locked)
        && let Some(prev) = &existing_lockfile
        && !prev.matches_patch_state(&active_patch_records, &active_replacement_records)
    {
        bail!(
            "--locked cannot be used because active patch / source-replacement policy differs from {}; re-run without --locked to refresh the lockfile",
            lockfile_path.display()
        );
    }

    if root_deps.is_empty() {
        // No versioned deps to resolve.  Print a clear empty result
        // and never touch the lockfile.  The patch-staleness check
        // above already ran, so `--locked` will already have bailed
        // if the patch set diverged from the lockfile's record.
        let output = ResolveOutput {
            packages: vec![ResolvedPackage {
                name: root_name,
                version: root_version,
                source: ResolvedSource::Root,
            }],
            held_back: Vec::new(),
        };
        emit_resolve_output(&output, request.format)?;
        return Ok(());
    }

    // Locked mode (with versioned deps) still requires an existing
    // lockfile - the staleness check above is a no-op when one is
    // missing.
    if existing_lockfile.is_none() && matches!(request.mode, LockMode::Locked) {
        bail!(
            "cannot resolve with --locked because {} does not exist",
            lockfile_path.display()
        );
    }

    let index = match (effective_index_path, effective_index_url) {
        (None, None) => {
            bail!(crate::cli::VERSIONED_DEPS_REQUIRE_INDEX)
        }
        (Some(path), None) => load_local_index(path, request.experimental_features)?,
        // The resolve pipeline performs no artifact downloads, so the
        // HTTP client the helper returns for connection reuse is
        // dropped here.
        (None, Some(url)) => {
            load_http_index(url, &root_deps, request.experimental_features, reporter)?.0
        }
        (Some(_), Some(_)) => {
            unreachable!("cli::config::resolve_index_source guarantees only one variant is set")
        }
    };

    let resolver_mode = request.mode.resolve_mode()?;

    let mut input = ResolveInput::new(root_name, root_version, root_deps);
    if let Some(lock) = &existing_lockfile {
        for pkg in &lock.packages {
            input.locked.insert(
                pkg.name.clone(),
                LockedVersion {
                    version: pkg.version.clone(),
                    checksum: pkg.checksum.clone(),
                },
            );
        }
    }
    input.mode = resolver_mode;
    // Standard-aware version preference: the workspace consumer
    // standards order candidates under `fallback` (the default); the
    // knob comes from `[resolver] incompatible-standards` / env.  Never
    // changes solvability, so this is safe on every resolve path.
    // Consumers reached only through active `[patch]` overrides are not
    // folded in: a patch is a dependency override, not a workspace
    // member, and the index / pre-patch graph does not carry its
    // compile levels.  This shares the documented consumer-proxy
    // optimism of `preference-mode.md` section 1 - it can only pick a
    // too-new version that the post-resolution validation (on the
    // patched reload) then refuses, never one `allow` would have
    // avoided.
    input.consumer_standards = graph.consumer_standards(
        &resolved_selection.closure(&graph),
        &resolved_selection.packages,
        &crate::cli::enabled_features_by_package(&features),
        &dev_for,
    );
    input.incompatible_standards =
        crate::cli::config::resolve_incompatible_standards(&effective_config)?;

    let output = cabin_resolver::resolve(&input, &index).context("dependency resolution failed")?;

    let mut new_lockfile = lockfile_from_resolution(&output, &index);
    new_lockfile.patches = active_patch_records;
    new_lockfile.source_replacements = active_replacement_records;

    if request.allow_write {
        let needs_write = match &existing_lockfile {
            Some(prev) => prev != &new_lockfile,
            None => true,
        };
        if needs_write {
            cabin_lockfile::write_lockfile(&lockfile_path, &new_lockfile)
                .with_context(|| format!("failed to write {}", lockfile_path.display()))?;
            reporter.aux_verbose(format_args!("cabin: wrote {}", lockfile_path.display()));
        } else {
            reporter.aux_verbose(format_args!(
                "cabin: {} is up to date",
                lockfile_path.display()
            ));
        }
    } else if matches!(request.mode, LockMode::Locked)
        && let Some(prev) = &existing_lockfile
        && prev != &new_lockfile
    {
        // We allowed PreferLocked-style search inside the
        // resolver but Locked mode forces selection to come
        // from the lockfile; this branch is a defensive
        // fallback if a future change loosens that.
        bail!(
            "{} is stale; run `cabin resolve` or `cabin update` to refresh it",
            lockfile_path.display()
        );
    }

    emit_resolve_output(&output, request.format)?;
    Ok(())
}

fn emit_resolve_output(output: &ResolveOutput, format: ResolveFormat) -> Result<()> {
    match format {
        ResolveFormat::Human => print_resolve_human(output),
        ResolveFormat::Json => print_resolve_json(output),
    }
}

fn print_resolve_human(output: &ResolveOutput) -> Result<()> {
    let root = output
        .packages
        .iter()
        .find(|p| p.source == ResolvedSource::Root)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "failed to render human resolve output: resolver output is missing a root package"
            )
        })?;
    println!(
        "Resolved dependencies for {} {}:",
        root.name.as_str(),
        root.version
    );
    let mut others: Vec<&cabin_resolver::ResolvedPackage> = output
        .packages
        .iter()
        .filter(|p| p.source != ResolvedSource::Root)
        .collect();
    others.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    if others.is_empty() {
        println!("  (no versioned dependencies)");
    } else {
        for pkg in others {
            println!("  {} {}", pkg.name.as_str(), pkg.version);
        }
    }
    if !output.held_back.is_empty() {
        println!("Held back for standard compatibility:");
        for held in &output.held_back {
            println!("  {}", held.message());
        }
    }
    Ok(())
}

fn print_resolve_json(output: &ResolveOutput) -> Result<()> {
    let root = output
        .packages
        .iter()
        .find(|p| p.source == ResolvedSource::Root)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "failed to render JSON resolve output: resolver output is missing a root package"
            )
        })?;
    let json_root = serde_json::json!({
        "name": root.name.as_str(),
        "version": root.version.to_string(),
    });
    let json_packages: Vec<_> = output
        .packages
        .iter()
        .filter(|p| p.source != ResolvedSource::Root)
        .map(|p| {
            serde_json::json!({
                "name": p.name.as_str(),
                "version": p.version.to_string(),
                "source": p.source.as_str(),
            })
        })
        .collect();
    let json_held_back: Vec<_> = output
        .held_back
        .iter()
        .map(|held| {
            serde_json::json!({
                "name": held.name.as_str(),
                "selected": held.selected.to_string(),
                "newest": held.newest.as_ref().map(ToString::to_string),
                "message": held.message(),
            })
        })
        .collect();
    let value = serde_json::json!({
        "root": json_root,
        "packages": json_packages,
        "held_back": json_held_back,
    });
    crate::print_pretty_json(&value, "failed to serialize resolve output as JSON")
}
