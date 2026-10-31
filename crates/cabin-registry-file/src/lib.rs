//! Local file-registry layout, package-index mutation, and
//! atomic publish for Cabin.
//!
//! Layout with the default `config.json` subdirectories; `cabin-index`
//! reads the same shape:
//!
//! ```text
//! <registry>/
//! config.json
//! packages/<scope>/<name>.json
//! artifacts/<scope>/<name>/<scope>-<name>-<version>-<revision>.zip
//! ```
//!
//! A legacy bare name has no `<scope>/` directory and no `<scope>-`
//! filename prefix.
//!
//! This crate owns the layout, the package-index file format, the
//! atomic write helpers that keep partially-written state from
//! sticking around, and an OS advisory lock on `.cabin-registry.lock`
//! so concurrent `cabin publish --registry-dir` invocations are
//! detected.
//!
//! Crate boundaries:
//! - this crate must not implement HTTP / sparse / OCI publish;
//! - it must not implement server-side functionality;
//! - it must not run the resolver, parse arbitrary `cabin.toml`s, or
//!   build packages - `cabin-package` produces the
//!   [`cabin_package::StagedPackage`] this crate consumes;
//! - `cabin publish` orchestration lives in `cabin-publish`, which
//!   combines staging with this crate's writers.

#![allow(
    // `field: Default::default()` in the test-fixture builders reads
    // fine and clippy's per-field type suggestion is only MaybeIncorrect.
    clippy::default_trait_access
)]

mod atomic;
pub mod error;
pub mod index;
pub mod layout;
pub mod lock;
pub mod publish;

pub use error::RegistryError;
pub use index::{PACKAGE_INDEX_SCHEMA, read_published_standards};
pub use layout::{FileRegistry, REGISTRY_CONFIG_FILENAME, RegistryConfig};
pub use lock::RegistryLock;
pub use publish::{
    RegistryPublishOutcome, RegistryPublishRequest, publish_to_registry, validate_publish,
};
