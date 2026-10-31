//! Small filesystem helpers shared by Cabin production crates.
//!
//! The crate is intentionally narrow: it owns the atomic-write
//! boilerplate, the lexical path-safety predicates, and the
//! canonicalization and `..` resolution that multiple production
//! crates would otherwise duplicate.  Callers keep responsibility for
//! parent-directory creation, for archive- or context-specific
//! extraction policy, and for mapping the returned [`std::io::Error`]
//! onto their own domain error types so the destination path stays
//! visible in diagnostics.

pub mod path;

use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use atomic_write_file::AtomicWriteFile;

/// Atomically replace `path` with `contents`.
///
/// The new bytes are staged in a sibling temporary file and only
/// renamed onto `path` after a successful write, so an interrupted
/// run leaves any previous contents of `path` intact.  The parent
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

/// Resolve `path` to Cabin's canonical spelling: the real,
/// symlink-resolved absolute path, but *without* the Windows `\\?\`
/// verbatim prefix that [`std::fs::canonicalize`] prepends.
///
/// This is the single canonicalization boundary for the project.  Every
/// Cabin path-identity decision (workspace member resolution, source
/// discovery exclusions, build-dir/source overlap checks) routes
/// through here so one file has exactly one canonical form across
/// crates and a comparison never spuriously fails on a spelling
/// difference.
///
/// On non-Windows this is exactly [`std::fs::canonicalize`].  On Windows
/// it additionally collapses 8.3 short names (`RUNNER~1` ->
/// `runneradmin`) and strips the verbatim prefix, which MSVC's
/// front-end cannot open and which breaks naive [`PathBuf`] equality
/// against the non-verbatim paths the rest of the toolchain produces.
///
/// # Errors
/// Propagates the [`std::io::Error`] from the underlying
/// canonicalization (most commonly when `path` does not exist).
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Like [`canonicalize`], but fall back to `path` unchanged when it
/// cannot be resolved (for example, an exclusion entry that names a
/// file that does not exist).
///
/// Use this for best-effort comparisons where an unresolvable path
/// should still be usable as-is rather than aborting the operation.
pub fn canonicalize_or_input(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Spell the absolute `path` without the `..` components the OS would
/// resolve through the filesystem, so a tool that drops `..` lexically
/// (Ninja) names the same directory as the OS.  Nothing is created.
///
/// On Unix, `link/../x` is the parent of the symlink's target, not the
/// directory holding `link`.  Each component up to the last `..`
/// resolves through the filesystem while it exists; a name that does
/// not exist yet stays as spelled, and a `..` after it removes it, as
/// the OS will once it is created.  Windows resolves `..` lexically, so
/// there `path` comes back as is.
///
/// # Errors
/// A `..` after a file or a dangling symlink, which the OS refuses, or
/// any other error reading a component.
pub fn resolve_dot_dots(path: &Path) -> io::Result<PathBuf> {
    let components: Vec<_> = path.components().collect();
    let Some(last) = components
        .iter()
        .rposition(|component| *component == Component::ParentDir)
        .filter(|_| cfg!(unix))
    else {
        return Ok(path.to_path_buf());
    };
    let mut resolved = PathBuf::new();
    let mut missing = Vec::new();
    for component in &components[..=last] {
        match component {
            Component::ParentDir if missing.pop().is_some() => {}
            Component::Normal(_) if !missing.is_empty() => missing.push(component.as_os_str()),
            Component::ParentDir | Component::Normal(_) => {
                // macOS `realpath` accepts `file/..`; `stat` refuses it.
                let step = resolved.join(component);
                match std::fs::metadata(&step).and_then(|_| canonicalize(&step)) {
                    Ok(path) => resolved = path,
                    // A dangling symlink is a name the OS cannot pass through.
                    Err(err)
                        if err.kind() == io::ErrorKind::NotFound
                            && std::fs::symlink_metadata(&step).is_err() =>
                    {
                        missing.push(component.as_os_str());
                    }
                    Err(err) => return Err(err),
                }
            }
            other => resolved.push(other),
        }
    }
    resolved.extend(missing);
    resolved.extend(&components[last + 1..]);
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_fs::TempDir;

    #[cfg(unix)]
    #[test]
    fn resolve_dot_dots_follows_what_exists_without_creating_anything() {
        let tmp = TempDir::new().unwrap();
        let root = canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::os::unix::fs::symlink(root.join("a/b"), root.join("link")).unwrap();
        std::fs::write(root.join("file"), "").unwrap();
        std::os::unix::fs::symlink(root.join("gone"), root.join("dangling")).unwrap();
        let resolve = |spelling: &str| resolve_dot_dots(&root.join(spelling));
        assert_eq!(resolve("link/../build").unwrap(), root.join("a/build"));
        assert_eq!(
            resolve("link/new/../build").unwrap(),
            root.join("a/b/build")
        );
        assert_eq!(
            resolve("link/../new/../x/build").unwrap(),
            root.join("a/x/build")
        );
        assert_eq!(
            resolve("new/../link/../build").unwrap(),
            root.join("a/build")
        );
        assert_eq!(resolve("link/build").unwrap(), root.join("link/build"));
        assert!(resolve("file/../build").is_err());
        assert!(resolve("dangling/../build").is_err());
        assert!(!root.join("a/b/new").exists() && !root.join("a/new").exists());
    }
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
        // owns that policy.  A missing parent must surface as an
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
        // on files outside its own destination path.  The failure
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

    #[test]
    fn canonicalize_resolves_existing_file_without_verbatim_prefix() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, b"x").unwrap();

        let canonical = canonicalize(&path).unwrap();
        assert!(canonical.is_absolute());
        // dunce never yields the Windows `\\?\` verbatim prefix; assert
        // it on every host so the contract is checked even off-Windows.
        assert!(!canonical.to_string_lossy().starts_with(r"\\?\"));
        // The canonical form must still name the same file.
        assert_eq!(std::fs::read(&canonical).unwrap(), b"x");
    }

    #[test]
    fn canonicalize_errors_on_missing_path() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(canonicalize(&missing).is_err());
    }

    #[test]
    fn canonicalize_or_input_falls_back_to_the_given_path() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert_eq!(canonicalize_or_input(&missing), missing);
    }
}
