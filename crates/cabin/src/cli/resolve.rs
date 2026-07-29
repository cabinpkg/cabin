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
    LinksClaim, LockedVersion, ResolveInput, ResolveMode, ResolveOutput, ResolvedPackage,
    ResolvedSource,
};
use cabin_workspace::{PackageGraph, RegistryPackageSource};

use super::{
    Context, FROZEN_INDEX_URL_ERR, FetchArgs, Reporter, ResolveArgs, ResolveFormat, Result,
    UpdateArgs, WorkspaceSelectionArgsForUpdate, absolutise, bail, build_selection_request,
    build_workspace_selection, collect_patched_versioned_deps, compute_feature_resolution,
    emit_fetch_output, enabled_features_by_package, resolve_invocation_manifest,
};

/// Declared `links` claims of the local packages a command's
/// resolution actually includes - the feature resolver's exact
/// reachable set, so a disabled optional path dependency never
/// contributes a claim it could not collide with.  Patched packages
/// are graph packages like any local, so an included patch's claims
/// participate; what never participates is the patched *name*'s
/// index metadata - stripped from root deps, and skipped by the
/// resolver for transitive selections via
/// [`ResolveInput::locally_supplied`].
///
/// Callers validate the returned claims against each other with
/// [`enforce_local_links_uniqueness`] *before* any
/// no-versioned-deps fast path, then hand the same claims to
/// [`ResolveInput::local_links`] so the resolver's post-solve check
/// sees local and registry claims together.
pub(crate) fn local_links_claims(
    graph: &PackageGraph,
    features: &cabin_feature::FeatureResolution,
) -> Vec<LinksClaim> {
    let mut claims = links_claims_of(graph, &features.included);
    claims.sort();
    claims.dedup();
    claims
}

/// The declared `links` claims of the given graph packages, in
/// index order (callers sort/dedup the combined seed themselves).
fn links_claims_of(graph: &PackageGraph, indices: &BTreeSet<usize>) -> Vec<LinksClaim> {
    let mut claims = Vec::new();
    for &idx in indices {
        let package = &graph.packages[idx].package;
        for target in &package.targets {
            if let Some(links) = &target.links {
                claims.push(LinksClaim {
                    package: package.name.clone(),
                    version: package.version.clone(),
                    target: target.name.as_str().to_owned(),
                    links: links.clone(),
                });
            }
        }
    }
    claims
}

/// Local-vs-local `links` uniqueness, run unconditionally before a
/// command's fast paths: a workspace with no versioned dependencies
/// must reject duplicate claims exactly like a resolved graph.
pub(crate) fn enforce_local_links_uniqueness(claims: &[LinksClaim]) -> Result<()> {
    cabin_resolver::detect_links_collision(claims).context("dependency resolution failed")?;
    Ok(())
}

/// [`enforce_local_links_uniqueness`] over the full pre-resolution
/// seed: the included local packages' claims plus the activated
/// patch forks' - a purely-local patched chain still links its
/// forks, so their claims cannot wait for a resolution that never
/// runs.
pub(crate) fn enforce_seed_links_uniqueness(
    graph: &PackageGraph,
    features: &cabin_feature::FeatureResolution,
    active_patches: &cabin_workspace::ActivePatchSet,
    activated: &BTreeSet<String>,
) -> Result<()> {
    let mut claims = local_links_claims(graph, features);
    claims.extend(activated_fork_claims(graph, active_patches, activated)?);
    claims.sort();
    claims.dedup();
    enforce_local_links_uniqueness(&claims)
}

/// Everything the iterative patch activation below needs from the
/// call site.  Both resolution pipelines build one of these so the
/// activation policy cannot drift between `cabin resolve` and the
/// artifact pipeline.
pub(crate) struct PatchActivationContext<'a> {
    pub graph: &'a PackageGraph,
    pub features: &'a cabin_feature::FeatureResolution,
    pub active_patches: &'a cabin_workspace::ActivePatchSet,
    pub patched_names: &'a BTreeSet<String>,
    /// Closure-collected versioned deps, patched names excluded.
    pub base_root_deps: &'a BTreeMap<PackageName, semver::VersionReq>,
    /// Patched names the local closure walk reached on active edges.
    pub closure_referenced: &'a BTreeSet<String>,
    /// Declared claims of the included local packages.
    pub local_links: &'a [LinksClaim],
}

/// Resolve with iterative patch activation.  A patch referenced only
/// through a transitive registry edge (`app -> indexed A -> patched
/// B`) is invisible to the local closure until resolution surfaces
/// its name, so: resolve, activate every patched name the solution
/// reached (fold the fork island's versioned deps - the fork's own
/// plus its path dependencies' - into the root set, extend a sparse
/// index with fork-dep names its pre-activation crawl never
/// fetched, contribute the fork's `links` claims, and suppress the
/// upstream index claims via [`ResolveInput::locally_supplied`]), and
/// re-resolve until the activation set is stable.  The set grows
/// monotonically and is bounded by the patch count, so the loop
/// terminates; a dormant patch's name never appears in any solution
/// and never activates.  Claims and suppression then follow the
/// *final* selection: an activation whose injected deps flipped its
/// parent away from the patched name is pruned before the checked
/// solve (the pruning site explains why its root deps remain).
///
/// Returns the final activated set alongside the output: the
/// patched names whose forks participated in resolution, which is
/// exactly the set the build pipeline's strict workspace reload may
/// require dependencies for (a dormant patch's deps were
/// deliberately never resolved).
pub(crate) fn resolve_with_patch_activation(
    input: &mut ResolveInput,
    index: &mut PackageIndex,
    sparse: Option<&cabin_index_http::HttpIndex>,
    ctx: &PatchActivationContext<'_>,
    lean: bool,
) -> Result<(ResolveOutput, BTreeSet<String>)> {
    let included_names: BTreeSet<PackageName> = ctx
        .features
        .included
        .iter()
        .map(|&idx| ctx.graph.packages[idx].package.name.clone())
        .collect();
    // The whole loader-stitch closure joins the suppression set -
    // feature-disabled optional path deps included - for the same
    // reason the forks do: the reload loads those packages locally
    // whatever their feature state, so a same-named index selection
    // must never be fetched alongside.  Claims stay narrower
    // (feature-included only) - see `activated_fork_claims`.
    let supplied_for = |activated: &BTreeSet<String>| -> BTreeSet<PackageName> {
        let closure =
            cabin_workspace::activated_patch_closure(ctx.graph, ctx.active_patches, activated);
        included_names
            .iter()
            .cloned()
            .chain(
                activated
                    .iter()
                    .filter_map(|name| PackageName::new(name.clone()).ok()),
            )
            .chain(
                closure
                    .iter()
                    .map(|&idx| ctx.graph.packages[idx].package.name.clone()),
            )
            .collect()
    };
    let claims_and_supplied =
        |activated: &BTreeSet<String>| -> Result<(Vec<LinksClaim>, BTreeSet<PackageName>)> {
            let mut local_links = ctx.local_links.to_vec();
            local_links.extend(activated_fork_claims(
                ctx.graph,
                ctx.active_patches,
                activated,
            )?);
            local_links.sort();
            local_links.dedup();
            Ok((local_links, supplied_for(activated)))
        };
    // For liveness walks every patched selection is a boundary,
    // activated or not: once a patched name activates its upstream
    // index edges die (the fork replaces them), so a walk expanding
    // through a not-yet-activated patched selection would admit
    // names only those doomed edges reach.  Chains re-enter through
    // the fork's folded deps, which join the walk roots.
    let patched_package_names: BTreeSet<PackageName> = ctx
        .patched_names
        .iter()
        .filter_map(|name| PackageName::new(name.clone()).ok())
        .collect();
    let mut activated = ctx.closure_referenced.clone();
    let mut solution_discovered = false;
    loop {
        let mut root_deps = ctx.base_root_deps.clone();
        let patched =
            collect_patched_versioned_deps(ctx.active_patches, ctx.patched_names, &activated)?;
        // The fold reaches chained patches (a fork depending on
        // another patched name) the solution alone never surfaces;
        // their forks link too, so they claim and suppress like any
        // activated patch.
        activated.extend(patched.activated);
        merge_versioned_deps(&mut root_deps, patched.deps)?;
        let island = activated_fork_island_versioned_deps(ctx, &activated)?;
        activated.extend(island.activated.iter().cloned());
        merge_versioned_deps(&mut root_deps, island.deps)?;
        // A sparse index was crawled from the pre-activation root
        // set, so a fork dependency folded in here may name a
        // package that walk never fetched.  Extend the index before
        // solving; a local index loads its whole directory and is
        // never missing a name it could supply.
        if let Some(http) = sparse {
            let missing: Vec<PackageName> = root_deps
                .keys()
                .filter(|name| !index.packages.contains_key(*name))
                .cloned()
                .collect();
            if !missing.is_empty() {
                let extra = http
                    .load_package_index(&missing)
                    .context("failed to extend the sparse index with patched dependencies")?;
                for (name, entry) in extra.packages {
                    index.packages.entry(name).or_insert(entry);
                }
            }
        }
        input.root_dependencies = root_deps;
        let (local_links, locally_supplied) = claims_and_supplied(&activated)?;
        input.local_links = local_links;
        input.locally_supplied = locally_supplied;
        // While un-activated patched names remain, probe with the
        // unchecked solve first: activation must see the selection
        // even when a stale upstream claim would fail the checked
        // path (the exact claim activation is about to suppress).
        // After any solution-driven activation the probe keeps
        // running so the pruning below sees the post-activation
        // selection.  Patch-free resolutions skip it entirely.
        if solution_discovered
            || ctx
                .patched_names
                .iter()
                .any(|name| !activated.contains(name))
        {
            let probe = cabin_resolver::resolve_packages_unchecked(input, index)
                .context("dependency resolution failed")?;
            // Activation follows the same live boundary as claims: a
            // patched name selected only behind a locally-supplied
            // (or patched) selection's dead upstream edges never
            // links, so its fork must stay dormant rather than
            // inject deps, claims, or late ports into a build that
            // never reaches it.
            let mut stop = input.locally_supplied.clone();
            stop.extend(patched_package_names.iter().cloned());
            let dead = cabin_resolver::unreachable_index_selections(
                &probe.packages,
                index,
                &input.root_dependencies.keys().cloned().collect(),
                &stop,
            );
            let discovered: Vec<String> = probe
                .packages
                .iter()
                .filter(|package| {
                    package.source == ResolvedSource::Index && !dead.contains(&package.name)
                })
                .map(|package| package.name.as_str().to_owned())
                .filter(|name| ctx.patched_names.contains(name) && !activated.contains(name))
                .collect();
            if !discovered.is_empty() {
                activated.extend(discovered);
                solution_discovered = true;
                continue;
            }
            // A dormant patched name selected only behind dead edges
            // must not be fetched either: the reload stitches every
            // patch manifest, so a fetched same-named upstream would
            // collide with the dormant fork.
            input.locally_supplied.extend(
                dead.iter()
                    .filter(|name| ctx.patched_names.contains(name.as_str()))
                    .cloned(),
            );
            // A solution-driven activation can also unselect itself:
            // the fork deps it injected may flip its parent to a
            // version that no longer reaches the patched name.  A
            // fork absent from the final graph never links, so its
            // claims and suppression are dropped before the checked
            // solve, and the pruned set is what this function
            // returns - the build pipeline must not port-prep or
            // strict-load a fork the graph never reaches.  The
            // injected root deps stay - withdrawing them re-selects
            // the parent that reached the patch and oscillates - but
            // the orphaned selections they pull in never link
            // either, so their index claims are suppressed
            // alongside; the residue is an unused fetched package,
            // never a claim.
            //
            // Membership in the probe is not liveness: that residue
            // can itself select a patched name (a pruned fork's
            // injected dep with a back-edge onto one), and keeping
            // such a fork activated would claim and port-prep for a
            // package the final graph never links.  A patched name is
            // live only when reachable from the original roots or a
            // live fork's injected deps - the same reachability the
            // residue suppression below uses - and liveness feeds the
            // root set, so iterate to a fixed point (monotone in
            // `kept`, bounded by the patch count).
            if solution_discovered {
                let mut kept = ctx.closure_referenced.clone();
                let roots = loop {
                    let refolded = collect_patched_versioned_deps(
                        ctx.active_patches,
                        ctx.patched_names,
                        &kept,
                    )?;
                    let mut grown = kept.clone();
                    grown.extend(refolded.activated.iter().cloned());
                    let island = activated_fork_island_versioned_deps(ctx, &grown)?;
                    grown.extend(island.activated.iter().cloned());
                    let roots: BTreeSet<PackageName> = ctx
                        .base_root_deps
                        .keys()
                        .chain(refolded.deps.keys())
                        .chain(island.deps.keys())
                        .cloned()
                        .collect();
                    let mut stop = supplied_for(&grown);
                    stop.extend(patched_package_names.iter().cloned());
                    let orphaned = cabin_resolver::unreachable_index_selections(
                        &probe.packages,
                        index,
                        &roots,
                        &stop,
                    );
                    grown.extend(
                        probe
                            .packages
                            .iter()
                            .filter(|package| package.source == ResolvedSource::Index)
                            .filter(|package| !orphaned.contains(&package.name))
                            .map(|package| package.name.as_str().to_owned())
                            .filter(|name| ctx.patched_names.contains(name)),
                    );
                    if grown == kept {
                        break roots;
                    }
                    kept = grown;
                };
                if kept != activated {
                    let (local_links, mut locally_supplied) = claims_and_supplied(&kept)?;
                    let mut stop = locally_supplied.clone();
                    stop.extend(patched_package_names.iter().cloned());
                    let orphaned = cabin_resolver::unreachable_index_selections(
                        &probe.packages,
                        index,
                        &roots,
                        &stop,
                    );
                    locally_supplied.extend(orphaned);
                    input.local_links = local_links;
                    input.locally_supplied = locally_supplied;
                    activated = kept;
                }
            }
        }
        let output = if lean {
            cabin_resolver::resolve_packages(input, index)
        } else {
            cabin_resolver::resolve(input, index)
        }
        .context("dependency resolution failed")?;
        enforce_activated_fork_versions(&output, index, ctx, &activated, &input.locally_supplied)?;
        return Ok((output, activated));
    }
}

/// The docs-promised version validation (`patch-overrides.md`,
/// "validates each entry") for the edges the local layer cannot
/// see: an index dependency edge onto a patched name surfaces only
/// in the solution, after `resolve_active_patches` already ran over
/// the local graph alone.  This assembles the inputs the typed
/// check needs - fork versions from the stitched graph, walk roots
/// from the base closure plus both patched-dep folds - and defers
/// the validation itself to
/// [`cabin_resolver::enforce_fork_version_requirements`].
fn enforce_activated_fork_versions(
    output: &ResolveOutput,
    index: &PackageIndex,
    ctx: &PatchActivationContext<'_>,
    activated: &BTreeSet<String>,
    locally_supplied: &BTreeSet<PackageName>,
) -> Result<()> {
    if activated.is_empty() {
        return Ok(());
    }
    let fork_versions: BTreeMap<PackageName, semver::Version> =
        cabin_workspace::activated_fork_indices(ctx.graph, ctx.active_patches, activated)
            .into_iter()
            .map(|idx| {
                let package = &ctx.graph.packages[idx].package;
                (package.name.clone(), package.version.clone())
            })
            .collect();
    let refolded =
        collect_patched_versioned_deps(ctx.active_patches, ctx.patched_names, activated)?;
    let island = activated_fork_island_versioned_deps(ctx, activated)?;
    let roots: BTreeSet<PackageName> = ctx
        .base_root_deps
        .keys()
        .chain(refolded.deps.keys())
        .chain(island.deps.keys())
        .cloned()
        .collect();
    cabin_resolver::enforce_fork_version_requirements(
        &output.packages,
        index,
        &fork_versions,
        &roots,
        locally_supplied,
    )?;
    Ok(())
}

/// The declared `links` claims of the activated patches' forks and
/// of everything the forks' manifests pull into the graph (path
/// dependencies, prepared ports) - the local packages the final
/// reload will link in place of, or alongside, the upstream
/// packages the activation suppressed.  Feature resolution never
/// included any of them (nothing selected reaches a
/// transitively-activated fork locally), so
/// [`cabin_feature::resolve_fork_island_features`] runs a dedicated
/// under-approximating pass over the fork islands - see it for why
/// only mandatory members claim here; the exact, edge-aware check
/// runs in the build pipeline over the final reloaded graph, whose
/// feature resolution applies the fetched manifests' real edge
/// requests.  Scoping to the pass's `included` set - not the
/// loader-stitch closure - matches [`local_links_claims`]: a
/// feature-disabled optional path dependency is loaded into the
/// graph but never linked, so its claims must not participate.
fn activated_fork_claims(
    graph: &PackageGraph,
    active_patches: &cabin_workspace::ActivePatchSet,
    activated: &BTreeSet<String>,
) -> Result<Vec<LinksClaim>> {
    let fork_roots = cabin_workspace::activated_fork_indices(graph, active_patches, activated);
    if fork_roots.is_empty() {
        return Ok(Vec::new());
    }
    let resolution = cabin_feature::resolve_fork_island_features(
        graph,
        &fork_roots,
        &cabin_core::TargetPlatform::current(),
    )?;
    Ok(links_claims_of(graph, &resolution.included))
}

/// Versioned dependencies across the activated forks' islands - the
/// forks plus the feature-included local packages their path edges
/// pull in.  Complements [`collect_patched_versioned_deps`] (fork
/// manifests only): a solution-discovered fork's path dependency
/// declaring a registry dependency is invisible both to that fold
/// and to the pre-resolve selection-closure pass, so without this
/// pass the dependency never resolves and the final reload drops
/// the edge without a diagnostic.  Runs island feature passes to a
/// fixed point because a fold can chain-activate further patches
/// whose islands then contribute deps of their own; the returned
/// `activated` is the fixed-point set.
fn activated_fork_island_versioned_deps(
    ctx: &PatchActivationContext<'_>,
    activated: &BTreeSet<String>,
) -> Result<cabin_workspace::PatchedVersionedDeps> {
    let host = cabin_core::TargetPlatform::current();
    let mut folded = activated.clone();
    loop {
        let fork_roots =
            cabin_workspace::activated_fork_indices(ctx.graph, ctx.active_patches, &folded);
        if fork_roots.is_empty() {
            return Ok(cabin_workspace::PatchedVersionedDeps {
                deps: BTreeMap::new(),
                activated: folded,
            });
        }
        let islands = cabin_feature::resolve_fork_island_features(ctx.graph, &fork_roots, &host)?;
        let island = cabin_workspace::collect_island_versioned_deps(
            ctx.graph,
            ctx.active_patches,
            &islands.included,
            ctx.patched_names,
        )?;
        let mut grown = folded.clone();
        grown.extend(island.activated.iter().cloned());
        if grown == folded {
            return Ok(cabin_workspace::PatchedVersionedDeps {
                deps: island.deps,
                activated: folded,
            });
        }
        folded = grown;
    }
}

pub(super) fn resolve(args: &ResolveArgs, reporter: Reporter) -> Result<()> {
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
        },
        reporter,
    )
}

pub(super) fn update(args: &UpdateArgs, reporter: Reporter) -> Result<()> {
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
    // Checked below, after the patched-deps preview computes the
    // activated patch set, so fork claims join the local ones.

    // scope the index requirement to the selected
    // closure.  Unrelated members' versioned deps no longer force a
    // user who passed `--package <selected>` to also pass
    // `--index-path`.  Patched manifests contribute their own
    // versioned deps too, so a workspace whose only versioned
    // edge comes from `[patch]` still needs the index.
    let dev_for: BTreeSet<String> = BTreeSet::new();
    let closure_deps_preview = collect_closure_versioned_deps_excluding_patches(
        &initial_graph,
        &resolved_selection,
        &initial_features,
        &patched_names,
        &dev_for,
    )?;
    let patched_root_deps_preview = collect_patched_versioned_deps(
        &active_patches,
        &patched_names,
        &closure_deps_preview.referenced_excluded,
    )?;
    enforce_seed_links_uniqueness(
        &initial_graph,
        &initial_features,
        &active_patches,
        &patched_root_deps_preview.activated,
    )?;
    if patched_root_deps_preview.deps.is_empty() && closure_deps_preview.deps.is_empty() {
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
    let inputs = crate::cli::config::resolve_pipeline_inputs(
        resolved_index_source.as_ref(),
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
    let local_links = local_links_claims(&graph, &features);
    let dev_for: BTreeSet<String> = BTreeSet::new();
    let closure_deps = collect_closure_versioned_deps_excluding_patches(
        &graph,
        &resolved_selection,
        &features,
        &patched_names,
        &dev_for,
    )?;
    let base_root_deps = closure_deps.deps;
    let mut root_deps = base_root_deps.clone();
    // Patched manifests live outside the workspace graph, so
    // their own versioned deps never reached the closure walk.
    // Fold in the *referenced* patches' deps so `cabin resolve`
    // (and `--package` validation below) sees the same root set
    // the artifact pipeline does; dormant patches contribute
    // nothing, and transitively-referenced patches join through
    // the activation loop inside `resolve_with_patch_activation`.
    let patched_root_deps = collect_patched_versioned_deps(
        &active_patches,
        &patched_names,
        &closure_deps.referenced_excluded,
    )?;
    // Checked before the no-versioned-deps fast path below returns
    // without resolving: a purely-local patched chain still links
    // its forks.
    enforce_seed_links_uniqueness(
        &graph,
        &features,
        &active_patches,
        &patched_root_deps.activated,
    )?;
    merge_versioned_deps(&mut root_deps, patched_root_deps.deps)?;
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

    // With versioned deps present and nothing configured, fall back
    // to the default hosted registry (subject to the same source
    // replacement and offline / frozen rules as a config-supplied
    // URL).  The fallback sits after the empty-resolution early
    // return above, so dep-less runs never observe it.
    let effective_index_source = match effective_index_source {
        Some(locator) => locator,
        None => crate::cli::config::default_index_locator(
            resolution_offline,
            request.policy.frozen(),
            &effective_config,
            request.no_patches,
        )?,
    };
    let (mut index, sparse_index) = match &effective_index_source {
        cabin_core::SourceLocator::IndexPath { path } => {
            (load_local_index(path.as_std_path())?, None)
        }
        // The resolve pipeline performs no artifact downloads, so the
        // HTTP client the helper returns for connection reuse is
        // dropped here; the opened index is kept so patch activation
        // can extend the crawl.
        cabin_core::SourceLocator::IndexUrl { url } => {
            let (index, http_index, _client) = load_http_index(url, &root_deps, reporter)?;
            (index, Some(http_index))
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

    let (output, _activated_patches) = resolve_with_patch_activation(
        &mut input,
        &mut index,
        sparse_index.as_ref(),
        &PatchActivationContext {
            graph: &graph,
            features: &features,
            active_patches: &active_patches,
            patched_names: &patched_names,
            base_root_deps: &base_root_deps,
            closure_referenced: &closure_deps.referenced_excluded,
            local_links: &local_links,
        },
        false,
    )?;

    let mut new_lockfile =
        lockfile_from_resolution(&output, &index, existing_lockfile.as_ref(), &input.mode);
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
///
/// The edge closure is intersected with the feature resolver's
/// `included` set: the raw closure follows optional path edges even
/// when disabled, and a package reachable only through a disabled
/// optional dependency must contribute nothing to resolution - its
/// registry deps would otherwise resolve (over-fetching) and their
/// index `links` claims would hard-fail graphs the build never
/// links.
fn closure_and_optional_filter<'a>(
    graph: &PackageGraph,
    selection: &cabin_workspace::ResolvedSelection,
    features: &'a cabin_feature::FeatureResolution,
) -> (BTreeSet<usize>, impl Fn(usize, &str) -> bool + 'a) {
    let closure: BTreeSet<usize> = selection
        .closure(graph)
        .intersection(&features.included)
        .copied()
        .collect();
    (closure, move |idx, name| {
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
) -> Result<cabin_workspace::ClosureVersionedDeps> {
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
                // The manifest-level and island folds overlap on the
                // forks' own deps; an identical requirement adds
                // nothing and must not grow into ">=1, >=1".
                if slot.get() == &req {
                    continue;
                }
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
    /// Patched names whose forks participated in resolution: the
    /// closure-referenced set plus every transitively-discovered
    /// activation, chains included.  Dormant patches are absent -
    /// their versioned deps were deliberately never resolved, so
    /// the strict workspace reload must not require them.
    pub(crate) activated_patches: BTreeSet<String>,
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
    let local_links = local_links_claims(graph, &features);
    let closure_deps = collect_closure_versioned_deps_excluding_patches(
        graph,
        &resolved_selection,
        &features,
        request.patched_names,
        request.dev_for,
    )?;
    let base_root_deps = closure_deps.deps;
    let mut root_deps = base_root_deps.clone();
    // Patched manifests are not part of the workspace graph at
    // this point, so their own `[dependencies]` never appeared
    // in the closure walk.  Fold the *referenced* patches' deps in
    // so a workspace whose only versioned dep is patched still
    // resolves and fetches the patched manifest's transitive
    // registry edges; dormant patches contribute nothing, and
    // transitively-referenced patches join through the activation
    // loop inside `resolve_with_patch_activation`.
    let patched_root_deps = collect_patched_versioned_deps(
        request.active_patches,
        request.patched_names,
        &closure_deps.referenced_excluded,
    )?;
    // Checked before the no-versioned-deps short-circuit below
    // skips resolution: a purely-local patched chain still links
    // its forks.
    enforce_seed_links_uniqueness(
        graph,
        &features,
        request.active_patches,
        &patched_root_deps.activated,
    )?;
    merge_versioned_deps(&mut root_deps, patched_root_deps.deps)?;
    // short-circuit when neither the selected closure nor the
    // active patch set introduces a versioned dependency.
    // Loading an index, walking the lockfile, and downloading
    // artifacts are all unnecessary in that case.
    if root_deps.is_empty() {
        return Ok(ArtifactPipeline {
            fetched: Vec::new(),
            lockfile_pinned: BTreeSet::new(),
            activated_patches: patched_root_deps.activated,
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

    let (mut index, access, sparse_index) = load_index_for_pipeline(
        request.index_source,
        request.policy.frozen(),
        &root_deps,
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
    let (output, activated_patches) = resolve_with_patch_activation(
        &mut input,
        &mut index,
        sparse_index.as_ref(),
        &PatchActivationContext {
            graph,
            features: &features,
            active_patches: request.active_patches,
            patched_names: request.patched_names,
            base_root_deps: &base_root_deps,
            closure_referenced: &closure_deps.referenced_excluded,
            local_links: &local_links,
        },
        true,
    )?;

    let mut new_lockfile =
        lockfile_from_resolution(&output, &index, existing_lockfile.as_ref(), &input.mode);
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

    let plan = build_fetch_plan(
        &output,
        &index,
        &access,
        &new_lockfile,
        &input.locally_supplied,
    )?;
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
        activated_patches,
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
    reporter: Reporter,
) -> Result<(
    PackageIndex,
    IndexAccess,
    Option<cabin_index_http::HttpIndex>,
)> {
    match index_source {
        cabin_core::SourceLocator::IndexPath { path } => Ok((
            load_local_index(path.as_std_path())?,
            IndexAccess::Local,
            None,
        )),
        cabin_core::SourceLocator::IndexUrl { url } => {
            if frozen {
                bail!(FROZEN_INDEX_URL_ERR);
            }
            let (index, http_index, client) = load_http_index(url, root_deps, reporter)?;
            Ok((index, IndexAccess::Http(client), Some(http_index)))
        }
    }
}

/// Load a [`PackageIndex`] from a local directory, resolving the
/// user-supplied path first so error messages name the absolute
/// location.  Shared by the resolve pipeline and the fetch / build
/// pipeline so the two paths cannot drift.
fn load_local_index(path: &Path) -> Result<PackageIndex> {
    let index_path =
        absolutise(path).with_context(|| format!("failed to resolve {}", path.display()))?;
    cabin_index::load_index(&index_path)
        .with_context(|| format!("failed to load index at {}", index_path.display()))
}

/// Load a [`PackageIndex`] over sparse HTTP for the given root
/// dependencies.  Returns the opened index so patch activation can
/// extend the crawl with fork-dep names, and the client so the
/// fetch / build pipeline can reuse the connection for downloads;
/// the resolve pipeline discards the client.
///
/// The client carries the stored credential (env override or
/// `credentials.toml`) for the index origin, so `config.json`,
/// package metadata, and artifact downloads all authenticate;
/// without a credential the client is tokenless.
pub(crate) fn load_http_index(
    url: &str,
    root_deps: &BTreeMap<PackageName, semver::VersionReq>,
    reporter: Reporter,
) -> Result<(
    PackageIndex,
    cabin_index_http::HttpIndex,
    cabin_index_http::HttpClient,
)> {
    let mut client = cabin_index_http::HttpClient::new();
    if let Some(auth) = crate::cli::login::registry_auth_for_index_url(url, reporter)? {
        client = client.with_auth(auth);
    }
    let http_index = cabin_index_http::HttpIndex::open(url, client.clone())?;
    let names: Vec<PackageName> = root_deps.keys().cloned().collect();
    let index = http_index.load_package_index(&names)?;
    Ok((index, http_index, client))
}

/// Build a [`FetchPlan`] from a resolver output, the index it ran
/// against, and the lockfile the run just settled on.  Each resolved
/// registry package contributes exactly one fetch entry.
///
/// The lockfile is the revision authority: its checksum names the
/// packaging revision to materialize, and a pinned-but-superseded
/// revision is fetched from its own `revisions` entry - that is the
/// fetchability guarantee that keeps existing lockfiles building
/// across respins.  A lockfile entry without a checksum (an index
/// without revisions) falls back to the version-level fields and
/// fails with the same missing-checksum error as before.
///
/// `access` decides whether HTTP-resolved sources get downloaded
/// here (so `cabin-artifact` stays HTTP-free) or whether the source
/// path is handed straight through as a local file.
fn build_fetch_plan(
    output: &ResolveOutput,
    index: &PackageIndex,
    access: &IndexAccess,
    lockfile: &Lockfile,
    locally_supplied: &BTreeSet<PackageName>,
) -> Result<FetchPlan> {
    let mut entries = Vec::new();
    for resolved in &output.packages {
        if resolved.source != ResolvedSource::Index {
            continue;
        }
        // A locally-supplied name (an activated `[patch]` fork the
        // resolver selected through a transitive registry edge, or a
        // pruned activation's orphaned residue) builds from a local
        // working copy or not at all: fetching the upstream bytes
        // would waste the download and hand the workspace reload a
        // second package with the fork's name.
        if locally_supplied.contains(&resolved.name) {
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
        let pinned_checksum = lockfile
            .find(&resolved.name)
            .filter(|locked| locked.version == resolved.version)
            .and_then(|locked| locked.checksum.clone());
        let (source, checksum) = if let Some(pinned) = pinned_checksum {
            let revision = meta
                .revisions
                .values()
                .find(|rev| rev.checksum == pinned)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "the lockfile pins `{} {}` to a packaging revision with checksum \
                         {pinned} which the index no longer lists; run `cabin update` if the \
                         index is the intended source",
                        resolved.name.as_str(),
                        resolved.version
                    )
                })?;
            (&revision.source, pinned)
        } else {
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
            (source, checksum)
        };
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

fn lockfile_from_resolution(
    output: &ResolveOutput,
    index: &cabin_index::PackageIndex,
    previous: Option<&Lockfile>,
    mode: &cabin_resolver::ResolveMode,
) -> Lockfile {
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
        // A previously pinned packaging revision is kept while it is
        // still published, exactly like a locked version: someone
        // else's respin must never churn this lockfile.  The update
        // modes deliberately move to the current revision - that is
        // what `cabin update` is for - and a pin the index no longer
        // lists falls back to the current revision the same way a
        // vanished version falls back to a fresh selection.
        let kept_pin = match mode {
            cabin_resolver::ResolveMode::UpdateAll => None,
            cabin_resolver::ResolveMode::UpdatePackage(updated) if updated == &pkg.name => None,
            _ => previous
                .and_then(|prev| prev.find(&pkg.name))
                .filter(|locked| locked.version == pkg.version)
                .and_then(|locked| locked.checksum.clone())
                .filter(|pin| meta.revisions.values().any(|rev| &rev.checksum == pin)),
        };
        packages.push(LockedPackage {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            checksum: kept_pin.or_else(|| meta.checksum.clone()),
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
