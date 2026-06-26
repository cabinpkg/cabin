//! Local workspace and package-graph loader for Cabin.
//!
//! Given a path to a `cabin.toml`, this crate discovers workspace members
//! (if any), follows local `path = "..."` dependencies, deduplicates by
//! canonical manifest path, detects duplicate `[package].name` and package
//! cycles, and returns a topologically-sorted [`PackageGraph`].
//!
//! Versioned dependencies are not resolved here.  The CLI resolves them,
//! fetches artifacts, and passes registry package sources back into this
//! crate through the registry-aware loading entry points.  Git sources are
//! not supported.

pub mod discovery;
pub mod error;
pub mod graph;
pub mod loader;
pub mod patch;
pub mod selection;

pub use discovery::{DiscoveredManifest, discover_workspace_root};
pub use error::WorkspaceError;
pub use graph::{
    DependencyEdge, PackageGraph, PackageKind, RootSettings, WorkspacePackage,
    synthetic_root_identity,
};
pub use loader::{
    PatchedPackageSource, PortPackageSource, PortPolicy, RegistryPackageSource, RegistryPolicy,
    WorkspaceLoadOptions, load_workspace, load_workspace_skip_ports, load_workspace_with_options,
};
pub use patch::{
    ActivePatch, ActivePatchSet, ConfigPatchInput, PatchResolutionError, PatchResolutionInputs,
    collect_patched_versioned_deps, resolve_active_patches,
};
pub use selection::{
    PackageSelection, ResolvedSelection, SelectionMode,
    closure_has_versioned_deps_excluding_with_dev,
    collect_closure_versioned_deps_excluding_with_dev, combine_version_reqs,
    resolve_package_selection,
};
