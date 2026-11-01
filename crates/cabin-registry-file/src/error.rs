use std::io;
use std::path::PathBuf;

use cabin_core::escape_control_chars;
use thiserror::Error;

/// Errors produced by the file-registry layer.
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("failed to access {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid file registry at {}: {message}", path.display())]
    InvalidConfig { path: PathBuf, message: String },

    /// serde quotes the offending key, so whoever wrote the registry files
    /// chose part of this text; it is escaped and deliberately not a
    /// `#[source]` - see [`escape_control_chars`].
    #[error(
        "failed to parse registry config at {}: {}",
        path.display(),
        escape_control_chars(&error.to_string()),
    )]
    ConfigJson {
        path: PathBuf,
        error: serde_json::Error,
    },

    #[error("invalid package index for {name:?}: name field is {actual_name:?}")]
    PackageIndexNameMismatch { name: String, actual_name: String },

    /// Escaped and not a `#[source]`, like [`RegistryError::ConfigJson`].
    #[error(
        "failed to parse package index at {}: {}",
        path.display(),
        escape_control_chars(&error.to_string()),
    )]
    PackageIndexJson {
        path: PathBuf,
        error: serde_json::Error,
    },

    #[error("invalid package index at {}: unsupported schema version {schema}", path.display())]
    PackageIndexUnsupportedSchema { path: PathBuf, schema: u32 },

    #[error("invalid package index at {}: {message}", path.display())]
    PackageIndexInvalid { path: PathBuf, message: String },

    #[error(
        "`{name} {version}` is already published to this registry with different bytes; published revisions are immutable - pass `--new-revision` to publish the changed bytes as a new packaging revision of the same version"
    )]
    NewRevisionRequiresOptIn { name: String, version: String },

    #[error(
        "packaging revision `{revision}` of `{name} {version}` already exists with a different checksum; two archives whose digests share a revision id cannot coexist"
    )]
    RevisionCollision {
        name: String,
        version: String,
        revision: String,
    },

    #[error(
        "artifact already exists for `{name} {version}` (revision `{revision}`) but the package index does not record that revision"
    )]
    OrphanedArtifact {
        name: String,
        version: String,
        revision: String,
    },

    #[error(
        "{index_error}; additionally, rolling back the just-written artifact `{}` failed ({cleanup}); remove the file manually before retrying, otherwise the next publish reports an orphaned artifact",
        artifact_path.display()
    )]
    PublishRollback {
        index_error: Box<RegistryError>,
        artifact_path: PathBuf,
        cleanup: io::Error,
    },

    #[error("file registry is locked by another process")]
    Locked,

    #[error("failed to render package index as JSON: {0}")]
    Json(#[from] serde_json::Error),
}
