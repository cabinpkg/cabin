use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Errors produced while reading, parsing, or writing a `cabin.lock`.
#[derive(Debug, Error)]
pub enum LockfileError {
    #[error("failed to read {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse cabin.lock: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("failed to serialize cabin.lock: {0}")]
    TomlSer(#[from] std::fmt::Error),

    #[error("unsupported cabin.lock version {version}; expected {expected}")]
    UnsupportedVersion { version: u32, expected: u32 },

    #[error("duplicate package entry in cabin.lock: {name:?}")]
    DuplicatePackage { name: String },

    #[error(
        "invalid cabin.lock package {name:?}: version {value:?} is not valid SemVer ({source})"
    )]
    InvalidVersion {
        name: String,
        value: String,
        #[source]
        source: semver::Error,
    },

    #[error("invalid cabin.lock package name {name:?}: {message}")]
    InvalidPackageName { name: String, message: String },

    #[error(
        "unknown source {value:?} for cabin.lock package {name:?}; only \"index\" is supported"
    )]
    UnknownSource { name: String, value: String },

    /// A `[[patch]]` entry's `kind` field carried an unsupported
    /// value.  Mirrors the closed [`crate::model::LockedPatchKind`]
    /// enum.
    #[error(
        "unknown cabin.lock patch kind {value:?} for package {package:?}; supported kinds are: path"
    )]
    UnknownPatchKind { package: String, value: String },

    /// A `[[source-replacement]]` entry's `original-kind` /
    /// `replacement-kind` field carried an unsupported value.
    #[error(
        "unknown cabin.lock source locator kind {value:?}; supported kinds are: index-path, index-url"
    )]
    UnknownSourceLocatorKind { value: String },
}
