use std::io;
use std::path::PathBuf;

use cabin_core::escape_control_chars as escape;
use thiserror::Error;

/// Errors produced by the artifact layer.
///
/// Messages are written to be useful as direct CLI output: they identify
/// the package by name + version where relevant, and the failure mode in
/// language a user can act on.
#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error(
        "source archive for `{name} {version}` is not a file inside the registry root: {}",
        path.display()
    )]
    MissingArchive {
        name: String,
        version: String,
        path: PathBuf,
    },

    /// The digest the bytes do hash to is deliberately not reported: a
    /// local index chooses which file is hashed, and naming its digest
    /// would disclose it.
    #[error(
        "checksum mismatch for `{name} {version}`: the archive does not hash to sha256:{expected}"
    )]
    ChecksumMismatch {
        name: String,
        version: String,
        expected: String,
    },

    #[error("refusing to extract unsafe archive entry `{}`", escape(.0))]
    UnsafeArchiveEntry(String),

    #[error(
        "source archive does not contain the declared strip_prefix directory `{}`",
        escape(strip_prefix)
    )]
    MissingStripPrefix { strip_prefix: String },

    #[error("refusing to extract unsupported archive entry `{}`", escape(.0))]
    UnsupportedArchiveEntry(String),

    #[error(
        "refusing to extract archive entry `{}`: its path exceeds the {limit}-byte length limit",
        escape(path)
    )]
    ArchiveEntryPathTooLong { path: String, limit: usize },

    #[error("refusing to extract duplicate archive entry `{}`", escape(.0))]
    DuplicateArchiveEntry(String),

    #[error(
        "refusing to extract archive entry `{}`: `{}` cannot be both a regular file and a directory",
        escape(path),
        escape(conflict)
    )]
    ConflictingArchiveEntry { path: String, conflict: String },

    #[error(
        "refusing to extract archive entry `{}`: its header declares {expected} bytes but the archive holds {actual}",
        escape(path)
    )]
    ArchiveEntryTruncated {
        path: String,
        expected: u64,
        actual: u64,
    },

    #[error(
        "refusing to extract archive: decompressed stream exceeds the {cap}-byte cap derived from its compressed size (potential decompression bomb)"
    )]
    ArchiveStreamTooLarge { cap: u64 },

    #[error(
        "refusing to extract archive: tar headers and metadata records exceed the {limit}-byte limit"
    )]
    ArchiveMetadataTooLarge { limit: u64 },

    #[error(
        "refusing to extract archive: its {size}-byte size exceeds the {limit}-byte limit (its metadata would not fit in bounded memory)"
    )]
    ArchiveFileTooLarge { size: u64, limit: u64 },

    #[error(
        "refusing to extract archive: two central-directory entries share a name (a zip reader would keep only one, so which bytes land is ambiguous)"
    )]
    ArchiveDuplicateNames,

    #[error(
        "refusing to extract archive entry `{}`: decompressed size exceeds the {limit}-byte per-entry limit (potential decompression bomb)",
        escape(path)
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

    #[error("failed to read {}: {source}", escape(&path.display().to_string()))]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The parser's message is kept as text rather than as a `#[source]`:
    /// tar and zip readers quote entry names in it, and a source would be
    /// re-appended verbatim by the CLI's error-chain rendering.
    #[error("failed to extract archive {}: {}", path.display(), escape(detail))]
    Extract { path: PathBuf, detail: String },

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Archive authors control entry names; a name carrying terminal
    /// escapes or bidi controls must reach the terminal escaped.
    #[test]
    fn archive_entry_names_are_escaped_in_messages() {
        let hostile = "ok\u{1b}[31m\u{202e}evil";
        let errors = [
            ArtifactError::UnsafeArchiveEntry(hostile.into()),
            ArtifactError::MissingStripPrefix {
                strip_prefix: hostile.into(),
            },
            ArtifactError::UnsupportedArchiveEntry(hostile.into()),
            ArtifactError::ArchiveEntryPathTooLong {
                path: hostile.into(),
                limit: 1,
            },
            ArtifactError::DuplicateArchiveEntry(hostile.into()),
            ArtifactError::ConflictingArchiveEntry {
                path: hostile.into(),
                conflict: hostile.into(),
            },
            ArtifactError::ArchiveEntryTruncated {
                path: hostile.into(),
                expected: 1,
                actual: 0,
            },
            ArtifactError::ArchiveEntryTooLarge {
                path: hostile.into(),
                limit: 1,
            },
            ArtifactError::Io {
                path: PathBuf::from(hostile),
                source: io::Error::other("disk"),
            },
            ArtifactError::Extract {
                path: PathBuf::from("pkg.zip"),
                detail: hostile.into(),
            },
        ];
        for err in errors {
            let message = err.to_string();
            assert!(
                !message.contains('\u{1b}') && !message.contains('\u{202e}'),
                "{message:?}"
            );
            assert!(
                message.contains("ok\\u{1b}[31m\\u{202e}evil"),
                "{message:?}"
            );
        }
    }
}
