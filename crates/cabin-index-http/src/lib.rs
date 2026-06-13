//! Sparse HTTP index client for Cabin.
//!
//! Read-only client that consumes a static HTTP serving of the
//! file-registry layout produced by `cabin-registry-file`:
//!
//! ```text
//! <base>/
//!   config.json
//!   packages/<name>.json
//!   artifacts/<name>/<name>-<version>.tar.gz
//! ```
//!
//! The crate is intentionally narrow:
//!
//! - it issues `GET` requests for `config.json`, `packages/<name>.json`,
//!   and (when the CLI calls [`HttpClient::download`]) artifact URLs;
//! - it never POSTs, PUTs, or otherwise mutates a remote registry;
//! - it never authenticates, never honors redirects to alternate
//!   registries, never persists a metadata cache;
//! - it produces the same [`cabin_index::IndexEntry`] / [`cabin_index::PackageIndex`]
//!   shape as the local file index, so the resolver and lockfile
//!   layers stay HTTP-free.
//!
//! HTTP publish, server-side functionality, OCI / GHCR, package
//! upload, authentication, and ownership are out of scope.

pub mod client;
pub mod error;
pub mod source;

pub use client::HttpClient;
pub use error::IndexHttpError;
pub use source::HttpIndex;
