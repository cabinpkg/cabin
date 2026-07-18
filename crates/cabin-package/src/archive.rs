use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipWriter};

use crate::error::PackageError;

/// Conventional package-archive root entry.
pub const ROOT_MANIFEST_NAME: &str = "cabin.toml";

/// Top-level directory names that are excluded from package archives
/// by default.  Matched anywhere in the tree, including below the root, so
/// nested submodules / build trees do not leak in.
pub const EXCLUDED_DIR_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".cabin",
    "build",
    "dist",
    "node_modules",
];

/// File names excluded from package archives by default.  Matched
/// anywhere in the tree.
pub const EXCLUDED_FILE_NAMES: &[&str] = &[
    ".DS_Store",
    "compile_commands.json",
    "build.ninja",
    "cabin.lock",
];

/// One file slated for inclusion in a package archive.
#[derive(Debug, Clone)]
pub struct PackageFile {
    /// Relative path under the package root, with forward slashes.
    /// Matches the on-disk shape that `cabin-artifact` extracts back
    /// out: `cabin.toml` lives at the top, sources / headers below.
    pub rel_path: String,
    /// Absolute filesystem path the contents will be read from at
    /// archive time.
    pub abs_path: PathBuf,
}

/// Walk `root` and collect every file that should appear in the
/// package archive, applying the include / exclude policy and
/// rejecting unsupported entry types (symlinks, hard links, special
/// files).
///
/// `exclude_dir`, when set, names one additional directory whose
/// contents must be omitted from the archive.  The walker compares
/// the absolute path of each descended directory against this
/// value and skips on equality.  Callers (notably `package_with_project`)
/// pass the resolved `--output-dir` so a previous run's archive
/// living inside the package source tree does not leak into the
/// next archive.
///
/// The returned list is sorted lexicographically by `rel_path` so
/// archive output is deterministic without callers having to sort
/// again.
///
/// # Errors
/// Returns [`PackageError::Io`] when reading a directory or querying
/// an entry's type fails, [`PackageError::NonUtf8Path`] for a
/// non-UTF-8 file name, [`PackageError::SymlinkNotSupported`] for a
/// symlink, and [`PackageError::UnsupportedFileType`] for any entry
/// that is neither a regular file nor a directory.
pub fn collect_package_files(
    root: &Path,
    exclude_dir: Option<&Path>,
) -> Result<Vec<PackageFile>, PackageError> {
    let mut out = Vec::new();
    walk(root, root, exclude_dir, &mut out)?;
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    reject_case_conflicts(&out)?;
    Ok(out)
}

/// Reject a file set that a case-insensitive filesystem (macOS,
/// Windows) cannot materialize faithfully, mirroring the registry
/// verifier's `case_conflict` rule so a source tree that packages on a
/// case-sensitive host does not earn an asynchronous registry
/// rejection. Two forms collide: two entries whose paths fold to the
/// same string under Unicode default lowercasing (`README` and
/// `readme`), and a file whose name folds to a directory component of
/// another entry (`A` alongside `a/b`). The exact-match forms the
/// verifier also handles (`duplicate_path`, `path_conflict`) cannot
/// arise from a real directory tree, so only the case-folded forms are
/// checked here.
fn reject_case_conflicts(files: &[PackageFile]) -> Result<(), PackageError> {
    let mut folded: std::collections::HashMap<String, String> =
        std::collections::HashMap::with_capacity(files.len());
    for file in files {
        if let Some(existing) = folded.insert(file.rel_path.to_lowercase(), file.rel_path.clone()) {
            return Err(PackageError::CaseConflictingPaths {
                first: existing,
                second: file.rel_path.clone(),
            });
        }
    }
    for file in files {
        let path = &file.rel_path;
        let mut boundary = 0;
        while let Some(slash) = path[boundary..].find('/') {
            boundary += slash;
            if let Some(existing) = folded.get(&path[..boundary].to_lowercase()) {
                return Err(PackageError::CaseConflictingPaths {
                    first: existing.clone(),
                    second: path.clone(),
                });
            }
            boundary += 1;
        }
    }
    Ok(())
}

/// Reject the archive if `cabin.toml` is not at its root.  Practically
/// the manifest is always in the source tree, but the check protects
/// callers from misuse (e.g. an output dir set to the package root,
/// where the archive contract - `cabin.toml` at the root - would
/// silently break).
///
/// # Errors
/// Returns [`PackageError::ArchiveMissingManifest`] when no entry has
/// a `rel_path` equal to `cabin.toml`.
pub fn ensure_manifest_included(files: &[PackageFile]) -> Result<(), PackageError> {
    if !files.iter().any(|f| f.rel_path == ROOT_MANIFEST_NAME) {
        return Err(PackageError::ArchiveMissingManifest);
    }
    Ok(())
}

/// Build a deterministic `.zip` for `files`, following the strict
/// registry archive profile (see `docs/remote-registry.md`).
///
/// `manifest_substitute`, when `Some`, replaces the `cabin.toml`
/// entry's on-disk contents with the given bytes.  The staging layer
/// uses it to normalize `{ workspace = true }` standard markers into
/// resolved literals so the archived manifest is self-contained.
///
/// Determinism rules baked into this writer:
/// - entries are written in the order their `rel_path` was sorted
///   into (files only - directories are implied by the extractor);
/// - the 1980-01-01 default timestamp is pinned in both DOS fields;
/// - `System::Unix` overrides the writer's platform-dependent
///   version-made-by (DOS on Windows) so the same logical input
///   produces the same bytes regardless of host;
/// - `large_file(false)` keeps the zip64 extra field out, and the
///   writer emits no extra fields, so the container stays minimal.
///
/// Every option pin is load-bearing for byte-reproducibility; a
/// zip/flate2 version bump that changes the output must be a
/// deliberate regeneration.
///
/// # Errors
/// Returns [`PackageError::Io`] when a file's bytes cannot be read,
/// and [`PackageError::ArchiveWrite`] when writing an entry or
/// finishing the zip stream fails.
pub fn build_zip(
    files: &[PackageFile],
    manifest_substitute: Option<&[u8]>,
) -> Result<Vec<u8>, PackageError> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(DateTime::default())
        .large_file(false)
        .system(System::Unix);
    for file in files {
        let bytes = match manifest_substitute {
            Some(substitute) if file.rel_path == ROOT_MANIFEST_NAME => substitute.to_vec(),
            _ => fs::read(&file.abs_path).map_err(|source| PackageError::Io {
                path: file.abs_path.clone(),
                source,
            })?,
        };
        writer
            .start_file(&file.rel_path, options)
            .map_err(zip_write_error)?;
        writer
            .write_all(&bytes)
            .map_err(PackageError::ArchiveWrite)?;
    }
    let cursor = writer.finish().map_err(zip_write_error)?;
    Ok(cursor.into_inner())
}

/// Map a `zip` writer failure into [`PackageError::ArchiveWrite`],
/// which wraps `io::Error`; the zip error's message is preserved.
fn zip_write_error(source: zip::result::ZipError) -> PackageError {
    PackageError::ArchiveWrite(std::io::Error::other(source))
}

/// Lower-case hex SHA-256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    cabin_core::hash::hex_digest(&hasher.finalize())
}

fn walk(
    root: &Path,
    dir: &Path,
    exclude_dir: Option<&Path>,
    out: &mut Vec<PackageFile>,
) -> Result<(), PackageError> {
    let read = fs::read_dir(dir).map_err(|source| PackageError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut entries: Vec<fs::DirEntry> = Vec::new();
    for entry in read {
        let entry = entry.map_err(|source| PackageError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        entries.push(entry);
    }
    // Sort by file_name so traversal is deterministic.
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let name_os = entry.file_name();
        let Some(name) = name_os.to_str() else {
            return Err(PackageError::NonUtf8Path { path });
        };
        let file_type = entry.file_type().map_err(|source| PackageError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_symlink() {
            return Err(PackageError::SymlinkNotSupported(rel_str(root, &path)));
        }
        if file_type.is_dir() {
            if EXCLUDED_DIR_NAMES.contains(&name) {
                continue;
            }
            if matches!(exclude_dir, Some(excluded) if path == excluded) {
                continue;
            }
            reject_non_portable(root, &path, name)?;
            walk(root, &path, exclude_dir, out)?;
        } else if file_type.is_file() {
            if EXCLUDED_FILE_NAMES.contains(&name) {
                continue;
            }
            reject_non_portable(root, &path, name)?;
            let rel = rel_str(root, &path);
            out.push(PackageFile {
                rel_path: rel,
                abs_path: path,
            });
        } else {
            return Err(PackageError::UnsupportedFileType(rel_str(root, &path)));
        }
    }
    Ok(())
}

/// Return `path` relative to `root`, with forward slashes. `path`
/// is assumed to have been produced by walking from `root`, so it
/// always starts with `root` as a prefix.
///
/// Every component the walker emits has already passed the UTF-8
/// gate in [`walk`] (a non-UTF-8 entry name is rejected with
/// [`PackageError::NonUtf8Path`] before it reaches here), so
/// `to_str()` always yields `Some` and no lossy conversion of this
/// package-relative path is needed.
fn rel_str(root: &Path, path: &Path) -> String {
    let stripped = path.strip_prefix(root).unwrap_or(path);
    stripped
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Fail packaging when the path component `name` would name a
/// different file on another platform (a Win32-reserved device name,
/// a trailing dot or space, a forbidden character).  Run for every
/// collected file and directory so authors hit the named rule locally
/// instead of an asynchronous registry rejection; the walk's existing
/// symlink / UTF-8 / entry-type gates are separate concerns.
fn reject_non_portable(root: &Path, path: &Path, name: &str) -> Result<(), PackageError> {
    if let Some(violation) = cabin_fs::path::component_portability(name) {
        return Err(PackageError::NonPortablePath {
            path: rel_str(root, path),
            detail: violation.detail(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    fn paths(files: &[PackageFile]) -> Vec<&str> {
        files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    #[test]
    fn collects_simple_tree() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str("x").unwrap();
        dir.child("src/main.cc").write_str("y").unwrap();
        dir.child("include/example.h").write_str("z").unwrap();
        let files = collect_package_files(dir.path(), None).unwrap();
        assert_eq!(
            paths(&files),
            vec!["cabin.toml", "include/example.h", "src/main.cc"]
        );
    }

    #[test]
    fn excludes_supplied_output_dir() {
        // Custom in-tree output directories (anything outside the
        // hard-coded EXCLUDED_DIR_NAMES list) must be skipped when
        // explicitly named so a previous packaging run's archive
        // does not leak into the next archive's contents.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str("x").unwrap();
        dir.child("src/main.cc").write_str("y").unwrap();
        dir.child("myoutput/stale.tar.gz")
            .write_str("old archive")
            .unwrap();
        dir.child("myoutput/stale.json")
            .write_str("old metadata")
            .unwrap();
        let files = collect_package_files(dir.path(), Some(&dir.path().join("myoutput"))).unwrap();
        let names = paths(&files);
        assert!(names.contains(&"cabin.toml"));
        assert!(names.contains(&"src/main.cc"));
        assert!(
            !names.iter().any(|n| n.starts_with("myoutput/")),
            "myoutput contents leaked: {names:?}"
        );
    }

    #[test]
    fn exclude_outside_tree_has_no_effect() {
        // An exclude_dir that lives outside the package root is
        // never walked into, so passing it through must not affect
        // the resulting file list.
        let dir = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str("x").unwrap();
        dir.child("src/main.cc").write_str("y").unwrap();
        let files = collect_package_files(dir.path(), Some(elsewhere.path())).unwrap();
        assert_eq!(paths(&files), vec!["cabin.toml", "src/main.cc"]);
    }

    #[test]
    fn excludes_default_directories() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str("x").unwrap();
        dir.child("src/main.cc").write_str("y").unwrap();
        dir.child(".git/config").write_str("ignore").unwrap();
        dir.child("build/build.ninja").write_str("ignore").unwrap();
        dir.child("dist/old.tar.gz").write_str("ignore").unwrap();
        dir.child(".cabin/cache/whatever")
            .write_str("ignore")
            .unwrap();
        dir.child("node_modules/foo/index.js")
            .write_str("ignore")
            .unwrap();
        let files = collect_package_files(dir.path(), None).unwrap();
        let names = paths(&files);
        assert!(names.contains(&"cabin.toml"));
        assert!(names.contains(&"src/main.cc"));
        for excluded in &[".git", "build", "dist", ".cabin", "node_modules"] {
            assert!(
                !names.iter().any(|n| n.contains(excluded)),
                "expected no entries from {excluded}, got {names:?}"
            );
        }
    }

    #[test]
    fn excludes_default_files() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str("x").unwrap();
        dir.child("compile_commands.json")
            .write_str("ignore")
            .unwrap();
        dir.child("build.ninja").write_str("ignore").unwrap();
        dir.child("cabin.lock").write_str("ignore").unwrap();
        dir.child(".DS_Store").write_str("ignore").unwrap();
        dir.child("src/main.cc").write_str("y").unwrap();
        let files = collect_package_files(dir.path(), None).unwrap();
        let names = paths(&files);
        assert!(names.contains(&"cabin.toml"));
        assert!(names.contains(&"src/main.cc"));
        for excluded in &[
            "compile_commands.json",
            "build.ninja",
            "cabin.lock",
            ".DS_Store",
        ] {
            assert!(!names.contains(excluded), "leaked: {excluded}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str("x").unwrap();
        std::os::unix::fs::symlink("cabin.toml", dir.path().join("link")).unwrap();
        let err = collect_package_files(dir.path(), None).unwrap_err();
        match err {
            PackageError::SymlinkNotSupported(p) => assert_eq!(p, "link"),
            other => panic!("expected SymlinkNotSupported, got {other:?}"),
        }
    }

    #[test]
    fn rejects_case_folded_name_collision() {
        // `README` and `readme` coexist on a case-sensitive host but
        // alias on macOS / Windows; the verifier rejects the pair, so
        // packaging must fail locally instead.
        let files = [
            PackageFile {
                rel_path: "README".to_owned(),
                abs_path: PathBuf::from("/x/README"),
            },
            PackageFile {
                rel_path: "readme".to_owned(),
                abs_path: PathBuf::from("/x/readme"),
            },
        ];
        let err = reject_case_conflicts(&files).unwrap_err();
        assert!(
            matches!(err, PackageError::CaseConflictingPaths { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_case_folded_file_and_directory() {
        // A file `A` and a directory component `a/` fold together, the
        // `case_conflict` the verifier flags as a file-used-as-parent.
        let files = [
            PackageFile {
                rel_path: "A".to_owned(),
                abs_path: PathBuf::from("/x/A"),
            },
            PackageFile {
                rel_path: "a/b".to_owned(),
                abs_path: PathBuf::from("/x/a/b"),
            },
        ];
        let err = reject_case_conflicts(&files).unwrap_err();
        assert!(
            matches!(err, PackageError::CaseConflictingPaths { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn nested_directories_of_the_same_case_do_not_conflict() {
        // Distinct nested paths sharing a real (same-case) directory
        // prefix must not be mistaken for a collision.
        let files = [
            PackageFile {
                rel_path: "src/a.c".to_owned(),
                abs_path: PathBuf::from("/x/src/a.c"),
            },
            PackageFile {
                rel_path: "src/b.c".to_owned(),
                abs_path: PathBuf::from("/x/src/b.c"),
            },
        ];
        reject_case_conflicts(&files).unwrap();
    }

    #[test]
    fn ensure_manifest_included_rejects_archive_without_root_manifest() {
        let files = vec![PackageFile {
            rel_path: "src/main.cc".to_owned(),
            abs_path: PathBuf::from("/abs/src/main.cc"),
        }];
        let err = ensure_manifest_included(&files).unwrap_err();
        assert!(matches!(err, PackageError::ArchiveMissingManifest));
    }

    #[test]
    fn deterministic_archive_for_same_input() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str("x").unwrap();
        dir.child("src/main.cc").write_str("y").unwrap();
        let files = collect_package_files(dir.path(), None).unwrap();
        let bytes_a = build_zip(&files, None).unwrap();
        let bytes_b = build_zip(&files, None).unwrap();
        assert_eq!(bytes_a, bytes_b, "archives must be byte-identical");
    }

    #[test]
    fn archive_can_be_extracted_back() {
        // Round-trip: archive a small tree, read the zip directory
        // back, check `cabin.toml` is at the archive root.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"x\"\n")
            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        let files = collect_package_files(dir.path(), None).unwrap();
        let bytes = build_zip(&files, None).unwrap();

        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut seen: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_owned())
            .collect();
        seen.sort();
        assert_eq!(seen, vec!["cabin.toml", "src/main.cc"]);
    }

    #[test]
    fn archive_entries_have_no_extra_fields_and_deflate() {
        // Pins the strict-profile risk that the zip writer emits an
        // extra field (zip64, timestamp, unix-extra): every entry
        // must carry none, deflate, and the 1980-01-01 timestamp.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str("x").unwrap();
        dir.child("src/main.cc").write_str("y").unwrap();
        let files = collect_package_files(dir.path(), None).unwrap();
        let bytes = build_zip(&files, None).unwrap();

        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(zip.len() >= 2);
        for i in 0..zip.len() {
            let entry = zip.by_index(i).unwrap();
            assert!(
                entry.extra_data().unwrap_or_default().is_empty(),
                "entry `{}` carries extra-field bytes",
                entry.name()
            );
            assert_eq!(entry.compression(), CompressionMethod::Deflated);
            assert_eq!(entry.last_modified(), Some(DateTime::default()));
        }
    }

    /// Pack-time portability gate (amendment A1): a source tree with a
    /// non-portable file name fails packaging locally, naming the
    /// violated rule, rather than deferring to an async registry
    /// rejection.  Unix-only because the fixtures create real files
    /// with names Windows would refuse; the exhaustive per-string
    /// matrix lives in `cabin-fs`.
    #[cfg(unix)]
    #[test]
    fn collect_rejects_non_portable_path_components() {
        for (name, detail) in [
            ("a:b.h", "colon"),
            ("CON", "windows device name"),
            ("file.", "trailing dot"),
            ("file ", "trailing space"),
            ("ctrl\tname", "control character"),
        ] {
            let dir = TempDir::new().unwrap();
            dir.child("cabin.toml").write_str("x").unwrap();
            fs::write(dir.path().join(name), b"x").unwrap();
            let err = collect_package_files(dir.path(), None).unwrap_err();
            match err {
                PackageError::NonPortablePath { path, detail: got } => {
                    assert!(path.contains(name), "unexpected path `{path}` for {name:?}");
                    assert_eq!(got, detail, "wrong rule for {name:?}");
                }
                other => panic!("expected NonPortablePath for {name:?}, got {other:?}"),
            }
        }
    }
}
