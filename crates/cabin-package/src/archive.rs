use std::fs;
use std::path::{Path, PathBuf};

use flate2::{Compression, GzBuilder};
use sha2::{Digest, Sha256};

use crate::error::PackageError;

/// Conventional package-archive root entry.
pub const ROOT_MANIFEST_NAME: &str = "cabin.toml";

/// Top-level directory names that are excluded from package archives
/// by default. Matched anywhere in the tree, not only at the root, so
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

/// File names excluded from package archives by default. Matched
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
/// contents must be omitted from the archive. The walker compares
/// the absolute path of each descended directory against this
/// value and skips on equality. Callers (notably `package_with_project`)
/// pass the resolved `--output-dir` so a previous run's archive
/// living inside the package source tree does not leak into the
/// next archive.
///
/// The returned list is sorted lexicographically by `rel_path` so
/// archive output is deterministic without callers having to sort
/// again.
pub fn collect_package_files(
    root: &Path,
    exclude_dir: Option<&Path>,
) -> Result<Vec<PackageFile>, PackageError> {
    let mut out = Vec::new();
    walk(root, root, exclude_dir, &mut out)?;
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// Reject the archive if `cabin.toml` is not at its root. Practically
/// the manifest is always in the source tree, but the check protects
/// callers from misuse (e.g. an output dir set to the package root,
/// where the archive contract — `cabin.toml` at the root — would
/// silently break).
pub fn ensure_manifest_included(files: &[PackageFile]) -> Result<(), PackageError> {
    if !files.iter().any(|f| f.rel_path == ROOT_MANIFEST_NAME) {
        return Err(PackageError::ArchiveMissingManifest);
    }
    Ok(())
}

/// Build a deterministic `.tar.gz` for `files`.
///
/// Determinism rules baked into this writer:
/// - tar entries are written in the order their `rel_path` was sorted
///   into;
/// - each entry has `mtime`, `uid`, `gid` zeroed and `uname` /
///   `gname` cleared;
/// - mode is `0o644` (regular files only — directories are implied
///   by the extractor);
/// - the gzip header carries `mtime = 0` and OS code `0xff`
///   (unknown), so the same logical input produces the same bytes
///   regardless of when or where the archive is built.
pub fn build_tar_gz(files: &[PackageFile]) -> Result<Vec<u8>, PackageError> {
    let mut buf: Vec<u8> = Vec::new();
    {
        let gz = GzBuilder::new()
            .mtime(0)
            .operating_system(0xff)
            .write(&mut buf, Compression::default());
        let mut tar_builder = tar::Builder::new(gz);
        for file in files {
            let bytes = fs::read(&file.abs_path).map_err(|source| PackageError::Io {
                path: file.abs_path.clone(),
                source,
            })?;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            // Zeroing the username / groupname means the archive does
            // not embed who built it.
            let _ = header.set_username("");
            let _ = header.set_groupname("");
            header.set_entry_type(tar::EntryType::Regular);
            tar_builder
                .append_data(&mut header, &file.rel_path, std::io::Cursor::new(bytes))
                .map_err(PackageError::ArchiveWrite)?;
        }
        let gz_inner = tar_builder
            .into_inner()
            .map_err(PackageError::ArchiveWrite)?;
        gz_inner.finish().map_err(PackageError::ArchiveWrite)?;
    }
    Ok(buf)
}

/// Lower-case hex SHA-256 of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
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
            walk(root, &path, exclude_dir, out)?;
        } else if file_type.is_file() {
            if EXCLUDED_FILE_NAMES.contains(&name) {
                continue;
            }
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
/// Is assumed to have been produced by walking from `root`, so it
/// always starts with `root` as a prefix.
fn rel_str(root: &Path, path: &Path) -> String {
    let stripped = path.strip_prefix(root).unwrap_or(path);
    let mut parts: Vec<String> = Vec::new();
    for component in stripped.components() {
        if let std::path::Component::Normal(name) = component {
            parts.push(name.to_string_lossy().into_owned());
        }
    }
    parts.join("/")
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
        let bytes_a = build_tar_gz(&files).unwrap();
        let bytes_b = build_tar_gz(&files).unwrap();
        assert_eq!(bytes_a, bytes_b, "archives must be byte-identical");
    }

    #[test]
    fn archive_can_be_extracted_back() {
        // Round-trip: archive a small tree, gunzip+untar it manually,
        // check `cabin.toml` is at the archive root and the bytes
        // match.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"x\"\n")
            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        let files = collect_package_files(dir.path(), None).unwrap();
        let bytes = build_tar_gz(&files).unwrap();

        let dec = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut tar = tar::Archive::new(dec);
        let mut seen: Vec<String> = Vec::new();
        for entry in tar.entries().unwrap() {
            let entry = entry.unwrap();
            seen.push(entry.path().unwrap().to_string_lossy().into_owned());
        }
        seen.sort();
        assert_eq!(seen, vec!["cabin.toml", "src/main.cc"]);
    }
}
