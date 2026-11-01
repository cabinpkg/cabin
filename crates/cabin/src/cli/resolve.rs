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

use super::seed::{ResolutionSeed, SeedInputs};
use super::{
    Context, FROZEN_INDEX_URL_ERR, FetchArgs, Reporter, ResolveArgs, ResolveFormat, Result,
    UpdateArgs, WorkspaceSelectionArgsForUpdate, absolutise, bail, build_selection_request,
    build_workspace_selection, emit_fetch_output, enabled_features_by_package,
    resolve_invocation_manifest,
};

/// Where resolution reads package metadata: a file registry or a
/// sparse HTTP one, each read one package document at a time by name.
enum MetadataSource {
    File(cabin_index::FileIndex),
    Http(Box<cabin_index_http::HttpIndex>),
}

impl MetadataSource {
    /// Read `roots` and what they reach through non-optional edges,
    /// skipping the names in `known`.
    fn load(&self, roots: &[PackageName], known: &BTreeSet<PackageName>) -> Result<PackageIndex> {
        match self {
            Self::File(index) => index
                .load_package_index(roots, known)
                .with_context(|| format!("failed to load index at {}", index.root().display())),
            Self::Http(index) => Ok(index.load_package_index(roots, known)?),
        }
    }
}

/// Resolve `input` against the metadata `source` serves, reading it on
/// demand: the initial walk follows non-optional edges only, so the
/// metadata of an optional dependency a feature enables, or of a fork's
/// dependency, is read here.  The solve itself stays free of I/O and
/// restarts after each read, which loads every unfetched name the round
/// reached; every round loads a new name, so the loop ends.  `lean`
/// skips the hold-back report.  Each round first gives the lockfile's
/// kept pins their revisions' metadata, which the fetch plan and
/// lockfile writer then read too.  Returns the index the solve read.
#[tracing::instrument(level = "info", skip_all, fields(rounds = tracing::field::Empty))]
fn resolve_with_lazy_index(
    input: &ResolveInput,
    source: &MetadataSource,
    lean: bool,
) -> Result<(ResolveOutput, PackageIndex)> {
    let roots: Vec<PackageName> = input.root_dependencies.keys().cloned().collect();
    let mut known: BTreeSet<PackageName> = input.local_packages.keys().cloned().collect();
    let mut index = source.load(&roots, &known)?;
    known.extend(index.packages.keys().cloned());
    let mut rounds: u64 = 0;
    loop {
        rounds += 1;
        tracing::Span::current().record("rounds", rounds);
        cabin_resolver::select_locked_revisions(input, &mut index);
        let result = if lean {
            cabin_resolver::resolve_packages(input, &index)
        } else {
            cabin_resolver::resolve(input, &index)
        };
        match result {
            Err(cabin_resolver::ResolveError::MetadataNeeded(names)) => {
                let names: Vec<PackageName> = names.into_iter().collect();
                let loaded = source.load(&names, &known)?;
                known.extend(loaded.packages.keys().cloned());
                index.packages.extend(loaded.packages);
            }
            result => {
                return result
                    .map(|output| (output, index))
                    .context("dependency resolution failed");
            }
        }
    }
}

/// The resolver input every resolving command builds from its seed:
/// the closure's root requirements and feature requests, the
/// workspace's local packages, their claims, and the lockfile pins.
fn resolve_input(
    graph: &PackageGraph,
    seed: &ResolutionSeed,
    root_deps: BTreeMap<PackageName, semver::VersionReq>,
    lockfile: Option<&Lockfile>,
    mode: ResolveMode,
) -> ResolveInput {
    let (root_name, root_version) = match graph.root_package {
        Some(idx) => (
            graph.packages[idx].package.name.clone(),
            graph.packages[idx].package.version.clone(),
        ),
        None => cabin_workspace::synthetic_root_identity(),
    };
    let mut input = ResolveInput::new(root_name, root_version, root_deps);
    for pkg in lockfile.iter().flat_map(|lock| &lock.packages) {
        input.locked.insert(
            pkg.name.clone(),
            LockedVersion {
                version: pkg.version.clone(),
                checksum: pkg.checksum.clone(),
            },
        );
    }
    input.mode = mode;
    input.root_features = seed.root_features();
    input.local_packages = super::seed::local_packages(graph);
    input.local_links.clone_from(&seed.local_links);
    input.link_domains.clone_from(&seed.link_domains);
    // Both index sources read metadata on demand.
    input.lazy_index = true;
    input
}

/// Refuse a registry package's edge that resolution satisfied with a
/// local package the build cannot link it to: the workspace loader
/// links a registry edge only to a fetched archive or a `[patch]`.
fn ensure_registry_edges_link(
    output: &ResolveOutput,
    input: &ResolveInput,
    patched_names: &BTreeSet<String>,
) -> Result<()> {
    for pkg in output
        .packages
        .iter()
        .filter(|p| p.source == ResolvedSource::Index)
    {
        if let Some(name) = pkg.dependencies.iter().find(|name| {
            input.local_packages.contains_key(*name)
                && **name != input.root_name
                && !patched_names.contains(name.as_str())
        }) {
            bail!(
                "registry package `{}` {} depends on `{name}`, which this workspace supplies \
                 locally without a `[patch]` entry; add `[patch] \"{name}\" = {{ path = ... }}` \
                 to build it against the local package",
                pkg.name,
                pkg.version
            );
        }
    }
    Ok(())
}

/// The patches whose forks a resolution uses: the ones the local
/// closure references plus every patched name the solution reached
/// through a registry edge (chains included - a fork's own edges
/// resolve like any other).  A dormant patch is in neither, so the
/// strict reload never requires its dependencies.
fn activated_patches(
    seed: &ResolutionSeed,
    output: &ResolveOutput,
    patched_names: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut activated = seed.patched_deps.activated.clone();
    activated.extend(
        output
            .packages
            .iter()
            .filter(|p| {
                p.source == ResolvedSource::Local && patched_names.contains(p.name.as_str())
            })
            .map(|p| p.name.as_str().to_owned()),
    );
    activated
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
    // `--package` names the dependency to update, so member
    // selection comes from a bundle without `-p`.
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
    let crate::cli::workspace_prep::WorkspacePrep {
        effective_config,
        active_patches,
        graph: initial_graph,
        ..
    } = crate::cli::workspace_prep::load_workspace_and_patches(&manifest_path, args.no_patches)?;
    let patched_names = active_patches.owned_patched_names();
    // `cabin fetch` does not currently expose feature flags,
    // so feature resolution runs with the documented defaults
    // (each selected root's `default` feature, no extras).  This
    // still excludes disabled optional dependencies from the
    // index-requirement check below - the user opts into them
    // via `cabin build --features ...` / `cabin resolve
    // --features ...`.
    let fetch_request = cabin_core::SelectionRequest::default();
    let dev_for: BTreeSet<String> = BTreeSet::new();
    // Computed before the fast path so a selection typo like
    // `--package missing` fails even when there is nothing to
    // fetch, and so only the selected closure decides whether an
    // index is read: an unrelated member's versioned deps never send
    // `--package <selected>` to a registry, while a closure whose
    // only versioned deps come from a `[patch]` fork still reads one.
    let seed = ResolutionSeed::compute(&SeedInputs {
        graph: &initial_graph,
        selection: &workspace_selection,
        request: &fetch_request,
        patched_names: &patched_names,
        active_patches: &active_patches,
        dev_for: &dev_for,
    })?;
    if !seed.has_versioned_deps() {
        enforce_locked_patch_state(
            &manifest_path,
            &LockPolicy::from_flags(args.locked, args.frozen),
            &active_patches,
            &effective_config.source_replacements,
            args.no_patches,
        )?;
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
        index_user_chosen: inputs.index_user_chosen,
        policy: inputs.policy,
        cache_dir: &inputs.cache_dir,
        reporter,
        seed: &seed,
        patched_names: &patched_names,
        active_patches: &active_patches,
        source_replacements: &effective_config.source_replacements,
        incompatible_standards: crate::cli::config::resolve_incompatible_standards(
            &effective_config,
        )?,
        no_patches: args.no_patches,
        dev_for: &dev_for,
        system_provided: &initial_graph.root_settings.system_provided,
    })?;

    // Every fetched archive was hashed this run, cache hits included.
    for pkg in &pipeline.fetched {
        reporter.aux_verbose(format_args!(
            "cabin: checksum matched for {} {} ({})",
            pkg.name.as_str(),
            pkg.version,
            pkg.checksum
        ));
    }
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

#[tracing::instrument(level = "info", skip_all)]
fn run_resolution(request: &ResolutionRequest<'_>, reporter: Reporter) -> Result<()> {
    let manifest_path = absolutise(request.manifest_path)
        .with_context(|| format!("failed to resolve {}", request.manifest_path.display()))?;
    // Read before the workspace load: `CABIN_NET_OFFLINE` is validated
    // by reading it, and a malformed value must fail ahead of any
    // manifest diagnosis.
    let resolution_offline = crate::cli::config::effective_offline(request.offline)?;
    // CLI flags win; otherwise consult the merged effective
    // config for a `[registry]` default.  The orchestration layer
    // owns the final reconciliation; cabin-resolver / cabin-index
    // see only a concrete index source.
    let crate::cli::workspace_prep::WorkspacePrep {
        effective_config,
        active_patches,
        graph,
        ..
    } = crate::cli::workspace_prep::load_workspace_and_patches(&manifest_path, request.no_patches)?;
    let patched_names = active_patches.owned_patched_names();
    let resolved_index_source = crate::cli::config::resolve_index_source(
        request.index_path,
        request.index_url,
        &effective_config,
    )?;
    crate::cli::config::enforce_offline_index_source(
        resolution_offline,
        resolved_index_source.as_ref(),
    )?;
    let (effective_index_source, index_user_chosen): (Option<cabin_core::SourceLocator>, bool) =
        match resolved_index_source.as_ref() {
            Some(source) => {
                let (resolved, user_chosen) = crate::cli::config::resolve_index_locator(
                    source,
                    &effective_config,
                    resolution_offline,
                    request.no_patches,
                )?;
                (Some(resolved.resolved), user_chosen)
            }
            // The default hosted registry the fallback below picks
            // passes the environment-override gate on origin equality,
            // so it needs no provenance of its own - and a
            // `[source-replacement]` that rewrites it must not inherit
            // one.
            None => (None, false),
        };
    if request.policy.frozen()
        && matches!(
            effective_index_source,
            Some(cabin_core::SourceLocator::IndexUrl { .. })
        )
    {
        bail!(crate::cli::FROZEN_INDEX_URL_ERR);
    }

    // The seed folds the *referenced* patches' deps into the selected
    // packages' versioned deps, so `cabin resolve` (and `--package`
    // validation below) sees the same root set the artifact pipeline
    // does; a patch reached only through a registry edge resolves to
    // its fork's local entry.
    let dev_for: BTreeSet<String> = BTreeSet::new();
    let seed = ResolutionSeed::compute(&SeedInputs {
        graph: &graph,
        selection: &request.selection,
        request: &request.selection_request,
        patched_names: &patched_names,
        active_patches: &active_patches,
        dev_for: &dev_for,
    })?;
    let root_deps = seed.root_deps()?;
    let lockfile_path = lockfile_path_for(&manifest_path);

    // Validate `cabin update --package` before the empty-resolution
    // fast path, so an unknown name fails even when the workspace has
    // no versioned deps.
    if let LockPolicy::UpdatePackage(name) = &request.policy
        && !root_deps.contains_key(name)
    {
        // Only a name in `root_deps` can be targeted: a versioned
        // dependency declared by a local package of the selected
        // closure or by a patch fork it references.  A transitive
        // locked package is refreshed by `cabin update` without
        // `--package`.  The message names the selection, the scope
        // `root_deps` was gathered from.
        let scope = match seed.resolved_selection.packages.as_slice() {
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
    // `--locked`; the no-versioned-deps fast path below settles it
    // too.
    let active_patch_records = crate::cli::patch::lockfile_patches(&active_patches);
    let active_replacement_records = crate::cli::patch::lockfile_source_replacements(
        &effective_config.source_replacements,
        request.no_patches,
    );

    if root_deps.is_empty() {
        // No versioned deps to resolve: print a clear empty result.
        // Only `cabin update` touches the lockfile here, to settle a
        // stale patch record.
        settle_fast_path_patch_state(
            &lockfile_path,
            &request.policy,
            existing_lockfile.as_ref(),
            active_patch_records,
            active_replacement_records,
        )?;
        let (name, version) = match graph.root_package {
            Some(idx) => (
                graph.packages[idx].package.name.clone(),
                graph.packages[idx].package.version.clone(),
            ),
            None => cabin_workspace::synthetic_root_identity(),
        };
        let output = ResolveOutput {
            packages: vec![ResolvedPackage {
                name,
                version,
                source: ResolvedSource::Root,
                features: BTreeSet::new(),
                dependencies: BTreeSet::new(),
            }],
            held_back: Vec::new(),
        };
        emit_resolve_output(&output, request.format)?;
        return Ok(());
    }

    if request.policy.locked()
        && let Some(prev) = &existing_lockfile
    {
        ensure_patch_state_current(
            prev,
            &lockfile_path,
            &active_patch_records,
            &active_replacement_records,
        )?;
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
    let mut input = resolve_input(
        &graph,
        &seed,
        root_deps,
        existing_lockfile.as_ref(),
        request.policy.resolve_mode(),
    );
    let source = match &effective_index_source {
        cabin_core::SourceLocator::IndexPath { path } => {
            MetadataSource::File(open_local_index(path.as_std_path())?)
        }
        // The resolve pipeline performs no artifact downloads, so the
        // HTTP client the helper returns for connection reuse is
        // dropped here.
        cabin_core::SourceLocator::IndexUrl { url } => MetadataSource::Http(Box::new(
            open_http_index(url, index_user_chosen, reporter)?.0,
        )),
    };
    // Standard-aware version preference: the workspace consumer
    // standards order candidates under `fallback` (the default); the
    // knob comes from `[resolver] incompatible-standards` / env.  Never
    // changes solvability, so this is safe on every resolve path.
    // Consumers reached only through active `[patch]` overrides are not
    // folded in: a patch is a dependency override, not a workspace
    // member, and the index / pre-patch graph does not carry its
    // compile levels.  Like the workspace-minimum consumer proxy, this
    // is optimistic - it can only pick a too-new version that the
    // post-resolution validation (on the patched reload) then refuses,
    // never one `allow` would have avoided.
    input.consumer_standards = graph.consumer_standards(
        &seed.resolved_selection.closure(&graph),
        &seed.resolved_selection.packages,
        &crate::cli::enabled_features_by_package(&seed.features),
        &dev_for,
    );
    input.incompatible_standards =
        crate::cli::config::resolve_incompatible_standards(&effective_config)?;

    let (output, index) = resolve_with_lazy_index(&input, &source, false)?;
    ensure_registry_edges_link(&output, &input, &patched_names)?;

    let mut new_lockfile = lockfile_from_resolution(
        &output,
        &index,
        existing_lockfile.as_ref(),
        &input.mode,
        !seed.resolved_selection.covers_all_primaries(&graph),
    );
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
        // Locked mode takes every version from the lockfile, but the
        // lockfile this resolution derives can still differ: a
        // selection covering every member prunes entries nothing
        // needs (see `lockfile_from_resolution`).
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
    // Local selections (a `[patch]` fork reached through a registry
    // edge) are the workspace's own packages, not resolved ones.
    let mut others: Vec<&cabin_resolver::ResolvedPackage> = output
        .packages
        .iter()
        .filter(|p| p.source == ResolvedSource::Index)
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
        .filter(|p| p.source == ResolvedSource::Index)
        .map(|p| {
            serde_json::json!({
                "name": p.name.as_str(),
                "version": p.version.to_string(),
                "source": p.source.as_str(),
                "features": p.features,
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

/// What kind of resolution the CLI is asking for, plus the write /
/// network permissions that follow from it.  The resolver mode,
/// `frozen`, and `allow_write` all derive from this one value, so
/// they cannot disagree.
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
    /// Whether the user chose the origin `index_source` names -
    /// [`PipelineInputs::index_user_chosen`](crate::cli::config::PipelineInputs).
    /// Gates the `CABIN_REGISTRY_TOKEN` override on the index reads
    /// and artifact downloads this pipeline performs.
    pub(crate) index_user_chosen: bool,
    pub(crate) policy: LockPolicy,
    pub(crate) cache_dir: &'a Path,
    pub(crate) reporter: Reporter,
    /// The pre-resolution seed computed over `initial_graph` with
    /// the same patches and dev activation as the fields below - the
    /// one the caller already gated its no-versioned-deps fast path
    /// on.
    pub(crate) seed: &'a ResolutionSeed,
    /// Names the active patch set supplies locally, as `seed` was
    /// computed with: a registry edge onto one resolves to its local
    /// fork, which then counts as activated.
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
    /// The packages to take from the system rather than fetch: the
    /// root's `[system-provided]` table, or empty for `cabin vendor`,
    /// whose tree must build where the system lacks them.
    pub(crate) system_provided: &'a cabin_core::SystemProvidedSettings,
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
    /// What resolution left to the system.
    pub(crate) system: crate::cli::system_provided::SystemSplit,
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

/// How [`build_fetch_plan`] reaches source archives: `Local` passes
/// local paths through and refuses HTTP URLs; `Http` downloads
/// through the index's client.
enum IndexAccess {
    Local,
    Http(cabin_index_http::HttpClient),
}

/// Run the resolve → lockfile → fetch pipeline used by `cabin fetch`,
/// `cabin vendor`, and the `build` / `run` / `test` preamble.
#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn run_artifact_pipeline(
    request: &ArtifactPipelineRequest<'_>,
) -> Result<ArtifactPipeline> {
    let manifest_path = request.manifest_path;
    let graph = request.initial_graph;
    let seed = request.seed;
    // The seed already folded the *referenced* patches' deps in, so a
    // workspace whose only versioned dep is patched still resolves
    // and fetches the patched manifest's transitive registry edges; a
    // patch reached only through a registry edge resolves to its
    // fork's local entry.
    let root_deps = seed.root_deps()?;
    if root_deps.is_empty() {
        return Ok(ArtifactPipeline {
            fetched: Vec::new(),
            lockfile_pinned: BTreeSet::new(),
            activated_patches: seed.patched_deps.activated.clone(),
            system: crate::cli::system_provided::SystemSplit::default(),
        });
    }
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

    let mut input = resolve_input(
        graph,
        seed,
        root_deps,
        existing_lockfile.as_ref(),
        request.policy.resolve_mode(),
    );
    let (source, access) = open_index_for_pipeline(
        request.index_source,
        request.index_user_chosen,
        request.policy.frozen(),
        request.reporter,
    )?;
    // Standard-aware version preference, matching `cabin resolve`, so a
    // fresh `cabin build` writes the same lockfile.  Scoped to the
    // selected closure - an unselected member must not lower it.
    input.consumer_standards = graph.consumer_standards(
        &seed.resolved_selection.closure(graph),
        &seed.resolved_selection.packages,
        &enabled_features_by_package(&seed.features),
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
    {
        ensure_patch_state_current(
            prev,
            &lockfile_path,
            &active_patch_records,
            &active_replacement_records,
        )?;
    }

    // No pipeline caller renders `held_back`, so use the lean resolve
    // that skips the second `Allow`-mode solve behind the report.
    let (output, index) = resolve_with_lazy_index(&input, &source, true)?;
    ensure_registry_edges_link(&output, &input, request.patched_names)?;
    let mut activated_patches = activated_patches(seed, &output, request.patched_names);

    let mut new_lockfile = lockfile_from_resolution(
        &output,
        &index,
        existing_lockfile.as_ref(),
        &input.mode,
        !seed.resolved_selection.covers_all_primaries(graph),
    );
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

    let system =
        crate::cli::system_provided::split_resolution(&output, &index, request.system_provided)?;
    // A fork only provided packages reach is dormant like an unused
    // patch: nothing builds it, and its dependencies are not fetched.
    activated_patches.retain(|name| !system.unfetched.iter().any(|p| p.as_str() == name));
    let plan = build_fetch_plan(
        output
            .packages
            .iter()
            .filter(|p| !system.unfetched.contains(&p.name)),
        &index,
        &access,
        &new_lockfile,
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
        system,
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

/// Open `index_source` for an artifact-pipeline run: the metadata
/// source the resolver reads and the [`IndexAccess`] the fetch plan
/// uses.  `--frozen` refuses an HTTP index.
fn open_index_for_pipeline(
    index_source: &cabin_core::SourceLocator,
    index_user_chosen: bool,
    frozen: bool,
    reporter: Reporter,
) -> Result<(MetadataSource, IndexAccess)> {
    match index_source {
        cabin_core::SourceLocator::IndexPath { path } => Ok((
            MetadataSource::File(open_local_index(path.as_std_path())?),
            IndexAccess::Local,
        )),
        cabin_core::SourceLocator::IndexUrl { url } => {
            if frozen {
                bail!(FROZEN_INDEX_URL_ERR);
            }
            let (http_index, client) = open_http_index(url, index_user_chosen, reporter)?;
            Ok((
                MetadataSource::Http(Box::new(http_index)),
                IndexAccess::Http(client),
            ))
        }
    }
}

/// Open a local directory index, resolving the user-supplied path
/// first so error messages name the absolute location.  Shared by the
/// resolve pipeline and the fetch / build pipeline so the two paths
/// cannot drift.
fn open_local_index(path: &Path) -> Result<cabin_index::FileIndex> {
    let index_path =
        absolutise(path).with_context(|| format!("failed to resolve {}", path.display()))?;
    cabin_index::FileIndex::open(&index_path)
        .with_context(|| format!("failed to load index at {}", index_path.display()))
}

/// Open a sparse HTTP index.  Returns the client too, so the fetch /
/// build pipeline can reuse the connection for downloads; the resolve
/// pipeline discards it.
///
/// The client carries the stored credential (env override or
/// `credentials.toml`) for the index origin, so `config.json`,
/// package metadata, and artifact downloads all authenticate;
/// without a credential the client is tokenless.
///
/// `user_chosen` reports how the caller arrived at `url`; it gates
/// the origin-key-less `CABIN_REGISTRY_TOKEN` override, see
/// [`index_origin_user_chosen`](crate::cli::config::index_origin_user_chosen).
fn open_http_index(
    url: &str,
    user_chosen: bool,
    reporter: Reporter,
) -> Result<(cabin_index_http::HttpIndex, cabin_index_http::HttpClient)> {
    let mut client = cabin_index_http::HttpClient::new();
    if let Some(auth) = crate::cli::login::registry_auth_for_index_url(url, user_chosen, reporter)?
    {
        client = client.with_auth(auth);
    }
    let http_index = cabin_index_http::HttpIndex::open(url, client.clone())?;
    Ok((http_index, client))
}

/// Build a [`FetchPlan`] from resolved packages, the index they were
/// resolved against, and the lockfile the run just settled on.  Each
/// registry package contributes exactly one fetch entry.
///
/// The lockfile is the revision authority: its checksum names the
/// packaging revision to materialize, and a pinned-but-superseded
/// revision is fetched from its own `revisions` entry - that is the
/// fetchability guarantee that keeps existing lockfiles building
/// across respins.  A lockfile entry without a checksum falls back to
/// the version-level source and checksum, and fails when the index
/// lists no checksum either.
///
/// `access` decides whether HTTP-resolved sources get downloaded
/// here (so `cabin-artifact` stays HTTP-free) or whether the source
/// path is handed straight through as a local file.
#[tracing::instrument(level = "info", skip_all)]
fn build_fetch_plan<'a>(
    packages: impl Iterator<Item = &'a ResolvedPackage>,
    index: &PackageIndex,
    access: &IndexAccess,
    lockfile: &Lockfile,
) -> Result<FetchPlan> {
    let mut entries = Vec::new();
    for resolved in packages {
        // Local selections (a `[patch]` fork reached through a
        // registry edge, say) build from their working copy.
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
            (&revision.source, pinned.clone())
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
/// (`metadata` / `tree` / `explain`) and the no-versioned-deps fast
/// paths; a resolving run under `--locked` keeps its own bespoke
/// read so the missing-lockfile case stays a hard error there.
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

/// Under `--locked` the recorded `[[patch]]` / `[[source-replacement]]`
/// arrays must match the active policy exactly.
fn ensure_patch_state_current(
    prev: &Lockfile,
    lockfile_path: &Path,
    active_patches: &[cabin_lockfile::LockedPatch],
    active_replacements: &[cabin_lockfile::LockedSourceReplacement],
) -> Result<()> {
    if !prev.matches_patch_state(active_patches, active_replacements) {
        bail!(
            "--locked cannot be used because active patch / source-replacement policy differs from {}; re-run without --locked, or run cabin update when nothing is left to resolve, to refresh the record",
            lockfile_path.display()
        );
    }
    Ok(())
}

/// The patch / source-replacement check for a run that resolves
/// nothing: the active patches cover every versioned dependency, so
/// the comparison a resolving run makes is never reached.  `--locked`
/// refuses a stale record rather than silently succeeding.  Only
/// `cabin update` rewrites one, keeping the `[[package]]` list
/// verbatim: having resolved nothing, it cannot tell which entries
/// another selection or dependency activation still needs.  Every
/// other run leaves the lockfile alone, and a missing lockfile is
/// fine.
fn settle_fast_path_patch_state(
    lockfile_path: &Path,
    policy: &LockPolicy,
    prev: Option<&Lockfile>,
    patches: Vec<cabin_lockfile::LockedPatch>,
    source_replacements: Vec<cabin_lockfile::LockedSourceReplacement>,
) -> Result<()> {
    let Some(prev) = prev else {
        return Ok(());
    };
    if policy.locked() {
        return ensure_patch_state_current(prev, lockfile_path, &patches, &source_replacements);
    }
    if matches!(policy, LockPolicy::UpdateAll)
        && !prev.matches_patch_state(&patches, &source_replacements)
    {
        let refreshed = Lockfile {
            version: cabin_lockfile::LOCKFILE_VERSION,
            packages: prev.packages.clone(),
            patches,
            source_replacements,
        };
        cabin_lockfile::write_lockfile(lockfile_path, &refreshed)
            .with_context(|| format!("failed to write {}", lockfile_path.display()))?;
    }
    Ok(())
}

/// [`settle_fast_path_patch_state`] for the artifact commands'
/// no-versioned-deps fast paths, which read the lockfile only under
/// `--locked`: an unlocked run has no use for it.
pub(crate) fn enforce_locked_patch_state(
    manifest_path: &Path,
    policy: &LockPolicy,
    active_patches: &cabin_workspace::ActivePatchSet,
    source_replacements: &cabin_core::SourceReplacementSettings,
    no_patches: bool,
) -> Result<()> {
    if !policy.locked() {
        return Ok(());
    }
    let lockfile_path = lockfile_path_for(manifest_path);
    let prev = read_optional_lockfile(&lockfile_path)?;
    settle_fast_path_patch_state(
        &lockfile_path,
        policy,
        prev.as_ref(),
        crate::cli::patch::lockfile_patches(active_patches),
        crate::cli::patch::lockfile_source_replacements(source_replacements, no_patches),
    )
}

/// With `keep_unconsidered`, `previous` entries this resolution never
/// reached are carried over verbatim: a selection that leaves
/// workspace members out resolves only its own closure, and the
/// lockfile stays workspace-wide.  Only a selection covering every
/// member prunes entries nothing needs.  An entry for a patched name
/// is carried too: an unselected member may still reach that name
/// through a registry edge, which keeps it a registry entry.
fn lockfile_from_resolution(
    output: &ResolveOutput,
    index: &cabin_index::PackageIndex,
    previous: Option<&Lockfile>,
    mode: &cabin_resolver::ResolveMode,
    keep_unconsidered: bool,
) -> Lockfile {
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
        // Declared, not feature- or platform-active, edges: the list
        // must not churn across hosts.  A local selection has no entry.
        let deps: Vec<PackageName> = meta
            .dependencies
            .keys()
            .filter(|n| resolved_names.contains(n.as_str()))
            .cloned()
            .collect();
        // A previously pinned packaging revision is kept, exactly like
        // a locked version: someone else's respin must never churn
        // this lockfile.  The update modes move their targets to the
        // current revision; outside them the resolver has refused a
        // pin the index no longer lists.
        let kept_pin = match mode {
            cabin_resolver::ResolveMode::UpdateAll => None,
            cabin_resolver::ResolveMode::UpdatePackage(updated) if updated == &pkg.name => None,
            _ => previous
                .and_then(|prev| prev.find(&pkg.name))
                .filter(|locked| locked.version == pkg.version)
                .and_then(|locked| locked.checksum.clone()),
        };
        packages.push(LockedPackage {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            checksum: kept_pin.or_else(|| meta.checksum.clone()),
            dependencies: deps,
        });
    }
    if keep_unconsidered && let Some(prev) = previous {
        packages.extend(
            prev.packages
                .iter()
                .filter(|locked| !resolved_names.contains(locked.name.as_str()))
                .cloned(),
        );
    }
    packages.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    Lockfile {
        version: cabin_lockfile::LOCKFILE_VERSION,
        packages,
        patches: Vec::new(),
        source_replacements: Vec::new(),
    }
}
