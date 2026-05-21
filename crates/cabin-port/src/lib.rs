//! Foundation-port recipe layer for Cabin.
//!
//! A foundation port is a curated recipe (`port.toml`) that names
//! an upstream source archive, pins it by SHA-256, and ships an
//! overlay `cabin.toml` describing the upstream sources as a
//! Cabin C/C++ target. This crate owns:
//!
//! - the `port.toml` schema and parser ([`mod@parse`]),
//! - the typed [`PortDescriptor`] / [`PortSource`] model ([`model`]),
//! - the source-preparation pipeline ([`mod@prepare`]).
//!
//! Crate boundaries:
//! - this crate must not perform HTTP — the caller (the
//!   CLI orchestration layer) downloads archive bytes and
//!   passes them in as [`PortFetchSource::InMemoryArchive`];
//! - this crate must not call the resolver, the workspace
//!   loader, or the build planner;
//! - extraction safety (decompression-bomb caps, symlink
//!   rejection, path-traversal protection) is delegated to
//!   `cabin-artifact::safe_extract_tar_gz`.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod cache;
pub mod error;
pub mod model;
pub mod parse;
pub mod prepare;

pub use cache::PortCache;
pub use error::PortError;
pub use model::{OverlayManifest, PortChecksum, PortDescriptor, PortMetadata, PortSource};
pub use parse::{load_port, parse_port_str};
pub use prepare::{
    PortEntry, PortFetchSource, PortPlan, PortPrepareOptions, PortPrepareResult, PortProvenance,
    PreparedPort, prepare,
};
