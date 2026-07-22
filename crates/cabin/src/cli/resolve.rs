//! `cabin resolve` / `cabin update` / `cabin fetch`, plus the shared
//! artifact and lockfile orchestration every versioned-dependency
//! command runs: the lock policy, the resolve -> lockfile -> fetch
//! pipeline, and the index loading it depends on.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cabin_artifact::{ArtifactCache, FetchEntry, FetchOptions, FetchPlan, FetchedPackage};
use cabin_core::PackageName;
use cabin_index::PackageIndex;
use cabin_lockfile::{LockedPackage, Lockfile};
use cabin_resolver::{
    LockedVersion, ResolveInput, ResolveMode, ResolveOutput, ResolvedPackage, ResolvedSource,
};
use cabin_workspace::{PackageGraph, RegistryPackageSource};

use super::{
    Context, FROZEN_INDEX_URL_ERR, FetchArgs, Reporter, ResolveArgs, ResolveFormat, Result,
    UpdateArgs, WorkspaceSelectionArgsForUpdate, absolutise, bail, build_selection_request,
    build_workspace_selection, collect_patched_versioned_deps, compute_feature_resolution,
    emit_fetch_output, enabled_features_by_package, resolve_invocation_manifest,
};

pub(super) fn resolve(
    args: &ResolveArgs,
    reporter: Reporter,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<()> {
    let policy = LockPolicy::from_flags(args.locked, args.frozen);
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
            policy,
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
    let policy = match &args.package {
        Some(name) => LockPolicy::UpdatePackage(
            PackageName::new(name.clone())
                .map_err(|err| anyhow::anyhow!("invalid --package value {name:?}: {err}"))?,
        ),
        None => LockPolicy::UpdateAll,
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
            policy,
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
    let crate::cli::port::WorkspacePrep {
        effective_config,
        active_patches,
        graph: initial_graph,
        ..
    } = crate::cli::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        args.cache_dir.as_deref(),
        offline,
        args.frozen,
        false,
        &workspace_selection,
        args.no_patches,
        None,
    )?;
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
        emit_fetch_output(&[], args.format, &manifest_path)?;
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
        index_source: &inputs.index_source,
        policy: inputs.policy,
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

    emit_fetch_output(&pipeline.fetched, args.format, &manifest_path)?;
    Ok(())
}

struct ResolutionRequest<'a> {
    manifest_path: &'a Path,
    index_path: Option<&'a Path>,
    index_url: Option<&'a str>,
    format: ResolveFormat,
    policy: LockPolicy,
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
    // CLI flags win; otherwise consult the merged effective
    // config for a `[registry]` default.  The orchestration layer
    // owns the final reconciliation; cabin-resolver / cabin-index
    // see only a concrete index source.
    let crate::cli::port::WorkspacePrep {
        effective_config,
        active_patches,
        graph,
        ..
    } = crate::cli::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        None,
        offline,
        request.policy.frozen(),
        false,
        &request.selection,
        request.no_patches,
        None,
    )?;
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
    let effective_index_source: Option<cabin_core::SourceLocator> = match resolved_index_source
        .as_ref()
    {
        Some(source) => {
            let initial = crate::cli::config::index_source_kind_to_locator(&source.kind);
            let resolved = crate::cli::patch::apply_source_replacement(
                initial,
                &effective_config,
                request.no_patches,
            )?;
            crate::cli::config::enforce_offline_post_replacement(resolution_offline, &resolved)?;
            Some(resolved.resolved)
        }
        None => None,
    };
    if request.policy.frozen()
        && matches!(
            effective_index_source,
            Some(cabin_core::SourceLocator::IndexUrl { .. })
        )
    {
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
    if let LockPolicy::UpdatePackage(name) = &request.policy
        && !root_deps.contains_key(name)
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
            name = name.as_str(),
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
    if request.policy.locked()
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
    if existing_lockfile.is_none() && request.policy.locked() {
        bail!(
            "cannot resolve with --locked because {} does not exist",
            lockfile_path.display()
        );
    }

    let index = match &effective_index_source {
        None => {
            bail!(crate::cli::VERSIONED_DEPS_REQUIRE_INDEX)
        }
        Some(cabin_core::SourceLocator::IndexPath { path }) => {
            load_local_index(path.as_std_path(), request.experimental_features)?
        }
        // The resolve pipeline performs no artifact downloads, so the
        // HTTP client the helper returns for connection reuse is
        // dropped here.
        Some(cabin_core::SourceLocator::IndexUrl { url }) => {
            load_http_index(url, &root_deps, request.experimental_features, reporter)?.0
        }
    };

    let resolver_mode = request.policy.resolve_mode();

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

    if request.policy.allow_write() {
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
    } else if request.policy.locked()
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

/// Build the selection's closure once and adapt a
/// [`cabin_feature::FeatureResolution`] handle into the
/// `Fn(usize, &str) -> bool` optional-dep filter the workspace
/// versioned-dep helpers consume.  Shared by the collect / has shims
/// below so the closure build + filter adapter live in one place.
fn closure_and_optional_filter<'a>(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    features: &'a cabin_feature::FeatureResolution,
) -> (BTreeSet<usize>, impl Fn(usize, &str) -> bool + 'a) {
    (selection.closure(graph), move |idx, name| {
        features.is_optional_dep_enabled(idx, name)
    })
}

/// Collect every versioned dependency reachable from `selection`
/// after dropping patched names.  Thin shim around the typed API
/// in `cabin-workspace`.
pub(crate) fn collect_closure_versioned_deps_excluding_patches(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    features: &cabin_feature::FeatureResolution,
    patched_names: &BTreeSet<String>,
    dev_for: &BTreeSet<String>,
) -> Result<BTreeMap<PackageName, semver::VersionReq>> {
    let (closure, is_optional_dep_enabled) =
        closure_and_optional_filter(graph, selection, features);
    cabin_workspace::collect_closure_versioned_deps_excluding_with_dev(
        graph,
        &closure,
        is_optional_dep_enabled,
        patched_names,
        dev_for,
    )
    .map_err(Into::into)
}

/// Merge `extra` into `into`, joining version requirements for
/// names that appear in both so the resolver sees a single
/// requirement per package.  Mirrors the join-and-reparse pattern
/// the workspace closure walker uses.
fn merge_versioned_deps(
    into: &mut BTreeMap<PackageName, semver::VersionReq>,
    extra: BTreeMap<PackageName, semver::VersionReq>,
) -> Result<()> {
    for (name, req) in extra {
        match into.entry(name.clone()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(req);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let parsed = cabin_workspace::combine_version_reqs(&[
                    slot.get().to_string(),
                    req.to_string(),
                ])
                .map_err(|(joined, err)| {
                    anyhow::anyhow!(
                        "conflicting dependency requirements for {}: {}: {}",
                        name.as_str(),
                        joined,
                        err
                    )
                })?;
                slot.insert(parsed);
            }
        }
    }
    Ok(())
}

/// Whether the selected closure carries any versioned
/// (registry-bound) dependency that the artifact pipeline would
/// need to fetch.  Thin shim around the typed API in
/// `cabin-workspace`.
pub(crate) fn closure_has_versioned_deps_excluding_patches(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    features: &cabin_feature::FeatureResolution,
    patched_names: &BTreeSet<String>,
    dev_for: &BTreeSet<String>,
) -> bool {
    let (closure, is_optional_dep_enabled) =
        closure_and_optional_filter(graph, selection, features);
    cabin_workspace::closure_has_versioned_deps_excluding_with_dev(
        graph,
        &closure,
        is_optional_dep_enabled,
        patched_names,
        dev_for,
    )
}

/// Pick the primary packages that contribute versioned
/// deps to a resolve / fetch / update run.  When the user passed
/// workspace-selection flags, only their selected packages
/// contribute.  Otherwise the documented default applies (root
/// package or every primary).
fn selected_resolution_packages(
    graph: &PackageGraph,
    selection: &cabin_workspace::PackageSelection,
) -> Result<cabin_workspace::ResolvedSelection> {
    cabin_workspace::resolve_package_selection(graph, selection).map_err(std::convert::Into::into)
}

/// What kind of resolution the CLI is asking for, plus the write /
/// network permissions that follow from it.  One value replaces the
/// previously separate lock mode + `frozen` + `allow_write`
/// threading, so the three can never disagree.
#[derive(Debug, Clone)]
pub(crate) enum LockPolicy {
    /// Default: reuse lockfile pins that still satisfy, refresh the
    /// rest, and write the result back.
    PreferLocked,
    /// `--locked`: selection must come from the lockfile, which is
    /// never rewritten.
    Locked,
    /// `--frozen`: `--locked` plus no network fetches and no cache
    /// population.
    Frozen,
    /// `cabin update`: re-resolve every locked package.
    UpdateAll,
    /// `cabin update --package <name>`: refresh one direct dep.
    UpdatePackage(PackageName),
}

impl LockPolicy {
    pub(crate) fn from_flags(locked: bool, frozen: bool) -> Self {
        if frozen {
            LockPolicy::Frozen
        } else if locked {
            LockPolicy::Locked
        } else {
            LockPolicy::PreferLocked
        }
    }

    /// Translate into the resolver's [`ResolveMode`].
    pub(crate) fn resolve_mode(&self) -> ResolveMode {
        match self {
            LockPolicy::PreferLocked => ResolveMode::PreferLocked,
            LockPolicy::Locked | LockPolicy::Frozen => ResolveMode::Locked,
            LockPolicy::UpdateAll => ResolveMode::UpdateAll,
            LockPolicy::UpdatePackage(name) => ResolveMode::UpdatePackage(name.clone()),
        }
    }

    /// Whether the lockfile may be written.
    pub(crate) fn allow_write(&self) -> bool {
        !self.locked()
    }

    /// Whether the lockfile is authoritative (`--locked` or
    /// `--frozen`): resolution must not diverge from it.
    pub(crate) fn locked(&self) -> bool {
        matches!(self, LockPolicy::Locked | LockPolicy::Frozen)
    }

    /// Whether `--frozen` additionally forbids network fetches and
    /// cache population.
    pub(crate) fn frozen(&self) -> bool {
        matches!(self, LockPolicy::Frozen)
    }
}

pub(crate) struct ArtifactPipelineRequest<'a> {
    pub(crate) manifest_path: &'a Path,
    pub(crate) initial_graph: &'a PackageGraph,
    pub(crate) index_source: &'a cabin_core::SourceLocator,
    pub(crate) policy: LockPolicy,
    pub(crate) cache_dir: &'a Path,
    pub(crate) reporter: Reporter,
    /// Workspace selection that contributes versioned deps
    /// to the resolution.  Defaults to every primary package when
    /// the user passes no selection flags.
    pub(crate) selection: cabin_workspace::PackageSelection,
    /// Feature flags from the CLI.  Drives optional-dependency
    /// inclusion.
    pub(crate) selection_request: &'a cabin_core::SelectionRequest,
    /// Names of patched packages - the pipeline must skip them
    /// because they ship from a local working copy and never need
    /// to be fetched from the index.
    pub(crate) patched_names: &'a BTreeSet<String>,
    /// Active patches recorded into the new lockfile and
    /// compared against the existing lockfile under `--locked`.
    pub(crate) active_patches: &'a cabin_workspace::ActivePatchSet,
    /// Active source-replacement entries (post-merge) recorded
    /// into the new lockfile.
    pub(crate) source_replacements: &'a cabin_core::SourceReplacementSettings,
    /// Whether `--no-patches` was supplied - suppresses
    /// source-replacement records on the lockfile to match the
    /// "no local override policy" semantics.
    pub(crate) no_patches: bool,
    /// Names of packages whose `[dev-dependencies]` should be
    /// activated for this invocation.  Empty for `cabin build`;
    /// `cabin test` passes the selected primary packages' names
    /// so the resolver / fetch path picks up dev-deps the test
    /// executables need.
    pub(crate) dev_for: &'a BTreeSet<String>,
    /// The `[resolver] incompatible-standards` preference for this
    /// invocation (resolved from env / config).  Applied to the
    /// pipeline's resolution so `build` / `run` / `test` / `fetch`
    /// select the same versions `cabin resolve` / `cabin update` would.
    pub(crate) incompatible_standards: cabin_core::IncompatibleStandards,
    /// Experimental `-Z` features enabled for this invocation.
    /// Consulted by index loading, which gates the remote-registry
    /// `config.json` fields on `-Z remote-registry`.
    pub(crate) experimental_features: &'a cabin_core::ExperimentalFeatures,
}

pub(crate) struct ArtifactPipeline {
    pub(crate) fetched: Vec<FetchedPackage>,
    /// Registry selections that came straight out of a pre-existing
    /// `cabin.lock`: the (name, version) pairs recorded there that
    /// resolution re-selected.  Empty when no lockfile existed, when
    /// an update mode ignored it, and never containing a selection
    /// the resolver re-resolved past a stale pin.  Drives the
    /// lockfile-staleness note on standard-compat violations.
    pub(crate) lockfile_pinned: BTreeSet<(String, String)>,
}

impl ArtifactPipeline {
    /// Project each fetched package into the
    /// [`RegistryPackageSource`] the workspace loader consumes,
    /// pinning every manifest at `<source_dir>/cabin.toml`.  Shared
    /// by `build` / `run` / `test`, which all feed the fetched
    /// closure back into a strict workspace reload.
    pub(crate) fn registry_sources(&self) -> Vec<RegistryPackageSource> {
        self.fetched
            .iter()
            .map(|p| RegistryPackageSource {
                name: p.name.clone(),
                version: p.version.clone(),
                manifest_path: p.source_dir.join("cabin.toml"),
            })
            .collect()
    }
}

/// Resolved index access: either a directory on disk we already
/// turned into a [`PackageIndex`], or a live HTTP client we will use
/// to download artifacts.
enum IndexAccess {
    Local,
    Http(cabin_index_http::HttpClient),
}

/// Run the resolve → lockfile → fetch pipeline used by both
/// `cabin fetch` and `cabin build`.
pub(crate) fn run_artifact_pipeline(
    request: &ArtifactPipelineRequest<'_>,
) -> Result<ArtifactPipeline> {
    let manifest_path = request.manifest_path;
    let graph = request.initial_graph;
    let resolved_selection = selected_resolution_packages(graph, &request.selection)?;
    let features = compute_feature_resolution(
        graph,
        &resolved_selection,
        request.selection_request,
        request.dev_for,
    )?;
    let mut root_deps = collect_closure_versioned_deps_excluding_patches(
        graph,
        &resolved_selection,
        &features,
        request.patched_names,
        request.dev_for,
    )?;
    // Patched manifests are not part of the workspace graph at
    // this point, so their own `[dependencies]` never appeared
    // in the closure walk.  Fold them in so a workspace whose only
    // versioned dep is patched still resolves and fetches the
    // patched manifest's transitive registry edges.
    let patched_root_deps =
        collect_patched_versioned_deps(request.active_patches, request.patched_names)?;
    merge_versioned_deps(&mut root_deps, patched_root_deps)?;
    // short-circuit when neither the selected closure nor the
    // active patch set introduces a versioned dependency.
    // Loading an index, walking the lockfile, and downloading
    // artifacts are all unnecessary in that case.
    if root_deps.is_empty() {
        return Ok(ArtifactPipeline {
            fetched: Vec::new(),
            lockfile_pinned: BTreeSet::new(),
        });
    }
    // pick a stable synthetic root identity for pure
    // workspace roots; fall back to the [package] root otherwise.
    let (root_name, root_version) = match graph.root_package {
        Some(idx) => (
            graph.packages[idx].package.name.clone(),
            graph.packages[idx].package.version.clone(),
        ),
        None => cabin_workspace::synthetic_root_identity(graph),
    };

    let lockfile_path = lockfile_path_for(manifest_path);

    let existing_lockfile: Option<Lockfile> = if lockfile_path.is_file() {
        Some(
            cabin_lockfile::read_lockfile(&lockfile_path)
                .with_context(|| format!("failed to read {}", lockfile_path.display()))?,
        )
    } else {
        if request.policy.locked() {
            bail!(
                "cannot resolve with --locked because {} does not exist",
                lockfile_path.display()
            );
        }
        None
    };

    let (index, access) = load_index_for_pipeline(
        request.index_source,
        request.policy.frozen(),
        &root_deps,
        request.experimental_features,
        request.reporter,
    )?;

    let resolver_mode = request.policy.resolve_mode();

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
    // Standard-aware version preference, matching `cabin resolve`, so a
    // fresh `cabin build` writes the same lockfile.  Scoped to the
    // selected closure - an unselected member must not lower it.
    input.consumer_standards = graph.consumer_standards(
        &resolved_selection.closure(graph),
        &resolved_selection.packages,
        &enabled_features_by_package(&features),
        request.dev_for,
    );
    input.incompatible_standards = request.incompatible_standards;

    // Patch / source-replacement state recorded into the new
    // lockfile and compared against the existing lockfile under
    // `--locked`.
    let active_patch_records = crate::cli::patch::lockfile_patches(request.active_patches);
    let active_replacement_records = crate::cli::patch::lockfile_source_replacements(
        request.source_replacements,
        request.no_patches,
    );
    if request.policy.locked()
        && let Some(prev) = &existing_lockfile
        && !prev.matches_patch_state(&active_patch_records, &active_replacement_records)
    {
        bail!(
            "--locked cannot be used because active patch / source-replacement policy differs from {}; re-run without --locked to refresh the lockfile",
            lockfile_path.display()
        );
    }

    // Build/run/test/vendor consume only the resolved graph (into the
    // lockfile) and never render `held_back`, so use the lean resolve
    // that skips the second `Allow`-mode solve behind the report.
    let output =
        cabin_resolver::resolve_packages(&input, &index).context("dependency resolution failed")?;

    let mut new_lockfile = lockfile_from_resolution(&output, &index);
    new_lockfile.patches = active_patch_records;
    new_lockfile.source_replacements = active_replacement_records;

    if request.policy.allow_write() {
        let needs_write = match &existing_lockfile {
            Some(prev) => prev != &new_lockfile,
            None => true,
        };
        if needs_write {
            cabin_lockfile::write_lockfile(&lockfile_path, &new_lockfile)
                .with_context(|| format!("failed to write {}", lockfile_path.display()))?;
            request
                .reporter
                .aux_verbose(format_args!("cabin: wrote {}", lockfile_path.display()));
        } else {
            request.reporter.aux_verbose(format_args!(
                "cabin: {} is up to date",
                lockfile_path.display()
            ));
        }
    }

    let plan = build_fetch_plan(&output, &index, &access)?;
    let cache = ArtifactCache::new(request.cache_dir);
    let result = cabin_artifact::fetch(
        &plan,
        &cache,
        FetchOptions {
            frozen: request.policy.frozen(),
        },
    )?;
    Ok(ArtifactPipeline {
        fetched: result.packages,
        // `PreferLocked` falls back to a fresh selection when a pin
        // no longer satisfies its constraint, so membership is
        // checked selection by selection - a re-resolved package
        // must not carry the lockfile-staleness note.  Update modes
        // ignore the locked map entirely.
        lockfile_pinned: match &existing_lockfile {
            Some(lock)
                if matches!(
                    request.policy,
                    LockPolicy::PreferLocked | LockPolicy::Locked | LockPolicy::Frozen
                ) =>
            {
                output
                    .packages
                    .iter()
                    .filter(|p| lock.find(&p.name).is_some_and(|l| l.version == p.version))
                    .map(|p| (p.name.as_str().to_owned(), p.version.to_string()))
                    .collect()
            }
            _ => BTreeSet::new(),
        },
    })
}

/// Pick the right index source for a fetch / build run, validate
/// CLI flag combinations, and return both the [`PackageIndex`] the
/// resolver consumes and a tag describing which access mode the
/// fetch plan should use.
fn load_index_for_pipeline(
    index_source: &cabin_core::SourceLocator,
    frozen: bool,
    root_deps: &BTreeMap<PackageName, semver::VersionReq>,
    experimental_features: &cabin_core::ExperimentalFeatures,
    reporter: Reporter,
) -> Result<(PackageIndex, IndexAccess)> {
    match index_source {
        cabin_core::SourceLocator::IndexPath { path } => Ok((
            load_local_index(path.as_std_path(), experimental_features)?,
            IndexAccess::Local,
        )),
        cabin_core::SourceLocator::IndexUrl { url } => {
            if frozen {
                bail!(FROZEN_INDEX_URL_ERR);
            }
            let (index, client) = load_http_index(url, root_deps, experimental_features, reporter)?;
            Ok((index, IndexAccess::Http(client)))
        }
    }
}

/// Load a [`PackageIndex`] from a local directory, resolving the
/// user-supplied path first so error messages name the absolute
/// location.  Shared by the resolve pipeline and the fetch / build
/// pipeline so the two paths cannot drift.
fn load_local_index(
    path: &Path,
    experimental_features: &cabin_core::ExperimentalFeatures,
) -> Result<PackageIndex> {
    let index_path =
        absolutise(path).with_context(|| format!("failed to resolve {}", path.display()))?;
    cabin_index::load_index_with_features(&index_path, experimental_features)
        .with_context(|| format!("failed to load index at {}", index_path.display()))
}

/// Load a [`PackageIndex`] over sparse HTTP for the given root
/// dependencies.  Returns the client alongside the index so the
/// fetch / build pipeline can reuse the connection for downloads;
/// the resolve pipeline discards it.
///
/// Under `-Z remote-registry` the client carries the stored
/// credential (env override or `credentials.toml`) for the index
/// origin, so `config.json`, package metadata, and artifact
/// downloads all authenticate; without the feature (or without a
/// credential) the client is tokenless, exactly as before.
pub(crate) fn load_http_index(
    url: &str,
    root_deps: &BTreeMap<PackageName, semver::VersionReq>,
    experimental_features: &cabin_core::ExperimentalFeatures,
    reporter: Reporter,
) -> Result<(PackageIndex, cabin_index_http::HttpClient)> {
    let mut client = cabin_index_http::HttpClient::new();
    if let Some(auth) =
        crate::cli::login::registry_auth_for_index_url(url, experimental_features, reporter)?
    {
        client = client.with_auth(auth);
    }
    let http_index = cabin_index_http::HttpIndex::open_with_features(
        url,
        client.clone(),
        experimental_features,
    )?;
    let names: Vec<PackageName> = root_deps.keys().cloned().collect();
    let index = http_index.load_package_index(&names)?;
    Ok((index, client))
}

/// Build a [`FetchPlan`] from a resolver output and the index it ran
/// against.  Each resolved registry package contributes exactly one
/// fetch entry; the index is the source of truth for `source` and
/// `checksum`.
///
/// `access` decides whether HTTP-resolved sources get downloaded
/// here (so `cabin-artifact` stays HTTP-free) or whether the source
/// path is handed straight through as a local file.
fn build_fetch_plan(
    output: &ResolveOutput,
    index: &PackageIndex,
    access: &IndexAccess,
) -> Result<FetchPlan> {
    let mut entries = Vec::new();
    for resolved in &output.packages {
        if resolved.source != ResolvedSource::Index {
            continue;
        }
        let entry = index.package(&resolved.name).ok_or_else(|| {
            anyhow::anyhow!(
                "resolver chose `{} {}`, but it is not in the index",
                resolved.name.as_str(),
                resolved.version
            )
        })?;
        let meta = entry.versions.get(&resolved.version).ok_or_else(|| {
            anyhow::anyhow!(
                "resolver chose `{} {}`, but the index has no entry for this version",
                resolved.name.as_str(),
                resolved.version
            )
        })?;
        let source = meta.source.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "package `{} {}` has no source artifact in the index",
                resolved.name.as_str(),
                resolved.version
            )
        })?;
        let checksum = meta.checksum.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "missing checksum for `{} {}`; cabin fetch requires a sha256:<hex> entry in the index",
                resolved.name.as_str(),
                resolved.version
            )
        })?;
        let fetch_source = match (source, access) {
            (cabin_index::SourceLocation::LocalPath(p), _) => {
                cabin_artifact::FetchSource::LocalArchive(p.clone())
            }
            (cabin_index::SourceLocation::HttpUrl(url), IndexAccess::Http(client)) => {
                let label = format!("{} {}", resolved.name.as_str(), resolved.version);
                let bytes = client.download(url, &label).with_context(|| {
                    format!(
                        "failed to download source archive for `{} {}`",
                        resolved.name.as_str(),
                        resolved.version
                    )
                })?;
                cabin_artifact::FetchSource::InMemoryArchive(bytes)
            }
            (cabin_index::SourceLocation::HttpUrl(_), IndexAccess::Local) => {
                bail!(
                    "package `{} {}` has an HTTP source URL but the run is using a local index",
                    resolved.name.as_str(),
                    resolved.version
                );
            }
        };
        entries.push(FetchEntry {
            name: resolved.name.clone(),
            version: resolved.version.clone(),
            checksum,
            source: fetch_source,
        });
    }
    Ok(FetchPlan { entries })
}

pub(crate) fn lockfile_path_for(manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf)
        .join("cabin.lock")
}

/// Read the lockfile at `lockfile_path` if it exists, attaching a
/// read-error context that names the path.  Returns `Ok(None)` when
/// the file is absent.  Shared by the read-only inspection commands
/// (`metadata` / `tree` / `explain`); the commands that enforce
/// `--locked` keep their own bespoke read so the missing-lockfile
/// case stays a hard error there.
pub(crate) fn read_optional_lockfile(lockfile_path: &Path) -> Result<Option<Lockfile>> {
    if lockfile_path.is_file() {
        Ok(Some(
            cabin_lockfile::read_lockfile(lockfile_path)
                .with_context(|| format!("failed to read {}", lockfile_path.display()))?,
        ))
    } else {
        Ok(None)
    }
}

fn lockfile_from_resolution(output: &ResolveOutput, index: &cabin_index::PackageIndex) -> Lockfile {
    // We need each resolved package's transitive deps to write the
    // lockfile's `dependencies = [...]` field.  The resolver doesn't
    // surface the dep edges directly, so we read them off the index
    // entry for the chosen version.
    let resolved_names: BTreeSet<&str> = output
        .packages
        .iter()
        .filter(|p| p.source == ResolvedSource::Index)
        .map(|p| p.name.as_str())
        .collect();
    let mut packages: Vec<LockedPackage> = Vec::new();
    for pkg in &output.packages {
        if pkg.source != ResolvedSource::Index {
            continue;
        }
        let entry = index
            .package(&pkg.name)
            .expect("index has every resolved package");
        let meta = entry
            .versions
            .get(&pkg.version)
            .expect("index has the resolved version");
        // Filter to only dep names that are also resolved (defensive).
        let mut deps: Vec<PackageName> = meta
            .dependencies
            .keys()
            .filter(|n| resolved_names.contains(n.as_str()))
            .cloned()
            .collect();
        deps.sort();
        packages.push(LockedPackage {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            checksum: meta.checksum.clone(),
            dependencies: deps,
        });
    }
    packages.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    Lockfile {
        version: cabin_lockfile::LOCKFILE_VERSION,
        packages,
        patches: Vec::new(),
        source_replacements: Vec::new(),
    }
}
