use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::error::RegistryError;

/// Filename used by [`RegistryLock`].
pub const LOCK_FILENAME: &str = ".cabin-registry.lock";

/// Best-effort registry lock backed by an `O_CREAT | O_EXCL` file in
/// the registry root.
///
/// Does not try to handle process crashes perfectly: if a
/// previous run was killed mid-publish, the user may have to remove
/// the lock file manually. The Drop impl removes the file on every
/// normal completion path (success or failure), which covers the
/// common cases.
#[derive(Debug)]
pub struct RegistryLock {
    path: PathBuf,
    held: bool,
}

impl RegistryLock {
    /// Acquire the registry lock by creating
    /// `<registry>/.cabin-registry.lock` with `create_new` semantics.
    ///
    /// # Errors
    /// Returns [`RegistryError::Locked`] when the lock file already
    /// exists (another process holds it), and [`RegistryError::Io`]
    /// when creating the registry root or opening the lock file fails
    /// for any other reason.
    pub fn acquire(registry_root: &Path) -> Result<Self, RegistryError> {
        fs::create_dir_all(registry_root).map_err(|source| RegistryError::Io {
            path: registry_root.to_path_buf(),
            source,
        })?;
        let path = registry_root.join(LOCK_FILENAME);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => Ok(Self { path, held: true }),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(RegistryError::Locked),
            Err(source) => Err(RegistryError::Io { path, source }),
        }
    }

    /// Release the lock immediately. Called automatically from
    /// [`Drop`]; exposed so callers can release on a deliberate
    /// success path before any later code runs.
    pub fn release(mut self) {
        if self.held {
            let _ = fs::remove_file(&self.path);
            self.held = false;
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        if self.held {
            let _ = fs::remove_file(&self.path);
            self.held = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use predicates::prelude::*;

    #[test]
    fn acquire_creates_lock_file() {
        let dir = TempDir::new().unwrap();
        let lock = RegistryLock::acquire(dir.path()).unwrap();
        dir.child(LOCK_FILENAME).assert(predicate::path::is_file());
        drop(lock);
        dir.child(LOCK_FILENAME).assert(predicate::path::missing());
    }

    #[test]
    fn second_acquire_fails_until_release() {
        let dir = TempDir::new().unwrap();
        let lock = RegistryLock::acquire(dir.path()).unwrap();
        let err = RegistryLock::acquire(dir.path()).unwrap_err();
        assert!(matches!(err, RegistryError::Locked));
        drop(lock);
        // After release, a fresh acquire works.
        let _again = RegistryLock::acquire(dir.path()).unwrap();
    }

    #[test]
    fn explicit_release_removes_file() {
        let dir = TempDir::new().unwrap();
        let lock = RegistryLock::acquire(dir.path()).unwrap();
        lock.release();
        dir.child(LOCK_FILENAME).assert(predicate::path::missing());
    }
}
