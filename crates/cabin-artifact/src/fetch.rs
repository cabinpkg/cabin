use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use atomic_write_file::AtomicWriteFile;
use cabin_core::PackageName;

use crate::cache::{ArtifactCache, extraction_marker_path, partial_dir_sibling};
use crate::error::ArtifactError;
use crate::extract;

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
    /// Digest carried in the index/lockfile, already parsed at
    /// those boundaries.
    pub checksum: cabin_core::Checksum,
    /// Where the archive lives at fetch time.  Local file index sources
    /// hand in a [`FetchSource::LocalArchive`]; the HTTP index source
    /// hands in a [`FetchSource::Remote`] that downloads it.
    pub source: FetchSource,
}

/// Opens a remote archive's bytes as a stream.
pub type OpenArchive =
    dyn Fn() -> Result<Box<dyn Read>, Box<dyn std::error::Error + Send + Sync>> + Send + Sync;

/// Where to read archive bytes from. `cabin-artifact` stays
/// HTTP-free: callers supply the download as a
/// [`FetchSource::Remote`] opener.
#[derive(Clone)]
pub enum FetchSource {
    /// An archive a local file index names: `path` as the index
    /// resolved it, and the `root` directory the index was opened at.
    /// The path is read only when it names a regular file inside
    /// `root`, reached without a symlink.
    LocalArchive { root: PathBuf, path: PathBuf },
    /// An archive fetched on demand (an HTTP download).  The opener is
    /// called only when the cache lacks the archive, and its stream is
    /// copied to the cache as it is read, never held in memory whole.
    Remote(Arc<OpenArchive>),
}

impl std::fmt::Debug for FetchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LocalArchive { root, path } => f
                .debug_struct("LocalArchive")
                .field("root", root)
                .field("path", path)
                .finish(),
            Self::Remote(_) => f.write_str("Remote"),
        }
    }
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
    pub checksum: cabin_core::Checksum,
    pub archive_path: PathBuf,
    pub source_dir: PathBuf,
}

/// Materialize every entry in `plan` into the cache, observing
/// `options`.
///
/// # Errors
/// Returns an [`ArtifactError`] for the first entry that fails to
/// materialize: [`ArtifactError::FrozenCacheMiss`] when `options.frozen`
/// is set and the archive or extracted tree is not already cached and
/// valid; [`ArtifactError::MissingArchive`] when a
/// [`FetchSource::LocalArchive`] path is not a regular file inside its
/// root; [`ArtifactError::Download`] when a [`FetchSource::Remote`]
/// cannot be opened or its stream fails;
/// [`ArtifactError::ChecksumMismatch`] when fetched bytes do not hash to
/// the expected digest; [`ArtifactError::Io`] for filesystem failures;
/// any extraction error from [`crate::safe_extract_zip`] (such as
/// [`ArtifactError::UnsafeArchiveEntry`]); and the validation errors
/// [`ArtifactError::MissingArchiveManifest`],
/// [`ArtifactError::ManifestMismatch`], or [`ArtifactError::Manifest`].
#[tracing::instrument(name = "fetch_artifacts", level = "info", skip_all, fields(packages = plan.entries.len()))]
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

#[tracing::instrument(level = "debug", skip_all, fields(package = %entry.name))]
fn fetch_one(
    entry: &FetchEntry,
    cache: &ArtifactCache,
    options: FetchOptions,
) -> Result<FetchedPackage, ArtifactError> {
    let hex = entry.checksum.hex();
    let archive_path = cache.archive_path(hex);
    let source_dir = cache.source_dir(hex);

    ensure_archive(entry, &archive_path, hex, options.frozen)?;
    ensure_source(entry, &archive_path, &source_dir, options.frozen)?;

    Ok(FetchedPackage {
        name: entry.name.clone(),
        version: entry.version.clone(),
        checksum: entry.checksum.clone(),
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

fn download_error(
    entry: &FetchEntry,
    source: Box<dyn std::error::Error + Send + Sync>,
) -> ArtifactError {
    ArtifactError::Download {
        name: entry.name.as_str().to_owned(),
        version: entry.version.to_string(),
        source,
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
#[tracing::instrument(level = "debug", skip_all, fields(hit = tracing::field::Empty))]
fn ensure_archive(
    entry: &FetchEntry,
    archive_path: &Path,
    expected_hex: &str,
    frozen: bool,
) -> Result<(), ArtifactError> {
    if archive_path.is_file() {
        let actual = hash_file(archive_path)?;
        if actual == expected_hex {
            tracing::Span::current().record("hit", 1);
            return Ok(());
        }
        if frozen {
            return Err(frozen_cache_miss(entry));
        }
    } else if frozen {
        return Err(frozen_cache_miss(entry));
    }

    let src: Box<dyn Read + '_> = match &entry.source {
        FetchSource::LocalArchive { root, path } => {
            Box::new(open_local_archive(entry, root, path)?)
        }
        FetchSource::Remote(open) => open().map_err(|source| download_error(entry, source))?,
    };

    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|source| ArtifactError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    tracing::Span::current().record("hit", 0);
    populate_archive(entry, src, archive_path, expected_hex)
}

/// Open the archive a local index names, refusing every path that does
/// not lead, without a symlink, to a regular file inside the index's
/// `root`.
///
/// The index is untrusted input: its `source.path` may be absolute,
/// climb with `..`, or pass through a symlink.  The loader joins it onto
/// a directory under `root`, so it is checked lexically first - `..`
/// may climb only back to `root` - and only then are the components
/// below `root` examined, each with `symlink_metadata`.  Resolving the
/// path on disk instead would consult whatever it names outside the
/// root on the way (macOS even resolves `file/..`), as would following
/// a symlink, so either would let the outcome reveal whether an outside
/// path exists.  Every refusal, a missing file included, is the same
/// [`ArtifactError::MissingArchive`], and the metadata check precedes
/// the open so a FIFO cannot block it.
fn open_local_archive(entry: &FetchEntry, root: &Path, path: &Path) -> Result<File, ArtifactError> {
    let missing = || ArtifactError::MissingArchive {
        name: entry.name.as_str().to_owned(),
        version: entry.version.to_string(),
        path: path.to_path_buf(),
    };
    let mut names = Vec::new();
    for component in path.strip_prefix(root).map_err(|_| missing())?.components() {
        match component {
            Component::Normal(name) => {
                // Windows reads some names as something else: `C:x` as a
                // drive-relative path that `push` would put in place of
                // the whole path, `NUL.zip` as a device, and `pad ` as
                // `pad` in a final component only.
                if name
                    .to_str()
                    .is_none_or(|name| cabin_fs::path::component_portability(name).is_some())
                {
                    return Err(missing());
                }
                names.push(name);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                names.pop().ok_or_else(missing)?;
            }
            _ => return Err(missing()),
        }
    }
    let mut inside = root.to_path_buf();
    let mut is_file = false;
    for name in names {
        inside.push(name);
        let file_type = fs::symlink_metadata(&inside)
            .map_err(|_| missing())?
            .file_type();
        if file_type.is_symlink() {
            return Err(missing());
        }
        is_file = file_type.is_file();
    }
    if !is_file {
        return Err(missing());
    }
    File::open(&inside).map_err(|_| missing())
}

/// Stream the entry's bytes into a staging file this call alone
/// owns - a randomly named sibling of `final_target` - hashing them
/// as they are written, and rename it into place only when the digest
/// matches.  The staging file has no predictable path, so a second
/// Cabin process populating the same entry (one whose index lies about
/// the digest, say) can neither truncate nor swap the bytes this call
/// verified before they are promoted; a mismatch discards the staging
/// file.
fn populate_archive(
    entry: &FetchEntry,
    mut src: impl Read,
    final_target: &Path,
    expected_hex: &str,
) -> Result<(), ArtifactError> {
    let io_error = |source| ArtifactError::Io {
        path: final_target.to_path_buf(),
        source,
    };
    let mut staged = AtomicWriteFile::open(final_target).map_err(io_error)?;
    // A mid-stream failure is blamed on the likelier side: the network
    // for a download, and for an opened local source the cache target
    // being written.
    let actual =
        cabin_core::hash::hash_copy(&mut src, &mut staged).map_err(|source| {
            match entry.source {
                FetchSource::Remote(_) => download_error(entry, source.into()),
                FetchSource::LocalArchive { .. } => io_error(source),
            }
        })?;

    if actual != expected_hex {
        let _ = staged.discard();
        return Err(ArtifactError::ChecksumMismatch {
            name: entry.name.as_str().to_owned(),
            version: entry.version.to_string(),
            expected: expected_hex.to_owned(),
        });
    }
    staged.commit().map_err(io_error)
}

#[tracing::instrument(level = "debug", skip_all, fields(hit = tracing::field::Empty))]
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
        tracing::Span::current().record("hit", 1);
        return Ok(());
    }
    tracing::Span::current().record("hit", 0);
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
    // the final path.
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
    // scratch so no partial state leaks, and surfaces the error rather
    // than adopting a tree this process did not build.  The cache is
    // content-addressed, so a re-run finds the winner's now-valid
    // entry on its cache-hit check.
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
        format!(
            r#"[package]
name = "{name}"
version = "{version}"
"#
        )
    }

    fn cache_root(dir: &std::path::Path) -> ArtifactCache {
        ArtifactCache::new(dir.join("cache"))
    }

    /// A remote source that serves `bytes` and then fails with `error`
    /// if one is given, counting how often it is opened.
    fn remote(
        bytes: &[u8],
        error: Option<&'static str>,
        opened: &Arc<std::sync::atomic::AtomicUsize>,
    ) -> FetchSource {
        struct Broken(&'static str);
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other(self.0))
            }
        }

        let (bytes, opened) = (bytes.to_vec(), Arc::clone(opened));
        FetchSource::Remote(Arc::new(move || {
            opened.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let served = std::io::Cursor::new(bytes.clone());
            Ok(match error {
                Some(error) => Box::new(served.chain(Broken(error))),
                None => Box::new(served),
            })
        }))
    }

    /// `archive` as a local index rooted at `dir` names it.
    fn local(dir: &TempDir, archive: &ChildPath) -> FetchSource {
        FetchSource::LocalArchive {
            root: dir.path().to_path_buf(),
            path: archive.to_path_buf(),
        }
    }

    #[test]
    fn fetch_copies_archive_into_cache_and_extracts_source() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("archives/fmt-10.2.1.tar.gz");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: local(&dir, &archive),
            }],
        };
        let result = fetch(&plan, &cache, FetchOptions::default()).unwrap();
        assert_eq!(result.packages.len(), 1);
        let pkg_result = &result.packages[0];
        assert_eq!(pkg_result.archive_path, cache.archive_path(&hex));
        assert!(pkg_result.archive_path.is_file());
        assert!(pkg_result.source_dir.join("cabin.toml").is_file());
    }

    /// A concurrent population of the same cache entry whose index
    /// lies about the digest (it serves other bytes under the honest
    /// checksum) can neither corrupt nor remove the honest download:
    /// the honest fetch succeeds and the promoted archive hashes to
    /// the expected digest.
    #[test]
    fn concurrent_hostile_population_cannot_corrupt_the_honest_archive() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = TempDir::new().unwrap();
        let archive = dir.child("archives/big.zip");
        // Pseudo-random letters, large enough that hashing the archive
        // takes long enough for the hostile loop below to overlap it.
        let body: String = (0..6_000_000u32)
            .map(|i| char::from(b'a' + (i.wrapping_mul(2_654_435_761) >> 27) as u8 % 26))
            .collect();
        let hex = write_archive(
            &archive,
            &[
                ("cabin.toml", &manifest("big", "1.0.0")),
                ("blob.txt", &body),
            ],
        );
        let cache = cache_root(dir.path());
        let archive_path = cache.archive_path(&hex);
        fs::create_dir_all(archive_path.parent().unwrap()).unwrap();
        let entry = |source| FetchEntry {
            name: pkg("big"),
            version: ver("1.0.0"),
            checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
            source,
        };
        let honest = entry(local(&dir, &archive));
        let hostile = entry(remote(b"not the archive", None, &Arc::default()));

        let done = AtomicBool::new(false);
        // Rendezvous so the hostile writer is running before the
        // honest copy starts; otherwise the loop could first be
        // scheduled after `done` is set and exercise nothing.
        let started = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                started.wait();
                while !done.load(Ordering::Relaxed) {
                    let _ = ensure_archive(&hostile, &archive_path, &hex, false);
                }
            });
            started.wait();
            let result = ensure_archive(&honest, &archive_path, &hex, false);
            done.store(true, Ordering::Relaxed);
            result.expect("the honest population succeeds");
        });
        assert_eq!(hash_file(&archive_path).unwrap(), hex);
    }

    #[test]
    fn already_cached_archive_is_reused() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("archives/fmt.tar.gz");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: local(&dir, &archive),
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
    fn a_remote_archive_is_opened_only_when_the_cache_lacks_it() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("served/fmt.zip");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let opened = Arc::default();
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: remote(&fs::read(archive.path()).unwrap(), None, &opened),
            }],
        };

        let first = fetch(&plan, &cache, FetchOptions::default()).unwrap();
        assert!(first.packages[0].source_dir.join("cabin.toml").is_file());
        fetch(&plan, &cache, FetchOptions::default()).unwrap();

        assert_eq!(opened.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    /// A download that dies part-way is an error naming the package,
    /// and neither the archive nor its staging file stays in the cache.
    #[test]
    fn a_remote_stream_that_fails_leaves_nothing_in_the_cache() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("served/fmt.zip");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let bytes = fs::read(archive.path()).unwrap();
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: remote(&bytes[..bytes.len() / 2], Some("reset"), &Arc::default()),
            }],
        };

        let err = fetch(&plan, &cache, FetchOptions::default()).unwrap_err();

        match &err {
            ArtifactError::Download { name, source, .. } => {
                assert_eq!(name, "fmt");
                assert_eq!(source.to_string(), "reset");
            }
            other => panic!("expected Download, got {other:?}"),
        }
        let archives = cache.archive_path(&hex);
        let left: Vec<_> = fs::read_dir(archives.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(left.is_empty(), "left in the cache: {left:?}");
    }

    #[test]
    fn checksum_mismatch_is_reported() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("archives/fmt.tar.gz");
        let _hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let bogus = format!("sha256:{}", "0".repeat(64));
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&bogus).unwrap(),
                source: local(&dir, &archive),
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
                checksum: cabin_core::Checksum::parse(&format!("sha256:{}", "a".repeat(64)))
                    .unwrap(),
                source: local(&dir, &dir.child("nope.tar.gz")),
            }],
        };
        let err = fetch(&plan, &cache, FetchOptions::default()).unwrap_err();
        assert!(matches!(err, ArtifactError::MissingArchive { .. }));
    }

    /// A hostile local index can name any path in `source.path`; only a
    /// regular file inside the index root, reached without a symlink, is
    /// read.  Every refusal is the same error and names no digest, so the
    /// index learns neither a file's existence nor its sha256 - not even
    /// through a path that passes an outside file and returns to an
    /// archive inside.
    #[test]
    fn archive_outside_the_index_root_is_refused_without_an_oracle() {
        let dir = TempDir::new().unwrap();
        let root = dir.child("registry");
        root.child("probe.zip").write_str("inside").unwrap();
        let secret = dir.child("secret.txt");
        secret.write_str("not for the index").unwrap();
        let secret_hex = hash_file(secret.path()).unwrap();
        let cache = cache_root(dir.path());
        let fetch_path = |path: std::path::PathBuf| {
            let plan = FetchPlan {
                entries: vec![FetchEntry {
                    name: pkg("evil"),
                    version: ver("1.0.0"),
                    checksum: cabin_core::Checksum::parse(&format!("sha256:{}", "a".repeat(64)))
                        .unwrap(),
                    source: FetchSource::LocalArchive {
                        root: root.to_path_buf(),
                        path,
                    },
                }],
            };
            fetch(&plan, &cache, FetchOptions::default()).unwrap_err()
        };
        // The probe itself is read, so the refusals below are not
        // explained by the probe being unreadable.
        assert!(matches!(
            fetch_path(root.path().join("probe.zip")),
            ArtifactError::ChecksumMismatch { .. }
        ));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut escapes = vec![
            secret.to_path_buf(),
            root.path().join("packages/../../secret.txt"),
            root.path().join("../missing.txt"),
            root.path().join("../secret.txt/../registry/probe.zip"),
            root.path().join("../registry/probe.zip"),
            root.to_path_buf(),
            root.path().join("probe.zip."),
            root.path().join("probe.zip "),
            root.path().join("NUL.zip"),
        ];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(secret.path(), root.child("link.zip").path()).unwrap();
            std::os::unix::fs::symlink(
                root.child("probe.zip").path(),
                root.child("inner.zip").path(),
            )
            .unwrap();
            escapes.extend([root.path().join("link.zip"), root.path().join("inner.zip")]);
        }
        for path in escapes {
            let err = fetch_path(path.clone());
            assert!(
                matches!(err, ArtifactError::MissingArchive { .. }),
                "{path:?}: {err:?}"
            );
            assert!(!err.to_string().contains(&secret_hex), "{err}");
        }
    }

    /// On Windows a component such as `D:Cargo.toml` is a drive-relative
    /// path of its own; joined onto the root it would name the file in
    /// the current directory of that drive, outside the root.
    #[cfg(windows)]
    #[test]
    fn a_drive_relative_component_does_not_leave_the_index_root() {
        let dir = TempDir::new().unwrap();
        let root = dir.child("registry");
        root.child("pad").create_dir_all().unwrap();
        let cwd = std::env::current_dir().unwrap();
        assert!(cwd.join("Cargo.toml").is_file());
        let drive = match cwd.components().next() {
            Some(Component::Prefix(prefix)) => prefix.as_os_str().to_str().unwrap().to_owned(),
            other => panic!("{other:?}"),
        };
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("evil"),
                version: ver("1.0.0"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{}", "a".repeat(64)))
                    .unwrap(),
                source: FetchSource::LocalArchive {
                    root: root.to_path_buf(),
                    path: PathBuf::from(format!(
                        "{}\\pad\\{drive}Cargo.toml",
                        root.path().display()
                    )),
                },
            }],
        };
        let err = fetch(&plan, &cache_root(dir.path()), FetchOptions::default()).unwrap_err();
        assert!(
            matches!(err, ArtifactError::MissingArchive { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn archive_inside_the_index_root_may_climb_through_parent_components() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("archives/acme/fmt/fmt.zip");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        dir.child("packages/acme").create_dir_all().unwrap();
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                // The scoped file-registry layout's relative path.
                source: FetchSource::LocalArchive {
                    root: dir.to_path_buf(),
                    path: dir
                        .path()
                        .join("packages/acme/../../archives/acme/fmt/fmt.zip"),
                },
            }],
        };
        fetch(&plan, &cache_root(dir.path()), FetchOptions::default()).unwrap();
    }

    #[test]
    fn frozen_uses_existing_cache() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("archives/fmt.tar.gz");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: local(&dir, &archive),
            }],
        };
        fetch(&plan, &cache, FetchOptions::default()).unwrap();
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
                checksum: cabin_core::Checksum::parse(&format!("sha256:{}", "b".repeat(64)))
                    .unwrap(),
                source: local(&dir, &dir.child("ignored.tar.gz")),
            }],
        };
        let err = fetch(&plan, &cache, FetchOptions { frozen: true }).unwrap_err();
        assert!(matches!(err, ArtifactError::FrozenCacheMiss { .. }));
    }

    #[test]
    fn re_extracts_when_existing_source_dir_is_incomplete() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("archives/fmt.tar.gz");
        let hex = write_archive(&archive, &[("cabin.toml", &manifest("fmt", "10.2.1"))]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: local(&dir, &archive),
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
        // A source tree without the completion marker is not a
        // cache hit, even when its `cabin.toml` validates: the next
        // fetch re-extracts it.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("archives/fmt.tar.gz");
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
        fs::create_dir_all(&extracted).unwrap();
        fs::write(extracted.join("cabin.toml"), manifest("fmt", "10.2.1")).unwrap();
        assert!(!marker.is_file());
        assert!(!extracted.join("src/main.cc").is_file());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: local(&dir, &archive),
            }],
        };
        fetch(&plan, &cache, FetchOptions::default()).unwrap();
        assert!(marker.is_file());
        assert!(extracted.join("src/main.cc").is_file());
    }

    #[test]
    fn marker_sibling_path_resists_tarball_forgery() {
        // Every archive entry extracts under `source_dir`, so none
        // can land on the sibling marker path.
        let dir = TempDir::new().unwrap();
        let cache = cache_root(dir.path());
        let extracted = cache.source_dir(&"a".repeat(64));
        let marker = extraction_marker_path(&extracted);
        assert!(!marker.starts_with(&extracted));
        assert_eq!(marker.parent(), extracted.parent());
    }

    #[test]
    fn frozen_fails_when_marker_missing_even_if_manifest_present() {
        // The marker-missing setup above, in frozen mode.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("archives/fmt.tar.gz");
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
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: local(&dir, &archive),
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
        let archive = dir.child("archives/evil.zip");
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
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: local(&dir, &archive),
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
        let archive = dir.child("archives/fmt.tar.gz");
        let hex = write_archive(&archive, &[("src/main.cc", "int main() { return 0; }\n")]);
        let cache = cache_root(dir.path());
        let plan = FetchPlan {
            entries: vec![FetchEntry {
                name: pkg("fmt"),
                version: ver("10.2.1"),
                checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                source: local(&dir, &archive),
            }],
        };
        let err = fetch(&plan, &cache, FetchOptions::default()).unwrap_err();
        assert!(matches!(err, ArtifactError::MissingArchiveManifest { .. }));
    }
}
