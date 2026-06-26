//! Publish-workflow orchestration for Cabin.
//!
//! Two paths share a single staging step:
//!
//! - [`dry_run()`] / [`DryRunRequest`] stage the package and write
//!   The archive + canonical metadata to an output directory
//!   Without touching any registry.
//! - [`publish_to_file_registry`] /
//!   [`dry_run_against_file_registry`] call into
//!   `cabin-registry-file` to mutate (or validate without
//!   Mutating) a local file registry.
//!
//! Crate boundaries:
//! - this crate must not implement HTTP / sparse / OCI publish;
//! - it must not implement server-side functionality;
//! - file-registry layout, atomic-ish writes, and the lock file all
//!   Live in `cabin-registry-file`;
//! - this crate is the layer where staging meets writing.  Nothing
//!   Higher-level (CLI flag handling, output formatting) belongs
//!   Here.

// `PublishError` aggregates package, registry-file, and dry-run
// errors.  The union crosses clippy's default
// `result_large_err` threshold once `cabin_package::PackageError`
// (which flows in via `?`) gains its own larger variants.
// Boxing the enum at every call site would be churny; we accept
// the larger `Result` instead.

pub mod dry_run;
pub mod error;
pub mod registry;

pub use dry_run::{DryRunReport, DryRunRequest, dry_run};
pub use error::PublishError;
pub use registry::{
    RegistryPublishReport, RegistryPublishWorkflow, dry_run_against_file_registry,
    publish_to_file_registry,
};
