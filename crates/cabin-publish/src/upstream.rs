//! The canonical staging step for a committed package that declares
//! `[package.upstream]`: resolve its pinned archive through a
//! content-addressed cache, materialize it through
//! [`cabin_artifact::materialize_upstream`] (the pipeline the registry
//! verifier replays), lay the committed patch files on top, and write
//! the committed manifest last.  The result is an ordinary package
//! directory, ready for [`crate::publish_to_file_registry`] or
//! `cabin publish`.
//!
//! The foundation-port publisher and Cabin's own `cabin-ports/*`
//! registry fixtures both stage through this module, so the fixtures
//! are byte-identical to what the publisher stages.  Port discovery,
//! the `cabin-ports` naming policy, publication order, and preflight
//! probes belong to the publisher, not here.
//!
//! Only the archive download uses the network, through
//! [`cabin_index_http::HttpClient`]; its integrity comes from the
//! declared SHA-256, never from the transport.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cabin_core::{PackageName, UpstreamFormat, UpstreamProvenance};
use camino::Utf8Path;
use thiserror::Error;

use crate::{PublishError, RegistryPublishReport, RegistryPublishWorkflow};

/// One committed package directory and the declarations staging it
/// reads.
#[derive(Debug, Clone, Copy)]
pub struct UpstreamPackage<'a> {
    pub name: &'a PackageName,
    pub version: &'a semver::Version,
    /// The declared provenance the shared materializer runs.
    pub upstream: &'a UpstreamProvenance,
    /// The committed package directory; declared patches are read
    /// from it.
    pub committed_dir: &'a Path,
    /// The manifest that publishes, verbatim.
    pub manifest: &'a str,
}

/// Where [`resolve_archive`] may get a package's pinned archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFetch {
    /// Reuse a cached archive, downloading on a miss.
    CacheOrDownload,
    /// Reuse a cached archive only, failing on a miss.  Hermetic test
    /// fixtures pre-seed the cache and rely on this so a checksum or
    /// cache-layout regression surfaces as an error instead of a
    /// network attempt against the (unreachable) upstream pin.
    CacheOnly,
}

/// A staging failure.
#[derive(Debug, Error)]
pub enum UpstreamStageError {
    #[error("{action} {}: {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },

    #[error(
        "no cached archive for {package} at {} and downloads are disabled; seed the port cache \
         before staging",
        path.display()
    )]
    CacheMiss { package: String, path: PathBuf },

    #[error("failed to download {url}: {message}")]
    Download { url: String, message: String },

    #[error("downloaded archive for {label} hashes to {actual}, expected {expected}")]
    ChecksumMismatch {
        label: String,
        actual: String,
        expected: String,
    },

    #[error(
        "package directory {} already exists; stage into a fresh sources directory",
        path.display()
    )]
    PackageDirExists { path: PathBuf },

    #[error(
        "declared patch path {} is (or crosses) a symlink; commit regular files",
        path.display()
    )]
    SymlinkedPatch { path: PathBuf },

    #[error("materializing {package}: {source}")]
    Materialize {
        package: String,
        source: cabin_artifact::MaterializeError,
    },

    #[error("{}: {message}", path.display())]
    Manifest { path: PathBuf, message: String },

    #[error("publishing {package} into the file registry: {source}")]
    Publish {
        package: String,
        source: PublishError,
    },
}

impl UpstreamPackage<'_> {
    fn label(&self) -> String {
        format!("{} {}", self.name.as_str(), self.version)
    }
}

/// The pinned archive for `package`, cache-first: a cached archive
/// whose bytes hash to the declared SHA-256 is reused, otherwise it
/// downloads into the shared content-addressed slot under
/// `archive_cache` (`archives/sha256/<hex>.<format>`).
///
/// # Errors
/// A cache miss under [`ArchiveFetch::CacheOnly`], a failed download
/// or a downloaded archive that does not hash to the pin, and cache
/// I/O failures.
pub fn resolve_archive(
    package: &UpstreamPackage<'_>,
    archive_cache: &Path,
    fetch: ArchiveFetch,
) -> Result<PathBuf, UpstreamStageError> {
    let upstream = package.upstream;
    let expected_hex = upstream.checksum().hex();
    let cached = cached_archive_path(archive_cache, expected_hex, upstream.format());
    if archive_matches(&cached, expected_hex)? {
        return Ok(cached);
    }
    if fetch == ArchiveFetch::CacheOnly {
        return Err(UpstreamStageError::CacheMiss {
            package: package.label(),
            path: cached,
        });
    }
    let client = cabin_index_http::HttpClient::with_redirect_budget(5);
    let url = upstream.url().as_str();
    let label = format!("{}-{}", package.name.base_name(), package.version);
    let bytes = client
        .download(url, &label)
        .map_err(|err| UpstreamStageError::Download {
            url: url.to_owned(),
            message: err.to_string(),
        })?;
    eprintln!("fetched {url} ({} bytes)", bytes.len());
    let actual = cabin_core::hash::hash_reader(bytes.as_slice())
        .map_err(|source| io_error("hashing the download of", Path::new(url), source))?;
    if actual != expected_hex {
        return Err(UpstreamStageError::ChecksumMismatch {
            label,
            actual,
            expected: expected_hex.to_owned(),
        });
    }
    if let Some(parent) = cached.parent() {
        fs::create_dir_all(parent).map_err(|source| io_error("creating", parent, source))?;
    }
    // Atomic sibling-rename write, so a concurrent reader never sees
    // a partial archive at the content-addressed slot.  A checksum-
    // invalid entry may still occupy the slot (that is why this is a
    // miss); the rename replaces it, so cache corruption self-heals.
    let staging = cached.with_extension("part");
    fs::write(&staging, &bytes).map_err(|source| io_error("writing", &staging, source))?;
    fs::rename(&staging, &cached).map_err(|source| io_error("moving into", &cached, source))?;
    Ok(cached)
}

/// Materialize `package` from `archive` into `package_dir`, which
/// must not exist yet, and write the committed manifest last.  The
/// committed directory is never mutated.
///
/// # Errors
/// A pre-existing `package_dir`, a symlinked declared patch, a
/// materialization defect, and I/O failures.
pub fn materialize(
    package: &UpstreamPackage<'_>,
    archive: &Path,
    package_dir: &Path,
) -> Result<(), UpstreamStageError> {
    // Materialization extracts on top of this directory without ever
    // clearing it, so a directory left by a previous run could
    // smuggle stale files into the packaged archive.
    if package_dir.exists() {
        return Err(UpstreamStageError::PackageDirExists {
            path: package_dir.to_path_buf(),
        });
    }
    fs::create_dir_all(package_dir).map_err(|source| io_error("creating", package_dir, source))?;

    // Patch bytes come from the committed package directory - the
    // same files the packaged archive will carry.
    let mut fetch_patch = |path: &Utf8Path| {
        let committed = committed_patch_path(package.committed_dir, path).map_err(|err| {
            cabin_artifact::MaterializeError::Io {
                path: package.committed_dir.join(path.as_std_path()),
                source: io::Error::other(err.to_string()),
            }
        })?;
        match fs::read(&committed) {
            Ok(bytes) => Ok(cabin_artifact::PatchFetch::Found(bytes)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                Ok(cabin_artifact::PatchFetch::Missing)
            }
            Err(source) => Err(cabin_artifact::MaterializeError::Io {
                path: committed,
                source,
            }),
        }
    };
    cabin_artifact::materialize_upstream(package.upstream, archive, package_dir, &mut fetch_patch)
        .map_err(|source| UpstreamStageError::Materialize {
            package: package.label(),
            source,
        })?;

    // Place the declared patch files at their declared paths - the
    // published archive carries them, and the shared pipeline already
    // proved none shadows a produced file.
    for path in package.upstream.patches() {
        let committed = committed_patch_path(package.committed_dir, path)?;
        let placed = package_dir.join(path.as_std_path());
        if let Some(parent) = placed.parent() {
            fs::create_dir_all(parent).map_err(|source| io_error("creating", parent, source))?;
        }
        fs::copy(&committed, &placed).map_err(|source| io_error("placing", &placed, source))?;
    }

    // Written last: the upstream archive extracts on top of
    // `package_dir`, so an upstream shipping its own root `cabin.toml`
    // would otherwise overwrite the committed manifest and publish in
    // its place.
    let manifest_path = package_dir.join("cabin.toml");
    fs::write(&manifest_path, package.manifest)
        .map_err(|source| io_error("writing the manifest into", package_dir, source))
}

/// Stage the committed package at `committed_dir` into the file
/// registry at `registry_dir`: [`resolve_archive`] under
/// `archive_cache`, [`materialize`] into
/// `<sources_dir>/<base name>/<version>`, then publish that directory.
/// Returns the staged directory and the publish report.
///
/// # Errors
/// An unreadable or unparsable manifest, one without a `[package]`
/// table or `[package.upstream]` provenance, and every error of the
/// steps it runs.
pub fn stage_committed_package(
    committed_dir: &Path,
    archive_cache: &Path,
    sources_dir: &Path,
    registry_dir: &Path,
    fetch: ArchiveFetch,
) -> Result<(PathBuf, RegistryPublishReport), UpstreamStageError> {
    let manifest_path = committed_dir.join("cabin.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .map_err(|source| io_error("reading", &manifest_path, source))?;
    let invalid = |message: String| UpstreamStageError::Manifest {
        path: manifest_path.clone(),
        message,
    };
    let parsed =
        cabin_manifest::parse_manifest_str(&manifest).map_err(|err| invalid(err.to_string()))?;
    let declared = parsed
        .package
        .ok_or_else(|| invalid("no [package] table".to_owned()))?;
    let upstream = declared
        .upstream
        .as_ref()
        .ok_or_else(|| invalid("no [package.upstream] provenance".to_owned()))?;
    let package = UpstreamPackage {
        name: &declared.name,
        version: &declared.version,
        upstream,
        committed_dir,
        manifest: &manifest,
    };
    let archive = resolve_archive(&package, archive_cache, fetch)?;
    let package_dir = sources_dir
        .join(package.name.base_name())
        .join(package.version.to_string());
    materialize(&package, &archive, &package_dir)?;
    let report = crate::publish_to_file_registry(RegistryPublishWorkflow {
        manifest_path: &package_dir.join("cabin.toml"),
        registry_dir,
        resolved_project: None,
        workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        // Every caller stages into a fresh registry, so every publish
        // is a first revision.
        new_revision: false,
    })
    .map_err(|source| UpstreamStageError::Publish {
        package: package.label(),
        source,
    })?;
    Ok((package_dir, report))
}

fn io_error(action: &'static str, path: &Path, source: io::Error) -> UpstreamStageError {
    UpstreamStageError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

/// Resolve a declared patch path inside the committed package
/// directory, refusing symlinks on every component below it - the
/// bytes entering the published archive must be the committed
/// regular files themselves, never something a link points at.
/// Declared patch entries are exempt from the verifier's tree
/// comparison, so a followed link would smuggle uncommitted
/// host-local bytes into a verified package.
fn committed_patch_path(
    committed_dir: &Path,
    path: &Utf8Path,
) -> Result<PathBuf, UpstreamStageError> {
    let mut current = committed_dir.to_path_buf();
    for component in path.as_str().split('/') {
        current.push(component);
        if let Ok(metadata) = fs::symlink_metadata(&current)
            && metadata.file_type().is_symlink()
        {
            return Err(UpstreamStageError::SymlinkedPatch { path: current });
        }
    }
    Ok(current)
}

/// Content-addressed slot for a pinned upstream archive, keyed by the
/// declared SHA-256 and the declared format's manifest spelling.
/// This layout is distinct from `cabin-artifact`'s registry archive
/// cache, and an ordinary build never fetches an `[package.upstream]`
/// archive at all.  Keeping it stable lets repeat runs - and the
/// hermetic test fixtures that pre-seed it - reuse a download.
fn cached_archive_path(archive_cache: &Path, hex: &str, format: UpstreamFormat) -> PathBuf {
    archive_cache
        .join("archives")
        .join("sha256")
        .join(format!("{hex}.{}", format.as_str()))
}

/// `Ok(true)` when a cached archive exists and hashes to
/// `expected_hex`; a missing file is a clean miss, any other read
/// failure surfaces.
fn archive_matches(path: &Path, expected_hex: &str) -> Result<bool, UpstreamStageError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(io_error("opening cached archive", path, source)),
    };
    let actual = cabin_core::hash::hash_reader(file)
        .map_err(|source| io_error("hashing cached archive at", path, source))?;
    Ok(actual == expected_hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance(hex: &str, strip_prefix: Option<&str>) -> UpstreamProvenance {
        UpstreamProvenance::new(
            "https://ports.invalid/zlib-1.3.1.tar.gz",
            &format!("sha256:{hex}"),
            "tar.gz",
            strip_prefix.map(str::to_owned),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
    }

    fn zlib<'a>(
        name: &'a PackageName,
        version: &'a semver::Version,
        upstream: &'a UpstreamProvenance,
        committed_dir: &'a Path,
        manifest: &'a str,
    ) -> UpstreamPackage<'a> {
        UpstreamPackage {
            name,
            version,
            upstream,
            committed_dir,
            manifest,
        }
    }

    /// Build a gzipped tarball from `(path, contents)` entries and
    /// return `(bytes, sha256_hex)`.
    fn tar_gz(entries: &[(&str, &str)]) -> (Vec<u8>, String) {
        use sha2::Digest as _;
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (rel, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(&mut header, rel, body.as_bytes())
                .unwrap();
        }
        let bytes = builder.into_inner().unwrap().finish().unwrap();
        let hex = cabin_core::hash::hex_digest(&sha2::Sha256::digest(&bytes));
        (bytes, hex)
    }

    /// The committed-patch resolver is the only barrier between a
    /// symlinked patch file and the published archive.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_committed_patch_is_refused() {
        let dir = assert_fs::TempDir::new().unwrap();
        let package_dir = dir.path().join("ports/zlib/1.3.1");
        fs::create_dir_all(package_dir.join("patches")).unwrap();
        fs::write(dir.path().join("outside.patch"), "not committed\n").unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("outside.patch"),
            package_dir.join("patches/0001-fix.patch"),
        )
        .unwrap();

        let err = committed_patch_path(&package_dir, Utf8Path::new("patches/0001-fix.patch"))
            .unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
    }

    /// A cache miss under `CacheOnly` must fail before any network
    /// machinery runs: the fixture pins an RFC 2606 `.invalid` host,
    /// so an attempted download could only surface as a slow
    /// resolution error, never as this message.
    #[test]
    fn cache_only_staging_fails_on_a_cache_miss_without_downloading() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (name, version) = (
            PackageName::new("cabin-ports/zlib").unwrap(),
            semver::Version::new(1, 3, 1),
        );
        let upstream = provenance(&"a".repeat(64), None);
        let package = zlib(&name, &version, &upstream, dir.path(), "");
        let err = resolve_archive(
            &package,
            &dir.path().join("cache/ports"),
            ArchiveFetch::CacheOnly,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("no cached archive for cabin-ports/zlib 1.3.1")
                && message.contains("downloads are disabled"),
            "{message}"
        );
    }

    /// The committed manifest is written *after* materialization, so
    /// an upstream archive shipping its own root `cabin.toml` cannot
    /// displace what publishes.
    #[test]
    fn an_upstream_root_manifest_never_displaces_the_committed_one() {
        const COMMITTED: &str = r#"[package]
name = "cabin-ports/zlib"
version = "1.3.1"
"#;
        let dir = assert_fs::TempDir::new().unwrap();
        let (bytes, hex) = tar_gz(&[
            (
                "zlib-1.3.1/cabin.toml",
                r#"[package]
name = "upstream-decoy"
version = "9.9.9"
"#,
            ),
            ("zlib-1.3.1/zlib.c", "int z(void) { return 0; }\n"),
        ]);
        let cache = dir.path().join("cache/ports");
        let slot = cache.join("archives/sha256").join(format!("{hex}.tar.gz"));
        fs::create_dir_all(slot.parent().unwrap()).unwrap();
        fs::write(&slot, &bytes).unwrap();

        let (name, version) = (
            PackageName::new("cabin-ports/zlib").unwrap(),
            semver::Version::new(1, 3, 1),
        );
        let upstream = provenance(&hex, Some("zlib-1.3.1"));
        let package = zlib(&name, &version, &upstream, dir.path(), COMMITTED);
        let archive = resolve_archive(&package, &cache, ArchiveFetch::CacheOnly).unwrap();
        assert_eq!(archive, slot);
        let package_dir = dir.path().join("src/zlib/1.3.1");
        materialize(&package, &archive, &package_dir).unwrap();
        assert_eq!(
            fs::read_to_string(package_dir.join("cabin.toml")).unwrap(),
            COMMITTED
        );
        assert!(package_dir.join("zlib.c").is_file());
    }

    /// Materialization extracts without clearing, so staging into a
    /// package directory left by a previous run must refuse instead
    /// of packaging stale files.
    #[test]
    fn staging_refuses_a_reused_package_directory() {
        let dir = assert_fs::TempDir::new().unwrap();
        let stale = dir.path().join("src/zlib/1.3.1");
        fs::create_dir_all(&stale).unwrap();
        let (name, version) = (
            PackageName::new("cabin-ports/zlib").unwrap(),
            semver::Version::new(1, 3, 1),
        );
        let upstream = provenance(&"a".repeat(64), None);
        let package = zlib(&name, &version, &upstream, dir.path(), "");
        let err = materialize(&package, &dir.path().join("missing.tar.gz"), &stale).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }
}
