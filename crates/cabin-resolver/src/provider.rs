//! `PubGrub`-backed dependency provider.
//!
//! Cabin's manifest, index, and lockfile model is mapped onto
//! `PubGrub`'s [`DependencyProvider`] surface so the public
//! [`resolve`](crate::resolve) entry point can stay
//! Cabin-shaped while the actual conflict-driven backtracking
//! is delegated to `PubGrub`.
//!
//! ## Layering
//!
//! `PubGrub` solves over abstract packages, versions, and
//! version sets.  Cabin uses [`Node`] (a package, one of its
//! features, or the synthetic root), [`semver::Version`], and
//! [`Ranges<semver::Version>`](Ranges) (the latter built by
//! [`crate::range::req_to_range`]).  The root is the workspace
//! itself, which no index publishes; its edges are the root
//! dependencies and feature requests of [`ResolveInput`].  Packages
//! the workspace supplies locally are single-version candidates
//! that replace same-named index entries.
//!
//! ## Targeted errors
//!
//! `PubGrub`'s `NoSolution` variant only carries a derivation
//! tree, not Cabin's actionable error variants.  Root-level
//! errors (`UnknownPackage`, `NoMatchingVersion`,
//! `AllMatchingVersionsYanked`, every `Locked*` variant but
//! `LockedVersionLacksFeature` for direct dependencies) are
//! produced ahead of time in [`crate::preflight`].  Transitive
//! `Locked`-mode failures, and `LockedVersionLacksFeature` for any
//! package, surface here, returned through `PubGrub`'s
//! `ErrorChoosingVersion` as their original [`ResolveError`].
//!
//! ## Locked-mode constraint recording
//!
//! When the resolver runs in [`ResolveMode::Locked`], the
//! provider records every constraint emitted by
//! `get_dependencies` so a
//! [`ResolveError::LockedVersionViolatesConstraint`] on a
//! transitive package can cite the parents that imposed the
//! requirement.  The recorder is held as
//! `Option<LockedConstraintRecorder>` and constructed only in
//! `Locked` mode - see [`crate::locked`] for the invariant.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Bound;

use cabin_core::standard_compatibility::{ConsumerStandards, edge_compatible};
use cabin_core::{
    DEFAULT_FEATURE_KEY, FeatureEntry, IncompatibleStandards, PackageName, Requirement,
    StandardsMetadata, escape_control_chars,
};
use cabin_index::{IndexEntry, IndexPackageDependency, PackageIndex, VersionMetadata};
use pubgrub::{Dependencies, DependencyProvider, PackageResolutionStatistics, Ranges};
use semver::{Version, VersionReq};

use crate::error::{ResolveError, ResolverConstraint};
use crate::input::{LockedVersion, ResolveInput, ResolveMode};
use crate::locked::{LockedConstraintRecorder, validate_locked_metadata};
use crate::range::req_to_range;

/// A `PubGrub` package.  Feature requests are solver packages of
/// their own, so feature unification and version selection are one
/// problem: `Feature(b, f)`'s versions are the versions of `b` that
/// declare `f`, and selecting one pins `Package(b)` to that version.
/// A requested feature therefore selects an older version that
/// declares it over a newer one that does not, and a conflict
/// reached through a feature edge backtracks like any other.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Node {
    /// The synthetic root, labeled with the root package's name.
    Root(PackageName),
    Package(PackageName),
    Feature(PackageName, String),
}

impl Node {
    pub(crate) fn package_name(&self) -> &PackageName {
        match self {
            Node::Root(name) | Node::Package(name) | Node::Feature(name, _) => name,
        }
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Root(name) | Node::Package(name) => write!(f, "{name}"),
            // Index dependency edges carry feature strings no grammar
            // check has seen; keep them from steering the terminal.
            Node::Feature(name, feature) => write!(f, "{name}[{}]", escape_control_chars(feature)),
        }
    }
}

/// `name`'s candidate versions: the workspace's local entry when it
/// supplies the name, otherwise the index entry.
///
/// # Errors
/// [`ResolveError::ReservedRootName`] for a pure workspace's synthetic
/// root name, and [`ResolveError::MetadataNeeded`] when a lazily
/// loaded index has not fetched `name` yet.
pub(crate) fn lookup<'a>(
    input: &'a ResolveInput,
    index: &'a PackageIndex,
    name: &PackageName,
) -> Result<Option<&'a IndexEntry>, ResolveError> {
    if let Some(entry) = input.local_packages.get(name) {
        return Ok(Some(entry));
    }
    // A real root is a local package; only a pure workspace's
    // synthetic root name reaches here, and no index package may
    // stand in for it.
    if *name == input.root_name {
        return Err(ResolveError::ReservedRootName(name.to_string()));
    }
    if let Some(entry) = index.package(name) {
        return Ok(Some(entry));
    }
    if input.lazy_index {
        return Err(ResolveError::MetadataNeeded(BTreeSet::from([name.clone()])));
    }
    Ok(None)
}

/// The metadata of one selected `name@version`, local or indexed.
pub(crate) fn version_metadata<'a>(
    input: &'a ResolveInput,
    index: &'a PackageIndex,
    name: &PackageName,
    version: &Version,
) -> Option<&'a VersionMetadata> {
    input
        .local_packages
        .get(name)
        .or_else(|| index.package(name))
        .and_then(|entry| entry.versions.get(version))
}

/// Whether `meta` is a candidate for `feature` (`None` = the package
/// itself).  Every version offers `default`, possibly empty.
fn declares(meta: &VersionMetadata, feature: Option<&str>) -> bool {
    feature.is_none_or(|f| f == DEFAULT_FEATURE_KEY || meta.features.features.contains_key(f))
}

/// The three preference tiers of
/// `docs/design/standard-compatibility/preference-mode.md` section 1,
/// ordered best-first: a declared-and-compatible candidate is
/// preferred to an undeclared one (which merely does not deny
/// compatibility), which is preferred to a declared-but-incompatible
/// one.  The derived `Ord` is exactly that preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StandardTier {
    /// Declares an interface requirement the consumer satisfies.
    DeclaredCompatible,
    /// Declares nothing relevant to the consumer's languages
    /// (absent table, or all-unconstrained cells).
    Undeclared,
    /// Declares an interface requirement the consumer fails.
    DeclaredIncompatible,
}

/// Why a node's dependencies could not be produced: a fatal provider
/// error, or a version `PubGrub` should treat as unusable and
/// backtrack past.
enum EdgeError {
    Fatal(ResolveError),
    Unavailable(String),
}

impl From<ResolveError> for EdgeError {
    fn from(err: ResolveError) -> Self {
        EdgeError::Fatal(err)
    }
}

/// Dependency constraints under construction.  A node required twice
/// (a package edge and a feature entry naming the same dependency)
/// keeps the intersection of its ranges.
#[derive(Default)]
struct Constraints(BTreeMap<Node, Ranges<Version>>);

impl Constraints {
    fn require(&mut self, node: Node, range: Ranges<Version>) {
        match self.0.entry(node) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(range);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let merged = slot.get().intersection(&range);
                slot.insert(merged);
            }
        }
    }
}

/// `PubGrub` [`DependencyProvider`] implementation used by
/// [`crate::resolve`].
pub(crate) struct Provider<'a> {
    input: &'a ResolveInput,
    index: &'a PackageIndex,
    root_dependencies: Vec<(PackageName, Ranges<Version>)>,
    locked: BTreeMap<PackageName, LockedVersion>,
    locked_constraints: Option<LockedConstraintRecorder>,
    /// The names a lazily loaded index has not fetched that the solve
    /// reached, all reported when it stops for one of them.
    pub(crate) unfetched: RefCell<BTreeSet<PackageName>>,
}

impl<'a> Provider<'a> {
    pub(crate) fn new(
        input: &'a ResolveInput,
        index: &'a PackageIndex,
        locked: BTreeMap<PackageName, LockedVersion>,
        root_constraints: BTreeMap<PackageName, Vec<ResolverConstraint>>,
        root_dependencies: Vec<(PackageName, Ranges<Version>)>,
    ) -> Self {
        // The recorder exists iff resolution routes through
        // `choose_locked_candidate`, encoding the locked-mode
        // invariant structurally - see [`crate::locked`].
        let locked_constraints = matches!(input.mode, ResolveMode::Locked)
            .then(|| LockedConstraintRecorder::new(root_constraints));
        Self {
            input,
            index,
            root_dependencies,
            locked,
            locked_constraints,
            unfetched: RefCell::default(),
        }
    }

    fn entry(&self, name: &PackageName) -> Result<Option<&'a IndexEntry>, ResolveError> {
        lookup(self.input, self.index, name)
    }

    fn is_local(&self, name: &PackageName) -> bool {
        self.input.local_packages.contains_key(name)
    }

    /// Whether a lazily loaded index has yet to fetch `name`, which
    /// joins the names this round stops for.
    fn unfetched(&self, name: &PackageName) -> bool {
        let unfetched = matches!(self.entry(name), Err(ResolveError::MetadataNeeded(_)));
        if unfetched {
            self.unfetched.borrow_mut().insert(name.clone());
        }
        unfetched
    }

    /// Candidate versions of `name` in `range` offering `feature`,
    /// without fetching anything: `prioritize` cannot fail, and a
    /// missing entry counts zero so `choose_version` reports it next.
    fn candidate_count(
        &self,
        name: &PackageName,
        range: &Ranges<Version>,
        feature: Option<&str>,
    ) -> usize {
        self.input
            .local_packages
            .get(name)
            .or_else(|| self.index.package(name))
            .map_or(0, |entry| {
                entry
                    .versions
                    .iter()
                    .filter(|(v, meta)| range.contains(v) && declares(meta, feature))
                    .count()
            })
    }

    /// Whether any version of `name` in `range` declares a non-empty
    /// `default` list.  An edge onto a package with none requests no
    /// `default` node, which keeps conflict explanations of
    /// feature-free graphs unchanged.
    fn declares_default_features(
        &self,
        name: &PackageName,
        range: &Ranges<Version>,
    ) -> Result<bool, ResolveError> {
        let entry = match self.entry(name) {
            // The edge requires `name` itself, so a round that reaches
            // it stops for its metadata before it can succeed; the next
            // round reads its features.
            Err(ResolveError::MetadataNeeded(_)) => {
                self.unfetched.borrow_mut().insert(name.clone());
                return Ok(false);
            }
            entry => entry?,
        };
        Ok(entry.is_some_and(|entry| {
            entry
                .versions
                .iter()
                .any(|(v, meta)| range.contains(v) && !meta.features.default.is_empty())
        }))
    }

    /// In `Locked` mode, record the edge `parent` places on `dep_name`
    /// and match it against the locked version.
    fn check_locked_edge(
        &self,
        parent: &PackageName,
        dep_name: &PackageName,
        dep: &IndexPackageDependency,
    ) -> Result<(), EdgeError> {
        if let Some(recorder) = &self.locked_constraints {
            recorder.record(dep_name, parent.clone(), dep.req.clone());
            // Matched per edge, not when `dep_name` is decided: an edge
            // arriving after that decision would conflict with it, and
            // the backjump would report the edge's parent instead.
            if let Some(locked) = self.locked.get(dep_name)
                && !self.is_local(dep_name)
                && !dep.req.matches(&locked.version)
            {
                return Err(EdgeError::Fatal(
                    ResolveError::LockedVersionViolatesConstraint {
                        name: dep_name.as_str().to_owned(),
                        version: locked.version.to_string(),
                        constraints: vec![ResolverConstraint {
                            origin: parent.clone(),
                            requirement: dep.req.clone(),
                        }],
                    },
                ));
            }
        }
        Ok(())
    }

    /// Require one active dependency edge of `parent@version`: the
    /// package itself plus the features the edge requests.
    fn require_edge(
        &self,
        deps: &mut Constraints,
        parent: &PackageName,
        version: &Version,
        dep_name: &PackageName,
        dep: &IndexPackageDependency,
    ) -> Result<(), EdgeError> {
        self.check_locked_edge(parent, dep_name, dep)?;
        // The trimmed edge range already excludes a local pre-release
        // this edge rejects; failing the edge here names the local
        // version in the conflict instead of a range with a hole.
        if let Some(local) = self.input.local_packages.get(dep_name)
            && let Some(local_version) = local.versions.keys().find(|v| !v.pre.is_empty())
            && !dep.req.matches(local_version)
        {
            return Err(EdgeError::Unavailable(format!(
                "{parent} {version} requires {dep_name} {}, which the local {dep_name} {local_version} does not satisfy",
                dep.req
            )));
        }
        let range = edge_range(self.input, self.index, parent, version, dep_name, dep)?;
        if dep.default_features && self.declares_default_features(dep_name, &range)? {
            deps.require(
                Node::Feature(dep_name.clone(), DEFAULT_FEATURE_KEY.to_owned()),
                range.clone(),
            );
        }
        for feature in &dep.features {
            deps.require(
                Node::Feature(dep_name.clone(), feature.clone()),
                range.clone(),
            );
        }
        deps.require(Node::Package(dep_name.clone()), range);
        Ok(())
    }

    fn constraints(&self, node: &Node, version: &Version) -> Result<Constraints, EdgeError> {
        let mut deps = Constraints::default();
        let (name, feature) = match node {
            Node::Root(_) => {
                for (name, range) in &self.root_dependencies {
                    deps.require(Node::Package(name.clone()), range.clone());
                }
                for (name, features) in &self.input.root_features {
                    let range = self
                        .root_dependencies
                        .iter()
                        .find(|(dep, _)| dep == name)
                        .map_or_else(Ranges::full, |(_, range)| range.clone());
                    deps.require(Node::Package(name.clone()), range.clone());
                    for feature in features {
                        if feature == DEFAULT_FEATURE_KEY
                            && !self.declares_default_features(name, &range)?
                        {
                            continue;
                        }
                        deps.require(Node::Feature(name.clone(), feature.clone()), range.clone());
                    }
                }
                return Ok(deps);
            }
            Node::Package(name) => (name, None),
            Node::Feature(name, feature) => (name, Some(feature.as_str())),
        };

        let entry = self
            .entry(name)?
            .ok_or_else(|| ResolveError::UnknownPackage(name.as_str().to_owned()))?;
        let Some(meta) = entry.versions.get(version) else {
            return Err(EdgeError::Unavailable(format!(
                "{name} {version} is not present in the index"
            )));
        };

        let Some(feature) = feature else {
            // Optional edges join through the features that enable
            // them.  A `cfg(...)`-gated edge participates whatever
            // its predicate: the lockfile serves every platform.
            for (dep_name, dep) in &meta.dependencies {
                if !dep.optional {
                    self.require_edge(&mut deps, name, version, dep_name, dep)?;
                }
            }
            return Ok(deps);
        };

        let exact = Ranges::singleton(version.clone());
        deps.require(Node::Package(name.clone()), exact.clone());
        if feature == DEFAULT_FEATURE_KEY {
            for implied in &meta.features.default {
                deps.require(Node::Feature(name.clone(), implied.clone()), exact.clone());
            }
            return Ok(deps);
        }
        let Some(entries) = meta.features.features.get(feature) else {
            return Err(EdgeError::Unavailable(format!(
                "{name} {version} does not declare feature {feature:?}"
            )));
        };
        for raw in entries {
            let entry = FeatureEntry::parse(raw).map_err(|kind| {
                EdgeError::Unavailable(format!(
                    "{name} {version} feature {feature:?} has an invalid entry {raw:?}: {}",
                    kind.message()
                ))
            })?;
            match entry {
                FeatureEntry::Local(implied) => {
                    deps.require(Node::Feature(name.clone(), implied), exact.clone());
                }
                FeatureEntry::OptionalDep(dep_name) => {
                    let (dep_name, dep) = declared_dep(name, version, feature, meta, &dep_name)?;
                    if !dep.optional {
                        return Err(EdgeError::Unavailable(format!(
                            "{name} {version} feature {feature:?} enables {dep_name}, which is not optional"
                        )));
                    }
                    self.require_edge(&mut deps, name, version, dep_name, dep)?;
                }
                FeatureEntry::DepFeature {
                    dep: dep_name,
                    feature: requested,
                } => {
                    let (dep_name, dep) = declared_dep(name, version, feature, meta, &dep_name)?;
                    if dep.optional {
                        self.require_edge(&mut deps, name, version, dep_name, dep)?;
                    } else {
                        self.check_locked_edge(name, dep_name, dep)?;
                    }
                    let range = edge_range(self.input, self.index, name, version, dep_name, dep)?;
                    deps.require(Node::Feature(dep_name.clone(), requested), range);
                }
            }
        }
        Ok(deps)
    }
}

/// The `PubGrub` range of one dependency edge, trimmed by
/// [`without_rejected_prereleases`].  An unsupported
/// requirement syntax on a transitive dep is a backtrackable miss,
/// not a fatal error: an older version of `parent` may declare the
/// same dep with a syntax this build understands.  The root path
/// catches unsupported syntax up front, where the error names the
/// user-authored requirement.
fn edge_range(
    input: &ResolveInput,
    index: &PackageIndex,
    parent: &PackageName,
    version: &Version,
    dep_name: &PackageName,
    dep: &IndexPackageDependency,
) -> Result<Ranges<Version>, EdgeError> {
    let range = req_to_range(&dep.req).map_err(|err| {
        EdgeError::Unavailable(format!(
            "{parent} {version} declares an unsupported version requirement for {dep_name}: {}",
            err.requirement,
        ))
    })?;
    Ok(without_rejected_prereleases(
        input, index, dep_name, &dep.req, range,
    ))
}

/// `range`, the numeric range of `req` on `name`, without each
/// published pre-release of `name` that `req` rejects.  Semver's
/// pre-release rule binds every requirement on its own, which no
/// interval expresses; with each requirement's range trimmed, a
/// pre-release stays in their intersection only when every one
/// admits it, in whatever order `PubGrub` intersects them.  A name
/// a lazily loaded index has not fetched keeps its whole range: a
/// round that reaches it stops for its metadata before it can
/// succeed, and the next round trims it.
pub(crate) fn without_rejected_prereleases(
    input: &ResolveInput,
    index: &PackageIndex,
    name: &PackageName,
    req: &VersionReq,
    range: Ranges<Version>,
) -> Ranges<Version> {
    let Ok(Some(entry)) = lookup(input, index, name) else {
        return range;
    };
    let holes: Vec<&Version> = entry
        .versions
        .keys()
        .filter(|v| !v.pre.is_empty() && range.contains(v) && !req.matches(v))
        .collect();
    if holes.is_empty() {
        return range;
    }
    // The complement of the holes, built in one pass from the sorted
    // keys: `(<h1)`, `(h1, h2)`, ..., `(>hn)`.
    let lower = std::iter::once(Bound::Unbounded)
        .chain(holes.iter().map(|v| Bound::Excluded((*v).clone())));
    let upper = holes
        .iter()
        .map(|v| Bound::Excluded((*v).clone()))
        .chain(std::iter::once(Bound::Unbounded));
    range.intersection(&lower.zip(upper).collect())
}

/// The normal dependency a feature entry of `name@version` names.
/// An undeclared one makes the version unusable: index metadata is
/// not validated against its own dependency table.
fn declared_dep<'m>(
    name: &PackageName,
    version: &Version,
    feature: &str,
    meta: &'m VersionMetadata,
    dep_name: &str,
) -> Result<(&'m PackageName, &'m IndexPackageDependency), EdgeError> {
    meta.dependencies
        .iter()
        .find(|(declared, _)| declared.as_str() == dep_name)
        .ok_or_else(|| {
            EdgeError::Unavailable(format!(
                "{name} {version} feature {feature:?} references undeclared dependency {dep_name}"
            ))
        })
}

impl DependencyProvider for Provider<'_> {
    type P = Node;
    type V = Version;
    type VS = Ranges<Version>;
    type M = String;
    /// Conflict count first, as `PubGrub` recommends; then feature
    /// nodes before package nodes, so a requested feature picks its
    /// package's version instead of the package picking its newest
    /// version and backtracking when that one lacks the feature;
    /// then fewest candidates first.
    type Priority = (u32, u8, Reverse<usize>);
    type Err = ResolveError;

    fn choose_version(
        &self,
        node: &Self::P,
        range: &Self::VS,
    ) -> Result<Option<Self::V>, Self::Err> {
        let (name, feature) = match node {
            Node::Root(_) => {
                let root = &self.input.root_version;
                return Ok(range.contains(root).then(|| root.clone()));
            }
            Node::Package(name) => (name, None),
            Node::Feature(name, feature) => (name, Some(feature.as_str())),
        };

        // A transitive package missing from the index is a
        // backtrackable miss, not a fatal error: an older
        // version of the parent might depend on a different
        // (present) package, and returning `Err` here would
        // abort resolution before `PubGrub` could try that
        // alternative.  Root-level unknowns are caught in
        // preflight where the error is unambiguous.
        let Some(entry) = self.entry(name)? else {
            return Ok(None);
        };

        // A local package offers exactly its working copy's version:
        // it is never yanked and never pinned by the lockfile.
        if self.is_local(name) {
            return Ok(entry
                .versions
                .iter()
                .find(|(v, meta)| {
                    range.contains(v)
                        && candidate_admits_prerelease(range, v)
                        && declares(meta, feature)
                })
                .map(|(v, _)| v.clone()));
        }

        if let Some(recorder) = &self.locked_constraints {
            let version = self.choose_locked_candidate(name, recorder, entry, range)?;
            if let Some(feature) = feature
                && !entry
                    .versions
                    .get(&version)
                    .is_some_and(|meta| declares(meta, Some(feature)))
            {
                return Err(ResolveError::LockedVersionLacksFeature {
                    name: name.as_str().to_owned(),
                    version: version.to_string(),
                    feature: feature.to_owned(),
                });
            }
            return Ok(Some(version));
        }

        Ok(self.choose_compatible_candidate(name, entry, range, feature))
    }

    fn prioritize(
        &self,
        node: &Self::P,
        range: &Self::VS,
        package_conflicts_counts: &PackageResolutionStatistics,
    ) -> Self::Priority {
        let (count, tier) = match node {
            Node::Root(_) => (usize::from(range.contains(&self.input.root_version)), 0),
            Node::Package(name) => (self.candidate_count(name, range, None), 0),
            Node::Feature(name, feature) => (self.candidate_count(name, range, Some(feature)), 1),
        };
        if count == 0 {
            // A package a lazily loaded index has not fetched, which
            // counts zero, goes last, so a round decides everything it
            // can before it stops, and the next fetches every name it
            // reached at once.
            if let Node::Package(name) | Node::Feature(name, _) = node
                && self.unfetched(name)
            {
                return (0, 0, Reverse(usize::MAX));
            }
            return (u32::MAX, tier, Reverse(0));
        }
        (
            package_conflicts_counts.conflict_count(),
            tier,
            Reverse(count),
        )
    }

    fn get_dependencies(
        &self,
        node: &Self::P,
        version: &Self::V,
    ) -> Result<Dependencies<Self::P, Self::VS, Self::M>, Self::Err> {
        match self.constraints(node, version) {
            Ok(deps) => Ok(Dependencies::Available(deps.0.into_iter().collect())),
            Err(EdgeError::Unavailable(reason)) => Ok(Dependencies::Unavailable(reason)),
            Err(EdgeError::Fatal(err)) => Err(err),
        }
    }
}

impl Provider<'_> {
    /// Pick a version of `entry` inside `range`, preferring the
    /// lockfile entry when it qualifies and otherwise applying the
    /// standard-preference ordering of
    /// `docs/design/standard-compatibility/preference-mode.md`.
    ///
    /// The admissible set - non-yanked, pre-release-filtered versions
    /// in `range` - is computed identically under both
    /// [`IncompatibleStandards`] values, and this method returns
    /// `None` in exactly the same cases (an empty admissible set), so
    /// standards never change solvability: `Fallback` only reorders
    /// choices the provider was already free to make.
    ///
    /// Returns `None` when no candidate qualifies; `PubGrub`
    /// then backtracks and may select a different version of a
    /// sibling package that loosens this range.
    fn choose_compatible_candidate(
        &self,
        package: &PackageName,
        entry: &IndexEntry,
        range: &Ranges<Version>,
        feature: Option<&str>,
    ) -> Option<Version> {
        let mut candidates: Vec<&Version> = entry
            .versions
            .iter()
            .filter(|(v, meta)| range.contains(*v) && !meta.yanked && declares(meta, feature))
            .map(|(v, _)| v)
            .collect();
        if candidates.is_empty() {
            return None;
        }

        candidates.retain(|v| candidate_admits_prerelease(range, v));
        if candidates.is_empty() {
            return None;
        }

        // Lockfile stability wins (preference-mode.md rule 4): a locked
        // version that still qualifies is kept regardless of standards,
        // and never counts as a hold-back - metadata alone never churns
        // a lockfile.
        if let Some(locked) = self.locked.get(package)
            && candidates.contains(&&locked.version)
        {
            return Some(locked.version.clone());
        }

        if self.input.incompatible_standards == IncompatibleStandards::Allow {
            // The pure-semver pick: the newest admissible version.
            return candidates.into_iter().max().cloned();
        }

        // Fallback: order by tier (best first), newest-first within a
        // tier.  Never filters, so the worst case is the newest of the
        // worst tier - the same version `Allow` selects.  Hold-back
        // reporting is not computed here; it is the diff against the
        // `Allow` solution (see [`crate::held_back_report`]).
        candidates
            .into_iter()
            .min_by(|a, b| {
                self.tier(entry, a)
                    .cmp(&self.tier(entry, b))
                    .then_with(|| b.cmp(a))
            })
            .cloned()
    }

    /// Classify a candidate version into its [`StandardTier`] against
    /// the consumer standards (preference-mode.md section 1).
    fn tier(&self, entry: &IndexEntry, version: &Version) -> StandardTier {
        entry
            .versions
            .get(version)
            .map_or(StandardTier::Undeclared, |meta| {
                classify(self.input.consumer_standards, &meta.standards)
            })
    }

    /// Pick the locked version for `package` in `Locked` mode,
    /// emitting the more specific `Locked*` variants when the
    /// locked entry conflicts with the index or with the range
    /// `PubGrub` derived during this resolve.  Preflight matches
    /// root requirements and [`Self::require_edge`] matches every
    /// transitive one against the locked version.
    fn choose_locked_candidate(
        &self,
        package: &PackageName,
        recorder: &LockedConstraintRecorder,
        entry: &IndexEntry,
        range: &Ranges<Version>,
    ) -> Result<Version, ResolveError> {
        let locked = self
            .locked
            .get(package)
            .ok_or_else(|| ResolveError::LockfileMissingPackage(package.as_str().to_owned()))?;
        validate_locked_metadata(package, locked, entry)?;
        if !range.contains(&locked.version) {
            return Err(ResolveError::LockedVersionViolatesConstraint {
                name: package.as_str().to_owned(),
                version: locked.version.to_string(),
                constraints: recorder.snapshot(package),
            });
        }
        Ok(locked.version.clone())
    }
}

/// Classify a candidate version's declared standards into its
/// [`StandardTier`] against `consumer` (preference-mode.md section 1).
/// The advertised requirement is the version-wide join (the transitive
/// fallback); "declared" means it constrains a language the consumer
/// actually compiles.  `!edge_compatible` is exactly the
/// declared-incompatible tier, since an unconstrained requirement is
/// satisfied at every level.
fn classify(consumer: ConsumerStandards, standards: &StandardsMetadata) -> StandardTier {
    let advertised = standards.version_wide_join();
    let declares_relevant = (consumer.c.is_some() && advertised.c != Requirement::Unconstrained)
        || (consumer.cxx.is_some() && advertised.cxx != Requirement::Unconstrained);
    if !declares_relevant {
        StandardTier::Undeclared
    } else if edge_compatible(consumer, advertised) {
        StandardTier::DeclaredCompatible
    } else {
        StandardTier::DeclaredIncompatible
    }
}

/// Decide whether `candidate` is admissible under `range`'s
/// pre-release rule.
///
/// Pre-release versions are excluded by default, mirroring
/// [`semver::VersionReq::matches`].  A pre-release is admitted
/// only when one of the comparator-derived bounds defining `range`
/// shares its `major.minor.patch` with a non-empty `pre` tag (the
/// `>=1.0.0-alpha, <1.0.0` style opt-in semver expects; the exact
/// form `= 1.0.0-alpha` carries the tag on its lower bound), or
/// when the range is exactly that singleton.
///
/// A lockfile-pinned pre-release is *not* bypassed here: if the
/// manifest constraint no longer admits it, `PreferLocked` must
/// fall back to a compatible release rather than carry the lock
/// forward in violation of the constraint.
pub(crate) fn candidate_admits_prerelease(range: &Ranges<Version>, candidate: &Version) -> bool {
    candidate.pre.is_empty()
        || range.as_singleton() == Some(candidate)
        || range_admits_prerelease_of(range, candidate)
}

/// Return `true` if `range` has a bound whose value shares
/// `candidate`'s `major.minor.patch` and carries a non-empty
/// pre-release tag.
///
/// This mirrors semver's `pre_is_compatible` rule: a pre-release
/// version is admissible against a requirement only when one of
/// its comparators names the same triple with a non-empty `pre`
/// field.  A range [`req_to_range`] builds from one requirement has
/// that requirement's comparators as its bounds, so the check agrees
/// with semver there up to the approximation the `range` module
/// documents.  On an intersection of several requirements it would
/// admit a pre-release any one of them opts into (`^1.2.0-beta.1`
/// and `^1.0` together admit `1.2.0-beta.3`), so each requirement's
/// range first drops the published pre-releases it rejects
/// ([`without_rejected_prereleases`]).
///
/// Bounds with non-empty build metadata are synthetic successors
/// (`range::just_above_all_builds`) - a comparator has no build
/// field, so no user-authored requirement produces one.  Their
/// pre-release tag encodes an interval boundary, not a comparator's
/// opt-in, so they must not admit pre-release candidates.
fn range_admits_prerelease_of(range: &Ranges<Version>, candidate: &Version) -> bool {
    let matches = |bound: &Bound<Version>| match bound {
        Bound::Included(v) | Bound::Excluded(v) => {
            v.build.is_empty()
                && !v.pre.is_empty()
                && v.major == candidate.major
                && v.minor == candidate.minor
                && v.patch == candidate.patch
        }
        Bound::Unbounded => false,
    };
    range.iter().any(|(lo, hi)| matches(lo) || matches(hi))
}
