//! Helpers shared by the root preflight and the transitive
//! `choose_version` path when validating a lockfile-pinned
//! candidate.
//!
//! Two pieces live here:
//!
//! * [`validate_locked_metadata`], the missing / yanked /
//!   checksum check both call sites need to run in the same
//!   order before the constraint check;
//! * [`LockedConstraintRecorder`], the per-resolve store the
//!   transitive path consults when reporting a
//!   [`ResolveError::LockedVersionViolatesConstraint`].
//!
//! ## Recorder invariant
//!
//! The recorder is *only* constructed when the resolver is
//! running in [`ResolveMode::Locked`].  In `Locked` mode every
//! package has at most one allowed version, so `PubGrub` does
//! not backtrack and `get_dependencies` is invoked at most once
//! per package; the recorded constraints therefore describe the
//! actual solution and stay stable.
//!
//! Outside `Locked` mode `PubGrub` may try, then discard, a
//! version of a package - leaving stale entries behind.  The
//! recorder API guards against that misuse by being unreachable
//! outside the locked code path: [`Provider`](crate::provider)
//! holds it as `Option<LockedConstraintRecorder>` and populates
//! it only in `Locked` mode.

use std::cell::RefCell;
use std::collections::BTreeMap;

use cabin_core::PackageName;
use cabin_index::IndexEntry;
use semver::VersionReq;

use crate::error::{ResolveError, ResolverConstraint};
use crate::input::LockedVersion;

/// Run the missing / yanked / checksum checks for a locked
/// candidate in the order their [`ResolveError`] variants imply.
pub(crate) fn validate_locked_metadata(
    name: &PackageName,
    locked: &LockedVersion,
    entry: &IndexEntry,
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
    // The lockfile checksum pins a packaging revision; a superseded
    // revision is still valid (published revisions stay fetchable),
    // so the pin must match *some* revision, not the current one.
    // Entries for an index version without revisions (resolver-only
    // fixtures) skip both checks - such a version cannot be fetched
    // at all, so there is no revision to pin.
    match (&locked.checksum, &meta.checksum) {
        (Some(locked_ck), Some(current_ck))
            if !meta.revisions.values().any(|rev| &rev.checksum == locked_ck) =>
        {
            return Err(ResolveError::LockedChecksumMismatch {
                name: name.as_str().to_owned(),
                version: locked.version.to_string(),
                expected: locked_ck.clone(),
                actual: current_ck.clone(),
            });
        }
        // A checksum-free lockfile record cannot name a revision of a
        // version that has them: falling back to the current revision
        // would silently break the locked-reproduction guarantee, so
        // this path (only reachable via a hand-edited lockfile) is a
        // hard error in `Locked` mode - the only mode that runs this
        // validation.
        (None, Some(_)) => {
            return Err(ResolveError::LockedChecksumMissing {
                name: name.as_str().to_owned(),
                version: locked.version.to_string(),
            });
        }
        _ => {}
    }
    Ok(())
}

/// Per-package constraint records observed during `Locked`-mode
/// resolution.
///
/// See the module docs for the invariant: this type is only
/// safe to read from a `Locked`-mode code path.  Outside that
/// mode the recorder would mix constraints from backtracked
/// `PubGrub` attempts with constraints from the eventual
/// solution.
pub(crate) struct LockedConstraintRecorder {
    by_package: RefCell<BTreeMap<PackageName, Vec<ResolverConstraint>>>,
}

impl LockedConstraintRecorder {
    /// Build a recorder pre-populated with the root-dependency
    /// constraints found during preflight, so a later
    /// `LockedVersionViolatesConstraint` on a package the root
    /// also depends on cites the root requirement.
    pub(crate) fn new(seed: BTreeMap<PackageName, Vec<ResolverConstraint>>) -> Self {
        Self {
            by_package: RefCell::new(seed),
        }
    }

    /// Record that `origin` imposes `requirement` on `package`.
    pub(crate) fn record(
        &self,
        package: &PackageName,
        origin: PackageName,
        requirement: VersionReq,
    ) {
        self.by_package
            .borrow_mut()
            .entry(package.clone())
            .or_default()
            .push(ResolverConstraint {
                origin,
                requirement,
            });
    }

    /// Return the constraints recorded so far for `package`.
    pub(crate) fn snapshot(&self, package: &PackageName) -> Vec<ResolverConstraint> {
        self.by_package
            .borrow()
            .get(package)
            .cloned()
            .unwrap_or_default()
    }
}
