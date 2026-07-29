//! Local JSON package index for Cabin.
//!
//! The on-disk index format is deliberately small: a directory
//! containing one `<package>.json` file per published package,
//! each enumerating the package's published versions, their
//! dependencies on other registry packages, and a `yanked` flag.
//! Each version's `revisions` map points at its downloadable
//! packaging revisions; the `revision` field names the one currently
//! served.
//!
//! This crate owns that format.  It loads the JSON files,
//! validates them, and exposes a typed [`PackageIndex`].
//! Resolution against the index lives in `cabin-resolver`.

pub mod error;
pub mod loader;
pub mod model;

pub use error::IndexError;
pub use loader::{SourceContext, load_index, parse_package_entry};
pub use model::{
    IndexEntry, IndexPackageDependency, IndexSystemDependency, PackageIndex, RevisionMetadata,
    SourceLocation, VersionMetadata,
};
// Re-exported so index consumers (the resolver's preference mode and
// publish lints) can name the standard-metadata types reachable on
// `VersionMetadata::standards` without depending on `cabin-core`
// directly.
pub use cabin_core::{StandardsMetadata, TargetStandards};
