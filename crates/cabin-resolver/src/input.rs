use std::collections::{BTreeMap, BTreeSet};

use cabin_core::standard_compatibility::ConsumerStandards;
use cabin_core::{IncompatibleStandards, PackageName};

/// Inputs to [`crate::resolve`].
///
/// The resolver does not know about manifests, workspaces, or
/// local path dependencies - those are surfaced separately.
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
    /// Previously resolved versions, keyed by package name.  Used as
    /// preferences in [`ResolveMode::PreferLocked`] and as the only
    /// allowed candidates in [`ResolveMode::Locked`].
    pub locked: BTreeMap<PackageName, LockedVersion>,
    pub mode: ResolveMode,
    /// The `[resolver] incompatible-standards` preference.  Under
    /// [`IncompatibleStandards::Fallback`] the provider orders
    /// candidate versions by standard compatibility against
    /// `consumer_standards`; under [`IncompatibleStandards::Allow`]
    /// standards never influence selection.  Never affects
    /// solvability under either value.
    pub incompatible_standards: IncompatibleStandards,
    /// The workspace consumer's effective compile levels (per
    /// language, the minimum across workspace member targets).  Used
    /// only for `Fallback` ordering; `{ c: None, cxx: None }` (the
    /// default) makes every candidate rank as undeclared, so
    /// `Fallback` reduces to `Allow`.
    pub consumer_standards: ConsumerStandards,
    /// Declared `links` claims of the packages the index never sees:
    /// the selected workspace members, path dependencies, and
    /// patched manifests.  The post-resolution uniqueness check
    /// validates these together with the resolved registry packages'
    /// index claims, because native symbol collisions ignore where a
    /// package came from.  Empty (the default) means the local side
    /// claims nothing.
    pub local_links: Vec<LinksClaim>,
    /// Names whose package the build takes from a local source
    /// (workspace member, path dependency, `[patch]`) even when
    /// resolution selects a same-named index candidate through a
    /// transitive registry edge.  The links check skips these
    /// candidates' index claims - the local replacement is what
    /// links, and its claims (if any) arrive via `local_links` -
    /// so a patched-away upstream cannot report a collision the
    /// final link can never have.
    pub locally_supplied: BTreeSet<PackageName>,
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
            incompatible_standards: IncompatibleStandards::default(),
            consumer_standards: ConsumerStandards { c: None, cxx: None },
            local_links: Vec::new(),
            locally_supplied: BTreeSet::new(),
        }
    }
}

/// One target's declared native-library identity claim, attributed
/// to the package and version that declares it.  The resolver builds
/// these from index metadata for resolved registry packages; callers
/// supply them via [`ResolveInput::local_links`] for packages the
/// index never sees.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LinksClaim {
    pub package: PackageName,
    pub version: semver::Version,
    /// Name of the claiming target inside `package`.
    pub target: String,
    /// The claimed native-library identity (`links = "z"`).
    pub links: String,
}

/// A previously-resolved version copied out of the lockfile.  Kept
/// resolver-internal so `cabin-resolver` does not depend on
/// `cabin-lockfile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedVersion {
    pub version: semver::Version,
    /// Optional content hash recorded in the lockfile.  In `Locked` mode
    /// the resolver checks this against the index entry's checksum and
    /// fails on mismatch.
    pub checksum: Option<String>,
}

/// How the resolver should treat the `locked` map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveMode {
    /// Default.  Try the locked version first; fall back to the newest
    /// compatible non-yanked version if the locked one no longer
    /// satisfies the current constraints.
    PreferLocked,
    /// Strict mode.  Locked versions must exactly satisfy every
    /// constraint encountered during resolution; any deviation is a
    /// hard error.  This is what `--locked` and `--frozen` use.
    Locked,
    /// Ignore the locked map entirely and pick newest compatible
    /// versions (default behavior).
    UpdateAll,
    /// Like `PreferLocked`, but the named package is never preferred -
    /// it is re-resolved from scratch.
    UpdatePackage(PackageName),
}
