//! Local source-archive layer for Cabin.
//!
//! The artifact layer turns a resolved registry package set into
//! on-disk source trees.  The crate owns:
//!
//! - cache layout ([`cache`]),
//! - SHA-256 verification and archive extraction ([`mod@fetch`], [`extract`]),
//! - the small typed surface in [`model`].
//!
//! Crate boundaries:
//! - this crate must not run the resolver, write Ninja, or invoke
//!   compilers;
//! - it must not implement networking, publishing, or any server
//!   functionality;
//! - extraction is fail-closed: archive entries that escape the
//!   destination, declare absolute paths, contain `..` components, or
//!   use unsupported tar entry types are rejected.

pub mod cache;
pub mod error;
pub mod extract;
pub mod fetch;
pub mod model;

pub use cache::ArtifactCache;
pub use error::ArtifactError;
pub use extract::{SafeExtractOptions, safe_extract_tar_gz, safe_extract_zip};
pub use fetch::{
    FetchEntry, FetchOptions, FetchPlan, FetchResult, FetchSource, FetchedPackage, fetch,
};
pub use model::{CHECKSUM_PREFIX, ChecksumDigest};
