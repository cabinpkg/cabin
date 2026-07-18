use std::fs::{self, File};
use std::path::{Path, PathBuf};

use cabin_core::PackageName;

use crate::cache::{ArtifactCache, extraction_marker_path, partial_dir_sibling, partial_sibling};
use crate::error::ArtifactError;
use crate::extract;
use crate::model::ChecksumDigest;

/// What to materialize into the cache.
#[derive(Debug, Clone)]
pub struct FetchPlan {
    pub entries: Vec<FetchEntry>,
}

/// One package the caller wants in the cache.  Built from a resolved
/// package + the index entry's `source` and `checksum`.
#[derive(Debug, Clone)]
pub struct FetchEntry {
    pub name: PackageName,
    pub version: semver::Version,
    /// Raw `sha256:<hex>` checksum carried in the index/lockfile.
    pub checksum: String,
    /// Where the archive lives at fetch time.  Local file index sources
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
    /// `--frozen`: do not populate the cache.  If a required archive or
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
///
/// # Errors
/// Returns an [`ArtifactError`] for the first entry that fails to
/// materialize: [`ArtifactError::InvalidChecksum`] for a malformed
/// `checksum`; [`ArtifactError::FrozenCacheMiss`] when `options.frozen`
/// is set and the archive or extracted tree is not already cached and
/// valid; [`ArtifactError::MissingArchive`] when a
/// [`FetchSource::LocalArchive`] path does not exist;
/// [`ArtifactError::ChecksumMismatch`] when fetched bytes do not hash to
/// the expected digest; [`ArtifactError::Io`] for filesystem failures;
/// any extraction error from [`crate::safe_extract_zip`] (such as
/// [`ArtifactError::UnsafeArchiveEntry`]); and the validation errors
/// [`ArtifactError::MissingArchiveManifest`],
/// [`ArtifactError::ManifestMismatch`], or [`ArtifactError::Manifest`].
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

/// Build the [`ArtifactError::FrozenCacheMiss`] for `entry`, raised when
/// frozen mode finds no already-correct cache entry to reuse.
fn frozen_cache_miss(entry: &FetchEntry) -> ArtifactError {
    ArtifactError::FrozenCacheMiss {
        name: entry.name.as_str().to_owned(),
        version: entry.version.to_string(),
    }
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
            return Err(frozen_cache_miss(entry));
        }
    } else if frozen {
        return Err(frozen_cache_miss(entry));
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
    // Errors mapped to `tmp_target`: a mid-stream failure is far more
    // likely to be a write to the cache target than the local source
    // going unreadable after a successful open.
    cabin_core::hash::hash_copy(&mut src, &mut dst).map_err(|source| ArtifactError::Io {
        path: tmp_target.to_path_buf(),
        source,
    })
}

/// Write `bytes` into `tmp_target`, hashing as it goes.
fn write_bytes_to_partial(bytes: &[u8], tmp_target: &Path) -> Result<String, ArtifactError> {
    let mut dst = File::create(tmp_target).map_err(|source| ArtifactError::Io {
        path: tmp_target.to_path_buf(),
        source,
    })?;
    cabin_core::hash::hash_copy(bytes, &mut dst).map_err(|source| ArtifactError::Io {
        path: tmp_target.to_path_buf(),
        source,
    })
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
        return Err(frozen_cache_miss(entry));
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
    // Extract and validate in a sibling temp directory, renamed
    // into place only on success: a hostile archive rejected
    // mid-extraction (or a crash) never leaves a partial tree at
    // the final path, mirroring the `.partial` + rename convention
    // the archive download above uses.
    let tmp_dir = partial_dir_sibling(source_dir);
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir).map_err(|source| ArtifactError::Io {
            path: tmp_dir.clone(),
            source,
        })?;
    }
    fs::create_dir_all(&tmp_dir).map_err(|source| ArtifactError::Io {
        path: tmp_dir.clone(),
        source,
    })?;
    let extracted = extract::extract_zip(archive_path, &tmp_dir)
        .and_then(|()| extract::validate_extracted(&tmp_dir, &entry.name, &entry.version));
    if let Err(err) = extracted {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(err);
    }
    // Rename the validated tree into place.  A failure - including a
    // concurrent process having populated `source_dir` first, which
    // makes the rename onto a non-empty directory fail - removes the
    // scratch so no partial state leaks, and surfaces the error so the
    // caller retries rather than adopting a tree it did not build.
    // The cache is content-addressed, so a retry finds the winner's
    // now-valid entry on its cache-hit check.
    if let Err(source) = fs::rename(&tmp_dir, source_dir) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(ArtifactError::Io {
            path: source_dir.to_path_buf(),
            source,
        });
    }
    // Write the marker only after the validated tree is renamed
    // into place.  A crash before this write leaves the marker
    // absent, so the next run treats the directory as interrupted
    // and re-extracts.
    File::create(&marker).map_err(|source| ArtifactError::Io {
        path: marker.clone(),
        source,
    })?;
    Ok(())
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
    use std::io::Write;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    /// Assemble a tiny `.zip` at the given destination with the given
    /// file contents.  Returns the archive's `sha256` hex digest.
    fn write_archive(archive: &ChildPath, files: &[(&str, &str)]) -> String {
        if let Some(parent) = archive.path().parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = File::create(archive.path()).unwrap();
        let mut writer = zip::ZipWriter::new(f);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (rel, body) in files {
            writer.start_file(*rel, options).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap().flush().unwrap();
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
        // Move the source archive away - the cached copy must still
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
        fetch(&plan, &cache, FetchOptions::default()).unwrap();
        // Now run with frozen - cache hit should succeed.
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
        // `cabin.toml` (the archive lists the manifest first)
        // before crashing without finishing the rest of the
        // source tree.  The next fetch must re-extract rather
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
        // Pretend a previous run extracted the manifest alone and
        // crashed.  No completion marker is written.
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
        // not inside it.  Even if a published archive were named
        // to look like the marker, `extract_zip` would only
        // place it under `source_dir` and our check would still
        // miss.  Confirm the marker path does not start with
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
        // frozen mode.  The incomplete cache must surface as a
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
    fn a_rejected_archive_leaves_no_partial_source_tree() {
        // A hostile archive whose first entries extract fine and whose
        // last is refused.  Extraction happens in a scratch sibling
        // directory, so the cache is left exactly as it was: no
        // half-populated source tree, no completion marker, no
        // scratch directory.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("artifacts/evil.zip");
        fs::create_dir_all(archive.path().parent().unwrap()).unwrap();
        let f = File::create(archive.path()).unwrap();
        let mut writer = zip::ZipWriter::new(f);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (path, body) in [
            ("cabin.toml", manifest("fmt", "10.2.1")),
            ("src/main.cc", "int main() {}\n".to_owned()),
        ] {
            writer.start_file(path, options).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        // A symlink entry: rejected by the entry-type gate, after the
        // two regular files above have already been written.
        writer.add_symlink("link", "cabin.toml", options).unwrap();
        writer.finish().unwrap().flush().unwrap();

        let hex = hash_file(archive.path()).unwrap();
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
        assert!(
            matches!(err, ArtifactError::UnsupportedArchiveEntry(_)),
            "{err:?}"
        );
        let source_dir = cache.source_dir(&hex);
        assert!(!source_dir.exists(), "partial source tree left behind");
        assert!(!extraction_marker_path(&source_dir).exists());
        assert!(
            !partial_dir_sibling(&source_dir).exists(),
            "scratch dir left"
        );
        // The verified archive itself is still cached: only the
        // extraction failed.
        assert!(cache.archive_path(&hex).is_file());
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
