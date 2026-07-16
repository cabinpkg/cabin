//! Foundation-port archive + extracted-source cache.
//!
//! Layout:
//!
//! ```text
//! <root>/
//! archives/sha256/<hex>.tar.gz   (or <hex>.zip for zip sources)
//! sources/<name>/<version>/sha256/<hex>/cabin.toml + upstream files
//! ```
//!
//! Archives are content-addressed (SHA-256): two ports declaring
//! the same upstream tarball share one cached download.  Extracted
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

use url::Url;

/// On-disk format of a port's source archive.  Decided by the
/// `[source].url` path: a URL ending in `.zip` (case-insensitive)
/// is a zip archive; everything else keeps the historical `.tar.gz`
/// interpretation, so existing recipes are untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    TarGz,
    Zip,
}

impl ArchiveKind {
    pub fn from_url(url: &Url) -> Self {
        if url.path().to_ascii_lowercase().ends_with(".zip") {
            Self::Zip
        } else {
            Self::TarGz
        }
    }

    /// Cache-file extension (no leading dot).
    pub fn extension(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PortCache {
    root: PathBuf,
}

impl PortCache {
    /// Build a cache rooted at `root`.  The directory is created on
    /// demand by [`crate::prepare()`]; this constructor does no I/O.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn archive_path(&self, hex: &str, kind: ArchiveKind) -> PathBuf {
        self.root
            .join("archives")
            .join("sha256")
            .join(format!("{hex}.{}", kind.extension()))
    }

    /// Identity-addressed source directory for the port `name@version`
    /// extracted from the archive whose SHA-256 is `hex`.  See the
    /// module-level docs for why `name`+`version` participate in
    /// the key - two ports sharing a tarball must not share their
    /// extracted overlay.  The name contributes its
    /// `path_components`, so a scoped port nests as
    /// `sources/<scope>/<name>/...` instead of embedding a `/` into
    /// one component.
    pub fn source_dir(&self, name: &cabin_core::PackageName, version: &str, hex: &str) -> PathBuf {
        name.path_components()
            .fold(self.root.join("sources"), |dir, c| dir.join(c))
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
            cache.archive_path(&hex, ArchiveKind::TarGz),
            PathBuf::from(format!("/cabin-cache/ports/archives/sha256/{hex}.tar.gz"))
        );
        assert_eq!(
            cache.archive_path(&hex, ArchiveKind::Zip),
            PathBuf::from(format!("/cabin-cache/ports/archives/sha256/{hex}.zip"))
        );
    }

    #[test]
    fn archive_kind_follows_url_extension() {
        let zip = Url::parse("https://example.com/dl/miniz-3.1.2.zip").unwrap();
        let upper = Url::parse("https://example.com/dl/MINIZ.ZIP").unwrap();
        let tar = Url::parse("https://example.com/dl/zlib-1.3.1.tar.gz").unwrap();
        // Query strings do not confuse the path-based decision.
        let query = Url::parse("https://example.com/dl/zlib-1.3.1.tar.gz?token=zip").unwrap();
        assert_eq!(ArchiveKind::from_url(&zip), ArchiveKind::Zip);
        assert_eq!(ArchiveKind::from_url(&upper), ArchiveKind::Zip);
        assert_eq!(ArchiveKind::from_url(&tar), ArchiveKind::TarGz);
        assert_eq!(ArchiveKind::from_url(&query), ArchiveKind::TarGz);
    }

    #[test]
    fn source_dirs_are_identity_keyed() {
        let cache = PortCache::new("/cabin-cache/ports");
        let hex = "deadbeef".to_string() + &"a".repeat(56);
        // Same archive hex resolves to *different* extraction
        // roots when name or version differ - the regression
        // path the cache contract enforces.
        let zlib_name = cabin_core::PackageName::new("zlib").unwrap();
        let zlib = cache.source_dir(&zlib_name, "1.3.1", &hex);
        let other = cache.source_dir(
            &cabin_core::PackageName::new("other").unwrap(),
            "1.3.1",
            &hex,
        );
        let zlib_v2 = cache.source_dir(&zlib_name, "2.0.0", &hex);
        assert_eq!(
            zlib,
            PathBuf::from(format!(
                "/cabin-cache/ports/sources/zlib/1.3.1/sha256/{hex}"
            ))
        );
        assert_ne!(zlib, other);
        assert_ne!(zlib, zlib_v2);
    }

    /// A scoped port name nests as `sources/<scope>/<name>/...`; the
    /// full string is never one path component.
    #[test]
    fn scoped_source_dirs_nest_per_component() {
        let cache = PortCache::new("/cabin-cache/ports");
        let hex = "deadbeef".to_string() + &"a".repeat(56);
        let scoped = cache.source_dir(
            &cabin_core::PackageName::new("madler/zlib").unwrap(),
            "1.3.1",
            &hex,
        );
        assert_eq!(
            scoped,
            PathBuf::from(format!(
                "/cabin-cache/ports/sources/madler/zlib/1.3.1/sha256/{hex}"
            ))
        );
    }
}
