//! Port-preparation pipeline.
//!
//! The pipeline turns a [`PortPlan`] (each entry is a parsed
//! `PortDescriptor` plus a [`PortFetchSource`]) into a list of
//! [`PreparedPort`]s on disk. Each prepared port directory looks
//! exactly like a regular Cabin path dependency: the upstream
//! source files plus the overlay `cabin.toml` at the directory
//! root. The workspace loader can then take over unchanged.
//!
//! For each entry the pipeline:
//!
//! 1. resolves the cache paths (archive + extracted source dir);
//! 2. ensures the archive is on disk and hashes to the declared
//!    SHA-256, populating from the supplied [`PortFetchSource`]
//!    if necessary (refused when frozen);
//! 3. extracts the archive into the source dir with the
//!    declared `strip_prefix`, reusing `cabin-artifact`'s
//!    decompression-bomb caps and path-safety rules;
//! 4. copies the overlay `cabin.toml` into the extracted source
//!    dir, overwriting any in-tree copy that already existed;
//! 5. cross-checks the overlay's `[package]` identity against
//!    the authoritative `port.toml`;
//! 6. writes the `<source_dir>.ok` completion marker so a future
//!    run can reuse the prep without re-extracting.
//!
//! A crash between extraction and marker write leaves the
//! marker absent; the next run treats the directory as
//! interrupted and re-extracts from scratch.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use cabin_artifact::cache::{extraction_marker_path, partial_sibling};
use cabin_artifact::{SafeExtractOptions, safe_extract_tar_gz};
use cabin_core::PackageName;
use cabin_core::hash::hex_digest;
use cabin_fs::write_atomic;
use semver::Version;
use sha2::{Digest, Sha256};
use url::Url;

use crate::cache::PortCache;
use crate::error::PortError;
use crate::model::{PortChecksum, PortDescriptor, PortSource};

/// Where to read archive bytes from. `cabin-port` stays HTTP-free:
/// callers handle any download themselves and pass the resulting
/// bytes via [`PortFetchSource::InMemoryArchive`].
#[derive(Debug, Clone)]
pub enum PortFetchSource {
    /// Filesystem path the caller has already resolved to a
    /// ready-to-open archive (e.g. a `file://` URL).
    LocalArchive(PathBuf),
    /// Archive bytes already in memory (HTTP downloads, custom
    /// fetchers, tests).
    InMemoryArchive(Vec<u8>),
}

/// Where a port's recipe came from. Determines whether
/// `ensure_overlay` reads the overlay text from disk (`PortDir`)
/// or from a `cabin_port::builtin::BuiltinPort` (`Builtin`).
/// It also discriminates how the workspace loader resolves a port
/// dependency: filesystem ports are keyed by their canonical
/// directory path, while bundled ports are keyed by package name.
#[derive(Debug, Clone)]
pub enum PortOrigin {
    /// Filesystem recipe: `<port_dir>/port.toml` plus the
    /// overlay manifest at the descriptor's relative path.
    PortDir(PathBuf),
    /// Bundled recipe by name. The overlay text comes from
    /// `cabin_port::builtin::lookup(name, &req).overlay_toml`.
    Builtin(&'static str),
}

/// One port to materialize.
#[derive(Debug, Clone)]
pub struct PortEntry {
    /// Parsed `port.toml`.
    pub descriptor: PortDescriptor,
    /// Where the port's recipe came from. Determines how the
    /// overlay manifest is sourced.
    pub origin: PortOrigin,
    /// Where the archive bytes come from.
    pub source: PortFetchSource,
}

/// A finalized preparation plan. Build it from the orchestration
/// layer and pass it to [`prepare`].
#[derive(Debug, Clone, Default)]
pub struct PortPlan {
    pub entries: Vec<PortEntry>,
}

/// Caller-controlled knobs.
#[derive(Debug, Clone, Copy, Default)]
pub struct PortPrepareOptions {
    /// `--frozen`: do not populate the cache. If a required
    /// archive or extracted source tree is not already cached
    /// and valid, fail with [`PortError::FrozenCacheMiss`].
    pub frozen: bool,
}

/// Outcome of one [`prepare`] invocation.
#[derive(Debug, Clone)]
pub struct PortPrepareResult {
    pub ports: Vec<PreparedPort>,
}

/// One fully materialized port: archive verified, source
/// extracted (with `strip_prefix`), overlay copied,
/// `[package]` identity cross-checked.
#[derive(Debug, Clone)]
pub struct PreparedPort {
    pub name: PackageName,
    pub version: Version,
    pub source_dir: PathBuf,
    pub origin: PortOrigin,
    pub provenance: PortProvenance,
}

/// Provenance recorded for downstream observability
/// (metadata / tree / explain).
#[derive(Debug, Clone)]
pub struct PortProvenance {
    pub url: Url,
    pub sha256_hex: String,
    pub strip_prefix: Option<String>,
    /// Absolute path to the overlay manifest inside the port
    /// directory (i.e. `port_dir.join(overlay.relative_path)`).
    /// `Some(<absolute path>)` for a filesystem (`PortDir`) port;
    /// `None` for a bundled (`Builtin`) port which has no on-disk
    /// overlay file.
    pub overlay_manifest: Option<PathBuf>,
}

/// Materialize every entry in `plan` into the cache.
pub fn prepare(
    plan: &PortPlan,
    cache: &PortCache,
    options: PortPrepareOptions,
) -> Result<PortPrepareResult, PortError> {
    let mut ports = Vec::with_capacity(plan.entries.len());
    for entry in &plan.entries {
        ports.push(prepare_one(entry, cache, options)?);
    }
    Ok(PortPrepareResult { ports })
}

fn prepare_one(
    entry: &PortEntry,
    cache: &PortCache,
    options: PortPrepareOptions,
) -> Result<PreparedPort, PortError> {
    let PortSource::Archive {
        url,
        sha256,
        strip_prefix,
    } = &entry.descriptor.source;

    let expected_hex = sha256.to_hex();
    let archive_path = cache.archive_path(&expected_hex);
    // Extracted sources are identity-keyed (name + version) so two
    // ports that share the same upstream archive but ship different
    // overlays do not clobber each other's `cabin.toml`.
    let source_dir = cache.source_dir(
        entry.descriptor.name.as_str(),
        &entry.descriptor.version.to_string(),
        &expected_hex,
    );

    ensure_archive(entry, &archive_path, sha256, options.frozen)?;
    ensure_source(
        entry,
        &archive_path,
        &source_dir,
        strip_prefix.as_deref(),
        options.frozen,
    )?;
    ensure_overlay(entry, &source_dir)?;
    cross_check_overlay_identity(entry, &source_dir)?;
    write_marker(&source_dir)?;

    let overlay_manifest = match &entry.origin {
        PortOrigin::PortDir(dir) => Some(dir.join(&entry.descriptor.overlay.relative_path)),
        PortOrigin::Builtin(_) => None,
    };
    Ok(PreparedPort {
        name: entry.descriptor.name.clone(),
        version: entry.descriptor.version.clone(),
        source_dir,
        origin: entry.origin.clone(),
        provenance: PortProvenance {
            url: url.clone(),
            sha256_hex: expected_hex,
            strip_prefix: strip_prefix.clone(),
            overlay_manifest,
        },
    })
}

fn ensure_archive(
    entry: &PortEntry,
    archive_path: &Path,
    expected: &PortChecksum,
    frozen: bool,
) -> Result<(), PortError> {
    let expected_hex = expected.to_hex();
    if archive_path.is_file() {
        let actual = hash_file(archive_path)?;
        if actual == expected_hex {
            return Ok(());
        }
        if frozen {
            return Err(PortError::FrozenCacheMiss {
                name: entry.descriptor.name.as_str().to_owned(),
                version: entry.descriptor.version.to_string(),
            });
        }
    } else if frozen {
        return Err(PortError::FrozenCacheMiss {
            name: entry.descriptor.name.as_str().to_owned(),
            version: entry.descriptor.version.to_string(),
        });
    }

    if let PortFetchSource::LocalArchive(path) = &entry.source
        && !path.is_file()
    {
        return Err(PortError::MissingArchive {
            name: entry.descriptor.name.as_str().to_owned(),
            version: entry.descriptor.version.to_string(),
            path: path.clone(),
        });
    }

    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).map_err(|source| PortError::Fs {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let tmp_target = partial_sibling(archive_path);
    let actual = match &entry.source {
        PortFetchSource::LocalArchive(path) => stream_local_to_partial(path, &tmp_target)?,
        PortFetchSource::InMemoryArchive(bytes) => write_bytes_to_partial(bytes, &tmp_target)?,
    };

    if actual != expected_hex {
        let _ = fs::remove_file(&tmp_target);
        return Err(PortError::ChecksumMismatch {
            name: entry.descriptor.name.as_str().to_owned(),
            version: entry.descriptor.version.to_string(),
            expected: expected_hex,
            actual,
        });
    }
    // Windows refuses `fs::rename` when the destination exists,
    // so a corrupted-cache recovery (stale archive at the
    // content-addressed path with the wrong hash) cannot
    // self-heal. Remove the stale file up-front; `NotFound` is
    // the common case (no stale file present) and surfaces as a
    // silent no-op rather than an error.
    match fs::remove_file(archive_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(PortError::Fs {
                path: archive_path.to_path_buf(),
                source,
            });
        }
    }
    fs::rename(&tmp_target, archive_path).map_err(|source| PortError::Fs {
        path: archive_path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn ensure_source(
    entry: &PortEntry,
    archive_path: &Path,
    source_dir: &Path,
    strip_prefix: Option<&str>,
    frozen: bool,
) -> Result<(), PortError> {
    let marker = extraction_marker_path(source_dir);
    if marker.is_file() && source_dir.join("cabin.toml").is_file() {
        // We trust the marker because:
        //   1. cabin-port wrote the marker only after a full
        //      successful extraction + overlay copy + identity
        //      cross-check, so the directory contents matched the
        //      port descriptor when the marker was written;
        //   2. the archive on disk has already been re-verified
        //      by `ensure_archive`, so the source tree we wrote
        //      from it is still correct under the recorded hash.
        return Ok(());
    }

    if frozen {
        return Err(PortError::FrozenCacheMiss {
            name: entry.descriptor.name.as_str().to_owned(),
            version: entry.descriptor.version.to_string(),
        });
    }

    // Drop a stale marker before re-extracting so a crash before
    // the new marker is written cannot leave a previous run's
    // "complete" flag pointing at a partially overwritten tree.
    if marker.exists() {
        fs::remove_file(&marker).map_err(|source| PortError::Fs {
            path: marker.clone(),
            source,
        })?;
    }
    if source_dir.exists() {
        fs::remove_dir_all(source_dir).map_err(|source| PortError::Fs {
            path: source_dir.to_path_buf(),
            source,
        })?;
    }
    fs::create_dir_all(source_dir).map_err(|source| PortError::Fs {
        path: source_dir.to_path_buf(),
        source,
    })?;

    safe_extract_tar_gz(
        archive_path,
        source_dir,
        SafeExtractOptions { strip_prefix },
    )
    .map_err(|err| match err {
        cabin_artifact::ArtifactError::MissingStripPrefix { strip_prefix } => {
            PortError::MissingStripPrefix {
                name: entry.descriptor.name.as_str().to_owned(),
                version: entry.descriptor.version.to_string(),
                strip_prefix,
            }
        }
        other => PortError::Extract {
            name: entry.descriptor.name.as_str().to_owned(),
            version: entry.descriptor.version.to_string(),
            source: Box::new(other),
        },
    })?;
    Ok(())
}

fn ensure_overlay(entry: &PortEntry, source_dir: &Path) -> Result<(), PortError> {
    let overlay_dest = source_dir.join("cabin.toml");
    let overlay_bytes: Vec<u8> = match &entry.origin {
        PortOrigin::PortDir(port_dir) => {
            let overlay_source = port_dir.join(&entry.descriptor.overlay.relative_path);
            if !overlay_source.is_file() {
                return Err(PortError::MissingOverlayManifest {
                    name: entry.descriptor.name.as_str().to_owned(),
                    version: entry.descriptor.version.to_string(),
                    path: overlay_source,
                });
            }
            fs::read(&overlay_source).map_err(|source| PortError::Fs {
                path: overlay_source,
                source,
            })?
        }
        PortOrigin::Builtin(name) => {
            // Pin the lookup to the version `build_plan_entries` already
            // resolved, so this fetch returns the same recipe in the
            // multi-version future. (With one bundled entry per name today,
            // the result is unchanged; the pin makes the code correct
            // whenever BUILTIN grows past size 1.)
            let pinned = semver::VersionReq::parse(&format!("={}", entry.descriptor.version))
                .expect("descriptor.version is a valid SemVer; the `=` requirement parses");
            let recipe =
                crate::builtin::lookup(name, &pinned).ok_or_else(|| PortError::UnknownBuiltin {
                    name: (*name).to_owned(),
                })?;
            recipe.overlay_toml.as_bytes().to_vec()
        }
    };
    write_atomic(&overlay_dest, &overlay_bytes).map_err(|source| PortError::Fs {
        path: overlay_dest,
        source,
    })?;
    Ok(())
}

fn cross_check_overlay_identity(entry: &PortEntry, source_dir: &Path) -> Result<(), PortError> {
    let overlay_manifest = source_dir.join("cabin.toml");
    let parsed = cabin_manifest::load_manifest(&overlay_manifest).map_err(|source| {
        PortError::OverlayManifestParse {
            name: entry.descriptor.name.as_str().to_owned(),
            version: entry.descriptor.version.to_string(),
            source: Box::new(source),
        }
    })?;
    let package = parsed
        .package
        .ok_or_else(|| PortError::OverlayMissingPackage {
            name: entry.descriptor.name.as_str().to_owned(),
            version: entry.descriptor.version.to_string(),
        })?;
    if package.name != entry.descriptor.name || package.version != entry.descriptor.version {
        return Err(PortError::OverlayIdentityMismatch {
            name: entry.descriptor.name.as_str().to_owned(),
            version: entry.descriptor.version.to_string(),
            actual_name: package.name.as_str().to_owned(),
            actual_version: package.version.to_string(),
        });
    }
    Ok(())
}

fn write_marker(source_dir: &Path) -> Result<(), PortError> {
    let marker = extraction_marker_path(source_dir);
    File::create(&marker)
        .map_err(|source| PortError::Fs {
            path: marker,
            source,
        })
        .map(|_| ())
}

fn stream_local_to_partial(source_path: &Path, tmp_target: &Path) -> Result<String, PortError> {
    let mut src = File::open(source_path).map_err(|source| PortError::Fs {
        path: source_path.to_path_buf(),
        source,
    })?;
    let mut dst = File::create(tmp_target).map_err(|source| PortError::Fs {
        path: tmp_target.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = src.read(&mut buf).map_err(|source| PortError::Fs {
            path: source_path.to_path_buf(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n]).map_err(|source| PortError::Fs {
            path: tmp_target.to_path_buf(),
            source,
        })?;
    }
    drop(dst);
    Ok(hex_digest(&hasher.finalize()))
}

fn write_bytes_to_partial(bytes: &[u8], tmp_target: &Path) -> Result<String, PortError> {
    let mut dst = File::create(tmp_target).map_err(|source| PortError::Fs {
        path: tmp_target.to_path_buf(),
        source,
    })?;
    dst.write_all(bytes).map_err(|source| PortError::Fs {
        path: tmp_target.to_path_buf(),
        source,
    })?;
    drop(dst);
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex_digest(&hasher.finalize()))
}

fn hash_file(path: &Path) -> Result<String, PortError> {
    let f = File::open(path).map_err(|source| PortError::Fs {
        path: path.to_path_buf(),
        source,
    })?;
    cabin_core::hash::hash_reader(f).map_err(|source| PortError::Fs {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::PortCache;
    use crate::model::{OverlayManifest, PortChecksum, PortDescriptor, PortMetadata, PortSource};
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use cabin_core::PackageName;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use semver::Version;
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use url::Url;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn make_archive(dir: &Path, name: &str, entries: &[(&str, &str)]) -> (PathBuf, String) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = fs::File::create(&path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        for (rel, body) in entries {
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
        let bytes = fs::read(&path).unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        (path, hex_digest(&h.finalize()))
    }

    fn lay_overlay(port_dir: &Path, body: &str) {
        assert_fs::fixture::ChildPath::new(port_dir.join("cabin.toml"))
            .write_str(body)
            .unwrap();
    }

    fn make_descriptor(url: Url, sha256_hex: &str) -> PortDescriptor {
        PortDescriptor {
            name: pkg("zlib"),
            version: Version::new(1, 3, 1),
            metadata: PortMetadata::default(),
            source: PortSource::Archive {
                url,
                sha256: PortChecksum::parse_hex(sha256_hex).unwrap(),
                strip_prefix: Some("zlib-1.3.1".to_owned()),
            },
            overlay: OverlayManifest {
                relative_path: PathBuf::from("cabin.toml"),
            },
        }
    }

    fn ok_overlay() -> &'static str {
        "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[target.zlib]\ntype = \"library\"\nsources = [\"zlib.c\"]\ninclude_dirs = [\".\"]\n"
    }

    #[test]
    fn prepares_port_from_local_archive() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "#define ZLIB_VERSION \"1.3.1\"\n"),
                ("zlib-1.3.1/zlib.c", "int zlib_dummy(void) { return 0; }\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir.clone()),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(result.ports.len(), 1);
        let prepared = &result.ports[0];
        assert!(prepared.source_dir.join("cabin.toml").is_file());
        assert!(prepared.source_dir.join("zlib.h").is_file());
        assert!(prepared.source_dir.join("zlib.c").is_file());
        // No `zlib-1.3.1/` survives the strip.
        assert!(!prepared.source_dir.join("zlib-1.3.1").exists());
        // Marker is a sibling.
        let mut marker = prepared.source_dir.as_os_str().to_owned();
        marker.push(".ok");
        assert!(Path::new(&marker).is_file());
        // Provenance is recorded.
        assert_eq!(prepared.provenance.sha256_hex, hex);
        assert_eq!(
            prepared.provenance.strip_prefix.as_deref(),
            Some("zlib-1.3.1")
        );
    }

    #[test]
    fn prepares_port_from_in_memory_archive() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let bytes = fs::read(&archive).unwrap();
        // No file URL for in-memory source.
        let descriptor = make_descriptor(
            Url::parse("https://example.com/zlib-1.3.1.tar.gz").unwrap(),
            &hex,
        );
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::InMemoryArchive(bytes),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert!(result.ports[0].source_dir.join("zlib.h").is_file());
    }

    #[test]
    fn reports_checksum_mismatch() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, _hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// stub\n")],
        );
        let bogus = "0".repeat(64);
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &bogus);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        match err {
            PortError::ChecksumMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, bogus);
                assert_ne!(actual, expected);
            }
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
    }

    #[test]
    fn reports_missing_strip_prefix() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[("other-1.0/zlib.h", "// nope\n")],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        match err {
            PortError::MissingStripPrefix {
                strip_prefix, name, ..
            } => {
                assert_eq!(strip_prefix, "zlib-1.3.1");
                assert_eq!(name, "zlib");
            }
            other => panic!("expected MissingStripPrefix, got {other:?}"),
        }
    }

    #[test]
    fn reports_overlay_identity_mismatch() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        // Overlay declares the wrong name/version.
        lay_overlay(
            &port_dir,
            "[package]\nname = \"other\"\nversion = \"9.9.9\"\n\n[target.zlib]\ntype = \"library\"\nsources = [\"zlib.c\"]\n",
        );
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        match err {
            PortError::OverlayIdentityMismatch {
                actual_name,
                actual_version,
                ..
            } => {
                assert_eq!(actual_name, "other");
                assert_eq!(actual_version, "9.9.9");
            }
            other => panic!("expected OverlayIdentityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn second_call_reuses_cached_prep_after_archive_disappears() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let make_plan = || PortPlan {
            entries: vec![PortEntry {
                descriptor: descriptor.clone(),
                origin: PortOrigin::PortDir(port_dir.clone()),
                source: PortFetchSource::LocalArchive(archive.clone()),
            }],
        };
        prepare(&make_plan(), &cache, PortPrepareOptions::default()).unwrap();
        fs::remove_file(&archive).unwrap();
        let r2 = prepare(&make_plan(), &cache, PortPrepareOptions::default()).unwrap();
        assert!(r2.ports[0].source_dir.join("cabin.toml").is_file());
    }

    #[test]
    fn re_extracts_when_marker_missing_even_if_manifest_present() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let source_dir = cache.source_dir(
            descriptor.name.as_str(),
            &descriptor.version.to_string(),
            &hex,
        );
        // Simulate an interrupted previous run: manifest present
        // but no completion marker.
        assert_fs::fixture::ChildPath::new(source_dir.join("cabin.toml"))
            .write_str("garbage")
            .unwrap();
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        let body = fs::read_to_string(source_dir.join("cabin.toml")).unwrap();
        assert!(
            body.contains("zlib"),
            "overlay should be re-applied: {body}"
        );
        let mut marker = source_dir.as_os_str().to_owned();
        marker.push(".ok");
        assert!(Path::new(&marker).is_file());
    }

    #[test]
    fn frozen_fails_on_cache_miss() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// stub\n")],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions { frozen: true }).unwrap_err();
        assert!(matches!(err, PortError::FrozenCacheMiss { .. }), "{err:?}");
    }

    #[test]
    fn frozen_succeeds_when_cache_is_populated() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let make_plan = || PortPlan {
            entries: vec![PortEntry {
                descriptor: descriptor.clone(),
                origin: PortOrigin::PortDir(port_dir.clone()),
                source: PortFetchSource::LocalArchive(archive.clone()),
            }],
        };
        prepare(&make_plan(), &cache, PortPrepareOptions::default()).unwrap();
        // Now run again with --frozen — should succeed.
        prepare(&make_plan(), &cache, PortPrepareOptions { frozen: true }).unwrap();
    }

    #[test]
    fn reports_missing_archive_for_nonexistent_local_path() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let descriptor = make_descriptor(
            Url::parse("file:///nonexistent/zlib.tar.gz").unwrap(),
            &"a".repeat(64),
        );
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(PathBuf::from("/nonexistent/zlib.tar.gz")),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(matches!(err, PortError::MissingArchive { .. }), "{err:?}");
    }

    #[test]
    fn reports_missing_overlay_manifest() {
        let dir = TempDir::new().unwrap();
        let port_dir_child = dir.child("port");
        // Port dir exists but overlay file does not.
        port_dir_child.create_dir_all().unwrap();
        let port_dir = port_dir_child.to_path_buf();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let err = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap_err();
        assert!(
            matches!(err, PortError::MissingOverlayManifest { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn prepares_port_from_builtin_origin() {
        use crate::builtin::lookup;
        let dir = TempDir::new().unwrap();
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", "// stub\n"),
                ("zlib-1.3.1/zlib.c", "// stub\n"),
            ],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));
        // The descriptor identity must match the bundled zlib overlay's
        // [package] block (zlib 1.3.1) so the identity cross-check passes.
        assert_eq!(descriptor.name.as_str(), "zlib");
        assert_eq!(descriptor.version, Version::new(1, 3, 1));
        assert!(
            lookup("zlib", &semver::VersionReq::parse(">=0").unwrap()).is_some(),
            "zlib must be bundled"
        );
        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::Builtin("zlib"),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(result.ports.len(), 1);
        let prepared = &result.ports[0];
        let overlay = std::fs::read_to_string(prepared.source_dir.join("cabin.toml")).unwrap();
        assert!(overlay.contains("name = \"zlib\""), "overlay: {overlay}");
        assert!(overlay.contains("target.zlib"), "overlay: {overlay}");
        assert!(matches!(&prepared.origin, PortOrigin::Builtin("zlib")));
    }

    /// Two port descriptors that intentionally reuse the same
    /// upstream archive — different package identities (different
    /// `[package].name`) shipping different overlays — must
    /// extract into distinct directories so the later overlay
    /// cannot clobber the earlier one's `cabin.toml`.
    #[test]
    fn distinct_identities_do_not_share_one_extracted_tree() {
        let dir = TempDir::new().unwrap();
        // Build one archive whose contents both descriptors claim
        // to ship. The archive uses neither port's name in its
        // strip prefix so we can point both descriptors at it.
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "shared.tar.gz",
            &[
                ("upstream/header.h", "// shared header\n"),
                ("upstream/source.c", "// shared source\n"),
            ],
        );

        // Two ports — different names — with the same archive.
        let alpha_dir = dir.path().join("port-a");
        lay_overlay(
            &alpha_dir,
            "[package]\nname = \"alpha\"\nversion = \"1.0.0\"\n",
        );
        let beta_dir = dir.path().join("port-b");
        lay_overlay(
            &beta_dir,
            "[package]\nname = \"beta\"\nversion = \"1.0.0\"\n",
        );

        let mk = |name_lit: &str| PortDescriptor {
            name: pkg(name_lit),
            version: Version::new(1, 0, 0),
            metadata: PortMetadata::default(),
            source: PortSource::Archive {
                url: Url::from_file_path(&archive).unwrap(),
                sha256: PortChecksum::parse_hex(&hex).unwrap(),
                strip_prefix: Some("upstream".to_owned()),
            },
            overlay: OverlayManifest {
                relative_path: PathBuf::from("cabin.toml"),
            },
        };

        let cache = PortCache::new(dir.path().join("cache"));
        let plan = PortPlan {
            entries: vec![
                PortEntry {
                    descriptor: mk("alpha"),
                    origin: PortOrigin::PortDir(alpha_dir),
                    source: PortFetchSource::LocalArchive(archive.clone()),
                },
                PortEntry {
                    descriptor: mk("beta"),
                    origin: PortOrigin::PortDir(beta_dir),
                    source: PortFetchSource::LocalArchive(archive),
                },
            ],
        };

        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(result.ports.len(), 2);
        let alpha = &result.ports[0];
        let beta = &result.ports[1];
        assert_ne!(
            alpha.source_dir, beta.source_dir,
            "distinct identities must not collide on one source dir"
        );
        let alpha_overlay = std::fs::read_to_string(alpha.source_dir.join("cabin.toml")).unwrap();
        let beta_overlay = std::fs::read_to_string(beta.source_dir.join("cabin.toml")).unwrap();
        assert!(alpha_overlay.contains("\"alpha\""), "{alpha_overlay}");
        assert!(beta_overlay.contains("\"beta\""), "{beta_overlay}");
    }

    /// Self-healing path: when the content-addressed archive
    /// already exists but its bytes do not match the recorded
    /// hash (corrupted cache entry, interrupted write), prepare
    /// must overwrite it rather than fail. Windows refuses
    /// `fs::rename` over an existing destination, so the recovery
    /// path has to remove the stale file first; this regression
    /// pins that behavior on every platform.
    #[test]
    fn stale_cached_archive_is_replaced_atomically() {
        let dir = TempDir::new().unwrap();
        let port_dir = dir.path().join("port");
        lay_overlay(&port_dir, ok_overlay());
        let (archive, hex) = make_archive(
            &dir.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[("zlib-1.3.1/zlib.h", "// good bytes\n")],
        );
        let descriptor = make_descriptor(Url::from_file_path(&archive).unwrap(), &hex);
        let cache = PortCache::new(dir.path().join("cache"));

        // Pre-populate the content-addressed slot with bytes that
        // do *not* hash to `hex`. A naive `fs::rename` over this
        // file would error on Windows.
        let cached_path = cache.archive_path(&hex);
        assert_fs::fixture::ChildPath::new(&cached_path)
            .write_binary(b"corrupt")
            .unwrap();

        let plan = PortPlan {
            entries: vec![PortEntry {
                descriptor,
                origin: PortOrigin::PortDir(port_dir),
                source: PortFetchSource::LocalArchive(archive),
            }],
        };
        let result = prepare(&plan, &cache, PortPrepareOptions::default()).unwrap();
        assert_eq!(result.ports.len(), 1);

        // The stale bytes are gone; the recovered archive hashes
        // to the declared SHA-256 again.
        let mut h = Sha256::new();
        h.update(fs::read(&cached_path).unwrap());
        assert_eq!(hex_digest(&h.finalize()), hex);
    }
}
