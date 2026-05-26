use std::collections::BTreeMap;

use cabin_core::PackageName;

/// Inputs to [`crate::resolve`].
///
/// The resolver does not know about manifests, workspaces, or
/// local path dependencies — those are surfaced separately.
/// `root_dependencies` must be the set of *versioned*
/// dependencies of the root package.
///
/// `locked` and `mode` let callers feed the previous lockfile
/// in as a preference (or, in `Locked` mode, as a hard
/// constraint) without pulling lockfile types into this crate.
#[derive(Debug, Clone)]
pub struct ResolveInput {
    pub root_name: PackageName,
    pub root_version: semver::Version,
    pub root_dependencies: BTreeMap<PackageName, semver::VersionReq>,
    /// Previously resolved versions, keyed by package name. Used as
    /// preferences in [`ResolveMode::PreferLocked`] and as the only
    /// allowed candidates in [`ResolveMode::Locked`].
    pub locked: BTreeMap<PackageName, LockedVersion>,
    pub mode: ResolveMode,
}

impl ResolveInput {
    /// Construct a request that uses the default mode
    /// ([`ResolveMode::PreferLocked`]) and no locked preferences.
    pub fn new(
        root_name: PackageName,
        root_version: semver::Version,
        root_dependencies: BTreeMap<PackageName, semver::VersionReq>,
    ) -> Self {
        Self {
            root_name,
            root_version,
            root_dependencies,
            locked: BTreeMap::new(),
            mode: ResolveMode::PreferLocked,
        }
    }
}

/// A previously-resolved version copied out of the lockfile. Kept
/// resolver-internal so `cabin-resolver` does not depend on
/// `cabin-lockfile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedVersion {
    pub version: semver::Version,
    /// Optional content hash recorded in the lockfile. In `Locked` mode
    /// the resolver checks this against the index entry's checksum and
    /// fails on mismatch.
    pub checksum: Option<String>,
}

/// How the resolver should treat the `locked` map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveMode {
    /// Default. Try the locked version first; fall back to the newest
    /// compatible non-yanked version if the locked one no longer
    /// satisfies the current constraints.
    PreferLocked,
    /// Strict mode. Locked versions must exactly satisfy every
    /// constraint encountered during resolution; any deviation is a
    /// hard error. This is what `--locked` and `--frozen` use.
    Locked,
    /// Ignore the locked map entirely and pick newest compatible
    /// versions (default behavior).
    UpdateAll,
    /// Like `PreferLocked`, but the named package is never preferred —
    /// it is re-resolved from scratch.
    UpdatePackage(PackageName),
}
