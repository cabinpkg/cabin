use crate::error::WorkspaceError;
use crate::graph::{DependencyEdge, PackageGraph, PackageKind, WorkspacePackage};
use cabin_core::{DependencyKind, DependencySource, PackageName, PortDepSource};
use cabin_manifest::ParsedManifest;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

mod members;
#[cfg(test)]
mod tests;
mod topo;

use self::members::{
    WorkspaceMembers, expand_workspace_members, parse_workspace_dep_source,
    resolve_workspace_dependencies, resolve_workspace_standards, validate_workspace_pattern,
};
use self::topo::topo_sort;

/// One registry package source that has already been fetched and
/// extracted by `cabin-artifact`. `cabin-workspace` accepts these
/// pre-resolved entries via [`load_workspace_with_options`] so it can
/// fold them into the package graph alongside local packages.
#[derive(Debug, Clone)]
pub struct RegistryPackageSource {
    pub name: PackageName,
    pub version: semver::Version,
    /// Absolute path to the extracted package's `cabin.toml`.
    pub manifest_path: PathBuf,
}

/// One patched package source.  Like [`RegistryPackageSource`],
/// the loader stitches the supplied `(name, version,
/// manifest_path)` into the graph; unlike a registry entry, the
/// resulting [`WorkspacePackage`] is tagged `kind = PackageKind::Local`
/// because the user pointed Cabin at a local working copy.  The
/// orchestration layer in `cabin` filters the regular
/// registry list so a patched name's only entry comes from
/// `patches`.
#[derive(Debug, Clone)]
pub struct PatchedPackageSource {
    pub name: PackageName,
    pub version: semver::Version,
    /// Absolute path to the patched package's `cabin.toml`.
    pub manifest_path: PathBuf,
}

/// One foundation-port package source.  Built by the CLI
/// orchestration layer after [`cabin_port::prepare()`] materializes
/// the port directory; the loader resolves a
/// [`DependencySource::Port`] declaration to the matching entry
/// here and inserts a [`WorkspacePackage`] tagged
/// `kind = PackageKind::Local` (foundation ports are local
/// development policy and never enter published metadata).
#[derive(Debug, Clone)]
pub struct PortPackageSource {
    /// Authoritative identity declared by `port.toml`.
    pub name: PackageName,
    pub version: semver::Version,
    /// Absolute path to the prepared port directory's overlay
    /// `cabin.toml`.  The workspace loader treats this as the
    /// dep's `manifest_path`.
    pub manifest_path: PathBuf,
    /// How the recipe was located.  Drives whether the dep
    /// walker looks this entry up by canonical port directory
    /// (`PortDir`) or by package name (`Builtin`).
    pub origin: cabin_port::PortOrigin,
}

/// Load a workspace or a single package starting from the given manifest
/// Path.  Workspace members and local path dependencies are resolved
/// recursively against the filesystem; a topologically-sorted
/// [`PackageGraph`] is returned.
///
/// This is the convenience form for callers that only have local
/// packages.  For registry / patch / dev-dep policy, use
/// [`load_workspace_with_options`].
///
/// # Errors
/// Returns a [`WorkspaceError`] when loading fails - the manifest is
/// missing or unreadable, contains neither `[package]` nor
/// `[workspace]`, a workspace member or local path dependency cannot
/// be resolved, package names collide, a dependency cycle is
/// detected, or (because this runs with the strict port policy) a
/// foundation-port dependency has not been prepared.
pub fn load_workspace(manifest_path: impl AsRef<Path>) -> Result<PackageGraph, WorkspaceError> {
    load_workspace_inner(
        manifest_path,
        &[],
        &[],
        &[],
        &RegistryEnforcement::strict(),
        &BTreeSet::new(),
        &PortMode::Strict,
    )
}

/// Load the workspace structure (members, profiles, package
/// names) without resolving foundation-port dependency edges.
///
/// Use this for commands that only need workspace topology -
/// `cabin clean`, `cabin package`, `cabin publish` - and that
/// must run on fresh checkouts where no port archive has been
/// downloaded yet.  Port deps are dropped from the loaded graph
/// (they never become [`DependencyEdge`]s) but the consuming
/// packages still load normally; foundation-port packages
/// themselves are absent from `graph.packages`.
///
/// # Errors
/// Returns a [`WorkspaceError`] when loading fails - the manifest is
/// missing or unreadable, contains neither `[package]` nor
/// `[workspace]`, a workspace member or local path dependency cannot
/// be resolved, package names collide, or a dependency cycle is
/// detected.  Because port edges are dropped, the
/// port-not-prepared / port-directory-missing variants never apply.
pub fn load_workspace_skip_ports(
    manifest_path: impl AsRef<Path>,
) -> Result<PackageGraph, WorkspaceError> {
    load_workspace_inner(
        manifest_path,
        &[],
        &[],
        &[],
        &RegistryEnforcement::strict(),
        &BTreeSet::new(),
        &PortMode::SkipAll,
    )
}

/// Options bag for the workspace loader.  Threads custom policy
/// (registry / patches / ports / dev-dep activation) through a
/// single call.
#[derive(Debug, Clone)]
pub struct WorkspaceLoadOptions<'a> {
    /// Already-resolved registry package sources.
    pub registry: &'a [RegistryPackageSource],
    /// Active patches (resolved by `cabin-workspace::patch`).
    pub patches: &'a [PatchedPackageSource],
    /// Foundation ports that have already been prepared by
    /// `cabin_port::prepare` (downloaded, checksum-verified,
    /// safely extracted with `strip_prefix`, overlay applied).
    /// The loader resolves a [`DependencySource::Port`]
    /// declaration to the matching entry here.
    pub ports: &'a [PortPackageSource],
    /// How the loader treats a missing-registry edge: every parent
    /// is strict by default; pre-resolution loads use
    /// [`RegistryPolicy::StrictFor`] to scope enforcement (or
    /// disable it with an empty set).
    pub registry_policy: RegistryPolicy<'a>,
    /// Names of packages whose `[dev-dependencies]` should be
    /// loaded as real graph edges.  Empty matches the
    /// `cabin build` policy of treating dev-deps as
    /// declaration-only; `cabin test` populates this with the
    /// names of the test-running packages.
    pub include_dev_for: &'a BTreeSet<String>,
    /// How the loader resolves `DependencySource::Port` entries.
    /// Defaults to [`PortPolicy::Strict`] - every port-dep must
    /// be present in `ports` (and on disk, for `port-path`).
    /// Callers that scope port preparation to a narrower
    /// selection than the full primary-package set use
    /// [`PortPolicy::TolerateExcept`] with the selected names
    /// so siblings' missing ports are silently skipped while
    /// selected packages still surface the typed
    /// `PortDependencyNotPrepared` / `PortDirectoryMissing`
    /// diagnostic.
    pub port_policy: PortPolicy<'a>,
}

/// How the loader treats `DependencySource::Port` declarations
/// from a [`WorkspaceLoadOptions`] call.
#[derive(Debug, Clone, Default)]
pub enum PortPolicy<'a> {
    /// A port dep must be either a `port-path` directory on disk
    /// plus present in `ports`, or a `port = true` name present in
    /// `ports`.  Anything else surfaces the typed
    /// `PortDependencyNotPrepared` / `PortDirectoryMissing`
    /// diagnostic.  Default.
    #[default]
    Strict,
    /// Tolerate missing port deps *except* for parent packages
    /// whose names appear in this set - the caller's selected
    /// closure.  Names in the set still surface the typed
    /// diagnostics; names outside the set silently skip the
    /// missing edge.
    ///
    /// Passing an empty set tolerates every parent (legacy
    /// "tolerate-all" behavior); pass a populated set to keep
    /// selected packages strict while unselected siblings
    /// tolerate.
    TolerateExcept(&'a BTreeSet<String>),
}

/// How the loader treats a versioned dependency edge whose name is
/// not present in `registry`.  Pre-resolution loads (port discovery,
/// `cabin metadata` fallback) carry no registry yet but may carry
/// patches that contribute names to the loader's internal name map;
/// the [`RegistryPolicy::StrictFor`] variant lets callers scope
/// enforcement so the resolver-less paths don't surface bogus
/// missing-registry diagnostics.
#[derive(Debug, Clone, Default)]
pub enum RegistryPolicy<'a> {
    /// Every parent's registry deps must be present in `registry`.
    /// Default.  Used after the resolver has populated `registry`
    /// with the closure's full pinned set.
    #[default]
    Strict,
    /// Strict only for parents whose names appear in the set;
    /// names outside silently skip a missing-registry edge.
    /// Passing an empty set tolerates every parent - used by
    /// pre-resolution loads.
    StrictFor(&'a BTreeSet<String>),
}

/// Load the workspace with a single options bag.  When
/// `include_dev_for` is empty the loader follows the
/// `cabin build` policy of treating dev-deps as
/// declaration-only; with a non-empty set, listed packages
/// contribute their `[dev-dependencies]` as real graph edges
/// (path-deps are materialized, version-deps reach the
/// resolver).  Dev-deps still don't propagate transitively -
/// only the listed packages activate them.
///
/// # Errors
/// Returns a [`WorkspaceError`] when loading fails - covering the
/// manifest, member-expansion, local-path, duplicate-name, and
/// cycle failures of [`load_workspace`], plus the policy-driven
/// variants this entry point enables: unresolved registry
/// dependencies, registry-source name/version mismatches, and
/// unprepared or missing foundation-port dependencies for parents
/// the registry / port policy treats as strict.
pub fn load_workspace_with_options(
    manifest_path: impl AsRef<Path>,
    options: &WorkspaceLoadOptions<'_>,
) -> Result<PackageGraph, WorkspaceError> {
    let policy = match &options.registry_policy {
        RegistryPolicy::Strict => RegistryEnforcement::strict(),
        RegistryPolicy::StrictFor(set) => RegistryEnforcement::scoped((*set).clone()),
    };
    let port_mode = match &options.port_policy {
        PortPolicy::Strict => PortMode::Strict,
        PortPolicy::TolerateExcept(strict) => PortMode::TolerateExcept((*strict).clone()),
    };
    load_workspace_inner(
        manifest_path,
        options.registry,
        options.patches,
        options.ports,
        &policy,
        options.include_dev_for,
        &port_mode,
    )
}

/// How strictly missing registry entries are enforced.  Internal
/// mirror of [`RegistryPolicy`] - public callers pick the policy via
/// the enum; the loader collapses it to this owned form so the rest
/// of the load path doesn't carry the lifetime parameter.
#[derive(Debug, Clone)]
struct RegistryEnforcement {
    /// `Some` -> only enforce missing-registry for the listed
    /// package names; `None` -> enforce for every package
    /// (the strict default).
    strict_packages: Option<BTreeSet<String>>,
}

impl RegistryEnforcement {
    fn strict() -> Self {
        Self {
            strict_packages: None,
        }
    }

    fn scoped(strict_packages: BTreeSet<String>) -> Self {
        Self {
            strict_packages: Some(strict_packages),
        }
    }

    fn requires_registry_for(&self, parent_name: &str) -> bool {
        match &self.strict_packages {
            None => true,
            Some(set) => set.contains(parent_name),
        }
    }
}

/// How the loader treats `DependencySource::Port` declarations.
/// Internal mirror of [`PortPolicy`] that also models the
/// "skip every port edge unconditionally" mode used by
/// [`load_workspace_skip_ports`].
#[derive(Debug, Clone)]
enum PortMode {
    /// Default: a port dep must be either a `port-path` directory
    /// on disk + present in `ports`, or a `port = true` name
    /// present in `ports`.  Anything else surfaces the typed
    /// `PortDependencyNotPrepared` / `PortDirectoryMissing`
    /// diagnostic.  Used by `load_workspace` /
    /// `load_workspace_with_options` against the full
    /// primary-package set.
    Strict,
    /// Drop every port-dep edge silently.  Used by
    /// [`load_workspace_skip_ports`] for commands that only need
    /// workspace topology (`cabin clean`, `cabin package`,
    /// `cabin publish`).
    SkipAll,
    /// Link present port deps as graph edges; silently skip ones
    /// whose source is absent from `ports` (or whose port-path
    /// directory is missing on disk) *except* for parents whose
    /// names appear in this set - the caller's selected closure
    /// still surfaces the typed diagnostics so a typoed
    /// `port-path` in a selected package fails fast instead of
    /// being silently dropped.
    TolerateExcept(BTreeSet<String>),
}

#[allow(clippy::too_many_lines)] // linear scan-then-load pipeline; splitting would scatter the load loop
fn load_workspace_inner(
    manifest_path: impl AsRef<Path>,
    registry: &[RegistryPackageSource],
    patches: &[PatchedPackageSource],
    ports: &[PortPackageSource],
    policy: &RegistryEnforcement,
    include_dev_for: &BTreeSet<String>,
    port_mode: &PortMode,
) -> Result<PackageGraph, WorkspaceError> {
    let manifest_path = canonicalize(manifest_path.as_ref())?;
    let root_dir = manifest_path
        .parent()
        .ok_or_else(|| WorkspaceError::Io {
            path: manifest_path.clone(),
            source: std::io::Error::other("manifest path has no parent directory"),
        })?
        .to_path_buf();

    let root_manifest = parse_manifest(&manifest_path)?;
    if root_manifest.package.is_none() && root_manifest.workspace.is_none() {
        return Err(WorkspaceError::EmptyManifest {
            path: manifest_path,
        });
    }

    // Target-conditional dep tables are evaluated against the
    // host platform - Cabin does not yet support
    // cross-compilation.  Future steps may thread an explicit
    // target context through this loader; for now the host is
    // the single source of truth.
    let host_platform = cabin_core::TargetPlatform::current();

    let is_workspace_root = root_manifest.workspace.is_some();

    let mut loader = Loader::default();

    let WorkspaceRootScan {
        primary_manifest_paths,
        excluded_member_paths,
        default_members: workspace_default_members,
        workspace_deps,
        workspace_standards,
    } = scan_workspace_root(&root_manifest, &root_dir, &manifest_path)?;

    let port_lookup = build_port_lookup(ports)?;
    let registry_lookup = build_registry_lookup(registry, patches)?;

    // Recursively load every primary manifest plus any path deps it pulls
    // in.  The loader is iterative - we maintain a stack of unloaded
    // manifests rather than recursing.
    let mut to_load = initial_load_queue(&primary_manifest_paths, registry, patches, ports)?;
    let root_manifest_path = manifest_path.clone();
    let ctx = DepResolutionContext {
        host_platform: &host_platform,
        skip_port_edges: matches!(port_mode, PortMode::SkipAll),
        tolerate_strict_set: match port_mode {
            PortMode::TolerateExcept(set) => Some(set),
            _ => None,
        },
        ports: &port_lookup,
        registry: &registry_lookup,
        policy,
        include_dev_for,
    };
    while let Some(manifest_path) = to_load.pop() {
        if loader.manifest_index.contains_key(&manifest_path) {
            continue;
        }
        let parsed = parse_manifest(&manifest_path)?;
        let package = parsed.package.ok_or_else(|| {
            // A path dependency that resolves to a workspace-only manifest.
            WorkspaceError::LocalDependencyIsWorkspace {
                dep_name: project_alias_for(&loader, &manifest_path),
                path: manifest_path.clone(),
            }
        })?;

        reject_member_manifest_overrides(&package, &manifest_path, &root_manifest_path)?;
        validate_registry_pin(&package, &manifest_path, &registry_lookup)?;

        let manifest_dir = manifest_path
            .parent()
            .expect("canonicalized manifest path has a parent")
            .to_path_buf();

        // Registry- and port-materialized manifests never reach the
        // workspace rewrites: their markers (dependency or standard
        // field) are rejected defensively first.
        reject_external_workspace_markers(
            &package,
            &manifest_path,
            &registry_lookup,
            &port_lookup,
        )?;

        // rewrite each `{ workspace = true }` dep into the
        // resolved source from `[workspace.dependencies]` before any
        // other consumer sees it.
        let package = resolve_workspace_dependencies(package, &workspace_deps)?;

        // rewrite each `{ workspace = true }` standard-field marker
        // into the value inherited from the root `[workspace]`
        // declaration, mirroring the dependency rewrite above.
        let package = resolve_workspace_standards(package, workspace_standards, &manifest_path)?;

        // With every declaration resolved, reject interface minimums
        // newer than the implementation standard the target's own
        // sources compile with - such a target could not include its
        // own public headers.
        if let Some(contradiction) = cabin_core::find_interface_standard_contradictions(&package)
            .into_iter()
            .next()
        {
            return Err(WorkspaceError::InterfaceStandardContradiction {
                path: manifest_path,
                source: contradiction,
            });
        }

        let dep_paths = resolve_dep_paths(&ctx, &package, &manifest_path, &manifest_dir)?;
        verify_dep_path_names(&dep_paths)?;

        let index = loader.packages.len();
        loader.manifest_index.insert(manifest_path.clone(), index);
        loader.packages.push(LoadedPackage {
            package,
            manifest_path,
            manifest_dir,
            dep_paths,
        });
        for dep in &loader.packages[index].dep_paths {
            to_load.push(dep.path.clone());
        }
    }

    reject_duplicate_package_names(&loader.packages)?;
    let packages = link_workspace_packages(&loader, &registry_lookup, &port_lookup);
    let (sorted, new_position) = apply_topo_order(packages)?;

    let primary_packages: Vec<usize> = primary_manifest_paths
        .iter()
        .map(|p| new_position[&loader.manifest_index[p]])
        .collect();

    let root_package = root_manifest
        .package
        .is_some()
        .then(|| new_position[&loader.manifest_index[&manifest_path]]);

    let default_members = resolve_default_members(
        &workspace_default_members,
        &root_dir,
        &loader,
        &new_position,
        &primary_packages,
    )?;

    Ok(PackageGraph {
        root_manifest_path: manifest_path,
        root_dir,
        is_workspace_root,
        root_package,
        root_settings: root_manifest.root_settings.into(),
        primary_packages,
        default_members,
        excluded_members: excluded_member_paths,
        packages: sorted,
    })
}

/// Workspace-root facts collected before the load loop runs: the
/// primary manifest set (root package plus expanded members), the
/// excluded member paths, the raw `default-members` entries, the
/// parsed shared `[workspace.<kind>-dependencies]` tables, and the
/// `[workspace]`-level language-standard defaults.
struct WorkspaceRootScan {
    primary_manifest_paths: Vec<PathBuf>,
    excluded_member_paths: Vec<PathBuf>,
    default_members: Vec<String>,
    workspace_deps: BTreeMap<DependencyKind, BTreeMap<String, DependencySource>>,
    workspace_standards: cabin_core::WorkspaceStandardDefaults,
}

/// Seed the primary-package set from the root manifest and, when a
/// `[workspace]` table is present, expand member globs (rejecting
/// nested workspaces) and parse the shared dependency tables.
fn scan_workspace_root(
    root_manifest: &ParsedManifest,
    root_dir: &Path,
    manifest_path: &Path,
) -> Result<WorkspaceRootScan, WorkspaceError> {
    // Roots are the entry points whose path-deps we recursively follow
    // and whose primary status we record.  They are: the root manifest if
    // it has a [package], and every workspace member.
    let mut primary_manifest_paths: Vec<PathBuf> = Vec::new();

    if root_manifest.package.is_some() {
        primary_manifest_paths.push(manifest_path.to_path_buf());
    }

    // Workspace.default_members captured here so we can validate it
    // against the resolved primary set after member expansion.
    let mut default_members: Vec<String> = Vec::new();
    // Workspace dependency tables captured up-front and parsed
    // once.  Member manifests with `dep = { workspace = true }`
    // resolve against the table that matches their declared
    // [`DependencyKind`] - `[workspace.dependencies]` for normal
    // deps, `[workspace.dev-dependencies]` for dev deps.
    // Each entry stores only the resolved `DependencySource` since
    // the inheriting dep already knows its own kind.
    let mut workspace_deps: BTreeMap<DependencyKind, BTreeMap<String, DependencySource>> =
        BTreeMap::new();
    // `[workspace]`-level standard defaults that members opt into
    // per field with `<field> = { workspace = true }`.  Literal
    // values only; absent fields stay `None` so an opt-in without a
    // matching declaration fails at resolution time.
    let mut workspace_standards = cabin_core::WorkspaceStandardDefaults::default();

    let mut excluded_member_paths: Vec<PathBuf> = Vec::new();
    if let Some(workspace) = &root_manifest.workspace {
        let WorkspaceMembers { included, excluded } =
            expand_workspace_members(root_dir, &workspace.members, &workspace.exclude)?;
        for canonical in included {
            // reject nested workspaces.  A member directory's
            // `cabin.toml` must not declare its own `[workspace]`
            // table, otherwise the load tries to honor two parent
            // workspaces at once.
            let parsed = parse_manifest(&canonical)?;
            if parsed.workspace.is_some() {
                return Err(WorkspaceError::NestedWorkspace { path: canonical });
            }
            primary_manifest_paths.push(canonical);
        }
        excluded_member_paths = excluded;
        default_members.clone_from(&workspace.default_members);
        workspace_standards = workspace.standards;
        for (kind, table) in [
            (DependencyKind::Normal, &workspace.dependencies),
            (DependencyKind::Dev, &workspace.dev_dependencies),
        ] {
            if table.is_empty() {
                continue;
            }
            let entry = workspace_deps.entry(kind).or_default();
            for (name, req) in table {
                entry.insert(name.clone(), parse_workspace_dep_source(name, req)?);
            }
        }
    }
    Ok(WorkspaceRootScan {
        primary_manifest_paths,
        excluded_member_paths,
        default_members,
        workspace_deps,
        workspace_standards,
    })
}

/// Lookup maps for prepared foundation ports.  The dep walker
/// resolves `DependencySource::Port` declarations via one of two
/// maps depending on the origin:
/// - `PortDir`: canonical `port_dir` -> prepared `manifest_path`
/// - `Builtin`: package name -> prepared `manifest_path`
///
/// The `port_dir` is canonicalized up-front so the lookup is a
/// single `HashMap` probe per dep - and so two consumers that reach
/// the same port through different relative paths still see the
/// same prepared source.
struct PortLookup {
    by_canonical_dir: HashMap<PathBuf, PathBuf>,
    by_name: HashMap<String, PathBuf>,
    /// Canonical overlay-manifest paths of every prepared foundation
    /// port.  Read both for the `Local` port classification and for
    /// the `is_port` graph tag.  Keep this the single source of
    /// truth - a second, separately populated set risks silently
    /// diverging.
    canonical_paths: HashSet<PathBuf>,
}

fn build_port_lookup(ports: &[PortPackageSource]) -> Result<PortLookup, WorkspaceError> {
    let mut by_canonical_dir: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut by_name: HashMap<String, PathBuf> = HashMap::new();
    let mut canonical_paths: HashSet<PathBuf> = HashSet::new();
    for entry in ports {
        match &entry.origin {
            cabin_port::PortOrigin::PortDir(port_dir) => {
                let port_dir_canonical = canonicalize(port_dir)?;
                if let Some(previous) =
                    by_canonical_dir.insert(port_dir_canonical, entry.manifest_path.clone())
                {
                    return Err(WorkspaceError::DuplicatePackageName {
                        name: entry.name.as_str().to_owned(),
                        first: previous,
                        second: entry.manifest_path.clone(),
                    });
                }
            }
            cabin_port::PortOrigin::Builtin(name) => {
                if let Some(previous) =
                    by_name.insert((*name).to_owned(), entry.manifest_path.clone())
                {
                    return Err(WorkspaceError::DuplicatePackageName {
                        name: entry.name.as_str().to_owned(),
                        first: previous,
                        second: entry.manifest_path.clone(),
                    });
                }
            }
        }
        canonical_paths.insert(canonicalize(&entry.manifest_path)?);
    }
    Ok(PortLookup {
        by_canonical_dir,
        by_name,
        canonical_paths,
    })
}

/// Name / canonical-path lookup maps for registry and patch
/// sources, canonicalizing paths so the dedup-by-canonical-path
/// steps see a consistent value.  Patches contribute the same
/// `(name, version, manifest_path)` information as registry entries
/// but ultimately produce local-kind packages.
struct RegistryLookup<'a> {
    /// name -> canonical manifest path (registry entries plus
    /// patches); the loader resolves versioned deps through this.
    by_name: HashMap<&'a str, PathBuf>,
    /// Canonical manifest path -> expected package name, so loading
    /// can compare the actual manifest contents against what the
    /// resolver pinned.
    canonical_names: HashMap<PathBuf, &'a PackageName>,
    /// Canonical manifest path -> expected version.
    canonical_versions: HashMap<PathBuf, &'a semver::Version>,
    canonical_paths: HashSet<PathBuf>,
    /// Canonical manifest paths that came from `patches` - these
    /// stay `PackageKind::Local` even though they also appear in
    /// the maps above.
    patch_canonical_paths: HashSet<PathBuf>,
}

fn build_registry_lookup<'a>(
    registry: &'a [RegistryPackageSource],
    patches: &'a [PatchedPackageSource],
) -> Result<RegistryLookup<'a>, WorkspaceError> {
    let mut by_name: HashMap<&str, PathBuf> = HashMap::new();
    let mut canonical_names: HashMap<PathBuf, &PackageName> = HashMap::new();
    let mut canonical_versions: HashMap<PathBuf, &semver::Version> = HashMap::new();
    let mut canonical_paths: HashSet<PathBuf> = HashSet::new();
    let mut patch_canonical_paths: HashSet<PathBuf> = HashSet::new();
    for entry in registry {
        let canonical = canonicalize(&entry.manifest_path)?;
        by_name.insert(entry.name.as_str(), canonical.clone());
        canonical_names.insert(canonical.clone(), &entry.name);
        canonical_versions.insert(canonical.clone(), &entry.version);
        canonical_paths.insert(canonical);
    }
    // Defensively reject overlap between patches and the registry
    // list so a caller bug never silently flips Local to Registry
    // mid-graph.
    for entry in patches {
        let canonical = canonicalize(&entry.manifest_path)?;
        if canonical_paths.contains(&canonical) {
            return Err(WorkspaceError::PatchConflictsWithRegistry {
                package: entry.name.as_str().to_owned(),
                path: canonical,
            });
        }
        if let Some(existing) = by_name.insert(entry.name.as_str(), canonical.clone()) {
            return Err(WorkspaceError::DuplicatePackageName {
                name: entry.name.as_str().to_owned(),
                first: existing,
                second: canonical,
            });
        }
        canonical_names.insert(canonical.clone(), &entry.name);
        canonical_versions.insert(canonical.clone(), &entry.version);
        canonical_paths.insert(canonical.clone());
        patch_canonical_paths.insert(canonical);
    }
    Ok(RegistryLookup {
        by_name,
        canonical_names,
        canonical_versions,
        canonical_paths,
        patch_canonical_paths,
    })
}

/// Assemble the initial load queue: every primary manifest plus the
/// registry, patch, and port manifests.  The externals are not
/// primary, but they must appear in the package graph.
fn initial_load_queue(
    primary_manifest_paths: &[PathBuf],
    registry: &[RegistryPackageSource],
    patches: &[PatchedPackageSource],
    ports: &[PortPackageSource],
) -> Result<Vec<PathBuf>, WorkspaceError> {
    let mut to_load: Vec<PathBuf> = primary_manifest_paths.to_vec();
    for entry in registry {
        to_load.push(canonicalize(&entry.manifest_path)?);
    }
    // Patches are external manifests too; load them so the
    // graph carries the patched `Package` value alongside the
    // workspace members and registry entries.
    for entry in patches {
        to_load.push(canonicalize(&entry.manifest_path)?);
    }
    // Ports are also external manifests.  They live in the
    // foundation-port cache directory; load them so the graph
    // carries the prepared overlay `Package` value alongside
    // workspace members.
    for entry in ports {
        to_load.push(canonicalize(&entry.manifest_path)?);
    }
    Ok(to_load)
}

/// `[profile.*]`, `[toolchain]`, `[build]`, and `[patch]`
/// tables are only honored on the entry-point manifest.  Member and
/// path-dep manifests that declare them surface a clear error
/// rather than being silently ignored, so a single workspace key
/// cannot mean different things in different members.
fn reject_member_manifest_overrides(
    package: &cabin_core::Package,
    manifest_path: &Path,
    root_manifest_path: &Path,
) -> Result<(), WorkspaceError> {
    if manifest_path == root_manifest_path {
        return Ok(());
    }
    if !package.profiles.is_empty() {
        return Err(WorkspaceError::MemberDeclaresProfiles {
            package: package.name.as_str().to_owned(),
            path: manifest_path.to_path_buf(),
        });
    }
    if !package.toolchain.is_empty() {
        return Err(WorkspaceError::MemberDeclaresToolchain {
            package: package.name.as_str().to_owned(),
            path: manifest_path.to_path_buf(),
        });
    }
    if package.compiler_wrapper.is_some() {
        return Err(WorkspaceError::MemberDeclaresCompilerWrapper {
            package: package.name.as_str().to_owned(),
            path: manifest_path.to_path_buf(),
        });
    }
    if !package.patches.is_empty() {
        return Err(WorkspaceError::MemberDeclaresPatches {
            package: package.name.as_str().to_owned(),
            path: manifest_path.to_path_buf(),
        });
    }
    Ok(())
}

/// If this manifest is a known registry package, the resolver
/// pinned a specific (name, version).  The artifact crate has
/// already validated the manifest against that pin, but the
/// workspace loader is the user-visible reporter, so we
/// double-check here and surface a clear error if they ever
/// disagree.  Both the expected name and version are validated: the
/// registry may have pointed at a directory whose manifest declares
/// a completely different package (a malicious or wrongly extracted
/// artifact); refusing here keeps a wrong package from sneaking
/// into the build graph.
fn validate_registry_pin(
    package: &cabin_core::Package,
    manifest_path: &Path,
    registry: &RegistryLookup<'_>,
) -> Result<(), WorkspaceError> {
    let Some(expected_version) = registry.canonical_versions.get(manifest_path) else {
        return Ok(());
    };
    let expected_name = registry.canonical_names.get(manifest_path).copied();
    let version_ok = &package.version == *expected_version;
    let name_ok = expected_name.is_none_or(|n| n.as_str() == package.name.as_str());
    if !name_ok {
        return Err(WorkspaceError::RegistryPackageNameMismatch {
            name: expected_name
                .map(|n| n.as_str().to_owned())
                .unwrap_or_default(),
            actual_name: package.name.as_str().to_owned(),
            path: manifest_path.to_path_buf(),
        });
    }
    if !version_ok {
        return Err(WorkspaceError::RegistryPackageMismatch {
            name: expected_name
                .map(|n| n.as_str().to_owned())
                .unwrap_or_default(),
            version: expected_version.to_string(),
            actual_name: package.name.as_str().to_owned(),
            actual_version: package.version.to_string(),
            path: manifest_path.to_path_buf(),
        });
    }
    Ok(())
}

/// Reject `{ workspace = true }` markers - standard fields and
/// dependency sources alike - on manifests that were materialized
/// from a registry archive or a prepared foundation port.  Their
/// standards and dependency requirements must be self-contained
/// literal values - resolving a marker against the *consuming*
/// workspace's `[workspace]` tables would silently let an external
/// package's compile standard or dependency source be chosen by the
/// consumer.  Publish-side normalization keeps legitimately
/// published archives marker-free, so this guard only fires on
/// hand-crafted inputs.
///
/// The origin classification mirrors [`link_workspace_packages`]:
/// patches take precedence and stay local (a patched working copy
/// is user-controlled, so it resolves markers like any other local
/// manifest), then ports, then registry entries.
fn reject_external_workspace_markers(
    package: &cabin_core::Package,
    manifest_path: &Path,
    registry: &RegistryLookup<'_>,
    ports: &PortLookup,
) -> Result<(), WorkspaceError> {
    if registry.patch_canonical_paths.contains(manifest_path) {
        return Ok(());
    }
    let origin = if ports.canonical_paths.contains(manifest_path) {
        "foundation-port"
    } else if registry.canonical_paths.contains(manifest_path) {
        "registry"
    } else {
        return Ok(());
    };
    if let Some(field) = package.language.workspace_marker_field() {
        return Err(WorkspaceError::ExternalPackageDeclaresWorkspaceStandard {
            origin,
            package: package.name.as_str().to_owned(),
            field,
            path: manifest_path.to_path_buf(),
        });
    }
    if let Some(dep) = package
        .dependencies
        .iter()
        .find(|dep| matches!(dep.source, DependencySource::Workspace))
    {
        return Err(WorkspaceError::ExternalPackageDeclaresWorkspaceDependency {
            origin,
            package: package.name.as_str().to_owned(),
            dep_name: dep.name.as_str().to_owned(),
            path: manifest_path.to_path_buf(),
        });
    }
    Ok(())
}

/// Loop-invariant inputs for [`resolve_dep_paths`]: the prepared
/// lookup maps, the active policies, and the host platform every
/// per-package dependency walk consults.
struct DepResolutionContext<'a> {
    host_platform: &'a cabin_core::TargetPlatform,
    /// `true` under [`PortMode::SkipAll`]: drop every port edge.
    skip_port_edges: bool,
    /// `Some` under [`PortMode::TolerateExcept`]: parents *not* in
    /// the set silently skip missing port deps.
    tolerate_strict_set: Option<&'a BTreeSet<String>>,
    ports: &'a PortLookup,
    registry: &'a RegistryLookup<'a>,
    policy: &'a RegistryEnforcement,
    include_dev_for: &'a BTreeSet<String>,
}

/// Resolve one loaded package's dependency declarations into
/// [`DepPath`] entries, applying the kind-activation, platform,
/// port, and registry policies.
fn resolve_dep_paths(
    ctx: &DepResolutionContext<'_>,
    package: &cabin_core::Package,
    manifest_path: &Path,
    manifest_dir: &Path,
) -> Result<Vec<DepPath>, WorkspaceError> {
    // Dev dependencies are declaration-only for ordinary
    // commands but become real graph edges when the loader is
    // told to "include dev for" this package - typically by
    // `cabin test` for the test-running packages.  The opt-in
    // never propagates: a transitive dep's own dev-deps stay
    // declaration-only.
    let dev_active_for_this_pkg = ctx.include_dev_for.contains(package.name.as_str());
    // A downloaded registry package is untrusted.  The publish step
    // rejects `path` and `port` dependencies (see cabin-package's
    // `validate`), so a legitimately published package only ever depends
    // on other packages by version.  Enforce the same invariant on the
    // consumer side: otherwise a malicious archive could ship a nested
    // `path` sub-package, which the loader would classify as a trusted
    // `PackageKind::Local` package and honor its compiler/linker flags -
    // build-time code execution one dependency hop away.
    //
    // This must match the `PackageKind::Registry` classification in
    // [`link_workspace_packages`]: patches and ports take precedence
    // and stay `Local`, so a patched fork or port overlay that happens
    // to replace a registry entry is still user-controlled and may
    // legitimately declare path/port deps.
    let parent_is_registry = ctx.registry.canonical_paths.contains(manifest_path)
        && !ctx.registry.patch_canonical_paths.contains(manifest_path)
        && !ctx.ports.canonical_paths.contains(manifest_path);
    let mut dep_paths: Vec<DepPath> = Vec::with_capacity(package.dependencies.len());
    for dep in &package.dependencies {
        // Skip dependencies that are not in this command's
        // active-kind set.  Dev deps remain inactive unless the
        // owning package is in `include_dev_for`.  System deps
        // never reach this loop (they live on a separate
        // `system_dependencies` list).
        let kind_active = dep.kind.is_resolved_by_default()
            || (dev_active_for_this_pkg && dep.kind == DependencyKind::Dev);
        if !kind_active {
            continue;
        }
        // Skip dependencies declared inside a non-matching
        // `[target.'cfg(...)'.<kind>]` table.  They stay on
        // `package.dependencies` for metadata round-trip but
        // never become package-graph edges or get loaded as
        // path-dep sub-projects on this platform.
        if !dep.matches_platform(ctx.host_platform) {
            continue;
        }
        if parent_is_registry {
            match &dep.source {
                DependencySource::Path(_) => {
                    return Err(WorkspaceError::RegistryPackageDeclaresPathDependency {
                        package: package.name.as_str().to_owned(),
                        dep_name: dep.name.as_str().to_owned(),
                        path: manifest_path.to_path_buf(),
                    });
                }
                DependencySource::Port(_) => {
                    return Err(WorkspaceError::RegistryPackageDeclaresPortDependency {
                        package: package.name.as_str().to_owned(),
                        dep_name: dep.name.as_str().to_owned(),
                        path: manifest_path.to_path_buf(),
                    });
                }
                DependencySource::Version(_) | DependencySource::Workspace => {}
            }
        }
        let Some(canonical) =
            resolve_dep_manifest_path(ctx, dep, package.name.as_str(), manifest_dir)?
        else {
            continue;
        };
        dep_paths.push(DepPath {
            name: dep.name.as_str().to_owned(),
            path: canonical,
            kind: dep.kind,
            condition: dep.condition.clone(),
            ignore_interface_standard: dep.ignore_interface_standard,
        });
    }
    Ok(dep_paths)
}

/// Resolve one dependency declaration to the canonical manifest
/// path of its source package.  Returns `Ok(None)` when the edge is
/// intentionally skipped: port edges under `SkipAll` / a tolerated
/// parent, or versioned deps without registry context.
fn resolve_dep_manifest_path(
    ctx: &DepResolutionContext<'_>,
    dep: &cabin_core::Dependency,
    parent_name: &str,
    manifest_dir: &Path,
) -> Result<Option<PathBuf>, WorkspaceError> {
    let canonical = match &dep.source {
        DependencySource::Path(rel) => {
            let candidate = manifest_dir.join(rel).join("cabin.toml");
            if !candidate.is_file() {
                return Err(WorkspaceError::LocalDependencyManifestMissing {
                    dep_name: dep.name.as_str().to_owned(),
                    expected: candidate,
                });
            }
            canonicalize(&candidate)?
        }
        DependencySource::Port(port_source) => {
            if ctx.skip_port_edges {
                return Ok(None);
            }
            // Tolerate when the *parent* package is not in
            // the selected strict set: discovery skipped
            // unselected siblings on purpose, so their
            // missing port deps are expected.  Selected
            // parents (or any parent when strict mode is
            // in effect) still surface the typed
            // diagnostics.
            let tolerate = ctx
                .tolerate_strict_set
                .is_some_and(|set| !set.contains(parent_name));
            // Locate the prepared port entry plus the typed error
            // to raise when it is missing; the tolerate policy
            // below is shared by both port sources.
            let (entry, missing) = match port_source {
                PortDepSource::Path(rel) => {
                    let port_dir = manifest_dir.join(rel);
                    if !port_dir.is_dir() {
                        if tolerate {
                            return Ok(None);
                        }
                        return Err(WorkspaceError::PortDirectoryMissing {
                            dep_name: dep.name.as_str().to_owned(),
                            parent: parent_name.to_owned(),
                            port_dir,
                        });
                    }
                    let port_dir_canonical = canonicalize(&port_dir)?;
                    (
                        ctx.ports.by_canonical_dir.get(&port_dir_canonical),
                        WorkspaceError::PortDependencyNotPrepared {
                            dep_name: dep.name.as_str().to_owned(),
                            parent: parent_name.to_owned(),
                            port_dir: port_dir_canonical,
                        },
                    )
                }
                PortDepSource::Builtin { name, .. } => (
                    ctx.ports.by_name.get(name.as_str()),
                    WorkspaceError::BuiltinPortDependencyNotPrepared {
                        dep_name: dep.name.as_str().to_owned(),
                        parent: parent_name.to_owned(),
                    },
                ),
            };
            match entry {
                Some(manifest_path) => canonicalize(manifest_path)?,
                None if tolerate => return Ok(None),
                None => return Err(missing),
            }
        }
        DependencySource::Version(_) => {
            // No registry context: keep the legacy behavior of
            // skipping versioned deps (used by `cabin metadata`
            // and `cabin resolve`, which don't materialize
            // sources).
            if ctx.registry.by_name.is_empty() {
                return Ok(None);
            }
            if let Some(path) = ctx.registry.by_name.get(dep.name.as_str()) {
                path.clone()
            } else {
                // a missing registry entry is
                // only an error when the *parent*
                // package is one the caller flagged as
                // strict (typically a member of the
                // selected closure).  Unselected
                // workspace members can declare
                // versioned deps the current command
                // did not fetch, so we skip them
                // silently.
                if !ctx.policy.requires_registry_for(parent_name) {
                    return Ok(None);
                }
                return Err(WorkspaceError::UnresolvedRegistryDependency {
                    dep_name: dep.name.as_str().to_owned(),
                    parent: parent_name.to_owned(),
                });
            }
        }
        DependencySource::Workspace => {
            // Workspace inheritance is resolved up-front via
            // `resolve_workspace_dependencies`.  A `Workspace`
            // source surviving this loop means the workspace
            // root did not declare the requested name in the
            // matching `[workspace.<kind>-dependencies]` table.
            return Err(WorkspaceError::UnresolvedWorkspaceDependency {
                dep_name: dep.name.as_str().to_owned(),
                parent: parent_name.to_owned(),
                kind: dep.kind,
            });
        }
    };
    Ok(Some(canonical))
}

/// Verify each dependency key matches the actual package name of
/// the manifest it resolved to.  We peek at the dep's manifest
/// before fully loading it.
fn verify_dep_path_names(dep_paths: &[DepPath]) -> Result<(), WorkspaceError> {
    for DepPath {
        name: dep_name,
        path: dep_manifest_path,
        ..
    } in dep_paths
    {
        let dep_parsed = parse_manifest(dep_manifest_path)?;
        let actual = dep_parsed.package.as_ref().ok_or_else(|| {
            WorkspaceError::LocalDependencyIsWorkspace {
                dep_name: dep_name.clone(),
                path: dep_manifest_path.clone(),
            }
        })?;
        if actual.name.as_str() != dep_name {
            return Err(WorkspaceError::DependencyNameMismatch {
                dep_name: dep_name.clone(),
                actual_name: actual.name.as_str().to_owned(),
                path: dep_manifest_path.clone(),
            });
        }
    }
    Ok(())
}

/// Detect duplicate package names *across* the loader's packages
/// (different filesystem paths, but the same `[package].name`).
fn reject_duplicate_package_names(packages: &[LoadedPackage]) -> Result<(), WorkspaceError> {
    let mut seen: HashMap<&str, &PathBuf> = HashMap::new();
    for pkg in packages {
        let name = pkg.package.name.as_str();
        if let Some(prev) = seen.insert(name, &pkg.manifest_path) {
            return Err(WorkspaceError::DuplicatePackageName {
                name: name.to_owned(),
                first: prev.clone(),
                second: pkg.manifest_path.clone(),
            });
        }
    }
    Ok(())
}

/// Resolve dep edges (path -> index in `loader.packages`) and
/// classify each package's [`PackageKind`].
fn link_workspace_packages(
    loader: &Loader,
    registry: &RegistryLookup<'_>,
    ports: &PortLookup,
) -> Vec<WorkspacePackage> {
    let mut packages: Vec<WorkspacePackage> = Vec::with_capacity(loader.packages.len());
    for pkg in &loader.packages {
        let mut deps = Vec::with_capacity(pkg.dep_paths.len());
        for dep in &pkg.dep_paths {
            let idx = *loader
                .manifest_index
                .get(&dep.path)
                .expect("dep manifest should have been loaded");
            deps.push(DependencyEdge {
                index: idx,
                kind: dep.kind,
                condition: dep.condition.clone(),
                ignore_interface_standard: dep.ignore_interface_standard,
            });
        }
        let kind = if registry.patch_canonical_paths.contains(&pkg.manifest_path) {
            // Patches resolve to local working copies; treat them
            // exactly like a path dep so downstream consumers
            // (build planner, lockfile, metadata view) do not
            // see a "registry" package that lives on the user's
            // filesystem.
            PackageKind::Local
        } else if ports.canonical_paths.contains(&pkg.manifest_path) {
            // Foundation ports are local development policy; their
            // prepared overlays live in the artifact cache but are
            // not registry packages.
            PackageKind::Local
        } else if registry.canonical_paths.contains(&pkg.manifest_path) {
            PackageKind::Registry
        } else {
            PackageKind::Local
        };
        let is_port = ports.canonical_paths.contains(&pkg.manifest_path);
        packages.push(WorkspacePackage {
            package: pkg.package.clone(),
            manifest_path: pkg.manifest_path.clone(),
            manifest_dir: pkg.manifest_dir.clone(),
            deps,
            kind,
            is_port,
        });
    }
    packages
}

/// Apply the topological permutation: sort the packages list and
/// rewrite every dep index so it refers to the new, sorted
/// positions.  Returns the sorted list plus the old-index ->
/// new-index map.
fn apply_topo_order(
    packages: Vec<WorkspacePackage>,
) -> Result<(Vec<WorkspacePackage>, HashMap<usize, usize>), WorkspaceError> {
    let topo = topo_sort(&packages)?;
    let new_position: HashMap<usize, usize> = topo
        .iter()
        .enumerate()
        .map(|(new_idx, &old_idx)| (old_idx, new_idx))
        .collect();
    // Move each package into its new slot instead of cloning; the
    // topo order is a permutation, so every index is taken exactly
    // once.
    let mut slots: Vec<Option<WorkspacePackage>> = packages.into_iter().map(Some).collect();
    let mut sorted: Vec<WorkspacePackage> = topo
        .iter()
        .map(|&old_idx| {
            slots[old_idx]
                .take()
                .expect("topo order visits each package exactly once")
        })
        .collect();
    for pkg in &mut sorted {
        for edge in &mut pkg.deps {
            edge.index = new_position[&edge.index];
        }
    }
    Ok((sorted, new_position))
}

/// Validate that every `workspace.default-members` entry resolves
/// to a primary package, then map them to graph indices.  The
/// default order matches the manifest, with stable deduplication.
fn resolve_default_members(
    entries: &[String],
    root_dir: &Path,
    loader: &Loader,
    new_position: &HashMap<usize, usize>,
    primary_packages: &[usize],
) -> Result<Vec<usize>, WorkspaceError> {
    let mut default_members: Vec<usize> = Vec::new();
    let mut seen_default: HashSet<usize> = HashSet::new();
    for entry in entries {
        // Same path-safety rules as members/exclude - reject
        // absolute and `..` defaults before any filesystem walk.
        validate_workspace_pattern("workspace.default-members", entry)?;
        let dir = root_dir.join(entry);
        let canonical_dir =
            canonicalize(&dir).map_err(|_| WorkspaceError::DefaultMemberNotInMembers {
                member: entry.clone(),
            })?;
        let manifest = canonical_dir.join("cabin.toml");
        let idx = loader
            .manifest_index
            .get(&manifest)
            .copied()
            .ok_or_else(|| WorkspaceError::DefaultMemberNotInMembers {
                member: entry.clone(),
            })?;
        let new_idx = new_position[&idx];
        if !primary_packages.contains(&new_idx) {
            return Err(WorkspaceError::DefaultMemberNotInMembers {
                member: entry.clone(),
            });
        }
        if seen_default.insert(new_idx) {
            default_members.push(new_idx);
        }
    }
    Ok(default_members)
}

#[derive(Default)]
struct Loader {
    packages: Vec<LoadedPackage>,
    /// Map canonical manifest path -> index in `packages`.
    manifest_index: HashMap<PathBuf, usize>,
}

struct LoadedPackage {
    package: cabin_core::Package,
    manifest_path: PathBuf,
    manifest_dir: PathBuf,
    /// One entry per resolved dep edge.  Only kinds that
    /// participate in ordinary resolution end up here; dev / system
    /// deps are filtered out earlier.
    dep_paths: Vec<DepPath>,
}

#[derive(Debug, Clone)]
struct DepPath {
    name: String,
    path: PathBuf,
    kind: cabin_core::DependencyKind,
    /// Condition under which this edge was declared.  `None`
    /// for unconditional edges; the loader filters out
    /// non-matching conditional edges before reaching this
    /// point, so any value here matches the host platform.
    condition: Option<cabin_core::Condition>,
    /// The declaration's `ignore-interface-standard` opt-out,
    /// carried onto the graph edge for the standard-compatibility
    /// check.
    ignore_interface_standard: bool,
}

/// Best-effort recovery of a friendly name to mention in the error when a
/// Path dependency turns out to point at a workspace-only manifest.  We
/// don't always know what dep we were following, so this falls back to the
/// Path itself.
fn project_alias_for(loader: &Loader, manifest_path: &Path) -> String {
    for pkg in &loader.packages {
        for dep in &pkg.dep_paths {
            if dep.path == manifest_path {
                return dep.name.clone();
            }
        }
    }
    manifest_path.display().to_string()
}

fn parse_manifest(path: &Path) -> Result<ParsedManifest, WorkspaceError> {
    cabin_manifest::load_manifest(path).map_err(|source| WorkspaceError::Manifest {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}

/// Canonicalize a manifest path and map any I/O failure onto the
/// crate's diagnostic error type.
///
/// Routes through [`cabin_fs::canonicalize`] so every manifest-path
/// identity - dedup, nested-workspace detection, member lookup - shares
/// the project's single Windows-safe canonical spelling (no `\\?\`
/// verbatim prefix, which MSVC's front-end cannot open).
pub(super) fn canonicalize(path: &Path) -> Result<PathBuf, WorkspaceError> {
    cabin_fs::canonicalize(path).map_err(|source| classify_manifest_io(path, source))
}

/// Classify an I/O error from a load-time `canonicalize` call.
/// `NotFound` becomes the dedicated [`WorkspaceError::ManifestNotFound`]
/// variant so the diagnostic layer can emit a structured report with
/// help text.  Everything else maps to
/// [`WorkspaceError::ManifestUnreadable`] (permission denied, the
/// path is a directory, …).
fn classify_manifest_io(path: &Path, source: std::io::Error) -> WorkspaceError {
    match source.kind() {
        std::io::ErrorKind::NotFound => WorkspaceError::ManifestNotFound {
            path: path.to_path_buf(),
        },
        _ => WorkspaceError::ManifestUnreadable {
            path: path.to_path_buf(),
            source,
        },
    }
}
