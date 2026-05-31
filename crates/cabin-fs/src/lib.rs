//! Small filesystem helpers shared by Cabin production crates.
//!
//! The crate is intentionally narrow: it owns the atomic-write
//! boilerplate and the lexical path-safety predicates that
//! multiple production crates would otherwise duplicate. Callers
//! keep responsibility for parent-directory creation, for archive-
//! or context-specific extraction policy, and for mapping the
//! returned [`std::io::Error`] onto their own domain error types so
//! the destination path stays visible in diagnostics.

pub mod path;

use std::io::{self, Write as _};
use std::path::Path;

use atomic_write_file::AtomicWriteFile;

/// Atomically replace `path` with `contents`.
///
/// The new bytes are staged in a sibling temporary file and only
/// renamed onto `path` after a successful write, so an interrupted
/// run leaves any previous contents of `path` intact. The parent
/// directory must already exist; this helper does not create it.
///
/// The replacement may swap a symlink at `path` for a regular file
/// rather than writing through the symlink target, and does not
/// promise to preserve timestamps, ACLs, xattrs, or ownership.
/// These limits come from the underlying `atomic-write-file` crate
/// and are not papered over here.
///
/// # Errors
/// Returns the [`std::io::Error`] from opening the staging temporary
/// file (for example when `path`'s parent directory does not exist),
/// from writing `contents`, or from the final commit/rename onto
/// `path`.
pub fn write_atomic(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    let mut file = AtomicWriteFile::open(path)?;
    file.write_all(contents.as_ref())?;
    file.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_fs::TempDir;

    #[test]
    fn write_atomic_creates_destination_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.txt");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn write_atomic_replaces_existing_destination_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.txt");
        std::fs::write(&path, b"stale").unwrap();
        write_atomic(&path, b"fresh").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
    }

    #[test]
    fn write_atomic_accepts_string_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.txt");
        write_atomic(&path, "string body").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "string body");
    }

    #[test]
    fn write_atomic_fails_when_parent_directory_missing() {
        // The helper does not create parent directories; the caller
        // owns that policy. A missing parent must surface as an
        // `io::Error` rather than silently materializing the path.
        let dir = TempDir::new().unwrap();
        let missing_parent = dir.path().join("nonexistent").join("out.txt");
        let err = write_atomic(&missing_parent, b"body").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!missing_parent.exists());
    }

    #[test]
    fn write_atomic_failure_does_not_touch_unrelated_destinations() {
        // A failing `write_atomic` call must not have side effects
        // on files outside its own destination path. The failure
        // path here is "parent directory missing"; the helper aborts
        // at `open` time, before any sibling temporary file is
        // staged, so a same-named file living elsewhere stays put.
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("kept.txt");
        std::fs::write(&dest, b"original").unwrap();
        let missing_parent = dir.path().join("nonexistent").join("kept.txt");
        let _ = write_atomic(&missing_parent, b"replacement").unwrap_err();
        assert_eq!(std::fs::read(&dest).unwrap(), b"original");
    }
}
