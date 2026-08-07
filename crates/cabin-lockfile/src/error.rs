use std::io;
use std::path::PathBuf;

use cabin_core::ValidationError;
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

    #[error(
        "locked package {name:?} version {value:?} carries build metadata; registry versions are plain upstream versions, so this lockfile predates the packaging-revision model - delete it and re-run `cabin resolve`"
    )]
    VersionBuildMetadata { name: String, value: String },

    /// A package name failed [`cabin_core::PackageName`] validation.
    /// The typed failure is rendered inline (deliberately not
    /// `#[source]`) so the top-level message the CLI prints stays a
    /// single line.
    #[error("invalid cabin.lock package name {name:?}: {reason}")]
    InvalidPackageName {
        name: String,
        reason: ValidationError,
    },

    #[error(
        "unknown source {value:?} for cabin.lock package {name:?}; only \"index\" is supported"
    )]
    UnknownSource { name: String, value: String },

    /// Rendered through the shared [`cabin_core::ChecksumError`]
    /// sentence so the checksum grammar has exactly one normative
    /// wording.  The typed failure is inlined (deliberately not a
    /// `source` chain, like [`LockfileError::InvalidPackageName`]) so
    /// the top-level message stays a single line ending in the
    /// recovery step.
    #[error(
        "invalid cabin.lock package {name:?}: {reason} - delete the lockfile and re-run `cabin resolve`"
    )]
    InvalidChecksum {
        name: String,
        reason: cabin_core::ChecksumError,
    },

    /// A `[[patch]]` entry's `kind` field carried a value other
    /// than [`crate::model::PATCH_KIND_PATH`].
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
