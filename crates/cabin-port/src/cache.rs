//! Foundation-port archive + extracted-source cache.
//!
//! Layout:
//!
//! ```text
//! <root>/
//!   archives/sha256/<hex>.tar.gz
//!   sources/<name>/<version>/sha256/<hex>/cabin.toml + upstream files
//! ```
//!
//! Archives are content-addressed (SHA-256): two ports declaring
//! the same upstream tarball share one cached download. Extracted
//! sources are *identity-addressed* (package name + version, with
//! the archive SHA-256 as a leaf invalidator): two ports that
//! reuse the same archive but ship different overlays no longer
//! clobber each other's `cabin.toml`, and a port that re-publishes
//! the same `name@version` with a fresh archive re-extracts
//! cleanly under the new hex.
//!
//! The `sha256` shard in both branches is the hash-algorithm
//! marker; future algorithms slot in alongside it without
//! perturbing existing entries.
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PortCache {
    root: PathBuf,
}

impl PortCache {
    /// Build a cache rooted at `root`. The directory is created on
    /// demand by [`crate::prepare()`]; this constructor does no I/O.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn archive_path(&self, hex: &str) -> PathBuf {
        self.root
            .join("archives")
            .join("sha256")
            .join(format!("{hex}.tar.gz"))
    }

    /// Identity-addressed source directory for the port `name@version`
    /// extracted from the archive whose SHA-256 is `hex`. See the
    /// module-level docs for why `name`+`version` participate in
    /// the key — two ports sharing a tarball must not share their
    /// extracted overlay.
    pub fn source_dir(&self, name: &str, version: &str, hex: &str) -> PathBuf {
        self.root
            .join("sources")
            .join(name)
            .join(version)
            .join("sha256")
            .join(hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_paths_are_checksum_addressed() {
        let cache = PortCache::new("/cabin-cache/ports");
        let hex = "deadbeef".to_string() + &"a".repeat(56);
        assert_eq!(
            cache.archive_path(&hex),
            PathBuf::from(format!("/cabin-cache/ports/archives/sha256/{hex}.tar.gz"))
        );
    }

    #[test]
    fn source_dirs_are_identity_keyed() {
        let cache = PortCache::new("/cabin-cache/ports");
        let hex = "deadbeef".to_string() + &"a".repeat(56);
        // Same archive hex resolves to *different* extraction
        // roots when name or version differ — the regression
        // path the cache contract enforces.
        let zlib = cache.source_dir("zlib", "1.3.1", &hex);
        let other = cache.source_dir("other", "1.3.1", &hex);
        let zlib_v2 = cache.source_dir("zlib", "2.0.0", &hex);
        assert_eq!(
            zlib,
            PathBuf::from(format!(
                "/cabin-cache/ports/sources/zlib/1.3.1/sha256/{hex}"
            ))
        );
        assert_ne!(zlib, other);
        assert_ne!(zlib, zlib_v2);
    }
}
