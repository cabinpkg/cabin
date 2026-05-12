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

    #[error("file registry is locked by another process")]
    Locked,

    #[error("failed to render package index as JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error(
        "package name `{name}` is not valid; package names must consist only of ASCII letters, ASCII digits, `_`, `-`, and `.`, must be non-empty, must not start with `.` or `-`, and must not be `.` or `..`"
    )]
    UnsafePackageName { name: String },
}
