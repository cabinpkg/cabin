//! Local source-archive layer for Cabin.
//!
//! The artifact layer turns a resolved registry package set into
//! on-disk source trees.  The crate owns:
//!
//! - cache layout ([`cache`]),
//! - SHA-256 verification and `.tar.gz` extraction ([`mod@fetch`], [`extract`]),
//! - the small typed surface in [`model`].
//!
//! Crate boundaries:
//! - this crate must not run the resolver, write Ninja, or invoke
//!   Compilers;
//! - it must not implement networking, publishing, or any server
//!   Functionality;
//! - extraction is fail-closed: archive entries that escape the
//!   Destination, declare absolute paths, contain `..` components, or
//!   Use unsupported tar entry types are rejected.

// `ArtifactError` aggregates lockfile, fetch, extract, and
// cache errors.  The union crosses clippy's default
// `result_large_err` threshold once `cabin_lockfile` (whose
// errors flow in via `?`) gains its own larger variants.
// Boxing the enum at every call site would be churny; we
// accept the larger `Result` instead.
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
