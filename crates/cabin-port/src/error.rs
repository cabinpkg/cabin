use std::io;
use std::path::{Path, PathBuf};

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
        "port descriptor at {} declares an unsafe `patches` entry `{value}`; expected `patches/<file>` inside the port directory",
        path.display()
    )]
    UnsafePatchPath { path: PathBuf, value: String },

    #[error("port descriptor at {} declares a conflicting patch plan: {source}", path.display())]
    InvalidPatchPlan {
        path: PathBuf,
        #[source]
        source: cabin_core::UpstreamError,
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

    #[error("patch file for port `{name} {version}` was not found at {}", path.display())]
    MissingPatchFile {
        name: String,
        version: String,
        path: PathBuf,
    },

    #[error(
        "patch `{path}` for port `{name} {version}` shadows a file already in the prepared tree; \
         a patch file must not name a path the upstream archive, a `[[copy]]` step, or another \
         patch produces (the registry verifier rejects such a version)"
    )]
    PatchShadowsTree {
        name: String,
        version: String,
        path: camino::Utf8PathBuf,
    },

    #[error(
        "patch file for port `{name} {version}` at {} is {size} bytes; at most {limit} are supported",
        path.display()
    )]
    PatchTooLarge {
        name: String,
        version: String,
        path: PathBuf,
        size: usize,
        limit: usize,
    },

    #[error("failed to apply patches for port `{name} {version}`: {source}")]
    PatchApply {
        name: String,
        version: String,
        #[source]
        source: Box<cabin_artifact::PatchError>,
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

/// Crate-internal sugar for the ubiquitous "map an `io::Error` into
/// [`PortError::Fs`] with the path that triggered it" pattern.
pub(crate) trait FsResultExt<T> {
    fn with_path(self, path: &Path) -> Result<T, PortError>;
}

impl<T> FsResultExt<T> for Result<T, io::Error> {
    fn with_path(self, path: &Path) -> Result<T, PortError> {
        self.map_err(|source| PortError::Fs {
            path: path.to_path_buf(),
            source,
        })
    }
}
