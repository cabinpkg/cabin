//! Local file-registry layout, package-index mutation, and
//! atomic publish for Cabin.
//!
//! Introduces a registry shape that the existing read path
//! already understands:
//!
//! ```text
//! <registry>/
//!   config.json
//!   packages/<name>.json
//!   artifacts/<name>/<name>-<version>.tar.gz
//! ```
//!
//! This crate owns the layout, the package-index file format, the
//! atomic write helpers that keep partially-written state from
//! sticking around, and a simple `.cabin-registry.lock` lock file so
//! concurrent `cabin publish --registry-dir` invocations are
//! detected.
//!
//! Crate boundaries:
//! - this crate must not implement HTTP / sparse / OCI publish;
//! - it must not implement server-side functionality;
//! - it must not run the resolver, parse arbitrary `cabin.toml`s, or
//!   Build packages — `cabin-package` produces the
//!   [`cabin_package::StagedPackage`] this crate consumes;
//! - actual real-world `cabin publish` orchestration lives in
//!   `cabin-publish`, which combines staging with this crate's
//!   Writers.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::default_trait_access
)]

mod atomic;
pub mod error;
pub mod index;
pub mod layout;
pub mod lock;
pub mod publish;

pub use error::RegistryError;
pub use index::PACKAGE_INDEX_SCHEMA;
pub use layout::{FileRegistry, REGISTRY_CONFIG_FILENAME, RegistryConfig};
pub use lock::RegistryLock;
pub use publish::{
    RegistryPublishOutcome, RegistryPublishRequest, publish_to_registry, validate_publish,
};
