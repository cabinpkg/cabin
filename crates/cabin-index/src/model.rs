use std::collections::BTreeMap;
use std::path::PathBuf;

use cabin_core::{Condition, DependencyKind, PackageName};

/// A loaded local JSON package index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageIndex {
    /// Absolute filesystem path the index was loaded from.
    pub root: PathBuf,
    /// All known packages, keyed by name.  Sorted iteration is needed for
    /// deterministic resolver behavior, so a `BTreeMap` is used directly.
    pub packages: BTreeMap<PackageName, IndexEntry>,
}

impl PackageIndex {
    /// Look up a package by name.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMetadata {
    /// Normal-kind direct dependencies.  Registry-only - local path
    /// dependencies do not appear in the index.
    pub dependencies: BTreeMap<PackageName, IndexPackageDependency>,
    /// `[dev-dependencies]` of this version.
    pub dev_dependencies: BTreeMap<PackageName, IndexPackageDependency>,
    /// `system = true` dependencies of this version.  Each entry is
    /// a declaration the consumer is responsible for resolving
    /// outside Cabin (system probe, package manager, etc.) - they
    /// never enter the Cabin resolver / fetcher / cache.
    pub system_dependencies: BTreeMap<PackageName, IndexSystemDependency>,
    /// Whether this version has been yanked.  Yanked versions are
    /// excluded from resolver candidate sets.
    pub yanked: bool,
    /// `sha256:<hex>` digest of the source archive.  Optional in the
    /// schema so pure-resolution fixtures can omit it; required for
    /// `cabin fetch` and `cabin build` to materialize a registry
    /// package.
    pub checksum: Option<String>,
    /// Source artifact metadata.  Optional so existing
    /// resolver-only fixtures keep working; required to fetch the
    /// version's source tree.
    pub source: Option<SourceArtifact>,
    /// Declared `[features]`, preserved as-is from the registry.
    /// `None` for older entries that omit the field.  The resolver
    /// does not currently consume features beyond passing them
    /// through; the feature resolver consumes them in a later
    /// step.
    pub features: Option<serde_json::Value>,
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
    /// fields, preserved as-is.  Round-trip only today; resolver
    /// consumption is deferred.  Older registries that omit the
    /// field continue to load.
    pub language: Option<serde_json::Value>,
    /// Declared per-target standard-compatibility table (spec D9
    /// `ReqOf`, header-only inference applied).  A **typed** view -
    /// unlike the `language` passthrough above, which is round-trip
    /// archival only - so index consumers (preference mode, publish
    /// lints) can read a candidate version's per-target interface
    /// requirements without downloading its archive.  An empty table
    /// (the default for pre-`standards` entries) means everything is
    /// unconstrained; the resolver does not consult it for version
    /// selection.  See `docs/design/standard-compatibility/registry-index.md`.
    pub standards: cabin_core::StandardsMetadata,
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
    /// `target` field.  When present and the host platform fails
    /// the predicate, the dependency is omitted from system-dep
    /// views.
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
    /// `target` field.  When present and the host platform fails
    /// the predicate, the dependency is skipped during resolution
    /// and metadata views.
    pub condition: Option<Condition>,
}

impl IndexPackageDependency {
    /// Whether this edge participates in resolution on `platform`:
    /// non-optional and, when `target`-conditioned, matching the
    /// platform.  The single source of truth the resolver and the
    /// sparse-HTTP prefetch both consult so they agree on which
    /// edges reach the index.
    pub fn is_active_for(&self, platform: &cabin_core::TargetPlatform) -> bool {
        // Index dependency gating is platform-only; feature
        // conditions are never present on registry index metadata,
        // and compiler-conditioned entries are rejected by the
        // loader (`cabin publish` cannot produce them and
        // hand-authored ones refuse to load), so the platform-only
        // context is correct.
        !self.optional
            && self
                .condition
                .as_ref()
                .is_none_or(|c| c.evaluate(&cabin_core::ConditionContext::platform_only(platform)))
    }
}

/// A reference to a source archive that materializes one version of a
/// package.
///
/// The location can be either a local path or an HTTP URL via
/// [`SourceLocation`] so the same `IndexEntry` shape can come from
/// A directory on disk or from a sparse HTTP registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceArtifact {
    pub kind: SourceArtifactKind,
    pub format: ArchiveFormat,
    /// Where the archive lives.  Already resolved by the loader: callers
    /// receive a path or a URL they can act on without further
    /// resolution.
    pub location: SourceLocation,
}

/// Where a [`SourceArtifact`] lives on disk or on the network.
///
/// The local-filesystem variant is what `cabin-index`'s file loader
/// returns; the HTTP variant is what `cabin-index-http` returns after
/// resolving relative `source.path` values against the package metadata
/// URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceLocation {
    /// Absolute filesystem path produced by the local file index
    /// loader.
    LocalPath(PathBuf),
    /// Absolute `http://` or `https://` URL produced by the HTTP index
    /// loader.
    HttpUrl(String),
}

/// What kind of source artifact backs a registry package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceArtifactKind {
    /// A `.tar.gz` source archive (the only currently supported kind).
    Archive,
}

/// Archive container format.  Only `tar.gz` is currently supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    TarGz,
}
