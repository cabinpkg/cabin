//! Root-dependency checks Cabin runs before invoking `PubGrub`.
//!
//! Three things happen here, in this order:
//!
//! 1. [`effective_locked`] applies the [`ResolveMode`] visibility
//!    rules to the caller-supplied lockfile map.
//! 2. [`preflight_root_dependencies`] walks the root's direct
//!    dependencies and emits the targeted [`ResolveError`]
//!    variants whose messages are sharper than a generic
//!    `PubGrub` derivation tree:
//! * `UnknownPackage`
//! * `NoMatchingVersion`
//! * `AllMatchingVersionsYanked`
//! * `LockfileMissingPackage`
//! * every `Locked*` variant for the root's direct
//!   dependencies (via [`validate_locked_root_dependency`]).
//! 3. The returned constraint map seeds the locked-mode
//!    constraint recorder so a later
//!    `LockedVersionViolatesConstraint` cites the root
//!    requirement that the parent originally imposed.

use std::collections::BTreeMap;

use cabin_core::PackageName;
use cabin_index::{IndexEntry, PackageIndex};
use semver::{Version, VersionReq};

use crate::error::{ResolveError, ResolverConstraint};
use crate::input::{LockedVersion, ResolveInput, ResolveMode};
use crate::locked::validate_locked_metadata;

/// Apply the [`ResolveMode`] visibility rules to the
/// caller-supplied locked map.  `UpdateAll` clears it; an
/// `UpdatePackage(name)` request drops only `name`.  The returned
/// map is the one the resolver sees.
pub(crate) fn effective_locked(input: &ResolveInput) -> BTreeMap<PackageName, LockedVersion> {
    match &input.mode {
        ResolveMode::UpdateAll => BTreeMap::new(),
        ResolveMode::UpdatePackage(name) => {
            let mut map = input.locked.clone();
            map.remove(name);
            map
        }
        ResolveMode::PreferLocked | ResolveMode::Locked => input.locked.clone(),
    }
}

/// Validate root dependencies before invoking `PubGrub`.
///
/// Returns the per-package constraint records that seed the
/// locked-mode constraint recorder, so a later
/// `LockedVersionViolatesConstraint` on a package the root also
/// depends on names the root requirement that imposed it.
pub(crate) fn preflight_root_dependencies(
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
            validate_locked_root_dependency(&input.root_name, name, locked_entry, entry, req)?;
            continue;
        }

        check_root_candidates(name, entry, req, &constraints)?;
    }

    Ok(constraints)
}

/// Run every `Locked`-mode check for a single root dependency in
/// the order the [`ResolveError`] variants imply.  Shared
/// metadata checks (missing / yanked / checksum) live in
/// [`validate_locked_metadata`]; the constraint check is inline
/// because the root path carries a single [`VersionReq`] while
/// the transitive path runs across an accumulated constraint
/// set.
fn validate_locked_root_dependency(
    root_name: &PackageName,
    name: &PackageName,
    locked: &LockedVersion,
    entry: &IndexEntry,
    req: &VersionReq,
) -> Result<(), ResolveError> {
    validate_locked_metadata(name, locked, entry)?;
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

/// Confirm `req` matches at least one non-yanked version of
/// `entry`.  Reports the precise variant the user can act on:
/// `NoMatchingVersion` if every version is out of range,
/// `AllMatchingVersionsYanked` if every matching version is
/// yanked.
fn check_root_candidates(
    name: &PackageName,
    entry: &IndexEntry,
    req: &VersionReq,
    constraints: &BTreeMap<PackageName, Vec<ResolverConstraint>>,
) -> Result<(), ResolveError> {
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
    Ok(())
}
