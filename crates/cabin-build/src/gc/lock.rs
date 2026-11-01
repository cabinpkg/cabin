//! The build-directory lock shared by every command that uses build
//! outputs.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use super::GcError;

/// File under the build directory that [`BuildDirLock`] locks.
pub const LOCK_FILENAME: &str = ".cabin-build.lock";

/// An OS advisory lock on `<dir>/.cabin-build.lock`: the build
/// directory's, and the catalog's, which [`super::record_use`] holds
/// while it records.
///
/// Commands that use build outputs hold the build directory's lock
/// shared for as long as they use them, including while `cabin run` and
/// `cabin test` run what they built; `cabin clean` and collection hold
/// it exclusively.  The OS releases a lock when its handle closes,
/// including when the process dies.  The build directory's file is never
/// removed, not even by `cabin clean`: a process already waiting on a
/// removed file would lock a different file than one that recreated the
/// path.  The catalog's goes with the catalog in a `cabin clean` of the
/// whole build directory, when no command holds or waits on it (see
/// [`super::record_use`]).
#[derive(Debug)]
pub struct BuildDirLock {
    file: File,
    path: PathBuf,
    /// False on a file system without file locks, where nothing is held.
    locked: bool,
}

impl BuildDirLock {
    /// Lock `build_dir` shared, creating it if needed.  `on_wait` runs
    /// once before blocking on an exclusive holder.
    ///
    /// # Errors
    /// [`GcError::Io`] when the directory or lock file cannot be
    /// created, opened, or locked.
    pub fn shared(build_dir: &Path, on_wait: impl FnOnce()) -> Result<Self, GcError> {
        Self::acquire(build_dir, false, on_wait)
    }

    /// Lock `build_dir` exclusively, creating it if needed.  `on_wait`
    /// runs once before blocking on another holder.
    ///
    /// # Errors
    /// As [`Self::shared`].
    pub fn exclusive(build_dir: &Path, on_wait: impl FnOnce()) -> Result<Self, GcError> {
        Self::acquire(build_dir, true, on_wait)
    }

    fn acquire(build_dir: &Path, exclusive: bool, on_wait: impl FnOnce()) -> Result<Self, GcError> {
        let path = build_dir.join(LOCK_FILENAME);
        let io_err = |source| GcError::Io {
            path: path.clone(),
            source,
        };
        fs::create_dir_all(build_dir).map_err(io_err)?;
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(io_err)?;
        let attempt = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        let locked = match attempt {
            Ok(()) => true,
            Err(TryLockError::WouldBlock) => {
                on_wait();
                if exclusive {
                    file.lock()
                } else {
                    file.lock_shared()
                }
                .map_err(io_err)?;
                true
            }
            // Like Cargo, build on a file system without file locks
            // (some network and virtual-machine mounts) unlocked.
            Err(TryLockError::Error(err)) if err.kind() == io::ErrorKind::Unsupported => false,
            Err(TryLockError::Error(source)) => return Err(io_err(source)),
        };
        Ok(Self { file, path, locked })
    }

    /// Run `f` with this shared lock held exclusively, or return
    /// `None` without waiting when another process holds the lock or
    /// the file system has no file locks.  The lock is shared again on
    /// return.  It is released between the two modes, so another
    /// process can take it there: `f` must not rely on anything
    /// observed under the shared lock.
    ///
    /// # Errors
    /// [`GcError::Io`] when changing the lock fails.
    pub fn try_exclusive<T>(&self, f: impl FnOnce() -> T) -> Result<Option<T>, GcError> {
        if !self.locked {
            return Ok(None);
        }
        let io_err = |source| GcError::Io {
            path: self.path.clone(),
            source,
        };
        self.file.unlock().map_err(io_err)?;
        let out = match self.file.try_lock() {
            Ok(()) => {
                let out = f();
                self.file.unlock().map_err(io_err)?;
                Some(out)
            }
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Error(source)) => {
                self.file.lock_shared().map_err(io_err)?;
                return Err(io_err(source));
            }
        };
        self.file.lock_shared().map_err(io_err)?;
        Ok(out)
    }
}
