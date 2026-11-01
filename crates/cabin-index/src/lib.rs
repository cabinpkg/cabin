//! Local JSON package index for Cabin.
//!
//! The on-disk index format is a directory containing one JSON
//! document per published package (`<name>.json`, or
//! `<scope>/<name>.json` for a scoped name), each enumerating the
//! package's published versions, their dependencies on other
//! registry packages, and a `yanked` flag.
//! Each version's `revisions` map points at its downloadable
//! packaging revisions; the `revision` field names the one currently
//! served.
//!
//! This crate owns that format.  It loads the JSON files,
//! validates them, and exposes a typed [`PackageIndex`], read one
//! package at a time by name with [`FileIndex`] so a resolution reads
//! only the documents it needs.
//! Resolution against the index lives in `cabin-resolver`.

pub mod error;
pub mod loader;
pub mod model;
mod walk;

pub use error::IndexError;
pub use loader::{FileIndex, SourceContext, parse_package_entry};
pub use model::{
    IndexEntry, IndexPackageDependency, IndexSystemDependency, PackageIndex, ResolutionMetadata,
    RevisionMetadata, SourceLocation, VersionMetadata,
};
pub use walk::walk_dependencies;
// Re-exported so index consumers can name the standard-metadata types
// reachable on `VersionMetadata::standards` through `cabin_index`.
pub use cabin_core::{StandardsMetadata, TargetStandards};
