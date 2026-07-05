//! Local JSON package index for Cabin.
//!
//! The on-disk index format is deliberately small: a directory
//! containing one `<package>.json` file per published package,
//! each enumerating the package's published versions, their
//! dependencies on other registry packages, and a `yanked` flag.
//! Optional `source` and `checksum` fields point at downloadable
//! archives.
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
    ArchiveFormat, IndexEntry, IndexPackageDependency, IndexSystemDependency, PackageIndex,
    SourceArtifact, SourceArtifactKind, SourceLocation, VersionMetadata,
};
// Re-exported so index consumers (the resolver's preference mode and
// publish lints) can name the standard-metadata types reachable on
// `VersionMetadata::standards` without depending on `cabin-core`
// directly.
pub use cabin_core::{StandardsMetadata, TargetStandards};
