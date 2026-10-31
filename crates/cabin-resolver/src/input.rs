use std::collections::{BTreeMap, BTreeSet};

use cabin_core::standard_compatibility::ConsumerStandards;
use cabin_core::{IncompatibleStandards, PackageName};
use cabin_index::IndexEntry;

/// Inputs to [`crate::resolve`].
///
/// The resolver does not read manifests or workspaces.  The caller
/// hands it the root's *versioned* dependencies, the feature
/// requests on them, and every package the workspace supplies
/// locally as a single-version entry (`local_packages`).
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
    /// Declared `links` claims of the local packages the caller
    /// includes without resolution (the selected closure).  The
    /// post-resolution uniqueness check validates these together with
    /// the claims of every resolved package, because native symbol
    /// collisions ignore where a package came from.
    pub local_links: Vec<LinksClaim>,
    /// The link/runtime domains of a root whose local packages
    /// declare build dependencies: the target context and the build
    /// context.  When non-empty, the post-resolution uniqueness check
    /// runs once per domain, in place of once over `local_links` and
    /// every resolved package, so a build tool and the target program
    /// may each claim the same identity (contracts section 1, rule 6).
    pub link_domains: Vec<LinkDomain>,
    /// The root's active edges onto versioned dependencies with the
    /// features they request, keyed by dependency name: `default` for
    /// an edge that keeps default features, plus edge `features` and
    /// `<dep>/<feature>` requests.  Every name is required, at its
    /// `root_dependencies` range or, absent there (a `[patch]`ed name
    /// the caller resolves locally), at any version.
    pub root_features: BTreeMap<PackageName, BTreeSet<String>>,
    /// Packages the workspace supplies locally (members, path
    /// dependencies, `[patch]` forks), each a single-version entry.
    /// A local entry replaces any same-named index entry for every
    /// edge that reaches the name, is exempt from the lockfile, and
    /// is reported as [`ResolvedSource::Local`](crate::ResolvedSource::Local).
    pub local_packages: BTreeMap<PackageName, IndexEntry>,
    /// Whether `index` holds only the metadata read so far, as Cabin's
    /// file and sparse HTTP sources load it.  A name missing from such
    /// an index fails with
    /// [`ResolveError::MetadataNeeded`](crate::ResolveError::MetadataNeeded)
    /// so the caller can fetch it and resolve again; otherwise a
    /// missing name is a package with no versions.
    pub lazy_index: bool,
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
            link_domains: Vec::new(),
            root_features: BTreeMap::new(),
            local_packages: BTreeMap::new(),
            lazy_index: false,
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

/// One link/runtime domain for [`ResolveInput::link_domains`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkDomain {
    /// The names the domain's local packages depend on directly; the
    /// resolved packages they reach belong to the domain.
    pub roots: BTreeSet<PackageName>,
    /// Declared claims of the domain's local packages.
    pub local_links: Vec<LinksClaim>,
}

/// A previously-resolved version copied out of the lockfile.  Defined
/// here so `cabin-resolver` does not depend on `cabin-lockfile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedVersion {
    pub version: semver::Version,
    /// The packaging-revision pin recorded in the lockfile.  In
    /// `Locked` mode, for an index version that has revisions, the
    /// resolver fails when the pin is `None` or no revision carries
    /// it; a version without revisions skips the check.
    pub checksum: Option<cabin_core::Checksum>,
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
    /// versions.
    UpdateAll,
    /// Like `PreferLocked`, but the named package is never preferred -
    /// it is re-resolved from scratch.
    UpdatePackage(PackageName),
}
