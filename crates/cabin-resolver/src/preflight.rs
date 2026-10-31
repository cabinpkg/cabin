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
//! * every `Locked*` variant except `LockedVersionLacksFeature`
//!   (raised by the provider) for the root's direct dependencies
//!   (via [`validate_locked_root_dependency`])
//! * `UnknownFeature` for a root feature request no in-range
//!   version declares.
//! 3. The returned constraint map seeds the locked-mode
//!    constraint recorder so a later
//!    `LockedVersionViolatesConstraint` cites the root
//!    requirement.

use std::collections::BTreeMap;

use cabin_core::{DEFAULT_FEATURE_KEY, PackageName};
use cabin_index::{IndexEntry, PackageIndex};
use semver::{Version, VersionReq};

use crate::error::{ResolveError, ResolverConstraint};
use crate::input::{LockedVersion, ResolveInput, ResolveMode};
use crate::locked::validate_locked_metadata;
use crate::provider::lookup;

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

        // A lazily loaded index records a package the registry does
        // not have as an entry without versions.
        let entry = lookup(input, index, name)?
            .filter(|entry| !entry.versions.is_empty())
            .ok_or_else(|| ResolveError::UnknownPackage(name.as_str().to_owned()))?;

        // Local packages never come from the lockfile.
        if matches!(input.mode, ResolveMode::Locked) && !input.local_packages.contains_key(name) {
            let locked_entry = locked
                .get(name)
                .ok_or_else(|| ResolveError::LockfileMissingPackage(name.as_str().to_owned()))?;
            validate_locked_root_dependency(&input.root_name, name, locked_entry, entry, req)?;
            continue;
        }

        check_root_candidates(name, entry, req, &constraints)?;
    }

    for (name, features) in &input.root_features {
        // A name with no entry at all is the solver's to report.
        let Some(entry) = lookup(input, index, name)? else {
            continue;
        };
        let req = input.root_dependencies.get(name);
        for feature in features {
            let declared = feature == DEFAULT_FEATURE_KEY
                || entry.versions.iter().any(|(version, meta)| {
                    req.is_none_or(|req| req.matches(version))
                        && meta.features.features.contains_key(feature)
                });
            if !declared {
                return Err(ResolveError::UnknownFeature {
                    package: name.as_str().to_owned(),
                    feature: feature.clone(),
                });
            }
        }
    }

    Ok(constraints)
}

/// Run every `Locked`-mode check for a single root dependency:
/// the shared metadata checks (missing / yanked / checksum) of
/// [`validate_locked_metadata`], then the constraint check.  The
/// constraint check is inline because the root path carries a
/// single [`VersionReq`] while the transitive path runs across an
/// accumulated constraint set.
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
