//! Small filesystem helpers shared by Cabin production crates.
//!
//! It owns the atomic-write boilerplate, the lexical path-safety
//! predicates, and the canonicalization and `..` resolution that
//! multiple production crates would otherwise duplicate.  Callers keep
//! responsibility for parent-directory creation, for archive- or
//! context-specific extraction policy, and for mapping the returned
//! [`std::io::Error`] onto their own domain error types so the
//! destination path stays visible in diagnostics.

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
/// These limits come from the underlying `atomic-write-file` crate.
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
/// symlink-resolved absolute path, without the Windows `\\?\` verbatim
/// prefix that [`std::fs::canonicalize`] prepends wherever the path can
/// be spelled without it.
///
/// Workspace member resolution, source discovery exclusions, and
/// build-dir/source overlap checks route through here so one file has
/// one canonical form across crates and a comparison never spuriously
/// fails on a spelling difference.
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

/// Whether names in `dir` match ignoring ASCII letter case, as on
/// Windows and macOS by default, without creating anything: whether an
/// entry whose name holds a letter is found under that name with each
/// letter's case swapped.  While `dir` is missing, its nearest existing
/// ancestor answers, since a directory created there inherits that
/// one's setting.  `None` when that directory holds no such entry or
/// the lookup fails.
///
/// Case sensitivity can be a directory's own setting (Linux casefold,
/// Windows per-directory) or another volume's behind a symlink, so an
/// existing directory never takes its parent's answer.  A
/// case-sensitive directory holding exactly the swapped name too reads
/// as ignoring case.
pub fn ignores_case(dir: &Path) -> Option<bool> {
    let dir = dir.ancestors().find(|dir| dir.exists())?;
    let name = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .find(|name| name.bytes().any(|byte| byte.is_ascii_alphabetic()))?;
    let swapped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    match std::fs::symlink_metadata(dir.join(swapped)) {
        Ok(_) => Some(true),
        // A miss means case matters only while the entry still exists.
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            std::fs::symlink_metadata(dir.join(&name))
                .is_ok()
                .then_some(false)
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_fs::TempDir;

    fn mkdir(path: PathBuf) -> PathBuf {
        std::fs::create_dir(&path).unwrap();
        path
    }

    /// Whether creating `probe`'s case-swapped spelling in `dir` finds
    /// the file already there: how the file system really treats case.
    /// Leaves `dir` as it found it.
    fn creating_ignores_case(dir: &Path) -> bool {
        std::fs::write(dir.join("probe"), "").unwrap();
        let ignores = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("PROBE"))
        {
            Ok(file) => {
                drop(file);
                std::fs::remove_file(dir.join("PROBE")).unwrap();
                false
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => true,
            Err(err) => panic!("{err}"),
        };
        std::fs::remove_file(dir.join("probe")).unwrap();
        ignores
    }

    #[test]
    fn ignores_case_answers_as_creating_files_does() {
        let tmp = TempDir::new().unwrap();
        let dir = mkdir(tmp.path().join("dir"));
        let truth = creating_ignores_case(&dir);
        std::fs::write(dir.join("build.ninja"), "").unwrap();
        assert_eq!(ignores_case(&dir), Some(truth));
        // A directory not created yet takes the answer of the nearest
        // one that exists, but an existing one never does.
        assert_eq!(ignores_case(&dir.join("dev/packages")), Some(truth));
        let empty = mkdir(dir.join("empty"));
        std::fs::write(empty.join("123"), "").unwrap();
        assert_eq!(ignores_case(&empty), None);
        assert_eq!(ignores_case(&empty.join("dev")), None);
    }

    /// Windows can make one directory case-sensitive; only where that
    /// is available (it needs the Windows Subsystem for Linux feature).
    #[cfg(windows)]
    #[test]
    fn ignores_case_reads_a_windows_case_sensitive_directory() {
        let tmp = TempDir::new().unwrap();
        let dir = mkdir(tmp.path().join("sensitive"));
        let enabled = std::process::Command::new("fsutil.exe")
            .args(["file", "setCaseSensitiveInfo"])
            .arg(&dir)
            .arg("enable")
            .output()
            .is_ok_and(|out| out.status.success());
        if !enabled {
            eprintln!("skipped: this machine cannot make a directory case-sensitive");
            return;
        }
        assert!(!creating_ignores_case(&dir));
        std::fs::write(dir.join("build.ninja"), "").unwrap();
        assert_eq!(ignores_case(&dir), Some(false));
        let parent = creating_ignores_case(tmp.path());
        assert_eq!(ignores_case(tmp.path()), Some(parent));
    }

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
        let dir = TempDir::new().unwrap();
        let missing_parent = dir.path().join("nonexistent").join("out.txt");
        let err = write_atomic(&missing_parent, b"body").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!missing_parent.exists());
    }

    #[test]
    fn write_atomic_failure_does_not_touch_unrelated_destinations() {
        // The missing parent fails `open` before any sibling temporary
        // file is staged.
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
        assert!(!canonical.to_string_lossy().starts_with(r"\\?\"));
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
