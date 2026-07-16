//! Cabin's hosted registry service: the server side of the sparse HTTP file
//! registry contract in `docs/remote-registry.md` at the repository root.
//!
//! Domain logic (token hashing, route validation, document composition, the
//! error envelope, cookie signing, session-API JSON shapes) lives in modules
//! that compile and unit-test on the host target; the Cloudflare-specific
//! glue in [`glue`] and [`web_glue`] only compiles for wasm32.

pub mod allowlist;
pub mod analytics;
pub mod auth;
pub mod backup;
pub mod breaker;
pub mod claim;
pub mod documents;
pub mod error;
pub mod publish;
pub mod quota;
pub mod routes;
pub mod session;
pub mod sql;
pub mod user_api;
pub mod verify;

#[cfg(target_arch = "wasm32")]
mod backup_glue;
#[cfg(target_arch = "wasm32")]
mod glue;
#[cfg(target_arch = "wasm32")]
mod web_glue;
