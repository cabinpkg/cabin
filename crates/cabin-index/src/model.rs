use std::collections::BTreeMap;
use std::path::PathBuf;

use cabin_core::{Condition, DependencyKind, PackageName};

/// A loaded JSON package index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageIndex {
    /// Where the index was loaded from: the directory the file loader
    /// read, or the base URL of a sparse HTTP index.
    pub root: PathBuf,
    /// All known packages, keyed by name.  Sorted iteration is needed for
    /// deterministic resolver behavior, so a `BTreeMap` is used directly.
    pub packages: BTreeMap<PackageName, IndexEntry>,
}

impl PackageIndex {
    pub fn package(&self, name: &PackageName) -> Option<&IndexEntry> {
        self.packages.get(name)
    }
}

/// A single package's index entry: every published version plus its
/// metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub name: PackageName,
    /// Versions, keyed for fast lookup and deterministic iteration.
    pub versions: BTreeMap<semver::Version, VersionMetadata>,
}

/// Metadata recorded for one version of a package.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionMetadata {
    /// Normal-kind direct dependencies.  Registry-only - local path
    /// dependencies do not appear in the index.
    pub dependencies: BTreeMap<PackageName, IndexPackageDependency>,
    /// `[dev-dependencies]` of this version.
    pub dev_dependencies: BTreeMap<PackageName, IndexPackageDependency>,
    /// `system = true` dependencies of this version.  Each entry is
    /// a declaration the consumer is responsible for resolving
    /// outside Cabin (system probe, package manager, etc.) - they
    /// never enter the Cabin resolver / fetcher / cache.  Keyed by
    /// pkg-config module name, which keeps its own grammar
    /// (`cabin_core::validate_system_dependency_name`).
    pub system_dependencies: BTreeMap<String, IndexSystemDependency>,
    /// Whether this version has been yanked.  Yanked versions are
    /// excluded from resolver candidate sets.
    pub yanked: bool,
    /// The current packaging revision's id: the entry the registry
    /// serves when no lockfile pins another revision.  `None` for
    /// resolver-only fixtures that carry no `revisions` map (such a
    /// version cannot be materialized).
    pub revision: Option<String>,
    /// Every fetchable packaging revision of this version, keyed by
    /// revision id (the leading
    /// [`cabin_core::registry::PACKAGING_REVISION_HEX_LEN`] hex chars
    /// of the archive's SHA-256).  Superseded revisions stay listed so
    /// lockfiles that pin them keep building.
    pub revisions: BTreeMap<String, RevisionMetadata>,
    /// Digest of the *current* revision's archive - derived by the
    /// loader from [`Self::revisions`] for the many consumers that
    /// only ever want the served revision.  `None` exactly when
    /// `revisions` is empty.  Revision-aware consumers (locked
    /// validation, fetch-by-pin, vendor) consult the map.
    pub checksum: Option<cabin_core::Checksum>,
    /// Where the *current* revision's source archive lives - the same
    /// loader-derived convenience as [`Self::checksum`].  Already
    /// resolved: callers receive a path or a URL they can act on
    /// without further resolution.
    pub source: Option<SourceLocation>,
    /// Declared `[features]`, validated by the loader.  Empty for
    /// entries that omit the field.  The resolver unifies feature
    /// requests against these tables, so a version that lacks a
    /// requested feature is not a candidate for that request.
    pub features: cabin_core::Features,
    /// Declared `[profile.*]` tables, preserved as-is.  Profiles
    /// are local build configuration; the resolver never
    /// consumes them.  Older registries that omit the field
    /// continue to load.
    pub profiles: Option<serde_json::Value>,
    /// Manifest-declared `[toolchain]` block, preserved as-is.
    /// Local build configuration; never consulted by the
    /// resolver.  Older registries that omit the field continue
    /// to load.
    pub toolchain: Option<serde_json::Value>,
    /// Manifest-declared `[profile]` block, preserved as-is.
    /// Local build configuration; never consulted by the
    /// resolver.  Older registries that omit the field continue
    /// to load.
    pub build: Option<serde_json::Value>,
    /// Manifest-declared `[build] compiler-wrapper`, preserved as-is.
    /// Local build configuration; never consulted by the resolver.
    /// Older registries that omit the field continue to load.
    pub compiler_wrapper: Option<serde_json::Value>,
    /// Manifest-declared `[package]`-level language standard
    /// fields, preserved as-is.  Round-trip only; the resolver reads
    /// the typed `standards` table below instead.  Older registries
    /// that omit the field continue to load.
    pub language: Option<serde_json::Value>,
    /// Declared per-target standard-compatibility table (spec D9
    /// `ReqOf`, header-only inference applied).  A **typed** view -
    /// unlike the `language` passthrough above, which is round-trip
    /// archival only - so index consumers (preference mode, publish
    /// lints) can read a candidate version's per-target interface
    /// requirements without downloading its archive.  An empty table
    /// (the default for pre-`standards` entries) means everything is
    /// unconstrained.  See `docs/design/standard-compatibility/registry-index.md`.
    pub standards: cabin_core::StandardsMetadata,
    /// Declared per-target native-library identity claims
    /// (`links`), keyed by target name.  A typed view like
    /// `standards` so the post-resolution uniqueness check can read
    /// a candidate version's claims without downloading its archive.
    /// Values are carried verbatim - the publish path validated the
    /// grammar, and the check only compares them for equality.
    /// Empty (the default for pre-`links` entries) means the version
    /// claims nothing.
    pub links: BTreeMap<String, String>,
    /// Declared `[package.upstream]` provenance, typed.  Inert for
    /// resolution and fetching - Cabin never downloads the upstream
    /// URL; the registry's external verifier consumes it.  `None`
    /// for entries that declare none.
    pub upstream: Option<cabin_core::UpstreamProvenance>,
}

impl VersionMetadata {
    /// Give this version the resolution metadata of its revision with
    /// `checksum`, as a lockfile pinning that revision must see it.  A
    /// no-op when that revision carries none of its own - it then has
    /// exactly the version's - or is not listed.
    pub fn select_revision(&mut self, checksum: &cabin_core::Checksum) {
        let Some(resolution) = self
            .revisions
            .values()
            .find(|revision| &revision.checksum == checksum)
            .and_then(|revision| revision.resolution.clone())
        else {
            return;
        };
        self.dependencies = resolution.dependencies;
        self.features = resolution.features;
        self.standards = resolution.standards;
        self.links = resolution.links;
    }
}

/// One packaging revision of a published version: the immutable
/// `(name, version, revision)` unit's archive identity plus its
/// publish time.  A lockfile's `checksum` selects one, for fetching and
/// (through [`VersionMetadata::select_revision`]) for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionMetadata {
    /// Digest of this revision's archive bytes.  The revision id is
    /// this digest's leading hex prefix, so the pair can never
    /// disagree in a loaded index.
    pub checksum: cabin_core::Checksum,
    /// When this revision was published, as recorded by the registry
    /// that accepted it.  Carried verbatim; Cabin never orders
    /// revisions by it client-side (the `revision` pointer names the
    /// current one).
    pub published_at: String,
    /// Where this revision's source archive lives, already resolved
    /// by the loader.
    pub source: SourceLocation,
    /// This revision's resolution metadata when it differs from the
    /// version's, which belongs to the current revision.  `None` means
    /// the revision resolves exactly as the version does.  Boxed: few
    /// revisions carry one, and each `revisions` map node reserves room
    /// for eleven revisions.
    pub resolution: Option<Box<ResolutionMetadata>>,
}

/// The metadata resolution reads from a version, recorded per
/// packaging revision because a respin may change it: the archive's
/// bytes and the graph resolved from them must always agree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolutionMetadata {
    pub dependencies: BTreeMap<PackageName, IndexPackageDependency>,
    pub features: cabin_core::Features,
    pub standards: cabin_core::StandardsMetadata,
    pub links: BTreeMap<String, String>,
}

/// One `system = true` dependency entry as it appears in an index
/// version metadata document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSystemDependency {
    /// Free-form version requirement string.  Not interpreted as a
    /// `SemVer` constraint.
    pub version: String,
    /// Dependency table the system declaration came from.
    /// Defaults to `normal` when omitted by older registries.
    pub dependency_kind: DependencyKind,
    /// Optional `cfg(...)` predicate copied from the on-disk
    /// `target` field.
    pub condition: Option<Condition>,
}

/// One Cabin package dependency entry inside an index version
/// document.  Carries the full per-edge information so the
/// resolver can apply optional / features / default-features
/// when expanding registry packages.
///
/// Older index entries use a bare requirement string; the loader
/// normalizes both shapes to this struct (defaults: not optional,
/// no extra features, default-features = true).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexPackageDependency {
    pub req: semver::VersionReq,
    pub optional: bool,
    pub features: Vec<String>,
    pub default_features: bool,
    /// Optional `cfg(...)` predicate copied from the on-disk
    /// `target` field.  Resolution ignores it - the lockfile serves
    /// every platform - and builds evaluate the package manifest's
    /// own copy for the platform they build for.
    pub condition: Option<Condition>,
}

/// Where a version's source archive lives on disk or on the network,
/// so the same `IndexEntry` shape can come from a directory on disk
/// or from a sparse HTTP registry.
///
/// The local-filesystem variant is what `cabin-index`'s file loader
/// returns; the HTTP variant is what `cabin-index-http` returns after
/// resolving relative `source.path` values against the package metadata
/// URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceLocation {
    /// Absolute filesystem path.
    LocalPath(PathBuf),
    /// Absolute `http://` or `https://` URL.
    HttpUrl(String),
}
