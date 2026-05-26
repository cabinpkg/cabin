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
//! version sets. Cabin uses [`PackageName`], [`semver::Version`],
//! and [`Ranges<semver::Version>`](Ranges) (the latter built by
//! [`crate::range::req_to_range`]). The provider also models a
//! synthetic root package — Cabin's workspace root is not
//! published in the index, so the solver is given the root's
//! identity and the resolved root-dependency requirements at
//! construction time.
//!
//! ## Targeted errors
//!
//! `PubGrub`'s `NoSolution` variant only carries a derivation
//! tree, not Cabin's actionable error variants. Root-level
//! errors (`UnknownPackage`, `NoMatchingVersion`,
//! `AllMatchingVersionsYanked`, every `Locked*` variant for
//! direct dependencies) are produced ahead of time in
//! [`crate::preflight`]. Transitive `Locked`-mode failures
//! surface here, returned through `PubGrub`'s
//! `ErrorChoosingVersion` as their original [`ResolveError`].
//!
//! ## Locked-mode constraint recording
//!
//! When the resolver runs in [`ResolveMode::Locked`], the
//! provider records every constraint emitted by
//! [`Self::get_dependencies`] so a
//! [`ResolveError::LockedVersionViolatesConstraint`] on a
//! transitive package can cite the parents that imposed the
//! requirement. The recorder is held as
//! `Option<LockedConstraintRecorder>` and constructed only in
//! `Locked` mode — see [`crate::locked`] for the invariant.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::ops::Bound;

use cabin_core::{PackageName, TargetPlatform};
use cabin_index::{IndexEntry, PackageIndex};
use pubgrub::{
    Dependencies, DependencyConstraints, DependencyProvider, PackageResolutionStatistics, Ranges,
};
use semver::Version;

use crate::error::{ResolveError, ResolverConstraint};
use crate::input::{LockedVersion, ResolveInput, ResolveMode};
use crate::locked::{LockedConstraintRecorder, validate_locked_metadata};
use crate::range::req_to_range;

/// `PubGrub` [`DependencyProvider`] implementation used by
/// [`crate::resolve`].
///
/// One provider is constructed per resolve. In `Locked` mode it
/// owns a [`LockedConstraintRecorder`] seeded with the
/// preflight-collected root constraints; outside `Locked` mode
/// the recorder is absent because backtracking would invalidate
/// it (see [`crate::locked`]).
pub(crate) struct Provider<'a> {
    index: &'a PackageIndex,
    root_name: PackageName,
    root_version: Version,
    root_dependencies: Vec<(PackageName, Ranges<Version>)>,
    locked: BTreeMap<PackageName, LockedVersion>,
    platform: TargetPlatform,
    locked_constraints: Option<LockedConstraintRecorder>,
}

impl<'a> Provider<'a> {
    pub(crate) fn new(
        input: &ResolveInput,
        index: &'a PackageIndex,
        locked: BTreeMap<PackageName, LockedVersion>,
        platform: TargetPlatform,
        root_constraints: BTreeMap<PackageName, Vec<ResolverConstraint>>,
        root_dependencies: Vec<(PackageName, Ranges<Version>)>,
    ) -> Self {
        // The recorder exists iff resolution routes through
        // `choose_locked_candidate`, encoding the locked-mode
        // invariant structurally — see [`crate::locked`].
        let locked_constraints = matches!(input.mode, ResolveMode::Locked)
            .then(|| LockedConstraintRecorder::new(root_constraints));
        Self {
            index,
            root_name: input.root_name.clone(),
            root_version: input.root_version.clone(),
            root_dependencies,
            locked,
            platform,
            locked_constraints,
        }
    }

    fn is_root(&self, package: &PackageName) -> bool {
        package == &self.root_name
    }
}

impl DependencyProvider for Provider<'_> {
    type P = PackageName;
    type V = Version;
    type VS = Ranges<Version>;
    type M = String;
    type Priority = (u32, Reverse<usize>);
    type Err = ResolveError;

    fn choose_version(
        &self,
        package: &Self::P,
        range: &Self::VS,
    ) -> Result<Option<Self::V>, Self::Err> {
        if self.is_root(package) {
            return Ok(if range.contains(&self.root_version) {
                Some(self.root_version.clone())
            } else {
                None
            });
        }

        // A transitive package missing from the index is a
        // backtrackable miss, not a fatal error: an older
        // version of the parent might depend on a different
        // (present) package, and returning `Err` here would
        // abort resolution before `PubGrub` could try that
        // alternative. Root-level unknowns are caught in
        // preflight where the error is unambiguous.
        let Some(entry) = self.index.package(package) else {
            return Ok(None);
        };

        if let Some(recorder) = &self.locked_constraints {
            return self.choose_locked_candidate(package, recorder, entry, range);
        }

        Ok(self.choose_compatible_candidate(package, entry, range))
    }

    fn prioritize(
        &self,
        package: &Self::P,
        range: &Self::VS,
        package_conflicts_counts: &PackageResolutionStatistics,
    ) -> Self::Priority {
        let count = if self.is_root(package) {
            usize::from(range.contains(&self.root_version))
        } else if let Some(entry) = self.index.package(package) {
            entry.versions.keys().filter(|v| range.contains(v)).count()
        } else {
            0
        };
        if count == 0 {
            return (u32::MAX, Reverse(0));
        }
        (package_conflicts_counts.conflict_count(), Reverse(count))
    }

    fn get_dependencies(
        &self,
        package: &Self::P,
        version: &Self::V,
    ) -> Result<Dependencies<Self::P, Self::VS, Self::M>, Self::Err> {
        if self.is_root(package) {
            return Ok(Dependencies::Available(
                self.root_dependencies.iter().cloned().collect(),
            ));
        }

        let entry = self
            .index
            .package(package)
            .ok_or_else(|| ResolveError::UnknownPackage(package.as_str().to_owned()))?;
        let Some(meta) = entry.versions.get(version) else {
            return Ok(Dependencies::Unavailable(format!(
                "{package} {version} is not present in the index"
            )));
        };

        let mut deps: Vec<(PackageName, Ranges<Version>)> = Vec::new();
        for (dep_name, dep_entry) in &meta.dependencies {
            // Optional registry deps stay out of resolution
            // until a feature enables them, matching
            // cabin-workspace::patch's conservative default.
            if dep_entry.optional {
                continue;
            }
            // Conditional deps whose `cfg(...)` predicate fails
            // on the host platform never enter resolution on
            // this machine.
            if let Some(cond) = &dep_entry.condition
                && !cond.evaluate(&self.platform)
            {
                continue;
            }
            if let Some(recorder) = &self.locked_constraints {
                recorder.record(dep_name, package.clone(), dep_entry.req.clone());
            }
            let range = req_to_range(&dep_entry.req).map_err(|err| {
                ResolveError::UnsupportedVersionRequirement {
                    package: dep_name.as_str().to_owned(),
                    requirement: err.requirement,
                }
            })?;
            deps.push((dep_name.clone(), range));
        }
        Ok(Dependencies::Available(DependencyConstraints::from_iter(
            deps,
        )))
    }
}

impl Provider<'_> {
    /// Pick the newest non-yanked, semver-admissible version of
    /// `entry` that lives inside `range`, preferring the
    /// lockfile entry when it qualifies.
    ///
    /// Returns `None` when no candidate qualifies; `PubGrub`
    /// then backtracks and may select a different version of a
    /// sibling package that loosens this range.
    fn choose_compatible_candidate(
        &self,
        package: &PackageName,
        entry: &IndexEntry,
        range: &Ranges<Version>,
    ) -> Option<Version> {
        let matching: Vec<&Version> = entry
            .versions
            .iter()
            .filter(|(v, _)| range.contains(*v))
            .map(|(v, _)| v)
            .collect();
        if matching.is_empty() {
            return None;
        }

        let mut non_yanked: Vec<&Version> = matching
            .iter()
            .copied()
            .filter(|v| !entry.versions.get(v).is_some_and(|m| m.yanked))
            .collect();
        if non_yanked.is_empty() {
            return None;
        }

        non_yanked.retain(|v| candidate_admits_prerelease(range, v));
        if non_yanked.is_empty() {
            return None;
        }

        if let Some(locked) = self.locked.get(package)
            && non_yanked.contains(&&locked.version)
        {
            return Some(locked.version.clone());
        }

        non_yanked.into_iter().max().cloned()
    }

    /// Pick the locked version for `package` in `Locked` mode,
    /// emitting the more specific `Locked*` variants when the
    /// locked entry conflicts with the index or with constraints
    /// observed during this resolve. Preflight covers root
    /// dependencies; this branch covers transitive ones.
    fn choose_locked_candidate(
        &self,
        package: &PackageName,
        recorder: &LockedConstraintRecorder,
        entry: &IndexEntry,
        range: &Ranges<Version>,
    ) -> Result<Option<Version>, ResolveError> {
        let locked = self
            .locked
            .get(package)
            .ok_or_else(|| ResolveError::LockfileMissingPackage(package.as_str().to_owned()))?;
        validate_locked_metadata(package, locked, entry)?;
        // The `Ranges` algebra is purely numeric, so a
        // pre-release locked version may sit inside `range`
        // while semver's pre-release rule still rejects it.
        // Re-check the recorded `VersionReq`s with full semver
        // semantics so `--locked` does not accept a lockfile
        // entry the user's manifest does not actually allow.
        let observed = recorder.snapshot(package);
        let semver_satisfies = observed
            .iter()
            .all(|c| c.requirement.matches(&locked.version));
        if !range.contains(&locked.version) || !semver_satisfies {
            return Err(ResolveError::LockedVersionViolatesConstraint {
                name: package.as_str().to_owned(),
                version: locked.version.to_string(),
                constraints: observed,
            });
        }
        Ok(Some(locked.version.clone()))
    }
}

/// Decide whether `candidate` is admissible under `range`'s
/// pre-release rule.
///
/// Pre-release versions are excluded by default, mirroring
/// [`semver::VersionReq::matches`]. A pre-release is admitted
/// only when one of the bounds defining `range` shares its
/// `major.minor.patch` with a non-empty `pre` tag (the
/// `>=1.0.0-alpha, <1.0.0` style opt-in semver expects), or when
/// the range is exactly that singleton (`= 1.0.0-alpha`).
///
/// A lockfile-pinned pre-release is *not* bypassed here: if the
/// manifest constraint no longer admits it, `PreferLocked` must
/// fall back to a compatible release rather than carry the lock
/// forward in violation of the constraint.
fn candidate_admits_prerelease(range: &Ranges<Version>, candidate: &Version) -> bool {
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
/// field. Because the range bounds come from those comparators
/// (via [`req_to_range`]), checking the bounds is equivalent in
/// practice and avoids carrying the original [`VersionReq`] set
/// alongside the range.
fn range_admits_prerelease_of(range: &Ranges<Version>, candidate: &Version) -> bool {
    let matches = |bound: &Bound<Version>| match bound {
        Bound::Included(v) | Bound::Excluded(v) => {
            !v.pre.is_empty()
                && v.major == candidate.major
                && v.minor == candidate.minor
                && v.patch == candidate.patch
        }
        Bound::Unbounded => false,
    };
    range.iter().any(|(lo, hi)| matches(lo) || matches(hi))
}
