use std::collections::BTreeMap;
use std::path::PathBuf;

use cabin_core::{
    CompilerWrapperManifestSettings, Condition, DependencyKind, Package, PatchManifestSettings,
    ProfileDefinition, ProfileName, ToolchainSettings,
};

/// Root-manifest policy settings that apply workspace-wide even
/// when the entry manifest is a pure `[workspace]` manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootSettings {
    pub profiles: BTreeMap<ProfileName, ProfileDefinition>,
    pub toolchain: ToolchainSettings,
    pub compiler_wrapper: CompilerWrapperManifestSettings,
    pub patches: PatchManifestSettings,
}

impl From<cabin_manifest::RootSettings> for RootSettings {
    fn from(value: cabin_manifest::RootSettings) -> Self {
        Self {
            profiles: value.profiles,
            toolchain: value.toolchain,
            compiler_wrapper: value.compiler_wrapper,
            patches: value.patches,
        }
    }
}

/// A loaded set of local Cabin packages with their dependency edges
/// resolved against the local filesystem.
///
/// Packages appear in topological order: a package's local dependencies
/// always appear before the package itself in [`PackageGraph::packages`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageGraph {
    /// Path to the manifest the user passed (canonicalized to absolute).
    pub root_manifest_path: PathBuf,
    /// Directory containing the root manifest.
    pub root_dir: PathBuf,
    /// Whether the root manifest declares a `[workspace]` table.
    pub is_workspace_root: bool,
    /// If the root manifest itself is a package (i.e. has a `[package]`
    /// Table), the index of that package in [`PackageGraph::packages`].
    pub root_package: Option<usize>,
    /// Root-manifest policy settings. For package roots this
    /// mirrors the root package's root-owned fields; for pure
    /// workspace roots this is the only place those settings are
    /// exposed.
    pub root_settings: RootSettings,
    /// Indices of packages that count as "primary" — i.e. would be built
    /// when no narrower package selection is given.
    ///
    /// For a single package this is just the root. For a workspace root it
    /// is every member declared by `[workspace.members]`. Path dependencies
    /// pulled in transitively are *not* primary.
    pub primary_packages: Vec<usize>,
    /// Indices of packages listed under
    /// `[workspace.default-members]`, validated to be members. Empty
    /// when the workspace declares no defaults — callers fall back to
    /// the documented "all members" behavior. Always a subset of
    /// `primary_packages`.
    pub default_members: Vec<usize>,
    /// Relative paths under `root_dir` for any directories
    /// dropped by `[workspace.exclude]`. Carried through purely for
    /// metadata reporting; the loader has already removed them from
    /// `primary_packages`.
    pub excluded_members: Vec<PathBuf>,
    /// All loaded packages, in topological order.
    pub packages: Vec<WorkspacePackage>,
}

impl PackageGraph {
    /// Find a package by name. Linear scan; package counts are small.
    pub fn package_by_name(&self, name: &str) -> Option<&WorkspacePackage> {
        self.packages
            .iter()
            .find(|p| p.package.name.as_str() == name)
    }

    /// Index of a package by name. Returned together with the reference
    /// for callers that need to record edges by index.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.packages
            .iter()
            .position(|p| p.package.name.as_str() == name)
    }
}

/// A single loaded package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePackage {
    pub package: Package,
    /// Absolute path to this package's `cabin.toml`.
    pub manifest_path: PathBuf,
    /// Absolute path to the directory containing `manifest_path`.
    pub manifest_dir: PathBuf,
    /// Resolved package-dependency edges, in declaration order.
    /// Each edge carries the index of the depended-on package
    /// inside [`PackageGraph::packages`] together with the
    /// [`DependencyKind`] under which it was declared.
    ///
    /// Only kinds that participate in ordinary resolution
    /// (`Normal`) appear here today: dev path-deps are
    /// declaration-only and therefore never enter the package
    /// graph. The kind is preserved per-edge so the resolver /
    /// fetch / closure-walk callers can iterate all edges
    /// consistently with future kinds.
    pub deps: Vec<DependencyEdge>,
    /// Whether this package was loaded from a local source tree
    /// or from an extracted registry archive.
    pub kind: PackageKind,
}

impl WorkspacePackage {
    /// Iterate dependency edges of a single kind. Used by the
    /// build planner so cross-package target lookups stay limited
    /// to `Normal`-kind edges.
    pub fn deps_of_kind(&self, kind: DependencyKind) -> impl Iterator<Item = usize> + '_ {
        self.deps
            .iter()
            .filter(move |edge| edge.kind == kind)
            .map(|edge| edge.index)
    }

    /// Iterate all dependency edges as bare indices, in
    /// declaration order. Used by closure walks (resolve / fetch /
    /// metadata) that include every package-graph-resident kind.
    pub fn all_dep_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.deps.iter().map(|edge| edge.index)
    }
}

/// A single resolved package-dependency edge in the package graph.
///
/// The graph only contains edges that *could* be active on the
/// evaluation platform (the loader filters out non-matching
/// `[target.'cfg(...)'.<kind>]` entries before constructing the
/// graph), so consumers never need to re-check the condition
/// against a different platform — the loader already did. The
/// edge still records the originating condition for diagnostics
/// and metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEdge {
    /// Index of the depended-on package in [`PackageGraph::packages`].
    pub index: usize,
    /// Which manifest section this edge was declared under.
    pub kind: DependencyKind,
    /// `Some` when this edge originated from a
    /// `[target.'cfg(...)'.<kind>]` table that matched the
    /// evaluation platform; `None` for unconditional edges.
    pub condition: Option<Condition>,
}

/// Where a [`WorkspacePackage`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    /// A local-filesystem package: workspace member, root, or a
    /// `path = "..."` dependency.
    Local,
    /// A registry package whose source archive was already fetched and
    /// extracted into the artifact cache.
    Registry,
}

/// Synthesize a root identity for resolving over a pure-workspace
/// root (no `[package]`). The name is a deterministic
/// `__workspace_<dirname>` value the resolver uses for diagnostic
/// output only; nothing else relies on it being canonical. Lives
/// here because it is derived purely from a [`PackageGraph`]'s
/// `root_dir`, keeping the synthetic-root naming rule out of the CLI.
pub fn synthetic_root_identity(graph: &PackageGraph) -> (cabin_core::PackageName, semver::Version) {
    let dirname = graph
        .root_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace");
    let mut sanitized = String::with_capacity(dirname.len() + 12);
    sanitized.push_str("__workspace_");
    for c in dirname.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
            sanitized.push(c);
        } else {
            sanitized.push('_');
        }
    }
    let name =
        cabin_core::PackageName::new(sanitized).expect("synthesized name is non-empty and ASCII");
    let version = semver::Version::new(0, 0, 0);
    (name, version)
}
