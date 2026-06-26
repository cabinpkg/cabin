use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Layout of an artifact cache rooted at a directory on disk.
///
/// The cache is intentionally checksum-addressed:
///
/// ```text
/// <root>/
/// Archives/sha256/<hex>.tar.gz
/// Sources/sha256/<hex>/cabin.toml
/// /...
/// ```
///
/// No per-package or per-version directories appear at the top level,
/// which keeps reuse trivial: the same hash always maps to the same
/// archive and the same extracted source tree.
#[derive(Debug, Clone)]
pub struct ArtifactCache {
    root: PathBuf,
}

impl ArtifactCache {
    /// Create a cache rooted at `root`.  The directory is not created on
    /// construction; the fetch path creates the leaf directories on
    /// demand.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Filesystem path for an archive identified by its `sha256` hex
    /// digest.
    pub fn archive_path(&self, hex: &str) -> PathBuf {
        self.root
            .join("archives")
            .join("sha256")
            .join(format!("{hex}.tar.gz"))
    }

    /// Filesystem path for the extracted source tree of an archive
    /// identified by its `sha256` hex digest.
    pub fn source_dir(&self, hex: &str) -> PathBuf {
        self.root.join("sources").join("sha256").join(hex)
    }
}

/// Sibling `<archive>.partial` path used while streaming a download
/// before the atomic rename into place.  Built by appending `.partial`
/// rather than via `Path::with_extension`, which would clobber the
/// trailing `.gz` of `<name>-<version>.tar.gz`.
pub fn partial_sibling(archive_path: &Path) -> PathBuf {
    let mut s: OsString = archive_path.as_os_str().to_owned();
    s.push(".partial");
    PathBuf::from(s)
}

/// Sibling `<source_dir>.ok` extraction-complete marker.  Kept as a
/// sibling of the source tree (not a file inside it) so an extracted
/// tarball cannot forge the marker, and so `remove_dir_all` on the
/// source tree does not delete it - the caller removes it explicitly
/// before re-extracting.
pub fn extraction_marker_path(source_dir: &Path) -> PathBuf {
    let mut s: OsString = source_dir.as_os_str().to_owned();
    s.push(".ok");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_checksum_addressed() {
        let cache = ArtifactCache::new("/abs/cache");
        let hex = "deadbeef".to_string() + &"a".repeat(56);
        assert_eq!(
            cache.archive_path(&hex),
            PathBuf::from(format!("/abs/cache/archives/sha256/{hex}.tar.gz"))
        );
        assert_eq!(
            cache.source_dir(&hex),
            PathBuf::from(format!("/abs/cache/sources/sha256/{hex}"))
        );
    }
}
