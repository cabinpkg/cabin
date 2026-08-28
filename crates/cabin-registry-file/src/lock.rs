use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

use crate::error::RegistryError;

/// Filename used by [`RegistryLock`].
pub const LOCK_FILENAME: &str = ".cabin-registry.lock";

/// Registry lock backed by an OS advisory lock on
/// `<registry>/.cabin-registry.lock`.
///
/// The OS releases the lock when the holding file handle closes -
/// including when the process crashes or is killed - so a previous
/// run can never leave the registry permanently locked.  The lock
/// file itself stays on disk after release: unlinking it would race
/// a concurrent acquirer that already opened the same path.
///
/// Pre-1.0 protocol change: earlier Cabin releases signaled
/// ownership by the file's mere existence and took no OS lock, so
/// this version does not observe a publish running under one of
/// them (the reverse direction is safe - an old publisher treats
/// the persistent file as held).  Running two different Cabin
/// versions against one registry concurrently is not a supported
/// configuration.
#[derive(Debug)]
pub struct RegistryLock {
    _file: File,
}

impl RegistryLock {
    /// Acquire the registry lock without blocking by taking an
    /// exclusive OS lock on `<registry>/.cabin-registry.lock`.
    ///
    /// # Errors
    /// Returns [`RegistryError::Locked`] when another process (or
    /// another handle in this process) holds the lock, and
    /// [`RegistryError::Io`] when creating the registry root or
    /// opening / locking the lock file fails for any other reason.
    pub fn acquire(registry_root: &Path) -> Result<Self, RegistryError> {
        fs::create_dir_all(registry_root).map_err(|source| RegistryError::Io {
            path: registry_root.to_path_buf(),
            source,
        })?;
        let path = registry_root.join(LOCK_FILENAME);
        // `create(true)` follows a planted symlink, and opening a
        // FIFO blocks until a peer appears, so refuse any pre-existing
        // entry that is not a regular file.  The check is not atomic,
        // but anyone who can race it can already rewrite the registry
        // itself; it keeps leftovers and planted objects from wedging
        // or redirecting the open.
        if let Ok(meta) = fs::symlink_metadata(&path)
            && !meta.is_file()
        {
            return Err(RegistryError::Io {
                path,
                source: io::Error::other("lock path exists but is not a regular file"),
            });
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| RegistryError::Io {
                path: path.clone(),
                source,
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(RegistryError::Locked),
            Err(TryLockError::Error(source)) => Err(RegistryError::Io { path, source }),
        }
    }

    /// Release the lock immediately.  Dropping the guard has the
    /// same effect; exposed so callers can release on a deliberate
    /// success path before any later code runs.
    pub fn release(self) {}
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
        let _lock = RegistryLock::acquire(dir.path()).unwrap();
        dir.child(LOCK_FILENAME).assert(predicate::path::is_file());
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
    fn stale_lock_file_does_not_block_acquire() {
        // A leftover file from a crashed run holds no OS lock, so a
        // fresh acquire must succeed instead of demanding manual
        // cleanup.
        let dir = TempDir::new().unwrap();
        dir.child(LOCK_FILENAME).write_binary(b"").unwrap();
        let _lock = RegistryLock::acquire(dir.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_at_lock_path_is_refused() {
        // A planted symlink must fail cleanly instead of locking (or
        // hanging on) whatever it points at.
        let dir = TempDir::new().unwrap();
        let target = dir.child("target");
        target.write_binary(b"").unwrap();
        std::os::unix::fs::symlink(target.path(), dir.path().join(LOCK_FILENAME)).unwrap();
        let err = RegistryLock::acquire(dir.path()).unwrap_err();
        assert!(matches!(err, RegistryError::Io { .. }), "{err:?}");
    }

    #[test]
    fn explicit_release_unlocks() {
        let dir = TempDir::new().unwrap();
        let lock = RegistryLock::acquire(dir.path()).unwrap();
        lock.release();
        // The file must survive release: unlinking it would race a
        // concurrent acquirer that already opened the same path.
        dir.child(LOCK_FILENAME).assert(predicate::path::is_file());
        let _again = RegistryLock::acquire(dir.path()).unwrap();
    }
}
