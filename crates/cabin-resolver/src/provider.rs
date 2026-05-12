//! Internal backtracking solver. Kept small on purpose — the local index
//! is local and bounded, so a recursive depth-first search with
//! per-package version preference is sufficient and easy to debug.
//!
//! Added lockfile preferences. The `locked` map plus a
//! [`ResolveMode`] flag drive whether the previously-resolved version
//! is just *preferred* (`PreferLocked`, `UpdatePackage`) or strictly
//! required (`Locked`).

use std::collections::BTreeMap;

use cabin_core::{PackageName, TargetPlatform};
use cabin_index::PackageIndex;

use crate::error::{ResolveError, ResolverConstraint};
use crate::input::{LockedVersion, ResolveInput, ResolveMode};
use crate::output::{ResolveOutput, ResolvedPackage, ResolvedSource};

/// Run the resolver. See [`super::resolve`] for the user-facing
/// docstring.
pub(crate) fn run(
    input: &ResolveInput,
    index: &PackageIndex,
) -> Result<ResolveOutput, ResolveError> {
    let mut state = State::new(input);
    state.solve(index, &TargetPlatform::current())?;

    let mut packages: Vec<ResolvedPackage> = Vec::with_capacity(state.selected.len() + 1);
    packages.push(ResolvedPackage {
        name: input.root_name.clone(),
        version: input.root_version.clone(),
        source: ResolvedSource::Root,
    });
    for (name, version) in &state.selected {
        packages.push(ResolvedPackage {
            name: name.clone(),
            version: version.clone(),
            source: ResolvedSource::Index,
        });
    }
    Ok(ResolveOutput { packages })
}

#[derive(Clone)]
struct State {
    /// `package -> chosen version` for already-selected packages.
    selected: BTreeMap<PackageName, semver::Version>,
    /// `package -> [(origin, requirement), ...]` of every constraint in
    /// effect right now.
    constraints: BTreeMap<PackageName, Vec<ResolverConstraint>>,
    /// Locked preferences carried in by the caller, possibly with one
    /// entry pruned for [`ResolveMode::UpdatePackage`].
    locked: BTreeMap<PackageName, LockedVersion>,
    /// How locked entries should be applied.
    mode: ResolveMode,
}

impl State {
    fn new(input: &ResolveInput) -> Self {
        let mut constraints: BTreeMap<PackageName, Vec<ResolverConstraint>> = BTreeMap::new();
        for (name, req) in &input.root_dependencies {
            constraints
                .entry(name.clone())
                .or_default()
                .push(ResolverConstraint {
                    origin: input.root_name.clone(),
                    requirement: req.clone(),
                });
        }
        let locked = match &input.mode {
            ResolveMode::UpdateAll => BTreeMap::new(),
            ResolveMode::UpdatePackage(name) => {
                let mut m = input.locked.clone();
                m.remove(name);
                m
            }
            _ => input.locked.clone(),
        };
        Self {
            selected: BTreeMap::new(),
            constraints,
            locked,
            mode: input.mode.clone(),
        }
    }

    /// Recursively resolve.
    fn solve(
        &mut self,
        index: &PackageIndex,
        platform: &TargetPlatform,
    ) -> Result<(), ResolveError> {
        let next = self
            .constraints
            .keys()
            .find(|p| !self.selected.contains_key(*p))
            .cloned();
        let Some(package) = next else {
            return Ok(());
        };

        let entry = index
            .package(&package)
            .ok_or_else(|| ResolveError::UnknownPackage(package.as_str().to_owned()))?;
        let constraints = self.constraints.get(&package).cloned().unwrap_or_default();

        let candidates = self.candidates_for(&package, entry, &constraints)?;

        let mut last_err: Option<ResolveError> = None;
        for version in candidates {
            let snapshot = self.clone();
            self.selected.insert(package.clone(), version.clone());

            let meta = entry
                .versions
                .get(&version)
                .expect("version was just enumerated from this entry");
            let mut dep_conflict: Option<ResolveError> = None;
            // Walk the documented resolve closure: normal + build
            // Dev deps stay declaration-only for the
            // ordinary commands the resolver serves; system deps
            // never enter resolution (they have a separate schema).
            let kinded = meta.dependencies.iter();
            for (dep_name, dep_entry) in kinded {
                // Skip disabled optional registry deps. The
                // resolver does not yet receive transitive
                // feature state for registry packages, so the
                // conservative default — matching
                // `cabin-workspace::patch::collect_version_requirements`
                // — is to leave optional edges out until a
                // future feature-resolution pass enables them.
                if dep_entry.optional {
                    continue;
                }
                // Skip conditional registry deps whose `cfg(...)`
                // predicate fails on the host platform — they
                // never enter resolution on this machine.
                if let Some(cond) = &dep_entry.condition
                    && !cond.evaluate(platform)
                {
                    continue;
                }
                self.constraints
                    .entry(dep_name.clone())
                    .or_default()
                    .push(ResolverConstraint {
                        origin: package.clone(),
                        requirement: dep_entry.req.clone(),
                    });
                if let Some(existing) = self.selected.get(dep_name)
                    && !dep_entry.req.matches(existing)
                {
                    dep_conflict = Some(ResolveError::Conflict {
                        package: dep_name.as_str().to_owned(),
                        detail: format!(
                            "{} requires {}, but {} {} was already selected",
                            package.as_str(),
                            dep_entry.req,
                            dep_name.as_str(),
                            existing
                        ),
                    });
                    break;
                }
            }

            if let Some(err) = dep_conflict {
                *self = snapshot;
                last_err = Some(err);
                continue;
            }

            match self.solve(index, platform) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_err = Some(err);
                    *self = snapshot;
                }
            }
        }

        Err(last_err.unwrap_or_else(|| ResolveError::NoMatchingVersion {
            package: package.as_str().to_owned(),
            constraints,
        }))
    }

    /// Compute the candidate versions to try for `package`.
    ///
    /// In `Locked` mode the candidate list is exactly `[locked.version]`,
    /// and any deviation (missing entry, missing index version, yanked
    /// index version, constraint violation, checksum mismatch) is
    /// surfaced immediately. In every other mode the list is the
    /// constraint-matching, non-yanked versions from the index, sorted
    /// newest-first, with the locked version (if any) lifted to the
    /// front.
    fn candidates_for(
        &self,
        package: &PackageName,
        entry: &cabin_index::IndexEntry,
        constraints: &[ResolverConstraint],
    ) -> Result<Vec<semver::Version>, ResolveError> {
        if matches!(self.mode, ResolveMode::Locked) {
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
            if !constraints
                .iter()
                .all(|c| c.requirement.matches(&locked.version))
            {
                return Err(ResolveError::LockedVersionViolatesConstraint {
                    name: package.as_str().to_owned(),
                    version: locked.version.to_string(),
                    constraints: constraints.to_vec(),
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
            return Ok(vec![locked.version.clone()]);
        }

        let matching: Vec<(semver::Version, bool)> = entry
            .versions
            .iter()
            .filter(|(version, _)| constraints.iter().all(|c| c.requirement.matches(version)))
            .map(|(v, m)| (v.clone(), m.yanked))
            .collect();

        if matching.is_empty() {
            return Err(ResolveError::NoMatchingVersion {
                package: package.as_str().to_owned(),
                constraints: constraints.to_vec(),
            });
        }

        let mut candidates: Vec<semver::Version> = matching
            .iter()
            .filter(|(_, yanked)| !*yanked)
            .map(|(v, _)| v.clone())
            .collect();
        if candidates.is_empty() {
            return Err(ResolveError::AllMatchingVersionsYanked(
                package.as_str().to_owned(),
            ));
        }
        candidates.sort_by(|a, b| b.cmp(a));

        // Lift the locked version (if present, non-yanked, and
        // constraint-satisfying) to the front so the search tries it
        // first.
        if let Some(locked) = self.locked.get(package)
            && let Some(idx) = candidates.iter().position(|v| v == &locked.version)
        {
            let v = candidates.remove(idx);
            candidates.insert(0, v);
        }

        Ok(candidates)
    }
}
