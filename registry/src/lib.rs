//! Cabin's hosted registry service: the server side of the sparse HTTP file
//! registry contract in `docs/remote-registry.md` at the repository root.
//!
//! Domain logic (token hashing, route validation, document composition, the
//! error envelope) lives in modules that compile and unit-test on the host
//! target; the Cloudflare-specific glue in [`glue`] only compiles for wasm32.

pub mod auth;
pub mod documents;
pub mod error;
pub mod routes;

#[cfg(target_arch = "wasm32")]
mod glue;
