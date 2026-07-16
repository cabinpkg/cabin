use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by the file-registry layer.
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("failed to read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid file registry at {}: {message}", path.display())]
    InvalidConfig { path: PathBuf, message: String },

    #[error("failed to parse registry config at {}: {source}", path.display())]
    ConfigJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid package index for {name:?}: name field is {actual_name:?}")]
    PackageIndexNameMismatch { name: String, actual_name: String },

    #[error("failed to parse package index at {}: {source}", path.display())]
    PackageIndexJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid package index at {}: unsupported schema version {schema}", path.display())]
    PackageIndexUnsupportedSchema { path: PathBuf, schema: u32 },

    #[error("invalid package index at {}: {message}", path.display())]
    PackageIndexInvalid { path: PathBuf, message: String },

    #[error("package `{name} {version}` already exists in the file registry")]
    DuplicateVersion { name: String, version: String },

    #[error(
        "artifact already exists for `{name} {version}` but the package index does not contain that version"
    )]
    OrphanedArtifact { name: String, version: String },

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

    #[error(
        "registry packages must be named `<scope>/<name>`; `{name}` is a bare name and cannot be published"
    )]
    BarePackageName { name: String },

    #[error(
        "staged package name `{staged}` does not match its metadata name `{metadata}`; refusing to write an index document that disagrees with its location"
    )]
    StagedMetadataNameMismatch { staged: String, metadata: String },
}
