//! `PubGrub`-backed solver. Cabin's manifest, index, and
//! lockfile model is mapped onto `PubGrub`'s
//! [`DependencyProvider`] surface so the public
//! [`resolve`](super::resolve) entry point can stay
//! Cabin-shaped while the actual conflict-driven backtracking
//! is delegated to `PubGrub`.
//!
//! ## Layering
//!
//! `PubGrub` solves over abstract packages, versions, and
//! version sets. Cabin uses `PackageName`, `semver::Version`,
//! and `Ranges<semver::Version>` (the latter built by
//! [`crate::range::req_to_range`]). The provider also models a
//! synthetic root package — Cabin's workspace root is not
//! published in the index, so the solver is given the root's
//! identity and the resolved root-dependency requirements at
//! construction time.
//!
//! ## Targeted errors
//!
//! `PubGrub`'s `NoSolution` variant only carries a derivation
//! tree, not Cabin's actionable error variants. To preserve
//! `UnknownPackage`, `NoMatchingVersion`, and
//! `AllMatchingVersionsYanked` for root-level failures, the
//! provider runs a small preflight against the root
//! dependencies before invoking `PubGrub`. The same preflight
//! handles all five `Locked`-mode error variants for root
//! dependencies. Transitive `Locked`-mode failures fall
//! through `choose_version` as targeted `ResolveError` values
//! returned via `PubGrub`'s `ErrorChoosingVersion`.
//!
//! Any failure `PubGrub` itself reports (`NoSolution`) is
//! collapsed into [`ResolveError::Conflict`] with a
//! deterministic human-readable explanation built from
//! `PubGrub`'s [`DefaultStringReporter`].

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::ops::Bound;

use cabin_core::{PackageName, TargetPlatform};
use cabin_index::{IndexEntry, PackageIndex};
use pubgrub::{
    Dependencies, DependencyConstraints, DependencyProvider, PackageResolutionStatistics,
    PubGrubError, Ranges, Reporter,
};
use semver::Version;

use crate::error::{ResolveError, ResolverConstraint};
use crate::input::{LockedVersion, ResolveInput, ResolveMode};
use crate::output::{ResolveOutput, ResolvedPackage, ResolvedSource};
use crate::range::req_to_range;

/// Entry point used by [`crate::resolve`].
pub(crate) fn run(
    input: &ResolveInput,
    index: &PackageIndex,
) -> Result<ResolveOutput, ResolveError> {
    let platform = TargetPlatform::current();
    let locked = effective_locked(input);

    // Preflight: the targeted error variants for root
    // dependencies are computable without invoking `PubGrub`
    // and produce cleaner messages than a derivation-tree
    // explanation would.
    let root_constraints = preflight(input, index, &locked)?;

    let provider = Provider::new(input, index, locked, platform, root_constraints);

    let solution = match pubgrub::resolve(
        &provider,
        input.root_name.clone(),
        input.root_version.clone(),
    ) {
        Ok(solution) => solution,
        Err(PubGrubError::NoSolution(mut tree)) => {
            tree.collapse_no_versions();
            let detail = pubgrub::DefaultStringReporter::report(&tree);
            return Err(ResolveError::Conflict {
                package: pick_conflict_package(&tree, &input.root_name),
                detail: normalize_explanation(&detail),
            });
        }
        // `ErrorChoosingVersion`, `ErrorRetrievingDependencies`,
        // and `ErrorInShouldCancel` all bubble a provider-side
        // `ResolveError` back out — collapse them so the caller
        // sees the original variant.
        Err(
            PubGrubError::ErrorChoosingVersion { source, .. }
            | PubGrubError::ErrorRetrievingDependencies { source, .. }
            | PubGrubError::ErrorInShouldCancel(source),
        ) => return Err(source),
    };

    Ok(build_output(input, solution))
}

/// Apply the `ResolveMode`-dependent visibility rules to the
/// caller-supplied locked map. `UpdateAll` clears it; an
/// `UpdatePackage(name)` request drops only `name`. The
/// returned map is the one the resolver actually sees.
fn effective_locked(input: &ResolveInput) -> BTreeMap<PackageName, LockedVersion> {
    match &input.mode {
        ResolveMode::UpdateAll => BTreeMap::new(),
        ResolveMode::UpdatePackage(name) => {
            let mut m = input.locked.clone();
            m.remove(name);
            m
        }
        _ => input.locked.clone(),
    }
}

/// Validate root dependencies before invoking `PubGrub`.
///
/// Returns the per-package constraint records that the
/// provider then exposes when a later failure needs to cite
/// the original requirements (for example
/// `LockedVersionViolatesConstraint` against a transitive
/// dependency).
fn preflight(
    input: &ResolveInput,
    index: &PackageIndex,
    locked: &BTreeMap<PackageName, LockedVersion>,
) -> Result<BTreeMap<PackageName, Vec<ResolverConstraint>>, ResolveError> {
    let mut constraints: BTreeMap<PackageName, Vec<ResolverConstraint>> = BTreeMap::new();

    for (name, req) in &input.root_dependencies {
        constraints
            .entry(name.clone())
            .or_default()
            .push(ResolverConstraint {
                origin: input.root_name.clone(),
                requirement: req.clone(),
            });

        let entry = index
            .package(name)
            .ok_or_else(|| ResolveError::UnknownPackage(name.as_str().to_owned()))?;

        if matches!(input.mode, ResolveMode::Locked) {
            let locked_entry = locked
                .get(name)
                .ok_or_else(|| ResolveError::LockfileMissingPackage(name.as_str().to_owned()))?;
            validate_locked_entry(&input.root_name, name, locked_entry, entry, req)?;
            continue;
        }

        let matching: Vec<&Version> = entry
            .versions
            .iter()
            .filter(|(v, _)| req.matches(v))
            .map(|(v, _)| v)
            .collect();
        if matching.is_empty() {
            return Err(ResolveError::NoMatchingVersion {
                package: name.as_str().to_owned(),
                constraints: constraints.get(name).cloned().unwrap_or_default(),
            });
        }
        if !matching
            .iter()
            .any(|v| !entry.versions.get(v).is_some_and(|m| m.yanked))
        {
            return Err(ResolveError::AllMatchingVersionsYanked(
                name.as_str().to_owned(),
            ));
        }
    }

    Ok(constraints)
}

/// Run every `Locked`-mode check for a single root dependency
/// in the order the existing `ResolveError` variants imply.
/// `root_name` is the actual root package name (e.g. `"app"`),
/// recorded as the constraint origin so the rendered message
/// stays user-facing rather than literal.
fn validate_locked_entry(
    root_name: &PackageName,
    name: &PackageName,
    locked: &LockedVersion,
    entry: &IndexEntry,
    req: &semver::VersionReq,
) -> Result<(), ResolveError> {
    let meta =
        entry
            .versions
            .get(&locked.version)
            .ok_or_else(|| ResolveError::LockedVersionMissing {
                name: name.as_str().to_owned(),
                version: locked.version.to_string(),
            })?;
    if meta.yanked {
        return Err(ResolveError::LockedVersionYanked {
            name: name.as_str().to_owned(),
            version: locked.version.to_string(),
        });
    }
    if let (Some(locked_ck), Some(index_ck)) = (&locked.checksum, &meta.checksum)
        && locked_ck != index_ck
    {
        return Err(ResolveError::LockedChecksumMismatch {
            name: name.as_str().to_owned(),
            version: locked.version.to_string(),
            expected: locked_ck.clone(),
            actual: index_ck.clone(),
        });
    }
    if !req.matches(&locked.version) {
        return Err(ResolveError::LockedVersionViolatesConstraint {
            name: name.as_str().to_owned(),
            version: locked.version.to_string(),
            constraints: vec![ResolverConstraint {
                origin: root_name.clone(),
                requirement: req.clone(),
            }],
        });
    }
    Ok(())
}

/// `PubGrub` `DependencyProvider` implementation.
///
/// The provider is constructed once per `resolve` call. It
/// caches per-package constraints seen during dependency
/// expansion so error variants that cite them
/// (`LockedVersionViolatesConstraint`) can be assembled
/// without re-walking the graph.
struct Provider<'a> {
    index: &'a PackageIndex,
    root_name: PackageName,
    root_version: Version,
    root_dependencies: Vec<(PackageName, Ranges<Version>)>,
    locked: BTreeMap<PackageName, LockedVersion>,
    mode: ResolveMode,
    platform: TargetPlatform,
    // Constraints accumulate without pruning on `PubGrub`
    // backtrack. Only sound while consumers are `Locked` mode,
    // where there is effectively no backtracking — every
    // package has a single allowed version. A future caller
    // that reads `observed_constraints` from a non-`Locked`
    // failure path must instead snapshot the constraints
    // alongside each provisional decision.
    constraints: RefCell<BTreeMap<PackageName, Vec<ResolverConstraint>>>,
}

impl<'a> Provider<'a> {
    fn new(
        input: &ResolveInput,
        index: &'a PackageIndex,
        locked: BTreeMap<PackageName, LockedVersion>,
        platform: TargetPlatform,
        constraints: BTreeMap<PackageName, Vec<ResolverConstraint>>,
    ) -> Self {
        let root_dependencies = input
            .root_dependencies
            .iter()
            .map(|(name, req)| (name.clone(), req_to_range(req)))
            .collect();
        Self {
            index,
            root_name: input.root_name.clone(),
            root_version: input.root_version.clone(),
            root_dependencies,
            locked,
            mode: input.mode.clone(),
            platform,
            constraints: RefCell::new(constraints),
        }
    }

    fn is_root(&self, package: &PackageName) -> bool {
        package == &self.root_name
    }

    fn record_constraint(
        &self,
        package: &PackageName,
        origin: PackageName,
        requirement: semver::VersionReq,
    ) {
        self.constraints
            .borrow_mut()
            .entry(package.clone())
            .or_default()
            .push(ResolverConstraint {
                origin,
                requirement,
            });
    }

    fn observed_constraints(&self, package: &PackageName) -> Vec<ResolverConstraint> {
        self.constraints
            .borrow()
            .get(package)
            .cloned()
            .unwrap_or_default()
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
        // pre-flight where the error is unambiguous.
        let Some(entry) = self.index.package(package) else {
            return Ok(None);
        };

        if matches!(self.mode, ResolveMode::Locked) {
            return self.choose_locked_version(package, entry, range);
        }

        let matching: Vec<&Version> = entry
            .versions
            .iter()
            .filter(|(v, _)| range.contains(*v))
            .map(|(v, _)| v)
            .collect();
        if matching.is_empty() {
            // Letting `PubGrub` backtrack covers the case where
            // a sibling package can pick a different version
            // that loosens the range.
            return Ok(None);
        }

        let mut non_yanked: Vec<&Version> = matching
            .iter()
            .copied()
            .filter(|v| !entry.versions.get(v).is_some_and(|m| m.yanked))
            .collect();
        if non_yanked.is_empty() {
            return Ok(None);
        }

        // Pre-release versions are excluded by default,
        // mirroring `semver::VersionReq::matches`. A
        // pre-release candidate is admitted only when one of
        // the bounds defining the current range shares its
        // major.minor.patch with a non-empty pre tag (the
        // `>=1.0.0-alpha, <1.0.0` style opt-in semver
        // expects), or when the range is exactly that
        // singleton (`= 1.0.0-alpha`). A lockfile-pinned
        // pre-release is *not* bypassed here: if the manifest
        // constraint no longer admits it, `PreferLocked` must
        // fall back to a compatible release rather than carry
        // the lock forward in violation of the constraint.
        non_yanked.retain(|v| {
            v.pre.is_empty()
                || range.as_singleton() == Some(*v)
                || range_admits_prerelease_of(range, v)
        });
        if non_yanked.is_empty() {
            return Ok(None);
        }

        if let Some(locked) = self.locked.get(package)
            && non_yanked.contains(&&locked.version)
        {
            return Ok(Some(locked.version.clone()));
        }

        Ok(non_yanked.into_iter().max().cloned())
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
            // Conditional deps whose `cfg(...)` predicate
            // fails on the host platform never enter
            // resolution on this machine.
            if let Some(cond) = &dep_entry.condition
                && !cond.evaluate(&self.platform)
            {
                continue;
            }
            self.record_constraint(dep_name, package.clone(), dep_entry.req.clone());
            deps.push((dep_name.clone(), req_to_range(&dep_entry.req)));
        }
        Ok(Dependencies::Available(DependencyConstraints::from_iter(
            deps,
        )))
    }
}

impl Provider<'_> {
    /// Pick the locked version for `package` in `Locked` mode,
    /// emitting the more specific `Locked*` variants when the
    /// locked entry conflicts with the index or current
    /// constraints. Pre-flight covers root dependencies; this
    /// branch covers transitive ones.
    fn choose_locked_version(
        &self,
        package: &PackageName,
        entry: &IndexEntry,
        range: &Ranges<Version>,
    ) -> Result<Option<Version>, ResolveError> {
        let locked = self
            .locked
            .get(package)
            .ok_or_else(|| ResolveError::LockfileMissingPackage(package.as_str().to_owned()))?;
        let meta = entry.versions.get(&locked.version).ok_or_else(|| {
            ResolveError::LockedVersionMissing {
                name: package.as_str().to_owned(),
                version: locked.version.to_string(),
            }
        })?;
        if meta.yanked {
            return Err(ResolveError::LockedVersionYanked {
                name: package.as_str().to_owned(),
                version: locked.version.to_string(),
            });
        }
        if let (Some(locked_ck), Some(index_ck)) = (&locked.checksum, &meta.checksum)
            && locked_ck != index_ck
        {
            return Err(ResolveError::LockedChecksumMismatch {
                name: package.as_str().to_owned(),
                version: locked.version.to_string(),
                expected: locked_ck.clone(),
                actual: index_ck.clone(),
            });
        }
        // The `Ranges` algebra is purely numeric, so a
        // pre-release locked version may sit inside `range`
        // while semver's pre-release rule still rejects it.
        // Re-check the recorded `VersionReq`s with full
        // `semver` semantics so `--locked` does not accept a
        // lockfile entry the user's manifest does not actually
        // allow.
        let observed = self.observed_constraints(package);
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

/// Return `true` if `range` has a bound whose value shares
/// `candidate`'s `major.minor.patch` and carries a non-empty
/// pre-release tag.
///
/// This mirrors semver's `pre_is_compatible` rule: a
/// pre-release version is admissible against a requirement
/// only when one of its comparators names the same triple
/// with a non-empty `pre` field. Because the range bounds
/// come from those comparators (via `req_to_range`), checking
/// the bounds is equivalent in practice and avoids carrying
/// the original `VersionReq` set alongside the range.
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

/// Pick a representative package name to attach to a
/// `Conflict` error. Walks the derivation tree and returns the
/// alphabetically-first non-root package mentioned, or falls
/// back to the root name when only the root appears.
fn pick_conflict_package(
    tree: &pubgrub::DerivationTree<PackageName, Ranges<Version>, String>,
    root: &PackageName,
) -> String {
    let mut names: Vec<&PackageName> = tree.packages().into_iter().filter(|p| *p != root).collect();
    names.sort();
    names
        .first()
        .map_or_else(|| root.as_str().to_owned(), |p| p.as_str().to_owned())
}

/// Normalize `PubGrub`'s reporter output for deterministic
/// inclusion in error messages: trim trailing whitespace per
/// line and at the end, leave line breaks intact otherwise.
fn normalize_explanation(detail: &str) -> String {
    detail
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_owned()
}

/// Assemble the [`ResolveOutput`] from `PubGrub`'s
/// `SelectedDependencies` map.
fn build_output(
    input: &ResolveInput,
    solution: pubgrub::SelectedDependencies<PackageName, Version>,
) -> ResolveOutput {
    let mut others: Vec<ResolvedPackage> = solution
        .into_iter()
        .filter(|(name, _)| name != &input.root_name)
        .map(|(name, version)| ResolvedPackage {
            name,
            version,
            source: ResolvedSource::Index,
        })
        .collect();
    others.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));

    let mut packages = Vec::with_capacity(others.len() + 1);
    packages.push(ResolvedPackage {
        name: input.root_name.clone(),
        version: input.root_version.clone(),
        source: ResolvedSource::Root,
    });
    packages.extend(others);
    ResolveOutput { packages }
}
