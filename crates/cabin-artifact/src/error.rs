use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by the artifact layer.
///
/// Messages are written to be useful as direct CLI output: they identify
/// the package by name + version where relevant, and the failure mode in
/// language a user can act on.
#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("package `{name} {version}` has no source artifact in the index")]
    MissingSource { name: String, version: String },

    #[error(
        "missing checksum for `{name} {version}`; cabin fetch requires a sha256:<hex> entry in the index"
    )]
    MissingChecksum { name: String, version: String },

    #[error(
        "invalid checksum {value:?} for `{name} {version}`: must be of the form sha256:<64 hex chars>"
    )]
    InvalidChecksum {
        name: String,
        version: String,
        value: String,
    },

    #[error(
        "source archive for `{name} {version}` does not exist: {}",
        path.display()
    )]
    MissingArchive {
        name: String,
        version: String,
        path: PathBuf,
    },

    #[error(
        "checksum mismatch for `{name} {version}`: expected sha256:{expected}, got sha256:{actual}"
    )]
    ChecksumMismatch {
        name: String,
        version: String,
        expected: String,
        actual: String,
    },

    #[error("refusing to extract unsafe archive entry `{0}`")]
    UnsafeArchiveEntry(String),

    #[error("refusing to extract unsupported archive entry `{0}`")]
    UnsupportedArchiveEntry(String),

    #[error(
        "refusing to extract archive entry `{path}`: decompressed size exceeds the {limit}-byte per-entry limit (potential decompression bomb)"
    )]
    ArchiveEntryTooLarge { path: String, limit: u64 },

    #[error(
        "refusing to extract archive: total decompressed size would exceed the {limit}-byte limit (potential decompression bomb)"
    )]
    ArchiveTooLarge { limit: u64 },

    #[error(
        "refusing to extract archive: entry count exceeds the {limit} limit (potential decompression bomb)"
    )]
    ArchiveTooManyEntries { limit: usize },

    #[error("source archive for `{name} {version}` does not contain cabin.toml at its root")]
    MissingArchiveManifest { name: String, version: String },

    #[error(
        "source archive for `{name} {version}` contains package `{actual_name} {actual_version}`"
    )]
    ManifestMismatch {
        name: String,
        version: String,
        actual_name: String,
        actual_version: String,
    },

    #[error(
        "cannot fetch artifact for `{name} {version}` because --frozen was specified and the artifact is not cached"
    )]
    FrozenCacheMiss { name: String, version: String },

    #[error("failed to read {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to extract archive {}: {source}", path.display())]
    Extract {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(
        "failed to parse extracted manifest at {}: {source}",
        path.display()
    )]
    Manifest {
        path: PathBuf,
        #[source]
        source: Box<cabin_manifest::ManifestError>,
    },
}
