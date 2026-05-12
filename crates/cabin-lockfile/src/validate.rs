use std::collections::HashSet;

use crate::error::LockfileError;
use crate::model::{LOCKFILE_VERSION, Lockfile};

/// Run the structural checks that must hold for an in-memory
/// [`Lockfile`]. Called by [`crate::io::parse_lockfile_str`] before
/// returning the value to the caller; safe to call again on a manually
/// constructed lockfile.
pub fn validate(lockfile: &Lockfile) -> Result<(), LockfileError> {
    if lockfile.version != LOCKFILE_VERSION {
        return Err(LockfileError::UnsupportedVersion {
            version: lockfile.version,
            expected: LOCKFILE_VERSION,
        });
    }

    let mut seen: HashSet<&str> = HashSet::new();
    for pkg in &lockfile.packages {
        if !seen.insert(pkg.name.as_str()) {
            return Err(LockfileError::DuplicatePackage {
                name: pkg.name.as_str().to_owned(),
            });
        }
    }

    Ok(())
}
