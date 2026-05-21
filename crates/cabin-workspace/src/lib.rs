//! Local workspace and package-graph loader for Cabin.
//!
//! Given a path to a `cabin.toml`, this crate discovers workspace members
//! (if any), follows local `path = "..."` dependencies, deduplicates by
//! canonical manifest path, detects duplicate `[package].name` and package
//! cycles, and returns a topologically-sorted [`PackageGraph`].
//!
//! Versioned dependencies are not resolved here. The CLI resolves them,
//! fetches artifacts, and passes registry package sources back into this
//! crate through the registry-aware loading entry points. Git sources are
//! not supported.

// `WorkspaceError` aggregates a wide set of typed leaf errors
// (manifest parse, registry / patch validation, dependency-edge
// resolution). Each variant is small on its own; the union
// crosses clippy's default `result_large_err` threshold with
// patch and source-replacement variants included. Boxing the enum
// at every call site would be churny and hide the variant on
// the happy path; we accept the larger `Result` here instead.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::redundant_closure_for_method_calls,
    clippy::manual_let_else,
    clippy::single_match_else,
    clippy::map_unwrap_or,
    clippy::needless_raw_string_hashes,
    clippy::match_wildcard_for_single_variants,
    clippy::too_many_lines,
    clippy::format_push_string,
    clippy::explicit_iter_loop,
    clippy::if_not_else
)]

pub mod discovery;
pub mod error;
pub mod graph;
pub mod loader;
pub mod patch;
pub mod selection;

pub use discovery::{DiscoveredManifest, discover_workspace_root};
pub use error::WorkspaceError;
pub use graph::{DependencyEdge, PackageGraph, PackageKind, RootSettings, WorkspacePackage};
pub use loader::{
    PatchedPackageSource, PortPackageSource, RegistryPackageSource, WorkspaceLoadOptions,
    load_workspace, load_workspace_skip_ports, load_workspace_with_options,
};
pub use patch::{
    ActivePatch, ActivePatchSet, ConfigPatchInput, PatchResolutionError, PatchResolutionInputs,
    collect_patched_versioned_deps, resolve_active_patches,
};
pub use selection::{
    PackageSelection, ResolvedSelection, SelectionMode,
    closure_has_versioned_deps_excluding_with_dev,
    collect_closure_versioned_deps_excluding_with_dev, resolve_package_selection,
};
