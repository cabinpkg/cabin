use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use cabin_core::PackageName;
use cabin_core::hash::hex_digest;
use sha2::{Digest, Sha256};

use crate::cache::ArtifactCache;
use crate::error::ArtifactError;
use crate::extract;
use crate::model::ChecksumDigest;

/// What to materialize into the cache.
#[derive(Debug, Clone)]
pub struct FetchPlan {
    pub entries: Vec<FetchEntry>,
}

/// One package the caller wants in the cache. Built from a resolved
/// package + the index entry's `source` and `checksum`.
#[derive(Debug, Clone)]
pub struct FetchEntry {
    pub name: PackageName,
    pub version: semver::Version,
    /// Raw `sha256:<hex>` checksum carried in the index/lockfile.
    pub checksum: String,
    /// Where the archive lives at fetch time. Local file index sources
    /// hand in a [`FetchSource::LocalArchive`]; the HTTP index source
    /// pre-downloads the archive bytes and hands in a
    /// [`FetchSource::InMemoryArchive`].
    pub source: FetchSource,
}

/// Where to read archive bytes from. `cabin-artifact` stays
/// HTTP-free: callers handle any download themselves and pass the
/// resulting bytes via [`FetchSource::InMemoryArchive`].
#[derive(Debug, Clone)]
pub enum FetchSource {
    /// Filesystem path that the caller (file index) already resolved
    /// to a ready-to-open archive.
    LocalArchive(PathBuf),
    /// Archive bytes already in memory (HTTP downloads, custom
    /// fetchers, tests).
    InMemoryArchive(Vec<u8>),
}

/// Caller-controlled knobs that change how `fetch` interacts with the
/// cache.
#[derive(Debug, Clone, Copy, Default)]
pub struct FetchOptions {
    /// `--frozen`: do not populate the cache. If a required archive or
    /// extracted source tree is not already cached and valid, fail with
    /// [`ArtifactError::FrozenCacheMiss`].
    pub frozen: bool,
}

/// Fetch result, carrying the materialized cache locations.
#[derive(Debug, Clone)]
pub struct FetchResult {
    pub packages: Vec<FetchedPackage>,
}

/// One fully-materialized package: archive verified, source extracted,
/// `cabin.toml` validated.
#[derive(Debug, Clone)]
pub struct FetchedPackage {
    pub name: PackageName,
    pub version: semver::Version,
    pub checksum: String,
    pub archive_path: PathBuf,
    pub source_dir: PathBuf,
}

/// Materialize every entry in `plan` into the cache, observing
/// `options`.
pub fn fetch(
    plan: &FetchPlan,
    cache: &ArtifactCache,
    options: FetchOptions,
) -> Result<FetchResult, ArtifactError> {
    let mut packages = Vec::with_capacity(plan.entries.len());
    for entry in &plan.entries {
        packages.push(fetch_one(entry, cache, options)?);
    }
    Ok(FetchResult { packages })
}

fn fetch_one(
    entry: &FetchEntry,
    cache: &ArtifactCache,
    options: FetchOptions,
) -> Result<FetchedPackage, ArtifactError> {
    let digest =
        ChecksumDigest::parse(&entry.checksum).ok_or_else(|| ArtifactError::InvalidChecksum {
            name: entry.name.as_str().to_owned(),
            version: entry.version.to_string(),
            value: entry.checksum.clone(),
        })?;
    let hex = digest.hex().to_owned();
    let archive_path = cache.archive_path(&hex);
    let source_dir = cache.source_dir(&hex);

    ensure_archive(entry, &archive_path, &hex, options.frozen)?;
    ensure_source(entry, &archive_path, &source_dir, options.frozen)?;

    Ok(FetchedPackage {
        name: entry.name.clone(),
        version: entry.version.clone(),
        checksum: digest.full(),
        archive_path,
        source_dir,
    })
}

/// Make sure the cache archive file exists and matches `expected_hex`.
///
/// Behavior:
/// - if the archive is already present and hashes correctly, reuse it;
/// - otherwise (missing, wrong hash, or corrupt) and not frozen,
///   read the archive from [`FetchSource`] while hashing, and fail
///   if the bytes don't match `expected_hex`;
/// - in frozen mode, refuse to populate; only an already-correct
///   cache entry is acceptable.
fn ensure_archive(
    entry: &FetchEntry,
    archive_path: &Path,
    expected_hex: &str,
    frozen: bool,
) -> Result<(), ArtifactError> {
    if archive_path.is_file() {
        let actual = hash_file(archive_path)?;
        if actual == expected_hex {
            return Ok(());
        }
        if frozen {
            return Err(ArtifactError::FrozenCacheMiss {
                name: entry.name.as_str().to_owned(),
                version: entry.version.to_string(),
            });
        }
    } else if frozen {
        return Err(ArtifactError::FrozenCacheMiss {
            name: entry.name.as_str().to_owned(),
            version: entry.version.to_string(),
        });
    }

    if let FetchSource::LocalArchive(path) = &entry.source
        && !path.is_file()
    {
        return Err(ArtifactError::MissingArchive {
            name: entry.name.as_str().to_owned(),
            version: entry.version.to_string(),
            path: path.clone(),
        });
    }

    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|source| ArtifactError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let tmp_target = partial_sibling(archive_path);
    populate_archive(entry, &tmp_target, archive_path, expected_hex)?;
    Ok(())
}

fn populate_archive(
    entry: &FetchEntry,
    tmp_target: &Path,
    final_target: &Path,
    expected_hex: &str,
) -> Result<(), ArtifactError> {
    let actual = match &entry.source {
        FetchSource::LocalArchive(path) => stream_local_to_partial(path, tmp_target)?,
        FetchSource::InMemoryArchive(bytes) => write_bytes_to_partial(bytes, tmp_target)?,
    };

    if actual != expected_hex {
        let _ = fs::remove_file(tmp_target);
        return Err(ArtifactError::ChecksumMismatch {
            name: entry.name.as_str().to_owned(),
            version: entry.version.to_string(),
            expected: expected_hex.to_owned(),
            actual,
        });
    }
    fs::rename(tmp_target, final_target).map_err(|source| ArtifactError::Io {
        path: final_target.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Stream `source_path` into `tmp_target`, hashing as it goes.
fn stream_local_to_partial(source_path: &Path, tmp_target: &Path) -> Result<String, ArtifactError> {
    let mut src = File::open(source_path).map_err(|source| ArtifactError::Io {
        path: source_path.to_path_buf(),
        source,
    })?;
    let mut dst = File::create(tmp_target).map_err(|source| ArtifactError::Io {
        path: tmp_target.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = src.read(&mut buf).map_err(|source| ArtifactError::Io {
            path: source_path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n])
            .map_err(|source| ArtifactError::Io {
                path: tmp_target.to_path_buf(),
                source,
            })?;
    }
    drop(dst);
    Ok(hex_digest(&hasher.finalize()))
}

/// Write `bytes` into `tmp_target`, hashing as it goes.
fn write_bytes_to_partial(bytes: &[u8], tmp_target: &Path) -> Result<String, ArtifactError> {
    let mut dst = File::create(tmp_target).map_err(|source| ArtifactError::Io {
        path: tmp_target.to_path_buf(),
        source,
    })?;
    dst.write_all(bytes).map_err(|source| ArtifactError::Io {
        path: tmp_target.to_path_buf(),
        source,
    })?;
    drop(dst);
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex_digest(&hasher.finalize()))
}

/// Build the completion-marker path for an extraction.
///
/// The marker lives as a SIBLING of `source_dir`, not inside it.
/// `extract::extract_tar_gz` writes every tarball entry under
/// `source_dir`, and `source_dir.starts_with(dest)` is enforced
/// per entry, so no published tarball can forge this marker no
/// matter what filenames it includes. `fs::remove_dir_all` on
/// `source_dir` does not remove the sibling marker either, so
/// the caller must delete the marker explicitly before
/// re-extracting — captured in `ensure_source` below.
fn extraction_marker_path(source_dir: &Path) -> PathBuf {
    let mut s: OsString = source_dir.as_os_str().to_owned();
    s.push(".ok");
    PathBuf::from(s)
}

fn ensure_source(
    entry: &FetchEntry,
    archive_path: &Path,
    source_dir: &Path,
    frozen: bool,
) -> Result<(), ArtifactError> {
    let marker = extraction_marker_path(source_dir);
    if marker.is_file()
        && extract::validate_extracted(source_dir, &entry.name, &entry.version).is_ok()
    {
        return Ok(());
    }
    if frozen {
        return Err(ArtifactError::FrozenCacheMiss {
            name: entry.name.as_str().to_owned(),
            version: entry.version.to_string(),
        });
    }

    // Drop a stale marker first so a crash before the new one is
    // written can never leave the previous run's "complete" flag
    // pointing at a freshly re-extracted (or in-progress) tree.
    if marker.exists() {
        fs::remove_file(&marker).map_err(|source| ArtifactError::Io {
            path: marker.clone(),
            source,
        })?;
    }
    if source_dir.exists() {
        fs::remove_dir_all(source_dir).map_err(|source| ArtifactError::Io {
            path: source_dir.to_path_buf(),
            source,
        })?;
    }
    fs::create_dir_all(source_dir).map_err(|source| ArtifactError::Io {
        path: source_dir.to_path_buf(),
        source,
    })?;
    extract::extract_tar_gz(archive_path, source_dir)?;
    extract::validate_extracted(source_dir, &entry.name, &entry.version)?;
    // Write the marker only after extraction and validation
    // succeed. A crash between extract_tar_gz and this write
    // leaves the marker absent, so the next run treats the
    // directory as interrupted and re-extracts.
    File::create(&marker).map_err(|source| ArtifactError::Io {
        path: marker.clone(),
        source,
    })?;
    Ok(())
}

/// `archive_path.with_extension("partial")` would clobber `.gz`, so
/// build the sibling path by hand.
fn partial_sibling(archive_path: &Path) -> PathBuf {
    let mut s: OsString = archive_path.as_os_str().to_owned();
    s.push(".partial");
    PathBuf::from(s)
}

fn hash_file(path: &Path) -> Result<String, ArtifactError> {
    let f = File::open(path).map_err(|source| ArtifactError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    cabin_core::hash::hash_reader(f).map_err(|source| ArtifactError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::fixture::ChildPath;
    use assert_fs::prelude::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    /// Assemble a tiny `.tar.gz` at the given destination with the
    /// given file contents. Returns the archive's `sha256` hex digest.
    fn write_archive(archive: &ChildPath, files: &[(&str, &str)]) -> String {
        if let Some(parent) = archive.path().parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = File::create(archive.path()).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        for (rel, body) in files {
            let bytes = body.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(&mut header, rel, &mut std::io::Cursor::new(bytes))
                .unwrap();
        }
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
        hash_file(archive.path()).unwrap()
    }

    fn manifest(name: &str, version: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n")
    }

    fn cache_root(dir: &std::path::Path) -> ArtifactCache {
        ArtifactCache::new(dir.join("cache"))
    }

    #[test]
    fn fetch_copies_archive_into_cache_and_extracts_source() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("artifacts/fmt-10.2.1.tar.gz");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: format!("sha256:{hex}"),
                source: FetchSource::LocalArchive(archive.to_path_buf()),
            }],
        };
        let result = fetch(&plan, &cache, FetchOptions::default()).unwrap();
        assert_eq!(result.packages.len(), 1);
        let pkg_result = &result.packages[0];
        assert_eq!(pkg_result.archive_path, cache.archive_path(&hex));
        assert!(pkg_result.archive_path.is_file());
        assert!(pkg_result.source_dir.join("cabin.toml").is_file());
    }

    #[test]
    fn already_cached_archive_is_reused() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("artifacts/fmt.tar.gz");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: format!("sha256:{hex}"),
                source: FetchSource::LocalArchive(archive.to_path_buf()),
            }],
        };
        fetch(&plan, &cache, FetchOptions::default()).unwrap();
        // Move the source archive away — the cached copy must still
        // satisfy a re-run.
        fs::remove_file(archive.path()).unwrap();
        let r2 = fetch(&plan, &cache, FetchOptions::default()).unwrap();
        assert!(r2.packages[0].archive_path.is_file());
    }

    #[test]
    fn checksum_mismatch_is_reported() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("artifacts/fmt.tar.gz");
        let _hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let bogus = format!("sha256:{}", "0".repeat(64));
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: bogus,
                source: FetchSource::LocalArchive(archive.to_path_buf()),
            }],
        };
        let err = fetch(&plan, &cache, FetchOptions::default()).unwrap_err();
        match err {
            ArtifactError::ChecksumMismatch { .. } => {}
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
    }

    #[test]
    fn missing_archive_is_reported() {
        let dir = TempDir::new().unwrap();
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: format!("sha256:{}", "a".repeat(64)),
                source: FetchSource::LocalArchive(dir.child("nope.tar.gz").to_path_buf()),
            }],
        };
        let err = fetch(&plan, &cache, FetchOptions::default()).unwrap_err();
        assert!(matches!(err, ArtifactError::MissingArchive { .. }));
    }

    #[test]
    fn invalid_checksum_is_reported() {
        let dir = TempDir::new().unwrap();
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: "sha256:not-hex".to_owned(),
                source: FetchSource::LocalArchive(dir.child("any.tar.gz").to_path_buf()),
            }],
        };
        let err = fetch(&plan, &cache, FetchOptions::default()).unwrap_err();
        assert!(matches!(err, ArtifactError::InvalidChecksum { .. }));
    }

    #[test]
    fn frozen_uses_existing_cache() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("artifacts/fmt.tar.gz");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: format!("sha256:{hex}"),
                source: FetchSource::LocalArchive(archive.to_path_buf()),
            }],
        };
        // Populate first.
        fetch(&plan, &cache, FetchOptions::default()).unwrap();
        // Now run with frozen — cache hit should succeed.
        fetch(&plan, &cache, FetchOptions { frozen: true }).unwrap();
    }

    #[test]
    fn frozen_fails_on_cache_miss() {
        let dir = TempDir::new().unwrap();
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: format!("sha256:{}", "b".repeat(64)),
                source: FetchSource::LocalArchive(dir.child("ignored.tar.gz").to_path_buf()),
            }],
        };
        let err = fetch(&plan, &cache, FetchOptions { frozen: true }).unwrap_err();
        assert!(matches!(err, ArtifactError::FrozenCacheMiss { .. }));
    }

    #[test]
    fn re_extracts_when_existing_source_dir_is_incomplete() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("artifacts/fmt.tar.gz");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: format!("sha256:{hex}"),
                source: FetchSource::LocalArchive(archive.to_path_buf()),
            }],
        };
        fetch(&plan, &cache, FetchOptions::default()).unwrap();
        // Corrupt the extracted manifest; next run should re-extract.
        let extracted = cache.source_dir(&hex);
        fs::write(extracted.join("cabin.toml"), "garbage").unwrap();
        fetch(&plan, &cache, FetchOptions::default()).unwrap();
        let body = fs::read_to_string(extracted.join("cabin.toml")).unwrap();
        assert!(body.contains("fmt"));
    }

    #[test]
    fn re_extracts_when_marker_missing_even_if_manifest_present() {
        // Simulates an interrupted previous run that wrote
        // `cabin.toml` (tar archives put the manifest at the
        // head) before crashing without finishing the rest of
        // the source tree. The next fetch must re-extract rather
        // than treat the directory as a complete cache hit.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("artifacts/fmt.tar.gz");
        let hex = write_archive(
            &archive,
            &[
                ("cabin.toml", &manifest("fmt", "10.2.1")),
                ("src/main.cc", "int main() { return 0; }\n"),
            ],
        );
        let cache = cache_root(dir.path());
        let extracted = cache.source_dir(&hex);
        let marker = extraction_marker_path(&extracted);
        // Pretend a previous run extracted just the manifest and
        // crashed. No completion marker is written.
        fs::create_dir_all(&extracted).unwrap();
        fs::write(extracted.join("cabin.toml"), manifest("fmt", "10.2.1")).unwrap();
        assert!(!marker.is_file());
        assert!(!extracted.join("src/main.cc").is_file());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: format!("sha256:{hex}"),
                source: FetchSource::LocalArchive(archive.to_path_buf()),
            }],
        };
        fetch(&plan, &cache, FetchOptions::default()).unwrap();
        assert!(marker.is_file());
        assert!(extracted.join("src/main.cc").is_file());
    }

    #[test]
    fn marker_sibling_path_resists_tarball_forgery() {
        // The completion marker is a sibling of `source_dir`,
        // not inside it. Even if a published tarball were named
        // to look like the marker, `extract_tar_gz` would only
        // place it under `source_dir` and our check would still
        // miss. Confirm the marker path does not start with
        // `source_dir` so the invariant is visible to readers.
        let dir = TempDir::new().unwrap();
        let cache = cache_root(dir.path());
        let extracted = cache.source_dir(&"a".repeat(64));
        let marker = extraction_marker_path(&extracted);
        assert!(!marker.starts_with(&extracted));
        assert_eq!(marker.parent(), extracted.parent());
    }

    #[test]
    fn frozen_fails_when_marker_missing_even_if_manifest_present() {
        // Same setup as the marker-missing test above, but in
        // frozen mode. The incomplete cache must surface as a
        // FrozenCacheMiss rather than being silently treated as
        // valid.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("artifacts/fmt.tar.gz");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        // Also lay down the archive so `ensure_archive` passes
        // and we exercise the source path.
        let dest_archive = cache.archive_path(&hex);
        fs::create_dir_all(dest_archive.parent().unwrap()).unwrap();
        fs::copy(archive.path(), &dest_archive).unwrap();
        let extracted = cache.source_dir(&hex);
        fs::create_dir_all(&extracted).unwrap();
        fs::write(extracted.join("cabin.toml"), manifest("fmt", "10.2.1")).unwrap();
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: format!("sha256:{hex}"),
                source: FetchSource::LocalArchive(archive.to_path_buf()),
            }],
        };
        let err = fetch(&plan, &cache, FetchOptions { frozen: true }).unwrap_err();
        assert!(matches!(err, ArtifactError::FrozenCacheMiss { .. }));
    }

    #[test]
    fn rejects_archive_without_root_cabin_toml() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("artifacts/fmt.tar.gz");
        let hex = write_archive(&archive, &[("src/main.cc", "int main() { return 0; }\n")]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: format!("sha256:{hex}"),
                source: FetchSource::LocalArchive(archive.to_path_buf()),
            }],
        };
        let err = fetch(&plan, &cache, FetchOptions::default()).unwrap_err();
        assert!(matches!(err, ArtifactError::MissingArchiveManifest { .. }));
    }
}
