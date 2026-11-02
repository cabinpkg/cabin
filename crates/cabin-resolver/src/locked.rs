//! Helpers shared by the root preflight and the transitive
//! `choose_version` path when validating a lockfile-pinned
//! candidate: [`validate_locked_metadata`], the missing / yanked /
//! checksum check both call sites need to run in the same order
//! before the constraint check, and [`check_locked_pin_listed`], its
//! missing / checksum part that every mode keeping pins runs over the
//! solution.

use cabin_core::PackageName;
use cabin_index::{IndexEntry, VersionMetadata};

use crate::error::ResolveError;
use crate::input::LockedVersion;

/// Run the missing / yanked / checksum checks for a locked
/// candidate, in that order.
pub(crate) fn validate_locked_metadata(
    name: &PackageName,
    locked: &LockedVersion,
    entry: &IndexEntry,
) -> Result<(), ResolveError> {
    let meta = locked_metadata(name, locked, entry)?;
    if meta.yanked {
        return Err(ResolveError::LockedVersionYanked {
            name: name.as_str().to_owned(),
            version: locked.version.to_string(),
        });
    }
    check_locked_revision(name, locked, meta)?;
    // A checksum-free lockfile record cannot name a revision of a
    // version that has them: falling back to the current revision
    // would silently break the locked-reproduction guarantee, so
    // this path (only reachable via a hand-edited lockfile) is a
    // hard error in `Locked` mode - the only mode that runs this
    // validation.
    if locked.checksum.is_none() && meta.checksum.is_some() {
        return Err(ResolveError::LockedChecksumMissing {
            name: name.as_str().to_owned(),
            version: locked.version.to_string(),
        });
    }
    Ok(())
}

/// Refuse a pin the index no longer lists, the locked version or its
/// pinned packaging revision.  Published revisions are immutable and
/// stay listed, so such an index is not the one the lockfile was
/// resolved against, or it was tampered with; `PreferLocked` must not
/// quietly re-pin to whatever it now offers.
pub(crate) fn check_locked_pin_listed(
    name: &PackageName,
    locked: &LockedVersion,
    entry: &IndexEntry,
) -> Result<(), ResolveError> {
    check_locked_revision(name, locked, locked_metadata(name, locked, entry)?)
}

fn locked_metadata<'a>(
    name: &PackageName,
    locked: &LockedVersion,
    entry: &'a IndexEntry,
) -> Result<&'a VersionMetadata, ResolveError> {
    entry
        .versions
        .get(&locked.version)
        .ok_or_else(|| ResolveError::LockedVersionMissing {
            name: name.as_str().to_owned(),
            version: locked.version.to_string(),
        })
}

/// The lockfile checksum pins a packaging revision; a superseded
/// revision is still valid (published revisions stay fetchable), so
/// the pin must match *some* revision, not the current one.  Entries
/// for an index version without revisions (resolver-only fixtures)
/// skip the check - such a version cannot be fetched at all, so there
/// is no revision to pin.
fn check_locked_revision(
    name: &PackageName,
    locked: &LockedVersion,
    meta: &VersionMetadata,
) -> Result<(), ResolveError> {
    if let (Some(locked_ck), Some(current_ck)) = (&locked.checksum, &meta.checksum)
        && !meta
            .revisions
            .values()
            .any(|rev| &rev.checksum == locked_ck)
    {
        return Err(ResolveError::LockedChecksumMismatch {
            name: name.as_str().to_owned(),
            version: locked.version.to_string(),
            expected: locked_ck.as_str().to_owned(),
            actual: current_ck.as_str().to_owned(),
        });
    }
    Ok(())
}
