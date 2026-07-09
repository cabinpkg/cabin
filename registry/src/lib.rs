//! Cabin's hosted registry service: the server side of the sparse HTTP file
//! registry contract in `docs/remote-registry.md` at the repository root.
//!
//! Domain logic (token hashing, route validation, document composition, the
//! error envelope, cookie signing, HTML rendering) lives in modules that
//! compile and unit-test on the host target; the Cloudflare-specific glue in
//! [`glue`] and [`web_glue`] only compiles for wasm32.

pub mod allowlist;
pub mod analytics;
pub mod auth;
pub mod breaker;
pub mod documents;
pub mod error;
pub mod pages;
pub mod publish;
pub mod quota;
pub mod routes;
pub mod session;

#[cfg(target_arch = "wasm32")]
mod glue;
#[cfg(target_arch = "wasm32")]
mod web_glue;
