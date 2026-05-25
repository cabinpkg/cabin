use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use cabin_core::PackageName;

use crate::error::ArtifactError;

/// Maximum decompressed bytes Cabin will write for a single tar
/// entry.  Single source files larger than 256 MiB do not occur in
/// any C/C++ package this tool is expected to ingest; the cap
/// exists to refuse a `.tar.gz` whose entry headers claim a huge
/// `size` and whose gzip stream expands to that size from a tiny
/// compressed payload (a "decompression bomb").
const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

/// Maximum aggregate decompressed bytes Cabin will write across
/// every entry in one archive.  Even with the per-entry cap, an
/// attacker could ship thousands of max-size entries to fill the
/// user's disk; the aggregate cap bounds total damage to ~1 GiB.
const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// Maximum number of tar entries Cabin will process from one
/// archive.  Headers alone (no body) can be cheap to ship and
/// expensive to materialise as filesystem inodes, so the count
/// is capped independently of the byte caps.
const MAX_ENTRIES: usize = 10_000;

/// Options accepted by [`safe_extract_tar_gz`].
///
/// `Default` produces the original artifact-layer behaviour: no
/// prefix stripping, archive is expected to contain `cabin.toml`
/// at its root.
#[derive(Debug, Clone, Copy, Default)]
pub struct SafeExtractOptions<'a> {
    /// If `Some`, every archive entry must start with this single
    /// directory component; the component is stripped before the
    /// path is joined into `dest`. The post-strip path is then
    /// re-checked by the same path-safety rules as a top-level
    /// entry, so a malicious archive that ships
    /// `<prefix>/../escape` is rejected after the strip.
    ///
    /// An archive that does not contain a single entry beginning
    /// with `strip_prefix` produces
    /// [`ArtifactError::MissingStripPrefix`].
    pub strip_prefix: Option<&'a str>,
}

/// Safely extract a `.tar.gz` archive into `dest` with the default
/// production caps and no prefix stripping. Kept as the
/// crate-internal entry point used by the source-archive fetcher.
pub(crate) fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), ArtifactError> {
    safe_extract_tar_gz_with_limits(
        archive,
        dest,
        MAX_ENTRY_BYTES,
        MAX_TOTAL_BYTES,
        MAX_ENTRIES,
        SafeExtractOptions::default(),
    )
}

/// Safely extract a `.tar.gz` archive into `dest`, with caller-
/// supplied options.
///
/// Fail-closed rules:
/// - reject entries with absolute paths or `..` components;
/// - reject entries whose joined destination escapes `dest`;
/// - accept only `Regular` files and `Directory` entries — every other
///   tar entry type (symlinks, hard links, char/block devices, fifos,
///   sparse, etc.) is rejected;
/// - cap per-entry decompressed bytes, aggregate decompressed
///   bytes, and total entry count so a decompression-bomb archive
///   (small compressed payload, huge decompressed output) cannot
///   fill the user's disk;
/// - when [`SafeExtractOptions::strip_prefix`] is set, require
///   every entry to begin with that single directory component
///   and re-run path-safety checks on the post-strip path. An
///   archive whose entries never match the declared prefix
///   surfaces [`ArtifactError::MissingStripPrefix`].
pub fn safe_extract_tar_gz(
    archive: &Path,
    dest: &Path,
    options: SafeExtractOptions<'_>,
) -> Result<(), ArtifactError> {
    safe_extract_tar_gz_with_limits(
        archive,
        dest,
        MAX_ENTRY_BYTES,
        MAX_TOTAL_BYTES,
        MAX_ENTRIES,
        options,
    )
}

fn safe_extract_tar_gz_with_limits(
    archive: &Path,
    dest: &Path,
    max_entry_bytes: u64,
    max_total_bytes: u64,
    max_entries: usize,
    options: SafeExtractOptions<'_>,
) -> Result<(), ArtifactError> {
    let f = File::open(archive).map_err(|source| ArtifactError::Io {
        path: archive.to_path_buf(),
        source,
    })?;
    let dec = flate2::read::GzDecoder::new(f);
    let mut tar = tar::Archive::new(dec);

    let entries = tar.entries().map_err(|source| ArtifactError::Extract {
        path: archive.to_path_buf(),
        source,
    })?;

    let mut total_bytes: u64 = 0;
    let mut entry_count: usize = 0;
    let mut saw_prefix = false;

    for entry_result in entries {
        entry_count += 1;
        if entry_count > max_entries {
            return Err(ArtifactError::ArchiveTooManyEntries { limit: max_entries });
        }

        let mut entry = entry_result.map_err(|source| ArtifactError::Extract {
            path: archive.to_path_buf(),
            source,
        })?;
        let entry_kind = entry.header().entry_type();
        // Skip GNU/PAX metadata records (long-path markers, extended
        // headers, global PAX state) *before* path validation: the
        // standard tar reader has already consumed their payload to
        // populate the next real entry's header, and their literal
        // path is a synthetic marker like `././@LongLink` that fails
        // the prefix check even though no file is being extracted.
        // Real archives produced by GNU `tar` routinely include
        // these records, so deferring this skip to `write_entry`
        // would let `MissingStripPrefix` reject otherwise-valid
        // tarballs.
        if matches!(
            entry_kind,
            tar::EntryType::GNULongName
                | tar::EntryType::GNULongLink
                | tar::EntryType::XHeader
                | tar::EntryType::XGlobalHeader
        ) {
            continue;
        }
        let entry_path: PathBuf = entry
            .path()
            .map_err(|source| ArtifactError::Extract {
                path: archive.to_path_buf(),
                source,
            })?
            .into_owned();
        let display = entry_path.to_string_lossy().into_owned();

        let Some(target) = resolve_safe_target(&entry_path, dest, options, &mut saw_prefix)? else {
            continue;
        };

        write_entry(
            &mut entry,
            entry_kind,
            &target,
            &display,
            max_entry_bytes,
            max_total_bytes,
            &mut total_bytes,
        )?;
    }
    if let Some(prefix) = options.strip_prefix
        && !saw_prefix
    {
        return Err(ArtifactError::MissingStripPrefix {
            strip_prefix: prefix.to_owned(),
        });
    }
    Ok(())
}

/// Apply path-safety + optional `strip_prefix` to `entry_path`
/// and return the absolute target under `dest`.
///
/// Returns `Ok(None)` when the entry was the prefix directory
/// itself (nothing to extract). Returns
/// [`ArtifactError::MissingStripPrefix`] when an entry's first
/// component does not match the declared prefix; this surfaces
/// the actionable diagnostic the user can fix by correcting
/// `port.toml`.
fn resolve_safe_target(
    entry_path: &Path,
    dest: &Path,
    options: SafeExtractOptions<'_>,
    saw_prefix: &mut bool,
) -> Result<Option<PathBuf>, ArtifactError> {
    let display = || entry_path.to_string_lossy().into_owned();

    // First pass: the raw entry path must be a safe relative
    // path even before stripping. Catches `../escape` and
    // absolute paths in the literal entry header.
    if !is_safe_relative_path(entry_path) {
        return Err(ArtifactError::UnsafeArchiveEntry(display()));
    }

    let stripped: PathBuf = match options.strip_prefix {
        None => entry_path.to_path_buf(),
        Some(prefix) => {
            let mut components = entry_path.components();
            // Skip leading `./` segments. GNU tar (and several
            // common archiving tools) emit `./<prefix>/...`
            // entries; treating them as missing the prefix would
            // reject otherwise-valid tarballs.
            let mut first = components.next();
            while matches!(first, Some(Component::CurDir)) {
                first = components.next();
            }
            match first {
                // Bare `./` (or any pure `./././…` chain): this is
                // a harmless root marker `tar` emits for archives
                // built from `.`. Skip the entry rather than
                // failing the whole extraction — the prefix gets
                // observed on subsequent real entries.
                None => return Ok(None),
                Some(Component::Normal(name)) if name == std::ffi::OsStr::new(prefix) => {
                    *saw_prefix = true;
                    components.as_path().to_path_buf()
                }
                _ => {
                    return Err(ArtifactError::MissingStripPrefix {
                        strip_prefix: prefix.to_owned(),
                    });
                }
            }
        }
    };

    if stripped.as_os_str().is_empty() {
        return Ok(None);
    }

    // Re-validate after stripping in case the post-strip path
    // picked up unsafe components.
    if !is_safe_relative_path(&stripped) {
        return Err(ArtifactError::UnsafeArchiveEntry(display()));
    }
    let target = dest.join(&stripped);
    if !target.starts_with(dest) {
        return Err(ArtifactError::UnsafeArchiveEntry(display()));
    }
    Ok(Some(target))
}

/// Write one tar entry to `target`. Enforces the byte caps and
/// removes any partial file when a cap is exceeded.
fn write_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    entry_kind: tar::EntryType,
    target: &Path,
    display: &str,
    max_entry_bytes: u64,
    max_total_bytes: u64,
    total_bytes: &mut u64,
) -> Result<(), ArtifactError> {
    match entry_kind {
        tar::EntryType::Directory => {
            fs::create_dir_all(target).map_err(|source| ArtifactError::Io {
                path: target.to_path_buf(),
                source,
            })?;
            Ok(())
        }
        tar::EntryType::Regular => {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|source| ArtifactError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            let mut out = File::create(target).map_err(|source| ArtifactError::Io {
                path: target.to_path_buf(),
                source,
            })?;
            // Cap the read at one byte over the per-entry
            // limit so a successful copy of exactly the limit
            // is distinguishable from an overflow.
            let mut limited = entry.take(max_entry_bytes + 1);
            let written = io::copy(&mut limited, &mut out).map_err(|source| ArtifactError::Io {
                path: target.to_path_buf(),
                source,
            })?;
            if written > max_entry_bytes {
                drop(out);
                let _ = fs::remove_file(target);
                return Err(ArtifactError::ArchiveEntryTooLarge {
                    path: display.to_owned(),
                    limit: max_entry_bytes,
                });
            }
            *total_bytes = total_bytes.saturating_add(written);
            if *total_bytes > max_total_bytes {
                drop(out);
                let _ = fs::remove_file(target);
                return Err(ArtifactError::ArchiveTooLarge {
                    limit: max_total_bytes,
                });
            }
            Ok(())
        }
        // Tar metadata entries carry side-band data the
        // standard tar reader already consumes (long paths, PAX
        // extended headers, global PAX state) — the subsequent
        // real file entry exposes the resolved path via its own
        // header, so skipping these is correct. Real source
        // archives, including ones produced by `git archive` and
        // GNU `tar` for foundation-port releases, routinely
        // include such records.
        tar::EntryType::GNULongName
        | tar::EntryType::GNULongLink
        | tar::EntryType::XHeader
        | tar::EntryType::XGlobalHeader => Ok(()),
        // Reject every other entry type by design (symlinks,
        // hard links, char/block devices, fifos, sparse, etc.).
        // Cabin source archives only need regular files and
        // directories.
        _ => Err(ArtifactError::UnsupportedArchiveEntry(display.to_owned())),
    }
}

/// Validate that an extracted source tree at `source_dir` matches the
/// resolved package's `name` and `version`.
pub(crate) fn validate_extracted(
    source_dir: &Path,
    name: &PackageName,
    version: &semver::Version,
) -> Result<(), ArtifactError> {
    let manifest_path = source_dir.join("cabin.toml");
    if !manifest_path.is_file() {
        return Err(ArtifactError::MissingArchiveManifest {
            name: name.as_str().to_owned(),
            version: version.to_string(),
        });
    }
    let parsed = cabin_manifest::load_manifest(&manifest_path).map_err(|source| {
        ArtifactError::Manifest {
            path: manifest_path.clone(),
            source: Box::new(source),
        }
    })?;
    let package = parsed
        .package
        .ok_or_else(|| ArtifactError::MissingArchiveManifest {
            name: name.as_str().to_owned(),
            version: version.to_string(),
        })?;
    if package.name != *name || package.version != *version {
        return Err(ArtifactError::ManifestMismatch {
            name: name.as_str().to_owned(),
            version: version.to_string(),
            actual_name: package.name.as_str().to_owned(),
            actual_version: package.version.to_string(),
        });
    }
    Ok(())
}

/// A path is safe-to-extract when every component is normal or `.` and
/// the path is relative.
fn is_safe_relative_path(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use predicates::prelude::*;
    use std::io::Write;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    /// Build a `.tar.gz` containing a regular file at `path` whose body
    /// is `body`.
    fn make_archive(archive_path: &Path, entries: &[(&str, &str)]) {
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = File::create(archive_path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        for (rel_path, body) in entries {
            let bytes = body.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(&mut header, rel_path, &mut std::io::Cursor::new(bytes))
                .unwrap();
        }
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
    }

    /// Build a `.tar.gz` whose first entry has its `name` field written
    /// directly. This bypasses `Header::set_path`'s validation, which
    /// would reject `..` and absolute paths.
    fn make_archive_with_raw_name(
        archive_path: &Path,
        raw_name: &str,
        entry_type: tar::EntryType,
        link_name: Option<&str>,
        body: &[u8],
    ) {
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let f = File::create(archive_path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);

        let mut header = tar::Header::new_old();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(entry_type);
        if let Some(target) = link_name {
            // `set_link_name` validates and rejects `..` / absolutes,
            // so write the bytes directly into the OldHeader's
            // `linkname` field.
            let bytes = target.as_bytes();
            let old = header.as_old_mut();
            for b in &mut old.linkname[..] {
                *b = 0;
            }
            let n = bytes.len().min(old.linkname.len());
            old.linkname[..n].copy_from_slice(&bytes[..n]);
        }
        {
            // Same trick for the entry name.
            let bytes = raw_name.as_bytes();
            let old = header.as_old_mut();
            for b in &mut old.name[..] {
                *b = 0;
            }
            let n = bytes.len().min(old.name.len());
            old.name[..n].copy_from_slice(&bytes[..n]);
        }
        header.set_cksum();
        builder.append(&header, body).unwrap();
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
    }

    #[test]
    fn extracts_simple_archive() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("ok.tar.gz");
        make_archive(
            archive.path(),
            &[
                (
                    "cabin.toml",
                    "[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n",
                ),
                ("src/main.cc", "int main() { return 0; }\n"),
            ],
        );

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        extract_tar_gz(archive.path(), dest.path()).unwrap();
        dest.child("cabin.toml").assert(predicate::path::is_file());
        dest.child("src/main.cc").assert(predicate::path::is_file());
    }

    #[test]
    fn rejects_parent_dir_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "../escape.txt",
            tar::EntryType::Regular,
            None,
            b"evil",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = extract_tar_gz(archive.path(), dest.path()).unwrap_err();
        match err {
            ArtifactError::UnsafeArchiveEntry(p) => assert!(p.contains("..")),
            other => panic!("expected UnsafeArchiveEntry, got {other:?}"),
        }
        // Nothing escaped.
        dir.child("escape.txt").assert(predicate::path::missing());
    }

    #[test]
    fn rejects_absolute_path_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "/etc/passwd",
            tar::EntryType::Regular,
            None,
            b"evil",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = extract_tar_gz(archive.path(), dest.path()).unwrap_err();
        assert!(matches!(err, ArtifactError::UnsafeArchiveEntry(_)));
    }

    #[test]
    fn rejects_symlink_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "evil",
            tar::EntryType::Symlink,
            Some("/etc/passwd"),
            b"",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = extract_tar_gz(archive.path(), dest.path()).unwrap_err();
        assert!(matches!(err, ArtifactError::UnsupportedArchiveEntry(_)));
    }

    #[test]
    fn rejects_hard_link_entry() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "alias",
            tar::EntryType::Link,
            Some("cabin.toml"),
            b"",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = extract_tar_gz(archive.path(), dest.path()).unwrap_err();
        assert!(matches!(err, ArtifactError::UnsupportedArchiveEntry(_)));
    }

    #[test]
    fn validate_extracted_accepts_matching_manifest() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        validate_extracted(dir.path(), &pkg("fmt"), &ver("10.2.1")).unwrap();
    }

    #[test]
    fn validate_extracted_rejects_missing_manifest() {
        let dir = TempDir::new().unwrap();
        let err = validate_extracted(dir.path(), &pkg("fmt"), &ver("10.2.1")).unwrap_err();
        assert!(matches!(err, ArtifactError::MissingArchiveManifest { .. }));
    }

    #[test]
    fn validate_extracted_rejects_name_mismatch() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"other\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let err = validate_extracted(dir.path(), &pkg("fmt"), &ver("10.2.1")).unwrap_err();
        match err {
            ArtifactError::ManifestMismatch {
                actual_name,
                actual_version,
                ..
            } => {
                assert_eq!(actual_name, "other");
                assert_eq!(actual_version, "10.2.1");
            }
            other => panic!("expected ManifestMismatch, got {other:?}"),
        }
    }

    #[test]
    fn rejects_archive_entry_exceeding_per_entry_limit() {
        // A single entry whose decompressed body would exceed the
        // per-entry cap is refused before the bomb is written to
        // disk. The half-written file is removed so a bomb does
        // not leave a max-size carcass behind.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bomb.tar.gz");
        let body = "x".repeat(2048);
        make_archive(archive.path(), &[("cabin.toml", body.as_str())]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz_with_limits(
            archive.path(),
            dest.path(),
            1024,
            1_000_000,
            1000,
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        match err {
            ArtifactError::ArchiveEntryTooLarge { path, limit } => {
                assert_eq!(path, "cabin.toml");
                assert_eq!(limit, 1024);
            }
            other => panic!("expected ArchiveEntryTooLarge, got {other:?}"),
        }
        dest.child("cabin.toml").assert(predicate::path::missing());
    }

    #[test]
    fn rejects_archive_exceeding_aggregate_size_limit() {
        // Each entry fits under the per-entry cap, but the sum
        // exceeds the aggregate cap. Refused on the entry whose
        // write pushes the running total over.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("aggregate-bomb.tar.gz");
        let body = "x".repeat(700);
        make_archive(
            archive.path(),
            &[("a.txt", body.as_str()), ("b.txt", body.as_str())],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz_with_limits(
            archive.path(),
            dest.path(),
            1024,
            1000,
            1000,
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        match err {
            ArtifactError::ArchiveTooLarge { limit } => assert_eq!(limit, 1000),
            other => panic!("expected ArchiveTooLarge, got {other:?}"),
        }
        dest.child("b.txt").assert(predicate::path::missing());
    }

    #[test]
    fn rejects_archive_with_too_many_entries() {
        // Headers can be cheap to ship and expensive to
        // materialise as inodes; the entry-count cap fires
        // independently of byte caps.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("many.tar.gz");
        make_archive(
            archive.path(),
            &[
                ("a.txt", "x"),
                ("b.txt", "x"),
                ("c.txt", "x"),
                ("d.txt", "x"),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz_with_limits(
            archive.path(),
            dest.path(),
            1024,
            1_000_000,
            3,
            SafeExtractOptions::default(),
        )
        .unwrap_err();
        match err {
            ArtifactError::ArchiveTooManyEntries { limit } => assert_eq!(limit, 3),
            other => panic!("expected ArchiveTooManyEntries, got {other:?}"),
        }
    }

    #[test]
    fn accepts_archive_just_under_limits() {
        // Positive control: the bomb caps must not regress the
        // happy path for archives that sit under every limit.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("ok.tar.gz");
        make_archive(
            archive.path(),
            &[
                (
                    "cabin.toml",
                    "[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n",
                ),
                ("src/main.cc", "int main() { return 0; }\n"),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_tar_gz_with_limits(
            archive.path(),
            dest.path(),
            4096,
            1_000_000,
            1000,
            SafeExtractOptions::default(),
        )
        .unwrap();
        dest.child("cabin.toml").assert(predicate::path::is_file());
        dest.child("src/main.cc").assert(predicate::path::is_file());
    }

    #[test]
    fn strip_prefix_removes_leading_dir() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("zlib.tar.gz");
        make_archive(
            archive.path(),
            &[
                ("zlib-1.3.1/zlib.h", "#define ZLIB_VERSION \"1.3.1\"\n"),
                (
                    "zlib-1.3.1/src/adler32.c",
                    "int adler32(void) { return 0; }\n",
                ),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
            },
        )
        .unwrap();
        dest.child("zlib.h").assert(predicate::path::is_file());
        dest.child("src/adler32.c")
            .assert(predicate::path::is_file());
        // The prefix directory must not have been re-created
        // inside the destination.
        dest.child("zlib-1.3.1").assert(predicate::path::missing());
    }

    /// GNU tar and `git archive --format=tar` commonly emit
    /// entries with a leading `./` segment. The strip-prefix
    /// matcher must skip those before comparing to the declared
    /// prefix; otherwise a perfectly valid tarball is rejected.
    #[test]
    fn strip_prefix_accepts_leading_dot_slash_segments() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("zlib.tar.gz");
        make_archive(
            archive.path(),
            &[
                ("./zlib-1.3.1/zlib.h", "#define ZLIB_VERSION \"1.3.1\"\n"),
                (
                    "./zlib-1.3.1/src/adler32.c",
                    "int adler32(void) { return 0; }\n",
                ),
            ],
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
            },
        )
        .unwrap();
        dest.child("zlib.h").assert(predicate::path::is_file());
        dest.child("src/adler32.c")
            .assert(predicate::path::is_file());
        dest.child("zlib-1.3.1").assert(predicate::path::missing());
    }

    #[test]
    fn strip_prefix_skips_the_prefix_directory_entry_itself() {
        // Archives commonly include a directory entry for the
        // prefix dir; stripping that entry must not produce an
        // empty target path or escape the destination.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("zlib.tar.gz");
        let f = File::create(archive.path()).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Directory);
            header.set_cksum();
            builder
                .append_data(&mut header, "zlib-1.3.1/", &mut std::io::Cursor::new(b""))
                .unwrap();
        }
        let body = b"ok\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                "zlib-1.3.1/zlib.h",
                &mut std::io::Cursor::new(&body[..]),
            )
            .unwrap();
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
            },
        )
        .unwrap();
        dest.child("zlib.h").assert(predicate::path::is_file());
    }

    #[test]
    fn strip_prefix_rejects_archive_without_matching_root() {
        let dir = TempDir::new().unwrap();
        let archive = dir.child("other.tar.gz");
        make_archive(archive.path(), &[("not-zlib/zlib.h", "// nope\n")]);
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::MissingStripPrefix { ref strip_prefix } if strip_prefix == "zlib-1.3.1"),
            "{err:?}"
        );
    }

    #[test]
    fn strip_prefix_reports_missing_prefix_on_empty_archive() {
        // An empty archive (or one whose entries never start
        // with the declared prefix) surfaces a dedicated
        // MissingStripPrefix error. Build a minimal archive
        // containing only the gzip footer.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("empty.tar.gz");
        let f = File::create(archive.path()).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let builder = tar::Builder::new(enc);
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();

        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, ArtifactError::MissingStripPrefix { ref strip_prefix } if strip_prefix == "zlib-1.3.1"),
            "{err:?}"
        );
    }

    #[test]
    fn strip_prefix_keeps_path_safety_after_strip() {
        // Even if the archive's root dir is stripped, the
        // post-strip path must still pass `is_safe_relative_path`.
        let dir = TempDir::new().unwrap();
        let archive = dir.child("bad.tar.gz");
        make_archive_with_raw_name(
            archive.path(),
            "zlib-1.3.1/../escape.txt",
            tar::EntryType::Regular,
            None,
            b"evil",
        );
        let dest = dir.child("out");
        dest.create_dir_all().unwrap();
        let err = safe_extract_tar_gz(
            archive.path(),
            dest.path(),
            SafeExtractOptions {
                strip_prefix: Some("zlib-1.3.1"),
            },
        )
        .unwrap_err();
        // The pre-strip path-safety check fires first because
        // the literal entry contains `..`.
        assert!(
            matches!(err, ArtifactError::UnsafeArchiveEntry(_)),
            "{err:?}"
        );
        dir.child("escape.txt").assert(predicate::path::missing());
    }

    #[test]
    fn validate_extracted_rejects_version_mismatch() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.1.0\"\n")
            .unwrap();
        let err = validate_extracted(dir.path(), &pkg("fmt"), &ver("10.2.1")).unwrap_err();
        assert!(matches!(err, ArtifactError::ManifestMismatch { .. }));
    }
}
