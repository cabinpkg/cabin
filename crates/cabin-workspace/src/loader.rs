use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use cabin_core::{DependencyKind, DependencySource, PackageName, PortDepSource};
use cabin_manifest::ParsedManifest;

use crate::error::WorkspaceError;
use crate::graph::{DependencyEdge, PackageGraph, PackageKind, WorkspacePackage};

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

/// One patched package source. Like [`RegistryPackageSource`],
/// the loader stitches the supplied `(name, version,
/// manifest_path)` into the graph; unlike a registry entry, the
/// resulting [`WorkspacePackage`] is tagged `kind = PackageKind::Local`
/// because the user pointed Cabin at a local working copy. The
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

/// One foundation-port package source. Built by the CLI
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
    /// `cabin.toml`. The workspace loader treats this as the
    /// dep's `manifest_path`.
    pub manifest_path: PathBuf,
    /// How the recipe was located. Drives whether the dep
    /// walker looks this entry up by canonical port directory
    /// (`PortDir`) or by package name (`Builtin`).
    pub origin: cabin_port::PortOrigin,
}

/// Load a workspace or a single package starting from the given manifest
/// Path. Workspace members and local path dependencies are resolved
/// recursively against the filesystem; a topologically-sorted
/// [`PackageGraph`] is returned.
///
/// This is the convenience form for callers that only have local
/// packages. For registry / patch / dev-dep policy, use
/// [`load_workspace_with_options`].
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
/// Use this for commands that only need workspace topology —
/// `cabin clean`, `cabin package`, `cabin publish` — and that
/// must run on fresh checkouts where no port archive has been
/// downloaded yet. Port deps are dropped from the loaded graph
/// (they never become [`DependencyEdge`]s) but the consuming
/// packages still load normally; foundation-port packages
/// themselves are simply absent from `graph.packages`.
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

/// Options bag for the workspace loader. Threads custom policy
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
    /// loaded as real graph edges. Empty matches the
    /// `cabin build` policy of treating dev-deps as
    /// declaration-only; `cabin test` populates this with the
    /// names of the test-running packages.
    pub include_dev_for: &'a BTreeSet<String>,
    /// How the loader resolves `DependencySource::Port` entries.
    /// Defaults to [`PortPolicy::Strict`] — every port-dep must
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
    /// `ports`. Anything else surfaces the typed
    /// `PortDependencyNotPrepared` / `PortDirectoryMissing`
    /// diagnostic. Default.
    #[default]
    Strict,
    /// Tolerate missing port deps *except* for parent packages
    /// whose names appear in this set — the caller's selected
    /// closure. Names in the set still surface the typed
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
/// not present in `registry`. Pre-resolution loads (port discovery,
/// `cabin metadata` fallback) carry no registry yet but may carry
/// patches that contribute names to the loader's internal name map;
/// the [`RegistryPolicy::StrictFor`] variant lets callers scope
/// enforcement so the resolver-less paths don't surface bogus
/// missing-registry diagnostics.
#[derive(Debug, Clone, Default)]
pub enum RegistryPolicy<'a> {
    /// Every parent's registry deps must be present in `registry`.
    /// Default. Used after the resolver has populated `registry`
    /// with the closure's full pinned set.
    #[default]
    Strict,
    /// Strict only for parents whose names appear in the set;
    /// names outside silently skip a missing-registry edge.
    /// Passing an empty set tolerates every parent — used by
    /// pre-resolution loads.
    StrictFor(&'a BTreeSet<String>),
}

/// Load the workspace with a single options bag. When
/// `include_dev_for` is empty the loader follows the
/// `cabin build` policy of treating dev-deps as
/// declaration-only; with a non-empty set, listed packages
/// contribute their `[dev-dependencies]` as real graph edges
/// (path-deps are materialized, version-deps reach the
/// resolver). Dev-deps still don't propagate transitively —
/// only the listed packages activate them.
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

/// How strictly missing registry entries are enforced. Internal
/// mirror of [`RegistryPolicy`] — public callers pick the policy via
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
    /// present in `ports`. Anything else surfaces the typed
    /// `PortDependencyNotPrepared` / `PortDirectoryMissing`
    /// diagnostic. Used by `load_workspace` /
    /// `load_workspace_with_options` against the full
    /// primary-package set.
    Strict,
    /// Drop every port-dep edge silently. Used by
    /// [`load_workspace_skip_ports`] for commands that only need
    /// workspace topology (`cabin clean`, `cabin package`,
    /// `cabin publish`).
    SkipAll,
    /// Link present port deps as graph edges; silently skip ones
    /// whose source is absent from `ports` (or whose port-path
    /// directory is missing on disk) *except* for parents whose
    /// names appear in this set — the caller's selected closure
    /// still surfaces the typed diagnostics so a typoed
    /// `port-path` in a selected package fails fast instead of
    /// being silently dropped.
    TolerateExcept(BTreeSet<String>),
}

fn load_workspace_inner(
    manifest_path: impl AsRef<Path>,
    registry: &[RegistryPackageSource],
    patches: &[PatchedPackageSource],
    ports: &[PortPackageSource],
    policy: &RegistryEnforcement,
    include_dev_for: &BTreeSet<String>,
    port_mode: &PortMode,
) -> Result<PackageGraph, WorkspaceError> {
    let skip_port_edges = matches!(port_mode, PortMode::SkipAll);
    let tolerate_strict_set: Option<&BTreeSet<String>> = match port_mode {
        PortMode::TolerateExcept(set) => Some(set),
        _ => None,
    };
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
    // host platform — Cabin does not yet support
    // cross-compilation. Future steps may thread an explicit
    // target context through this loader; for now the host is
    // the single source of truth.
    let host_platform = cabin_core::TargetPlatform::current();

    let is_workspace_root = root_manifest.workspace.is_some();

    let mut loader = Loader {
        packages: Vec::new(),
        manifest_index: HashMap::new(),
    };

    // Roots are the entry points whose path-deps we recursively follow
    // and whose primary status we record. They are: the root manifest if
    // it has a [package], and every workspace member.
    let mut primary_manifest_paths: Vec<PathBuf> = Vec::new();

    if root_manifest.package.is_some() {
        primary_manifest_paths.push(manifest_path.clone());
    }

    // Workspace.default_members captured here so we can validate it
    // against the resolved primary set after member expansion.
    let mut workspace_default_members: Vec<String> = Vec::new();
    // Workspace dependency tables captured up-front and parsed
    // once. Member manifests with `dep = { workspace = true }`
    // resolve against the table that matches their declared
    // [`DependencyKind`] — `[workspace.dependencies]` for normal
    // deps, `[workspace.dev-dependencies]` for dev deps.
    // Each entry stores only the resolved `DependencySource` since
    // the inheriting dep already knows its own kind.
    let mut workspace_deps: BTreeMap<DependencyKind, BTreeMap<String, DependencySource>> =
        BTreeMap::new();

    let mut excluded_member_paths: Vec<PathBuf> = Vec::new();
    if let Some(workspace) = &root_manifest.workspace {
        let WorkspaceMembers { included, excluded } =
            expand_workspace_members(&root_dir, &workspace.members, &workspace.exclude)?;
        for canonical in included {
            // reject nested workspaces. A member directory's
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
        workspace_default_members.clone_from(&workspace.default_members);
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

    // Build lookup maps for prepared foundation ports. The dep
    // walker resolves `DependencySource::Port` declarations via
    // one of two maps depending on the origin:
    //   - PortDir: canonical port_dir -> prepared manifest_path
    //   - Builtin: package name -> prepared manifest_path
    // We canonicalize the port_dir up-front so the lookup is a
    // single HashMap probe per dep — and so two consumers that
    // reach the same port through different relative paths still
    // see the same prepared source.
    let mut port_by_canonical_dir: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut port_by_name: HashMap<String, PathBuf> = HashMap::new();
    let mut port_canonical_paths: HashSet<PathBuf> = HashSet::new();
    for entry in ports {
        match &entry.origin {
            cabin_port::PortOrigin::PortDir(port_dir) => {
                let port_dir_canonical = canonicalize(port_dir)?;
                if let Some(previous) =
                    port_by_canonical_dir.insert(port_dir_canonical, entry.manifest_path.clone())
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
                    port_by_name.insert((*name).to_owned(), entry.manifest_path.clone())
                {
                    return Err(WorkspaceError::DuplicatePackageName {
                        name: entry.name.as_str().to_owned(),
                        first: previous,
                        second: entry.manifest_path.clone(),
                    });
                }
            }
        }
        port_canonical_paths.insert(canonicalize(&entry.manifest_path)?);
    }

    // Build a name -> registry source map (canonicalizing paths so the
    // dedup-by-canonical-path step below sees a consistent value), plus
    // parallel maps of canonical registry manifest paths to expected
    // (name, version) so loading can compare the actual manifest
    // contents against what the resolver pinned.
    let mut registry_by_name: HashMap<&str, PathBuf> = HashMap::new();
    let mut registry_canonical_names: HashMap<PathBuf, &PackageName> = HashMap::new();
    let mut registry_canonical_versions: HashMap<PathBuf, &semver::Version> = HashMap::new();
    let mut registry_canonical_paths: HashSet<PathBuf> = HashSet::new();
    let mut patch_canonical_paths: HashSet<PathBuf> = HashSet::new();
    for entry in registry {
        let canonical = canonicalize(&entry.manifest_path)?;
        registry_by_name.insert(entry.name.as_str(), canonical.clone());
        registry_canonical_names.insert(canonical.clone(), &entry.name);
        registry_canonical_versions.insert(canonical.clone(), &entry.version);
        registry_canonical_paths.insert(canonical);
    }
    // Patches contribute the same `(name, version, manifest_path)`
    // information as registry entries but ultimately produce
    // local-kind packages. Defensively reject overlap with the
    // registry list so a caller bug never silently flips Local
    // to Registry mid-graph.
    for entry in patches {
        let canonical = canonicalize(&entry.manifest_path)?;
        if registry_canonical_paths.contains(&canonical) {
            return Err(WorkspaceError::PatchConflictsWithRegistry {
                package: entry.name.as_str().to_owned(),
                path: canonical,
            });
        }
        if let Some(existing) = registry_by_name.insert(entry.name.as_str(), canonical.clone()) {
            return Err(WorkspaceError::DuplicatePackageName {
                name: entry.name.as_str().to_owned(),
                first: existing,
                second: canonical,
            });
        }
        registry_canonical_names.insert(canonical.clone(), &entry.name);
        registry_canonical_versions.insert(canonical.clone(), &entry.version);
        registry_canonical_paths.insert(canonical.clone());
        patch_canonical_paths.insert(canonical);
    }

    // Recursively load every primary manifest plus any path deps it pulls
    // in. The loader is iterative — we maintain a stack of unloaded
    // manifests rather than recursing.
    let mut to_load: Vec<PathBuf> = primary_manifest_paths.clone();
    // Make registry packages part of the load set too; they are not
    // primary, but they must appear in the package graph.
    for entry in registry {
        let canonical = canonicalize(&entry.manifest_path)?;
        to_load.push(canonical);
    }
    // Patches are external manifests too; load them so the
    // graph carries the patched `Package` value alongside the
    // workspace members and registry entries.
    for entry in patches {
        let canonical = canonicalize(&entry.manifest_path)?;
        to_load.push(canonical);
    }
    // Ports are also external manifests. They live in the
    // foundation-port cache directory; load them so the graph
    // carries the prepared overlay `Package` value alongside
    // workspace members.
    for entry in ports {
        let canonical = canonicalize(&entry.manifest_path)?;
        to_load.push(canonical);
    }
    let root_manifest_path = manifest_path.clone();
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

        // `[profile.*]` tables are only honored on the entry-
        // point manifest. Member and path-dep manifests that
        // declare them surface a clear error rather than being
        // silently ignored, so a single workspace key cannot
        // mean different things in different members.
        // Each of the per-table guards below moves `manifest_path`
        // into the returned error because the function returns
        // immediately on the error branch and the field carries
        // the path verbatim; the borrow checker preserves it for
        // the rest of the loop body via the early-return.
        if manifest_path != root_manifest_path && !package.profiles.is_empty() {
            return Err(WorkspaceError::MemberDeclaresProfiles {
                package: package.name.as_str().to_owned(),
                path: manifest_path,
            });
        }
        if manifest_path != root_manifest_path && !package.toolchain.is_empty() {
            return Err(WorkspaceError::MemberDeclaresToolchain {
                package: package.name.as_str().to_owned(),
                path: manifest_path,
            });
        }
        if manifest_path != root_manifest_path && !package.compiler_wrapper.is_empty() {
            return Err(WorkspaceError::MemberDeclaresCompilerWrapper {
                package: package.name.as_str().to_owned(),
                path: manifest_path,
            });
        }
        if manifest_path != root_manifest_path && !package.patches.is_empty() {
            return Err(WorkspaceError::MemberDeclaresPatches {
                package: package.name.as_str().to_owned(),
                path: manifest_path,
            });
        }

        // If this manifest is a known registry package, the resolver
        // pinned a specific (name, version). The artifact crate has
        // already validated the manifest against that pin, but the
        // workspace loader is the user-visible reporter, so we
        // double-check here and surface a clear error if they ever
        // disagree.
        // validate both expected name and version. The
        // registry may have pointed at a directory whose manifest
        // declares a completely different package (a malicious or
        // wrongly extracted artifact); refusing here keeps a wrong
        // package from sneaking into the build graph.
        if let Some(expected_version) = registry_canonical_versions.get(&manifest_path) {
            let expected_name = registry_canonical_names.get(&manifest_path).copied();
            let version_ok = &package.version == *expected_version;
            let name_ok = expected_name
                .map(|n| n.as_str() == package.name.as_str())
                .unwrap_or(true);
            if !name_ok {
                return Err(WorkspaceError::RegistryPackageNameMismatch {
                    name: expected_name
                        .map(|n| n.as_str().to_owned())
                        .unwrap_or_default(),
                    actual_name: package.name.as_str().to_owned(),
                    path: manifest_path.clone(),
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
                    path: manifest_path.clone(),
                });
            }
        }

        let manifest_dir = manifest_path
            .parent()
            .expect("canonicalized manifest path has a parent")
            .to_path_buf();

        // rewrite each `{ workspace = true }` dep into the
        // resolved source from `[workspace.dependencies]` before any
        // other consumer sees it. We hold the rewritten `Package` in
        // `resolved_project` and use it for the rest of this
        // iteration.
        let resolved_project = resolve_workspace_dependencies(package.clone(), &workspace_deps)?;
        let package = resolved_project;

        // Dev dependencies are declaration-only for ordinary
        // commands but become real graph edges when the loader is
        // told to "include dev for" this package — typically by
        // `cabin test` for the test-running packages. The opt-in
        // never propagates: a transitive dep's own dev-deps stay
        // declaration-only.
        let dev_active_for_this_pkg = include_dev_for.contains(package.name.as_str());
        let mut dep_paths: Vec<DepPath> = Vec::with_capacity(package.dependencies.len());
        for dep in &package.dependencies {
            // Skip dependencies that are not in this command's
            // active-kind set. Dev deps remain inactive unless the
            // owning package is in `include_dev_for`. System deps
            // never reach this loop (they live on a separate
            // `system_dependencies` list).
            let kind_active = dep.kind.is_resolved_by_default()
                || (dev_active_for_this_pkg && dep.kind == DependencyKind::Dev);
            if !kind_active {
                continue;
            }
            // Skip dependencies declared inside a non-matching
            // `[target.'cfg(...)'.<kind>]` table. They stay on
            // `package.dependencies` for metadata round-trip but
            // never become package-graph edges or get loaded as
            // path-dep sub-projects on this platform.
            if !dep.matches_platform(&host_platform) {
                continue;
            }
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
                DependencySource::Port(PortDepSource::Path(rel)) => {
                    if skip_port_edges {
                        continue;
                    }
                    // Tolerate when the *parent* package is not in
                    // the selected strict set: discovery skipped
                    // unselected siblings on purpose, so their
                    // missing port deps are expected. Selected
                    // parents (or any parent when strict mode is
                    // in effect) still surface the typed
                    // diagnostics.
                    let tolerate =
                        tolerate_strict_set.is_some_and(|set| !set.contains(package.name.as_str()));
                    let port_dir = manifest_dir.join(rel);
                    if !port_dir.is_dir() {
                        if tolerate {
                            continue;
                        }
                        return Err(WorkspaceError::PortDirectoryMissing {
                            dep_name: dep.name.as_str().to_owned(),
                            parent: package.name.as_str().to_owned(),
                            port_dir,
                        });
                    }
                    let port_dir_canonical = canonicalize(&port_dir)?;
                    match port_by_canonical_dir.get(&port_dir_canonical) {
                        Some(manifest_path) => canonicalize(manifest_path)?,
                        None => {
                            if tolerate {
                                continue;
                            }
                            return Err(WorkspaceError::PortDependencyNotPrepared {
                                dep_name: dep.name.as_str().to_owned(),
                                parent: package.name.as_str().to_owned(),
                                port_dir: port_dir_canonical,
                            });
                        }
                    }
                }
                DependencySource::Port(PortDepSource::Builtin { name, .. }) => {
                    if skip_port_edges {
                        continue;
                    }
                    let tolerate =
                        tolerate_strict_set.is_some_and(|set| !set.contains(package.name.as_str()));
                    match port_by_name.get(name.as_str()) {
                        Some(manifest_path) => canonicalize(manifest_path)?,
                        None => {
                            if tolerate {
                                continue;
                            }
                            return Err(WorkspaceError::BuiltinPortDependencyNotPrepared {
                                dep_name: dep.name.as_str().to_owned(),
                                parent: package.name.as_str().to_owned(),
                            });
                        }
                    }
                }
                DependencySource::Version(_) => {
                    // No registry context: keep the legacy behavior of
                    // skipping versioned deps (used by `cabin metadata`
                    // and `cabin resolve`, which don't materialize
                    // sources).
                    if registry_by_name.is_empty() {
                        continue;
                    }
                    match registry_by_name.get(dep.name.as_str()) {
                        Some(path) => path.clone(),
                        None => {
                            // a missing registry entry is
                            // only an error when the *parent*
                            // package is one the caller flagged as
                            // strict (typically a member of the
                            // selected closure). Unselected
                            // workspace members can declare
                            // versioned deps the current command
                            // did not fetch, so we skip them
                            // silently.
                            if !policy.requires_registry_for(package.name.as_str()) {
                                continue;
                            }
                            return Err(WorkspaceError::UnresolvedRegistryDependency {
                                dep_name: dep.name.as_str().to_owned(),
                                parent: package.name.as_str().to_owned(),
                            });
                        }
                    }
                }
                DependencySource::Workspace => {
                    // Workspace inheritance is resolved up-front via
                    // `resolve_workspace_dependencies`. A `Workspace`
                    // source surviving this loop means the workspace
                    // root did not declare the requested name in the
                    // matching `[workspace.<kind>-dependencies]` table.
                    return Err(WorkspaceError::UnresolvedWorkspaceDependency {
                        dep_name: dep.name.as_str().to_owned(),
                        parent: package.name.as_str().to_owned(),
                        kind: dep.kind,
                    });
                }
            };
            dep_paths.push(DepPath {
                name: dep.name.as_str().to_owned(),
                path: canonical,
                kind: dep.kind,
                condition: dep.condition.clone(),
            });
        }

        // Verify the dependency key matches the actual package name. We
        // need to peek at the dep's manifest before fully loading it.
        for DepPath {
            name: dep_name,
            path: dep_manifest_path,
            ..
        } in &dep_paths
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

        let index = loader.packages.len();
        loader.manifest_index.insert(manifest_path.clone(), index);
        loader.packages.push(LoadedPackage {
            package,
            manifest_path: manifest_path.clone(),
            manifest_dir,
            dep_paths,
        });
        for dep in &loader.packages[index].dep_paths {
            to_load.push(dep.path.clone());
        }
    }

    // Detect duplicate package names *across* the loader's packages
    // (different filesystem paths, but the same `[package].name`).
    {
        let mut seen: HashMap<&str, &PathBuf> = HashMap::new();
        for pkg in &loader.packages {
            let name = pkg.package.name.as_str();
            if let Some(prev) = seen.insert(name, &pkg.manifest_path) {
                return Err(WorkspaceError::DuplicatePackageName {
                    name: name.to_owned(),
                    first: prev.clone(),
                    second: pkg.manifest_path.clone(),
                });
            }
        }
    }

    // Resolve dep edges (path -> index in loader.packages).
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
            });
        }
        let kind = if patch_canonical_paths.contains(&pkg.manifest_path) {
            // Patches resolve to local working copies; treat them
            // exactly like a path dep so downstream consumers
            // (build planner, lockfile, metadata view) do not
            // see a "registry" package that lives on the user's
            // filesystem.
            PackageKind::Local
        } else if port_canonical_paths.contains(&pkg.manifest_path) {
            // Foundation ports are local development policy; their
            // prepared overlays live in the artifact cache but are
            // not registry packages.
            PackageKind::Local
        } else if registry_canonical_paths.contains(&pkg.manifest_path) {
            PackageKind::Registry
        } else {
            PackageKind::Local
        };
        packages.push(WorkspacePackage {
            package: pkg.package.clone(),
            manifest_path: pkg.manifest_path.clone(),
            manifest_dir: pkg.manifest_dir.clone(),
            deps,
            kind,
        });
    }

    let topo = topo_sort(&packages)?;

    // Apply the topological permutation to the packages list and rewrite
    // every dep index so it refers to the new, sorted positions.
    let new_position: HashMap<usize, usize> = topo
        .iter()
        .enumerate()
        .map(|(new_idx, &old_idx)| (old_idx, new_idx))
        .collect();

    let mut sorted: Vec<WorkspacePackage> = topo
        .iter()
        .map(|&old_idx| packages[old_idx].clone())
        .collect();
    for pkg in &mut sorted {
        for edge in &mut pkg.deps {
            edge.index = new_position[&edge.index];
        }
    }

    let primary_packages: Vec<usize> = primary_manifest_paths
        .iter()
        .map(|p| {
            let old_idx = loader.manifest_index[p];
            new_position[&old_idx]
        })
        .collect();

    let root_package = if root_manifest.package.is_some() {
        Some(new_position[&loader.manifest_index[&manifest_path]])
    } else {
        None
    };

    // validate that every workspace.default-members entry
    // resolves to a primary package, then map them to graph indices.
    // The default order matches the manifest, with stable
    // deduplication.
    let mut default_members: Vec<usize> = Vec::new();
    let mut seen_default: HashSet<usize> = HashSet::new();
    for entry in &workspace_default_members {
        // Same path-safety rules as members/exclude — reject
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

struct Loader {
    packages: Vec<LoadedPackage>,
    /// Map canonical manifest path -> index in `packages`.
    manifest_index: HashMap<PathBuf, usize>,
}

struct LoadedPackage {
    package: cabin_core::Package,
    manifest_path: PathBuf,
    manifest_dir: PathBuf,
    /// One entry per resolved dep edge: `(dep_name, canonical
    /// manifest path, dependency kind)`. Only kinds that
    /// participate in ordinary resolution end up here; dev / system
    /// deps are filtered out earlier.
    dep_paths: Vec<DepPath>,
}

#[derive(Debug, Clone)]
struct DepPath {
    name: String,
    path: PathBuf,
    kind: cabin_core::DependencyKind,
    /// Condition under which this edge was declared. `None`
    /// for unconditional edges; the loader filters out
    /// non-matching conditional edges before reaching this
    /// point, so any value here matches the host platform.
    condition: Option<cabin_core::Condition>,
}

/// Best-effort recovery of a friendly name to mention in the error when a
/// Path dependency turns out to point at a workspace-only manifest. We
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

fn canonicalize(path: &Path) -> Result<PathBuf, WorkspaceError> {
    std::fs::canonicalize(path).map_err(|source| classify_manifest_io(path, source))
}

/// Classify an I/O error from a load-time `canonicalize` call.
/// `NotFound` becomes the dedicated [`WorkspaceError::ManifestNotFound`]
/// variant so the diagnostic layer can emit a structured report with
/// help text. Everything else maps to
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

/// Expansion result for `[workspace.members]` /
/// `[workspace.exclude]`. `included` is a sorted, deduplicated list
/// of canonical manifest paths. `excluded` is the list of relative
/// paths (under `workspace_dir`) the loader removed from the
/// candidate set, surfaced for metadata.
struct WorkspaceMembers {
    included: Vec<PathBuf>,
    excluded: Vec<PathBuf>,
}

fn expand_workspace_members(
    workspace_dir: &Path,
    members: &[String],
    exclude: &[String],
) -> Result<WorkspaceMembers, WorkspaceError> {
    // Expand member patterns. Membership is tracked by canonicalized
    // directory path so two patterns matching the same dir collapse
    // to one entry.
    let mut included: BTreeSet<PathBuf> = BTreeSet::new();
    for pattern in members {
        let dirs = expand_member_pattern(workspace_dir, pattern)?;
        for dir in dirs {
            let manifest = dir.join("cabin.toml");
            if !manifest.is_file() {
                return Err(WorkspaceError::WorkspaceMemberMissing {
                    pattern: pattern.clone(),
                    root: workspace_dir.to_path_buf(),
                });
            }
            let canonical_dir = canonicalize(&dir)?;
            included.insert(canonical_dir);
        }
    }

    // Expand exclude patterns. Globs are best-effort: an exclude
    // pattern need not match any directory that contains a cabin.toml
    // (a partial match such as `third_party/*` covering some
    // subdirectories without manifests is fine), but the pattern as a
    // whole must hit at least one entry already in the member set so
    // typos surface.
    let mut excluded: BTreeSet<PathBuf> = BTreeSet::new();
    let canonical_root = canonicalize(workspace_dir)?;
    for pattern in exclude {
        if pattern.is_empty() {
            return Err(WorkspaceError::UnsupportedWorkspacePattern {
                pattern: pattern.clone(),
            });
        }
        let dirs = expand_exclude_pattern(workspace_dir, pattern)?;
        let mut hit_any = false;
        for dir in dirs {
            // We only canonicalize existing dirs; missing exclude
            // dirs collapse to no-op without erroring (the pattern
            // itself may have legitimately hit non-package
            // directories).
            if !dir.is_dir() {
                continue;
            }
            let canonical_dir = match canonicalize(&dir) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if included.remove(&canonical_dir) {
                hit_any = true;
                if let Ok(rel) = canonical_dir.strip_prefix(&canonical_root) {
                    excluded.insert(rel.to_path_buf());
                } else {
                    excluded.insert(canonical_dir.clone());
                }
            }
        }
        if !hit_any {
            return Err(WorkspaceError::UnusedExcludePattern {
                pattern: pattern.clone(),
                root: workspace_dir.to_path_buf(),
            });
        }
    }

    // Convert the surviving directories to canonical manifest paths.
    let mut out: Vec<PathBuf> = Vec::with_capacity(included.len());
    for dir in &included {
        let manifest = dir.join("cabin.toml");
        out.push(canonicalize(&manifest)?);
    }
    out.sort();
    let excluded_paths: Vec<PathBuf> = excluded.into_iter().collect();
    Ok(WorkspaceMembers {
        included: out,
        excluded: excluded_paths,
    })
}

/// Resolve every `DependencySource::Workspace` entry on
/// `package` by looking it up in the workspace table that matches
/// each entry's [`DependencyKind`]. Returns a `Package` whose
/// dependencies are entirely `Path` or `Version`. References that
/// have no matching workspace entry are surfaced as a clear
/// kind-aware error.
fn resolve_workspace_dependencies(
    mut package: cabin_core::Package,
    workspace_deps: &BTreeMap<DependencyKind, BTreeMap<String, DependencySource>>,
) -> Result<cabin_core::Package, WorkspaceError> {
    for dep in package.dependencies.iter_mut() {
        if !matches!(dep.source, DependencySource::Workspace) {
            continue;
        }
        let table = workspace_deps.get(&dep.kind);
        let resolved = table
            .and_then(|t| t.get(dep.name.as_str()))
            .ok_or_else(|| WorkspaceError::UnresolvedWorkspaceDependency {
                dep_name: dep.name.as_str().to_owned(),
                parent: package.name.as_str().to_owned(),
                kind: dep.kind,
            })?;
        dep.source = resolved.clone();
    }
    Ok(package)
}

/// Parse a `[workspace.<kind>-dependencies]` value into a
/// `DependencySource`. Uses the existing manifest-side parser so
/// requirement-string handling stays a single source of truth.
fn parse_workspace_dep_source(name: &str, req: &str) -> Result<DependencySource, WorkspaceError> {
    // Wrap the raw requirement in a tiny manifest to reuse the
    // existing dependency parser. We round-trip through the
    // manifest crate so error messages mention the dependency name
    // and the failing requirement consistently.
    let manifest = format!(
        "[package]\nname = \"__workspace_root__\"\nversion = \"0.0.0\"\n[dependencies]\n{name} = \"{}\"\n",
        req.replace('"', "\\\""),
    );
    let parsed = cabin_manifest::parse_manifest_str(&manifest).map_err(|source| {
        WorkspaceError::InvalidWorkspaceDependency {
            name: name.to_owned(),
            source: Box::new(source),
        }
    })?;
    let package = parsed
        .package
        .expect("inline manifest always has [package]");
    let dep = package
        .dependencies
        .into_iter()
        .next()
        .expect("inline manifest declared exactly one dependency");
    Ok(dep.source)
}

/// Reject workspace patterns that escape the workspace
/// root or that use absolute paths. Applied to every `members`,
/// `exclude`, and `default-members` entry so an unsafe pattern
/// fails fast with a clear error before any filesystem walk.
fn validate_workspace_pattern(field: &'static str, pattern: &str) -> Result<(), WorkspaceError> {
    if pattern.is_empty() {
        return Err(WorkspaceError::UnsupportedWorkspacePattern {
            pattern: pattern.to_owned(),
        });
    }
    let p = std::path::Path::new(pattern);
    if p.is_absolute() {
        return Err(WorkspaceError::WorkspacePatternEscapesRoot {
            field,
            pattern: pattern.to_owned(),
        });
    }
    for component in p.components() {
        if matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        ) {
            return Err(WorkspaceError::WorkspacePatternEscapesRoot {
                field,
                pattern: pattern.to_owned(),
            });
        }
    }
    Ok(())
}

/// Resolve a `[workspace.members]` pattern to a list of directories
/// containing `cabin.toml`. The supported syntaxes are:
///
/// - exact relative path (`tools/hello`)
/// - single-`*` glob in the final component (`packages/*`)
fn expand_member_pattern(
    workspace_dir: &Path,
    pattern: &str,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    validate_workspace_pattern("workspace.members", pattern)?;

    if !pattern.contains('*') {
        let dir = workspace_dir.join(pattern);
        return Ok(vec![dir]);
    }

    // Single trailing `/*` only.
    let trimmed = match pattern.strip_suffix("/*") {
        Some(t) => t,
        None => {
            return Err(WorkspaceError::UnsupportedWorkspacePattern {
                pattern: pattern.to_owned(),
            });
        }
    };
    if trimmed.contains('*') {
        return Err(WorkspaceError::UnsupportedWorkspacePattern {
            pattern: pattern.to_owned(),
        });
    }

    let prefix_dir = if trimmed.is_empty() {
        workspace_dir.to_path_buf()
    } else {
        workspace_dir.join(trimmed)
    };
    if !prefix_dir.is_dir() {
        return Err(WorkspaceError::WorkspaceMemberMissing {
            pattern: pattern.to_owned(),
            root: workspace_dir.to_path_buf(),
        });
    }

    let entries = std::fs::read_dir(&prefix_dir).map_err(|source| WorkspaceError::Io {
        path: prefix_dir.clone(),
        source,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| WorkspaceError::Io {
            path: prefix_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() && path.join("cabin.toml").is_file() {
            out.push(path);
        }
    }
    if out.is_empty() {
        return Err(WorkspaceError::WorkspaceMemberMissing {
            pattern: pattern.to_owned(),
            root: workspace_dir.to_path_buf(),
        });
    }
    out.sort();
    Ok(out)
}

/// Resolve a `[workspace.exclude]` pattern. Same grammar as
/// `expand_member_pattern`, but more lenient about empty matches:
/// The pattern may legitimately match directories that do not
/// contain a `cabin.toml`. The caller validates that the overall
/// pattern hit at least one declared member.
fn expand_exclude_pattern(
    workspace_dir: &Path,
    pattern: &str,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    validate_workspace_pattern("workspace.exclude", pattern)?;

    if !pattern.contains('*') {
        return Ok(vec![workspace_dir.join(pattern)]);
    }

    let trimmed = match pattern.strip_suffix("/*") {
        Some(t) => t,
        None => {
            return Err(WorkspaceError::UnsupportedWorkspacePattern {
                pattern: pattern.to_owned(),
            });
        }
    };
    if trimmed.contains('*') {
        return Err(WorkspaceError::UnsupportedWorkspacePattern {
            pattern: pattern.to_owned(),
        });
    }

    let prefix_dir = if trimmed.is_empty() {
        workspace_dir.to_path_buf()
    } else {
        workspace_dir.join(trimmed)
    };
    if !prefix_dir.is_dir() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&prefix_dir).map_err(|source| WorkspaceError::Io {
        path: prefix_dir.clone(),
        source,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| WorkspaceError::Io {
            path: prefix_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn topo_sort(packages: &[WorkspacePackage]) -> Result<Vec<usize>, WorkspaceError> {
    #[derive(Clone, Copy)]
    enum Color {
        Visiting,
        Done,
    }

    fn visit(
        node: usize,
        packages: &[WorkspacePackage],
        state: &mut Vec<Option<Color>>,
        path: &mut Vec<usize>,
        order: &mut Vec<usize>,
    ) -> Result<(), WorkspaceError> {
        match state[node] {
            Some(Color::Done) => return Ok(()),
            Some(Color::Visiting) => {
                let start = path.iter().position(|n| *n == node).unwrap_or(0);
                let mut cycle: Vec<String> = path[start..]
                    .iter()
                    .map(|i| packages[*i].package.name.as_str().to_owned())
                    .collect();
                cycle.push(packages[node].package.name.as_str().to_owned());
                return Err(WorkspaceError::PackageDependencyCycle(cycle));
            }
            None => {}
        }
        state[node] = Some(Color::Visiting);
        path.push(node);
        for edge in &packages[node].deps {
            visit(edge.index, packages, state, path, order)?;
        }
        path.pop();
        state[node] = Some(Color::Done);
        order.push(node);
        Ok(())
    }

    let mut state: Vec<Option<Color>> = vec![None; packages.len()];
    let mut order = Vec::with_capacity(packages.len());
    let mut path = Vec::new();

    // Visit packages in their original (insertion) order so the output is
    // deterministic for inputs that don't fully order themselves.
    for i in 0..packages.len() {
        if state[i].is_none() {
            visit(i, packages, &mut state, &mut path, &mut order)?;
        }
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn loads_single_package_with_no_deps() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "solo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        assert!(!graph.is_workspace_root);
        assert_eq!(graph.packages.len(), 1);
        assert_eq!(graph.packages[0].package.name.as_str(), "solo");
        assert_eq!(graph.packages[0].deps.len(), 0);
        assert_eq!(graph.primary_packages, vec![0]);
        assert_eq!(graph.root_package, Some(0));
    }

    #[test]
    fn loads_package_with_local_path_dep() {
        let dir = TempDir::new().unwrap();
        dir.child("greet/cabin.toml")
            .write_str(
                r#"[package]
name = "greet"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("app/cabin.toml")).unwrap();
        assert_eq!(graph.packages.len(), 2);
        // greet must come before app in topological order.
        assert_eq!(graph.packages[0].package.name.as_str(), "greet");
        assert_eq!(graph.packages[1].package.name.as_str(), "app");
        assert_eq!(
            graph.packages[1]
                .deps
                .iter()
                .map(|e| (e.index, e.kind))
                .collect::<Vec<_>>(),
            vec![(0, DependencyKind::Normal)]
        );
        assert_eq!(graph.primary_packages, vec![1]);
    }

    #[test]
    fn loads_transitive_local_path_deps() {
        let dir = TempDir::new().unwrap();
        dir.child("c/cabin.toml")
            .write_str(
                r#"[package]
name = "c"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("b/cabin.toml")
            .write_str(
                r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
c = { path = "../c" }
"#,
            )
            .unwrap();
        dir.child("a/cabin.toml")
            .write_str(
                r#"[package]
name = "a"
version = "0.1.0"

[dependencies]
b = { path = "../b" }
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("a/cabin.toml")).unwrap();
        assert_eq!(graph.packages.len(), 3);
        let names: Vec<&str> = graph
            .packages
            .iter()
            .map(|p| p.package.name.as_str())
            .collect();
        // Topo order: c before b before a.
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("c") < pos("b"));
        assert!(pos("b") < pos("a"));
    }

    #[test]
    fn detects_package_cycle() {
        let dir = TempDir::new().unwrap();
        dir.child("a/cabin.toml")
            .write_str(
                r#"[package]
name = "a"
version = "0.1.0"

[dependencies]
b = { path = "../b" }
"#,
            )
            .unwrap();
        dir.child("b/cabin.toml")
            .write_str(
                r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
a = { path = "../a" }
"#,
            )
            .unwrap();
        let err = load_workspace(dir.path().join("a/cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::PackageDependencyCycle(cycle) => {
                assert_eq!(cycle.first(), cycle.last());
                assert!(cycle.contains(&"a".to_owned()));
                assert!(cycle.contains(&"b".to_owned()));
            }
            other => panic!("expected PackageDependencyCycle, got {other:?}"),
        }
    }

    #[test]
    fn loads_workspace_with_exact_member_path() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/greet"]
"#,
            )
            .unwrap();
        dir.child("packages/greet/cabin.toml")
            .write_str(
                r#"[package]
name = "greet"
version = "0.1.0"
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        assert!(graph.is_workspace_root);
        assert!(graph.root_package.is_none());
        assert_eq!(graph.packages.len(), 1);
        assert_eq!(graph.packages[0].package.name.as_str(), "greet");
    }

    #[test]
    fn pure_workspace_root_policy_is_available_on_graph() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/greet"]

[profile.release]
opt-level = 0

[toolchain]
cxx = "clang++"

[profile.cache]
compiler-wrapper = "ccache"

[patch]
fmt = { path = "../fmt" }
"#,
            )
            .unwrap();
        dir.child("packages/greet/cabin.toml")
            .write_str(
                r#"[package]
name = "greet"
version = "0.1.0"
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        assert!(graph.is_workspace_root);
        assert!(graph.root_package.is_none());

        let release = cabin_core::ProfileName::new("release").unwrap();
        assert_eq!(
            graph
                .root_settings
                .profiles
                .get(&release)
                .and_then(|p| p.opt_level),
            Some(cabin_core::OptLevel::O0)
        );
        assert_eq!(
            graph
                .root_settings
                .toolchain
                .general
                .get(cabin_core::ToolKind::CxxCompiler)
                .map(cabin_core::ToolSpec::display)
                .as_deref(),
            Some("clang++")
        );
        assert_eq!(
            graph.root_settings.compiler_wrapper.general,
            Some(cabin_core::CompilerWrapperRequest::Use {
                wrapper: cabin_core::CompilerWrapperKind::Ccache,
            })
        );
        assert_eq!(graph.root_settings.patches.entries.len(), 1);
    }

    #[test]
    fn loads_workspace_with_glob_member_pattern() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        dir.child("packages/a/cabin.toml")
            .write_str(
                r#"[package]
name = "a"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("packages/b/cabin.toml")
            .write_str(
                r#"[package]
name = "b"
version = "0.1.0"
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        assert_eq!(graph.packages.len(), 2);
        let names: Vec<&str> = graph
            .packages
            .iter()
            .map(|p| p.package.name.as_str())
            .collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn rejects_duplicate_package_names_in_workspace() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        dir.child("packages/a/cabin.toml")
            .write_str(
                r#"[package]
name = "shared"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("packages/b/cabin.toml")
            .write_str(
                r#"[package]
name = "shared"
version = "0.2.0"
"#,
            )
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::DuplicatePackageName { name, .. } => assert_eq!(name, "shared"),
            other => panic!("expected DuplicatePackageName, got {other:?}"),
        }
    }

    #[test]
    fn missing_local_dependency_manifest_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
"#,
            )
            .unwrap();
        let err = load_workspace(dir.path().join("app/cabin.toml")).unwrap_err();
        assert!(matches!(
            err,
            WorkspaceError::LocalDependencyManifestMissing { .. }
        ));
    }

    #[test]
    fn dependency_name_mismatch_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("greet/cabin.toml")
            .write_str(
                r#"[package]
name = "actually-hello"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
"#,
            )
            .unwrap();
        let err = load_workspace(dir.path().join("app/cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::DependencyNameMismatch {
                dep_name,
                actual_name,
                ..
            } => {
                assert_eq!(dep_name, "greet");
                assert_eq!(actual_name, "actually-hello");
            }
            other => panic!("expected DependencyNameMismatch, got {other:?}"),
        }
    }

    #[test]
    fn versioned_dependencies_are_preserved_but_not_traversed() {
        let dir = TempDir::new().unwrap();
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("app/cabin.toml")).unwrap();
        // Only the root package is loaded — versioned deps don't pull in
        // any local manifests.
        assert_eq!(graph.packages.len(), 1);
        let app = &graph.packages[0];
        assert!(app.deps.is_empty());
        // But the Package still records the declared dependency.
        assert_eq!(app.package.dependencies.len(), 1);
        assert_eq!(app.package.dependencies[0].name.as_str(), "fmt");
        assert!(matches!(
            &app.package.dependencies[0].source,
            cabin_core::DependencySource::Version(_)
        ));
    }

    #[test]
    fn unsupported_glob_pattern_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*/foo"]
"#,
            )
            .unwrap();
        dir.child("packages/a/foo/cabin.toml")
            .write_str(
                r#"[package]
name = "a"
version = "0.1.0"
"#,
            )
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        assert!(matches!(
            err,
            WorkspaceError::UnsupportedWorkspacePattern { .. }
        ));
    }

    #[test]
    fn missing_workspace_member_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/missing"]
"#,
            )
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        assert!(matches!(err, WorkspaceError::WorkspaceMemberMissing { .. }));
    }

    // -------------------------------------------------------------------
    // registry package integration
    // -------------------------------------------------------------------

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    #[test]
    fn loads_registry_package_via_versioned_dep() {
        let dir = TempDir::new().unwrap();
        // Root depends on `fmt` versionally.
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        // Registry "extracted source" lives in a sibling directory.
        dir.child("registry/fmt/cabin.toml")
            .write_str(
                r#"[package]
name = "fmt"
version = "10.2.1"
"#,
            )
            .unwrap();
        let registry = vec![RegistryPackageSource {
            name: pkg("fmt"),
            version: ver("10.2.1"),
            manifest_path: dir.path().join("registry/fmt/cabin.toml"),
        }];
        let graph = load_workspace_with_options(
            dir.path().join("app/cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &registry,
                patches: &[],
                ports: &[],
                registry_policy: RegistryPolicy::Strict,
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .unwrap();
        assert_eq!(graph.packages.len(), 2);
        // Topological order: fmt before app.
        assert_eq!(graph.packages[0].package.name.as_str(), "fmt");
        assert_eq!(graph.packages[0].kind, PackageKind::Registry);
        assert_eq!(graph.packages[1].package.name.as_str(), "app");
        assert_eq!(graph.packages[1].kind, PackageKind::Local);
        // Only `app` is primary.
        assert_eq!(graph.primary_packages, vec![1]);
        // The dep edge is recorded so cabin-build can resolve target deps.
        let edges: Vec<(usize, DependencyKind)> = graph.packages[1]
            .deps
            .iter()
            .map(|e| (e.index, e.kind))
            .collect();
        assert_eq!(edges, vec![(0, DependencyKind::Normal)]);
    }

    #[test]
    fn unresolved_registry_dep_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"
spdlog = ">=1"
"#,
            )
            .unwrap();
        dir.child("registry/fmt/cabin.toml")
            .write_str(
                r#"[package]
name = "fmt"
version = "10.2.1"
"#,
            )
            .unwrap();
        // Only `fmt` is in the registry; `spdlog` is missing.
        let registry = vec![RegistryPackageSource {
            name: pkg("fmt"),
            version: ver("10.2.1"),
            manifest_path: dir.path().join("registry/fmt/cabin.toml"),
        }];
        let err = load_workspace_with_options(
            dir.path().join("app/cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &registry,
                patches: &[],
                ports: &[],
                registry_policy: RegistryPolicy::Strict,
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .unwrap_err();
        match err {
            WorkspaceError::UnresolvedRegistryDependency { dep_name, parent } => {
                assert_eq!(dep_name, "spdlog");
                assert_eq!(parent, "app");
            }
            other => panic!("expected UnresolvedRegistryDependency, got {other:?}"),
        }
    }

    #[test]
    fn registry_dep_chained_through_extracted_manifest() {
        let dir = TempDir::new().unwrap();
        // Root -> spdlog -> fmt.
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
spdlog = ">=1"
"#,
            )
            .unwrap();
        dir.child("registry/spdlog/cabin.toml")
            .write_str(
                r#"[package]
name = "spdlog"
version = "1.13.0"

[dependencies]
fmt = ">=10"
"#,
            )
            .unwrap();
        dir.child("registry/fmt/cabin.toml")
            .write_str(
                r#"[package]
name = "fmt"
version = "10.2.1"
"#,
            )
            .unwrap();
        let registry = vec![
            RegistryPackageSource {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                manifest_path: dir.path().join("registry/fmt/cabin.toml"),
            },
            RegistryPackageSource {
                name: pkg("spdlog"),
                version: ver("1.13.0"),
                manifest_path: dir.path().join("registry/spdlog/cabin.toml"),
            },
        ];
        let graph = load_workspace_with_options(
            dir.path().join("app/cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &registry,
                patches: &[],
                ports: &[],
                registry_policy: RegistryPolicy::Strict,
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .unwrap();
        assert_eq!(graph.packages.len(), 3);
        // Topological order: fmt before spdlog before app.
        let names: Vec<&str> = graph
            .packages
            .iter()
            .map(|p| p.package.name.as_str())
            .collect();
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("fmt") < pos("spdlog"));
        assert!(pos("spdlog") < pos("app"));
    }

    #[test]
    fn registry_package_version_mismatch_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"
"#,
            )
            .unwrap();
        dir.child("registry/fmt/cabin.toml")
            .write_str(
                r#"[package]
name = "fmt"
version = "10.1.0"
"#,
            )
            .unwrap();
        let registry = vec![RegistryPackageSource {
            name: pkg("fmt"),
            version: ver("10.2.1"),
            manifest_path: dir.path().join("registry/fmt/cabin.toml"),
        }];
        let err = load_workspace_with_options(
            dir.path().join("app/cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &registry,
                patches: &[],
                ports: &[],
                registry_policy: RegistryPolicy::Strict,
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            WorkspaceError::RegistryPackageMismatch { .. }
        ));
    }

    // -----------------------------------------------------------------
    // workspace.exclude / default-members / dependency
    // inheritance / nested workspaces.
    // -----------------------------------------------------------------

    #[test]
    fn exclude_drops_member_from_primary_set() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
exclude = ["packages/skipme"]
"#,
            )
            .unwrap();
        dir.child("packages/keep/cabin.toml")
            .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
            .unwrap();
        dir.child("packages/skipme/cabin.toml")
            .write_str("[package]\nname = \"skipme\"\nversion = \"0.1.0\"\n")
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let names: Vec<&str> = graph
            .primary_packages
            .iter()
            .map(|i| graph.packages[*i].package.name.as_str())
            .collect();
        assert_eq!(names, vec!["keep"]);
        assert_eq!(graph.excluded_members.len(), 1);
        assert!(
            graph.excluded_members[0]
                .to_string_lossy()
                .ends_with("skipme")
        );
    }

    #[test]
    fn unused_exclude_pattern_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/keep"]
exclude = ["packages/missing"]
"#,
            )
            .unwrap();
        dir.child("packages/keep/cabin.toml")
            .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::UnusedExcludePattern { pattern, .. } => {
                assert_eq!(pattern, "packages/missing");
            }
            other => panic!("expected UnusedExcludePattern, got {other:?}"),
        }
    }

    #[test]
    fn default_members_must_be_workspace_members() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/keep"]
default-members = ["packages/missing"]
"#,
            )
            .unwrap();
        dir.child("packages/keep/cabin.toml")
            .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::DefaultMemberNotInMembers { member } => {
                assert_eq!(member, "packages/missing");
            }
            other => panic!("expected DefaultMemberNotInMembers, got {other:?}"),
        }
    }

    #[test]
    fn default_members_resolved_to_indices() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
default-members = ["packages/a"]
"#,
            )
            .unwrap();
        dir.child("packages/a/cabin.toml")
            .write_str("[package]\nname = \"a\"\nversion = \"0.1.0\"\n")
            .unwrap();
        dir.child("packages/b/cabin.toml")
            .write_str("[package]\nname = \"b\"\nversion = \"0.1.0\"\n")
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        assert_eq!(graph.default_members.len(), 1);
        let name = graph.packages[graph.default_members[0]]
            .package
            .name
            .as_str();
        assert_eq!(name, "a");
    }

    #[test]
    fn workspace_dependency_inheritance() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10 <11"
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let app = graph
            .packages
            .iter()
            .find(|p| p.package.name.as_str() == "app")
            .unwrap();
        assert_eq!(app.package.dependencies.len(), 1);
        match &app.package.dependencies[0].source {
            cabin_core::DependencySource::Version(req) => {
                assert!(req.to_string().contains(">=10"));
            }
            other => panic!("expected resolved Version, got {other:?}"),
        }
    }

    #[test]
    fn workspace_dependency_inheritance_per_kind() {
        // Each `dep = { workspace = true }` looks up the matching
        // `[workspace.<kind>-dependencies]` table — never a sibling
        // table.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10"

[workspace.dev-dependencies]
gtest = "^1.14"
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }

[dev-dependencies]
gtest = { workspace = true }
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let app = graph
            .packages
            .iter()
            .find(|p| p.package.name.as_str() == "app")
            .unwrap();
        for (name, kind) in [
            ("fmt", DependencyKind::Normal),
            ("gtest", DependencyKind::Dev),
        ] {
            let dep = app
                .package
                .dependencies
                .iter()
                .find(|d| d.name.as_str() == name && d.kind == kind)
                .unwrap_or_else(|| panic!("expected {name} in {kind:?}"));
            assert!(
                matches!(dep.source, cabin_core::DependencySource::Version(_)),
                "workspace inheritance should rewrite {name} into a Version source"
            );
        }
    }

    #[test]
    fn workspace_dependency_kind_does_not_cross_tables() {
        // `[dev-dependencies] foo = { workspace = true }` must
        // *not* fall back to `[workspace.dependencies]` — the
        // lookup is strictly kind-specific.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10"
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dev-dependencies]
fmt = { workspace = true }
"#,
            )
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::UnresolvedWorkspaceDependency {
                dep_name,
                parent,
                kind,
            } => {
                assert_eq!(dep_name, "fmt");
                assert_eq!(parent, "app");
                assert_eq!(kind, DependencyKind::Dev);
            }
            other => panic!("expected UnresolvedWorkspaceDependency for dev, got {other:?}"),
        }
    }

    #[test]
    fn dev_path_dependency_is_not_loaded_into_graph() {
        // Dev path-deps are declaration-only: they appear on
        // `package.dependencies` but never become a graph node, so
        // a missing dev-dep target directory is *not* an error
        // for ordinary commands.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dev-dependencies]
harness = { path = "../harness-that-does-not-exist" }
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml"))
            .expect("dev path-dep should not be traversed by ordinary load");
        // Only the root package is loaded.
        assert_eq!(graph.packages.len(), 1);
        // But the package still records the declaration.
        let app = &graph.packages[0];
        assert_eq!(app.package.dependencies.len(), 1);
        assert_eq!(app.package.dependencies[0].kind, DependencyKind::Dev);
    }

    #[test]
    fn unresolved_workspace_dependency_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
            )
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::UnresolvedWorkspaceDependency {
                dep_name,
                parent,
                kind,
            } => {
                assert_eq!(dep_name, "fmt");
                assert_eq!(parent, "app");
                assert_eq!(kind, DependencyKind::Normal);
            }
            other => panic!("expected UnresolvedWorkspaceDependency, got {other:?}"),
        }
    }

    #[test]
    fn nested_workspace_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["nested"]
"#,
            )
            .unwrap();
        dir.child("nested/cabin.toml")
            .write_str(
                r#"[workspace]
members = []
"#,
            )
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::NestedWorkspace { path } => {
                assert!(path.to_string_lossy().contains("nested"));
            }
            other => panic!("expected NestedWorkspace, got {other:?}"),
        }
    }

    #[test]
    fn member_expansion_is_deterministic() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        for name in ["zeta", "alpha", "mu", "kappa"] {
            dir.child(format!("packages/{name}/cabin.toml"))
                .write_str(&format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"
                ))
                .unwrap();
        }
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let names: Vec<&str> = graph
            .primary_packages
            .iter()
            .map(|i| graph.packages[*i].package.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "kappa", "mu", "zeta"]);
    }

    // -----------------------------------------------------------------
    // Workspace pattern paths must be relative to the workspace root.
    // Absolute and `..` patterns are rejected.
    // -----------------------------------------------------------------

    fn workspace_with_outside_member(pattern: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(&format!("[workspace]\nmembers = [\"{pattern}\"]\n"))
            .unwrap();
        dir
    }

    #[test]
    fn member_pattern_with_absolute_path_rejected() {
        // `/tmp/outside` is platform-dependent but path::is_absolute
        // covers `\\` and `C:\` on Windows too — write a Unix path
        // for the test (the manifest never reaches the FS in the
        // failing branch).
        let dir = workspace_with_outside_member("/tmp/outside");
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::WorkspacePatternEscapesRoot { field, pattern } => {
                assert_eq!(field, "workspace.members");
                assert_eq!(pattern, "/tmp/outside");
            }
            other => panic!("expected WorkspacePatternEscapesRoot, got {other:?}"),
        }
    }

    #[test]
    fn member_pattern_with_parent_dir_rejected() {
        // Set up a sibling directory the pattern would pull in,
        // proving the validator stops the load *before* expansion.
        let dir = TempDir::new().unwrap();
        let workspace_dir = dir.child("ws");
        let outside_dir = dir.child("outside");
        workspace_dir
            .child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["../outside"]
"#,
            )
            .unwrap();
        outside_dir
            .child("cabin.toml")
            .write_str("[package]\nname = \"sneaky\"\nversion = \"0.1.0\"\n")
            .unwrap();
        let err = load_workspace(workspace_dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::WorkspacePatternEscapesRoot { field, pattern } => {
                assert_eq!(field, "workspace.members");
                assert_eq!(pattern, "../outside");
            }
            other => panic!("expected WorkspacePatternEscapesRoot, got {other:?}"),
        }
    }

    #[test]
    fn exclude_pattern_with_parent_dir_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/keep"]
exclude = ["../outside"]
"#,
            )
            .unwrap();
        dir.child("packages/keep/cabin.toml")
            .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::WorkspacePatternEscapesRoot { field, pattern } => {
                assert_eq!(field, "workspace.exclude");
                assert_eq!(pattern, "../outside");
            }
            other => panic!("expected WorkspacePatternEscapesRoot, got {other:?}"),
        }
    }

    #[test]
    fn default_member_with_parent_dir_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/keep"]
default-members = ["../outside"]
"#,
            )
            .unwrap();
        dir.child("packages/keep/cabin.toml")
            .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
            .unwrap();
        let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
        match err {
            WorkspaceError::WorkspacePatternEscapesRoot { field, pattern } => {
                assert_eq!(field, "workspace.default-members");
                assert_eq!(pattern, "../outside");
            }
            other => panic!("expected WorkspacePatternEscapesRoot, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // selection-aware registry materialization.
    // -----------------------------------------------------------------

    #[test]
    fn for_selection_skips_versioned_deps_outside_strict_set() {
        // app needs fmt; unrelated `b` declares spdlog. The
        // strict set is {app}; the registry only has fmt. Loading
        // must succeed because b's spdlog dep is skipped.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10 <11"
"#,
            )
            .unwrap();
        dir.child("packages/b/cabin.toml")
            .write_str(
                r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
spdlog = "^1"
"#,
            )
            .unwrap();
        // Pretend we already extracted fmt 10.2.1 somewhere on disk.
        dir.child("registry/fmt/cabin.toml")
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let registry = vec![RegistryPackageSource {
            name: PackageName::new("fmt").unwrap(),
            version: ver("10.2.1"),
            manifest_path: dir.path().join("registry/fmt/cabin.toml"),
        }];
        let mut strict: BTreeSet<String> = BTreeSet::new();
        strict.insert("app".into());
        let graph = load_workspace_with_options(
            dir.path().join("cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &registry,
                patches: &[],
                ports: &[],
                registry_policy: RegistryPolicy::StrictFor(&strict),
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .expect("selection-aware load should not require spdlog");
        // app, b, and fmt all loaded; no `spdlog` was added.
        let names: BTreeSet<&str> = graph
            .packages
            .iter()
            .map(|p| p.package.name.as_str())
            .collect();
        assert!(names.contains("app"));
        assert!(names.contains("b"));
        assert!(names.contains("fmt"));
        assert!(!names.contains("spdlog"));
    }

    #[test]
    fn for_selection_still_errors_when_strict_dep_missing() {
        // app is strict and depends on fmt, but the registry is
        // empty. The selection-aware loader must still error on
        // app's missing fmt.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10 <11"
"#,
            )
            .unwrap();
        // A non-empty registry shifts the loader out of the
        // legacy "skip versioned deps" mode. Build a sham entry
        // for some other package so registry_by_name is
        // populated but does not contain `fmt`.
        dir.child("registry/other/cabin.toml")
            .write_str("[package]\nname = \"other\"\nversion = \"1.0.0\"\n")
            .unwrap();
        let registry = vec![RegistryPackageSource {
            name: PackageName::new("other").unwrap(),
            version: ver("1.0.0"),
            manifest_path: dir.path().join("registry/other/cabin.toml"),
        }];
        let mut strict: BTreeSet<String> = BTreeSet::new();
        strict.insert("app".into());
        let err = load_workspace_with_options(
            dir.path().join("cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &registry,
                patches: &[],
                ports: &[],
                registry_policy: RegistryPolicy::StrictFor(&strict),
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .expect_err("expected UnresolvedRegistryDependency for selected closure dep");
        match err {
            WorkspaceError::UnresolvedRegistryDependency { dep_name, parent } => {
                assert_eq!(dep_name, "fmt");
                assert_eq!(parent, "app");
            }
            other => panic!("expected UnresolvedRegistryDependency, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Foundation-port resolution
    // ---------------------------------------------------------------

    #[test]
    fn resolves_port_dep_via_supplied_source() {
        let tmp = TempDir::new().unwrap();

        // Port directory (contains port.toml in real life, but
        // the workspace loader only cares about the canonical
        // path).
        let port_dir = tmp.child("ports/zlib/1.3.1");
        port_dir.create_dir_all().unwrap();

        // Prepared overlay manifest directory (the CLI
        // orchestration step writes the upstream sources here
        // before the loader runs).
        let prepared = tmp.child("cache/sources/sha256/abc");
        prepared
            .child("cabin.toml")
            .write_str(
                "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[target.zlib]\ntype = \"library\"\nsources = [\"zlib.c\"]\n",
            )
            .unwrap();
        prepared
            .child("zlib.c")
            .write_str("int zlib_dummy(void){return 0;}\n")
            .unwrap();

        // Consumer manifest that references the port by
        // relative path.
        let consumer = tmp.child("consumer");
        consumer
            .child("cabin.toml")
            .write_str(
                r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.consumer]
type = "executable"
sources = ["src/main.c"]
deps = ["zlib"]
"#,
            )
            .unwrap();
        consumer
            .child("src/main.c")
            .write_str("int main(void){return 0;}\n")
            .unwrap();

        let port_sources = vec![PortPackageSource {
            name: PackageName::new("zlib").unwrap(),
            version: semver::Version::new(1, 3, 1),
            manifest_path: prepared.path().join("cabin.toml"),
            origin: cabin_port::PortOrigin::PortDir(port_dir.to_path_buf()),
        }];
        let graph = load_workspace_with_options(
            consumer.path().join("cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &[],
                patches: &[],
                ports: &port_sources,
                registry_policy: RegistryPolicy::Strict,
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .unwrap();
        // Two packages: the consumer and the zlib port.
        assert_eq!(graph.packages.len(), 2);
        let zlib = graph
            .packages
            .iter()
            .find(|p| p.package.name.as_str() == "zlib")
            .unwrap();
        assert_eq!(
            zlib.manifest_dir,
            std::fs::canonicalize(prepared.path()).unwrap()
        );
        // Foundation ports are local development policy, so the
        // package kind is Local.
        assert_eq!(zlib.kind, PackageKind::Local);
    }

    #[test]
    fn resolves_builtin_port_dep_by_name() {
        let tmp = TempDir::new().unwrap();

        // The "prepared" overlay (in a real build this is in the
        // cabin cache). The loader only needs the [package] block
        // to match the dep, plus a source file for the target.
        let prepared = tmp.child("cache/sources/sha256/abc");
        prepared
            .child("cabin.toml")
            .write_str(
                "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[target.zlib]\ntype = \"library\"\nsources = [\"zlib.c\"]\n",
            )
            .unwrap();
        prepared
            .child("zlib.c")
            .write_str("int zlib_dummy(void){return 0;}\n")
            .unwrap();

        let consumer = tmp.child("consumer");
        consumer
            .child("cabin.toml")
            .write_str(
                r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^1.3" }

[target.consumer]
type = "executable"
sources = ["src/main.c"]
deps = ["zlib"]
"#,
            )
            .unwrap();
        consumer
            .child("src/main.c")
            .write_str("int main(void){return 0;}\n")
            .unwrap();

        let port_sources = vec![PortPackageSource {
            name: PackageName::new("zlib").unwrap(),
            version: semver::Version::new(1, 3, 1),
            manifest_path: prepared.path().join("cabin.toml"),
            origin: cabin_port::PortOrigin::Builtin("zlib"),
        }];
        let graph = load_workspace_with_options(
            consumer.path().join("cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &[],
                patches: &[],
                ports: &port_sources,
                registry_policy: RegistryPolicy::Strict,
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .unwrap();
        assert_eq!(graph.packages.len(), 2);
        let zlib = graph
            .packages
            .iter()
            .find(|p| p.package.name.as_str() == "zlib")
            .unwrap();
        assert_eq!(zlib.kind, PackageKind::Local);
    }

    #[test]
    fn rejects_port_dep_without_prepared_source() {
        let tmp = TempDir::new().unwrap();
        let port_dir = tmp.child("ports/zlib/1.3.1");
        port_dir.create_dir_all().unwrap();

        let consumer = tmp.child("consumer");
        consumer
            .child("cabin.toml")
            .write_str(
                r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }
"#,
            )
            .unwrap();

        let err = load_workspace_with_options(
            consumer.path().join("cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &[],
                patches: &[],
                ports: &[],
                registry_policy: RegistryPolicy::Strict,
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, WorkspaceError::PortDependencyNotPrepared { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_port_dep_with_missing_port_directory() {
        let tmp = TempDir::new().unwrap();

        let consumer = tmp.child("consumer");
        consumer
            .child("cabin.toml")
            .write_str(
                r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../nonexistent/zlib" }
"#,
            )
            .unwrap();

        let err = load_workspace_with_options(
            consumer.path().join("cabin.toml"),
            &WorkspaceLoadOptions {
                registry: &[],
                patches: &[],
                ports: &[],
                registry_policy: RegistryPolicy::Strict,
                include_dev_for: &BTreeSet::new(),
                port_policy: PortPolicy::Strict,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, WorkspaceError::PortDirectoryMissing { .. }),
            "{err:?}"
        );
    }
}
