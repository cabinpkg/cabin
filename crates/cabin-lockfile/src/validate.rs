use std::collections::HashSet;

use crate::error::LockfileError;
use crate::model::{LOCKFILE_VERSION, Lockfile};

/// Run the structural checks that must hold for an in-memory
/// [`Lockfile`].  [`crate::io::parse_lockfile_str`] and
/// [`crate::io::render_lockfile`] both call it, so a lockfile that
/// fails it is neither returned by the parser nor written.
///
/// # Errors
/// Returns [`LockfileError::UnsupportedVersion`] when `lockfile.version`
/// differs from [`LOCKFILE_VERSION`], and
/// [`LockfileError::DuplicatePackage`] when two package entries share the
/// same name.
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
