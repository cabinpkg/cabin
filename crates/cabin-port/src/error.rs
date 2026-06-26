use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Errors surfaced by the foundation-port layer.
///
/// Messages are written to be useful as direct CLI output: they
/// identify the port by name + version where relevant, and the
/// failure mode in language a user can act on.
#[derive(Debug, Error)]
pub enum PortError {
    #[error("failed to read port descriptor at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse port descriptor at {}: {source}", path.display())]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error(
        "port descriptor at {} declares unsupported source type `{kind}`; foundation ports require a pinned archive source with SHA-256",
        path.display()
    )]
    UnsupportedSourceType { path: PathBuf, kind: String },

    #[error(
        "port descriptor at {} is missing `[source].sha256`; foundation ports require a 64-character lowercase hex SHA-256",
        path.display()
    )]
    MissingChecksum { path: PathBuf },

    #[error(
        "port descriptor at {} declares an invalid SHA-256 ({value:?}); expected 64 lowercase hex characters",
        path.display()
    )]
    InvalidChecksum { path: PathBuf, value: String },

    #[error(
        "port descriptor at {} declares an invalid `{field}` URL ({value:?}): {message}",
        path.display()
    )]
    InvalidUrl {
        path: PathBuf,
        field: &'static str,
        value: String,
        message: String,
    },

    #[error("port descriptor at {} declares an invalid `{field}`: {message}", path.display())]
    InvalidField {
        path: PathBuf,
        field: &'static str,
        message: String,
    },

    #[error(
        "port descriptor at {} declares an unsafe overlay manifest path `{value}`; expected a relative path inside the port directory",
        path.display()
    )]
    UnsafeOverlayPath { path: PathBuf, value: String },

    #[error(
        "port descriptor at {} declares an unsafe `[[copy]]` `{field}` path `{value}`; expected a relative path inside the extracted source",
        path.display()
    )]
    UnsafeCopyPath {
        path: PathBuf,
        field: &'static str,
        value: String,
    },

    #[error(
        "checksum mismatch for port `{name} {version}`: expected sha256:{expected}, got sha256:{actual}"
    )]
    ChecksumMismatch {
        name: String,
        version: String,
        expected: String,
        actual: String,
    },

    #[error(
        "source archive for port `{name} {version}` does not contain the declared strip_prefix directory `{strip_prefix}`"
    )]
    MissingStripPrefix {
        name: String,
        version: String,
        strip_prefix: String,
    },

    #[error("overlay manifest for port `{name} {version}` was not found at {}", path.display())]
    MissingOverlayManifest {
        name: String,
        version: String,
        path: PathBuf,
    },

    #[error(
        "port `{name} {version}` declares a `[[copy]]` whose source file is missing from the extracted archive at {}",
        path.display()
    )]
    MissingCopySource {
        name: String,
        version: String,
        path: PathBuf,
    },

    #[error(
        "overlay manifest for port `{name} {version}` declares package `{actual_name} {actual_version}`; expected to match the port identity"
    )]
    OverlayIdentityMismatch {
        name: String,
        version: String,
        actual_name: String,
        actual_version: String,
    },

    #[error(
        "overlay manifest for port `{name} {version}` has no `[package]` table; expected `name = \"{name}\", version = \"{version}\"`"
    )]
    OverlayMissingPackage { name: String, version: String },

    #[error("source archive for port `{name} {version}` does not exist: {}", path.display())]
    MissingArchive {
        name: String,
        version: String,
        path: PathBuf,
    },

    #[error("failed to parse overlay manifest for port `{name} {version}`: {source}")]
    OverlayManifestParse {
        name: String,
        version: String,
        #[source]
        source: Box<cabin_manifest::ManifestError>,
    },

    #[error("failed to extract port `{name} {version}` archive: {source}")]
    Extract {
        name: String,
        version: String,
        #[source]
        source: Box<cabin_artifact::ArtifactError>,
    },

    #[error(
        "cannot prepare port `{name} {version}` because --frozen was specified and the port is not cached"
    )]
    FrozenCacheMiss { name: String, version: String },

    /// `--offline` was set and the port archive was not in the
    /// cache, so no download could be attempted.  Distinguished
    /// from [`PortError::FrozenCacheMiss`] so callers can decide
    /// whether to surface or silently skip the port (e.g. read-only
    /// metadata commands degrade gracefully on a fresh checkout).
    #[error(
        "cannot download port `{name} {version}` from {url} because --offline was specified; rerun without --offline or vendor the archive locally"
    )]
    OfflineCacheMiss {
        name: String,
        version: String,
        url: String,
    },

    #[error("filesystem error at {}: {source}", path.display())]
    Fs {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(
        "no bundled foundation port named `{name}`; run `cabin port list` to see available names"
    )]
    UnknownBuiltin { name: String },

    /// `port = true` named a bundled port whose available versions
    /// do not satisfy the requested requirement. `available` is
    /// non-empty by construction - the empty case is reported as
    /// `PortError::UnknownBuiltin` for a clearer diagnostic.
    #[error(
        "no bundled foundation port `{name}` satisfies `{requirement}` (available: {})",
        available.join(", ")
    )]
    BuiltinVersionNotFound {
        name: String,
        requirement: String,
        available: Vec<String>,
    },
}
