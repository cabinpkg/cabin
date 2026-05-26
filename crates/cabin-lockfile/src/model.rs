use std::path::PathBuf;

use cabin_core::PackageName;

/// Current `cabin.lock` schema version. Bumping this requires a
/// migration path in [`crate::io`].
pub const LOCKFILE_VERSION: u32 = 1;

/// In-memory representation of a `cabin.lock`.
///
/// Constructed by [`crate::io::parse_lockfile_str`] / [`crate::io::read_lockfile`]
/// and serialized by [`crate::io::render_lockfile`] /
/// [`crate::io::write_lockfile`]. The lockfile only records resolved
/// **registry** dependencies — local path packages are intentionally
/// not included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    /// Schema version (currently always [`LOCKFILE_VERSION`]).
    pub version: u32,
    /// Resolved registry packages, sorted by name for determinism.
    pub packages: Vec<LockedPackage>,
    /// Active patch entries recorded for stale-detection under
    /// `--locked`. Empty for projects with no patches; old
    /// lockfiles that omit the `[[patch]]` array continue to
    /// parse cleanly thanks to `#[serde(default)]` on the raw
    /// shape.
    pub patches: Vec<LockedPatch>,
    /// Active source-replacement entries, recorded for the same
    /// reason. Empty for projects with no replacements.
    pub source_replacements: Vec<LockedSourceReplacement>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::empty()
    }
}

impl Lockfile {
    /// An empty lockfile at the current schema version.
    pub fn empty() -> Self {
        Self {
            version: LOCKFILE_VERSION,
            packages: Vec::new(),
            patches: Vec::new(),
            source_replacements: Vec::new(),
        }
    }

    /// Look up a locked package by name. Linear scan — typical
    /// lockfiles are small enough that this stays cheap.
    pub fn find(&self, name: &PackageName) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| &p.name == name)
    }

    /// Whether the lockfile's recorded patch + source-replacement
    /// arrays equal the supplied active policy. Used by
    /// `cabin <command> --locked` to detect that the user changed
    /// patch policy since the lockfile was last written: the
    /// recorded arrays already serialize deterministically (sorted
    /// in [`crate::io::render_lockfile`]), so a slice comparison
    /// is the canonical staleness check and lives next to the
    /// types it compares.
    pub fn matches_patch_state(
        &self,
        active_patches: &[LockedPatch],
        active_source_replacements: &[LockedSourceReplacement],
    ) -> bool {
        active_patches == self.patches.as_slice()
            && active_source_replacements == self.source_replacements.as_slice()
    }
}

/// One resolved package recorded in the lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: PackageName,
    pub version: semver::Version,
    pub source: LockedSource,
    /// Optional content hash copied from the index. Used by the
    /// fetch / artifact-verification path; absent for index entries
    /// that predate checksum support.
    pub checksum: Option<String>,
    /// Names of other locked packages this one depends on.
    pub dependencies: Vec<PackageName>,
}

/// Where a locked package was sourced from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockedSource {
    /// Resolved from the local JSON package index.
    Index,
}

impl LockedSource {
    pub fn as_str(self) -> &'static str {
        match self {
            LockedSource::Index => "index",
        }
    }
}

/// One active patch entry recorded for stale-detection under
/// `--locked`. Carries enough information to reproduce the
/// patch decision: package name, patched version, source kind,
/// and the path *as written* in the declaring file (resolved
/// relative to the declaring file's directory at apply time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPatch {
    pub package: PackageName,
    pub version: semver::Version,
    pub kind: LockedPatchKind,
    pub provenance: String,
    pub path: PathBuf,
}

/// Source kind of a locked patch entry. Mirrors
/// [`cabin_core::PatchSourceKind`] but stays in this crate so the
/// lockfile model is self-contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockedPatchKind {
    /// Local path patch.
    Path,
}

impl LockedPatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LockedPatchKind::Path => "path",
        }
    }
}

/// One active source-replacement entry recorded for the same
/// stale-detection reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedSourceReplacement {
    pub original: String,
    pub original_kind: LockedSourceLocatorKind,
    pub replacement: String,
    pub replacement_kind: LockedSourceLocatorKind,
    pub provenance: String,
}

/// Stable locator-kind label for the lockfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockedSourceLocatorKind {
    IndexPath,
    IndexUrl,
}

impl LockedSourceLocatorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LockedSourceLocatorKind::IndexPath => "index-path",
            LockedSourceLocatorKind::IndexUrl => "index-url",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    fn patch(name: &str, version: &str) -> LockedPatch {
        LockedPatch {
            package: pkg(name),
            version: ver(version),
            kind: LockedPatchKind::Path,
            provenance: "manifest".into(),
            path: PathBuf::from("../").join(name),
        }
    }

    fn replacement(original: &str, replacement: &str) -> LockedSourceReplacement {
        LockedSourceReplacement {
            original: original.into(),
            original_kind: LockedSourceLocatorKind::IndexUrl,
            replacement: replacement.into(),
            replacement_kind: LockedSourceLocatorKind::IndexPath,
            provenance: "user-config".into(),
        }
    }

    #[test]
    fn matches_patch_state_returns_true_for_equal_slices() {
        let lock = Lockfile {
            version: LOCKFILE_VERSION,
            packages: Vec::new(),
            patches: vec![patch("fmt", "10.2.1")],
            source_replacements: vec![replacement("https://example.com/index", "../mirror")],
        };
        assert!(lock.matches_patch_state(
            &[patch("fmt", "10.2.1")],
            &[replacement("https://example.com/index", "../mirror")],
        ));
    }

    #[test]
    fn matches_patch_state_detects_added_patch() {
        let lock = Lockfile {
            version: LOCKFILE_VERSION,
            packages: Vec::new(),
            patches: Vec::new(),
            source_replacements: Vec::new(),
        };
        assert!(!lock.matches_patch_state(&[patch("fmt", "10.2.1")], &[]));
    }

    #[test]
    fn matches_patch_state_detects_removed_replacement() {
        let lock = Lockfile {
            version: LOCKFILE_VERSION,
            packages: Vec::new(),
            patches: Vec::new(),
            source_replacements: vec![replacement("https://example.com/index", "../mirror")],
        };
        assert!(!lock.matches_patch_state(&[], &[]));
    }

    #[test]
    fn matches_patch_state_is_order_sensitive() {
        // The lockfile's render path sorts both arrays (see
        // `render_lockfile`), so callers should always supply
        // sorted active state. A raw out-of-order comparison
        // surfacing as "stale" is the correct, conservative
        // outcome — silent acceptance would let two semantically
        // different policies look equal.
        let lock = Lockfile {
            version: LOCKFILE_VERSION,
            packages: Vec::new(),
            patches: vec![patch("a", "1.0.0"), patch("b", "1.0.0")],
            source_replacements: Vec::new(),
        };
        assert!(!lock.matches_patch_state(&[patch("b", "1.0.0"), patch("a", "1.0.0")], &[],));
    }
}
