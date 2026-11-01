//! Which workspace a build directory's outputs belong to.
//!
//! Two workspaces building into one directory overwrite each other's
//! outputs (both may have a package `app`), and a `cabin clean` or
//! collection run from one removes the other's.  So the first command
//! that builds into a directory records its workspace there
//! ([`OWNER_FILENAME`]), and every command that writes or removes
//! outputs checks the record first.
//!
//! The record names the workspace root by the `..` steps up to it when
//! the build directory lies inside the workspace, so moving the
//! workspace with its build directory keeps the record true, and by
//! its absolute path otherwise.  Both directories are compared
//! canonicalized, so symlinked or `..`-laden spellings of either name
//! one owner.  A record is written only under [`BuildDirLock`] held
//! exclusively, into a directory holding no subdirectory, and is never
//! removed by Cabin, not even by `cabin clean`: a build directory
//! changes hands only when someone removes it.
//!
//! The record names an owner, not the files: a directory may hold
//! regular files of its own when it is claimed, which is why
//! `cabin clean` never removes a regular file directly in it (see
//! [`crate::clean`]).  A directory with no record that holds a
//! subdirectory may be an older Cabin's build directory or no build
//! directory at all, and nothing in it tells which, so no command
//! builds into it or removes anything from it.
//!
//! [`BuildDirLock`]: crate::gc::BuildDirLock

use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File under the build directory naming the workspace that owns it.
pub const OWNER_FILENAME: &str = ".cabin-workspace";

const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    /// The workspace root as [`encode`] spells it.
    workspace: String,
}

/// Who [`owner`] found owns a build directory this workspace may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// The record names this workspace.
    This,
    /// No record, and no subdirectory in the build directory, or no
    /// build directory: [`claim`] records this workspace.
    Nobody,
}

#[derive(Debug, thiserror::Error)]
pub enum OwnerError {
    #[error(
        "build directory {} holds the outputs of the workspace at {}, not this one at {}; give \
         this workspace its own build directory with --build-dir, CABIN_BUILD_DIR, or `[paths] \
         build-dir`, or remove {} to hand it over",
        build_dir.display(),
        owner.display(),
        workspace.display(),
        build_dir.display()
    )]
    Foreign {
        build_dir: PathBuf,
        owner: PathBuf,
        workspace: PathBuf,
    },

    #[error(
        "build directory {} holds subdirectories but no record of the workspace they belong \
         to, so Cabin neither builds into it nor removes anything from it; if it holds only the \
         outputs of a Cabin release from before the record, remove {} yourself, otherwise give \
         this workspace its own build directory with --build-dir, CABIN_BUILD_DIR, or `[paths] \
         build-dir`",
        build_dir.display(),
        build_dir.display()
    )]
    Unrecorded { build_dir: PathBuf },

    #[error(
        "cannot read {}, the record of the workspace that owns build directory {} (a newer \
         Cabin may have written it); remove the build directory to start over",
        path.display(),
        build_dir.display()
    )]
    Unreadable { path: PathBuf, build_dir: PathBuf },

    #[error("failed to access {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Who owns `build_dir`, when the workspace at `workspace_root` may use
/// it.
///
/// # Errors
/// [`OwnerError::Foreign`] when the record names another workspace,
/// [`OwnerError::Unreadable`] when it cannot be parsed,
/// [`OwnerError::Unrecorded`] when there is none but the build
/// directory holds a subdirectory, and [`OwnerError::Io`] when reading
/// fails.
pub fn owner(build_dir: &Path, workspace_root: &Path) -> Result<Owner, OwnerError> {
    let path = build_dir.join(OWNER_FILENAME);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if holds_outputs(build_dir)? {
                return Err(OwnerError::Unrecorded {
                    build_dir: build_dir.to_path_buf(),
                });
            }
            return Ok(Owner::Nobody);
        }
        Err(source) => return Err(OwnerError::Io { path, source }),
    };
    let build_dir = canonical(build_dir)?;
    let workspace = canonical(workspace_root)?;
    let recorded = serde_json::from_slice::<Record>(&bytes)
        .ok()
        .filter(|record| record.version == FORMAT_VERSION)
        .and_then(|record| Some((resolve(&build_dir, &record.workspace)?, record.workspace)));
    let Some((owner, spelled)) = recorded else {
        return Err(OwnerError::Unreadable { path, build_dir });
    };
    if encode(&build_dir, &workspace).as_deref() == Some(spelled.as_str()) {
        Ok(Owner::This)
    } else {
        Err(OwnerError::Foreign {
            build_dir,
            owner,
            workspace,
        })
    }
}

/// Record the workspace at `workspace_root` as `build_dir`'s owner,
/// unless one is recorded.  The caller holds the build directory's
/// [`crate::gc::BuildDirLock`] exclusively, so two first commands in
/// one directory cannot both claim it.
///
/// # Errors
/// As [`owner`].
pub fn claim(build_dir: &Path, workspace_root: &Path) -> Result<(), OwnerError> {
    match owner(build_dir, workspace_root)? {
        Owner::This => Ok(()),
        Owner::Nobody => {
            let workspace = encode(&canonical(build_dir)?, &canonical(workspace_root)?);
            let path = build_dir.join(OWNER_FILENAME);
            workspace
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("workspace path {} is not UTF-8", workspace_root.display()),
                    )
                })
                .and_then(|workspace| {
                    let record = Record {
                        version: FORMAT_VERSION,
                        workspace,
                    };
                    Ok(serde_json::to_vec(&record)?)
                })
                .and_then(|bytes| cabin_fs::write_atomic(&path, bytes))
                .map_err(|source| OwnerError::Io { path, source })
        }
    }
}

/// Whether `build_dir` holds anything but regular files.  Cabin writes
/// every output under a subdirectory, `.cabin-gc` included, while a
/// regular file there, such as the lock file, a committed `.gitignore`,
/// or a record a crash left half written, is never an output.
fn holds_outputs(build_dir: &Path) -> Result<bool, OwnerError> {
    let io_err = |source| OwnerError::Io {
        path: build_dir.to_path_buf(),
        source,
    };
    let entries = match std::fs::read_dir(build_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(io_err(source)),
    };
    for entry in entries {
        if !entry
            .and_then(|entry| entry.file_type())
            .map_err(io_err)?
            .is_file()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn canonical(path: &Path) -> Result<PathBuf, OwnerError> {
    cabin_fs::canonicalize(path).map_err(|source| OwnerError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// `workspace` as seen from `build_dir`, both canonical: `.` or `..`
/// steps when it holds the build directory, else its absolute path;
/// `None` for a path that is not UTF-8, which a lossy spelling would
/// confuse with another workspace's.
fn encode(build_dir: &Path, workspace: &Path) -> Option<String> {
    match build_dir.strip_prefix(workspace) {
        Ok(inside) => Some(match inside.components().count() {
            0 => ".".to_owned(),
            depth => vec![".."; depth].join("/"),
        }),
        Err(_) => workspace.to_str().map(str::to_owned),
    }
}

/// The workspace root a record spells, from the canonical `build_dir`;
/// `None` for a spelling [`encode`] never writes.
fn resolve(build_dir: &Path, workspace: &str) -> Option<PathBuf> {
    let path = Path::new(workspace);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    let mut depth = 0;
    for component in path.components() {
        match component {
            Component::ParentDir => depth += 1,
            Component::CurDir => {}
            _ => return None,
        }
    }
    build_dir.ancestors().nth(depth).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_fs::TempDir;

    fn mkdir(path: PathBuf) -> PathBuf {
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn claimed_build_dir_serves_only_its_workspace() {
        let tmp = TempDir::new().unwrap();
        let ws = mkdir(tmp.path().join("ws"));
        let other = mkdir(tmp.path().join("other"));
        let build = mkdir(ws.join("build"));
        assert_eq!(owner(&build, &ws).unwrap(), Owner::Nobody);
        claim(&build, &ws).unwrap();
        assert_eq!(owner(&build, &ws).unwrap(), Owner::This);
        claim(&build, &ws).unwrap();
        assert!(matches!(
            owner(&build, &other),
            Err(OwnerError::Foreign { .. })
        ));
        assert!(matches!(
            claim(&build, &other),
            Err(OwnerError::Foreign { .. })
        ));
    }

    #[test]
    fn a_workspace_moved_with_its_build_dir_keeps_it() {
        let tmp = TempDir::new().unwrap();
        let ws = mkdir(tmp.path().join("ws"));
        let inside = mkdir(ws.join("out/build"));
        let outside = mkdir(tmp.path().join("shared"));
        claim(&inside, &ws).unwrap();
        claim(&outside, &ws).unwrap();
        let moved = tmp.path().join("moved");
        std::fs::rename(&ws, &moved).unwrap();
        assert_eq!(
            owner(&moved.join("out/build"), &moved).unwrap(),
            Owner::This
        );
        // A build directory outside the workspace still names where
        // the workspace was.
        assert!(matches!(
            owner(&outside, &moved),
            Err(OwnerError::Foreign { owner, .. }) if owner == canonical(tmp.path()).unwrap().join("ws")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_spellings_name_one_owner() {
        let tmp = TempDir::new().unwrap();
        let ws = mkdir(tmp.path().join("ws"));
        let build = mkdir(tmp.path().join("build"));
        std::os::unix::fs::symlink(&ws, tmp.path().join("ws-link")).unwrap();
        std::os::unix::fs::symlink(&build, tmp.path().join("build-link")).unwrap();
        claim(&tmp.path().join("build-link"), &tmp.path().join("ws-link")).unwrap();
        assert_eq!(owner(&build, &ws).unwrap(), Owner::This);
        assert_eq!(owner(&ws.join("../build-link"), &ws).unwrap(), Owner::This);
    }

    #[cfg(unix)]
    #[test]
    fn a_workspace_path_that_is_not_utf8_names_no_owner() {
        use std::os::unix::ffi::OsStrExt;
        let path = |bytes: &'static [u8]| Path::new(std::ffi::OsStr::from_bytes(bytes));
        // Lossily, `/ws\u{fffd}` would read as this workspace.
        assert_eq!(encode(Path::new("/build"), path(b"/ws\xff")), None);
        assert_eq!(
            encode(path(b"/ws\xff/build"), path(b"/ws\xff")),
            Some("..".to_owned())
        );
    }

    #[test]
    fn output_trees_without_a_record_are_never_claimed() {
        let tmp = TempDir::new().unwrap();
        let ws = mkdir(tmp.path().join("ws"));
        let build = mkdir(tmp.path().join("build"));
        std::fs::write(build.join(crate::gc::LOCK_FILENAME), "").unwrap();
        std::fs::write(build.join(".gitignore"), "*\n").unwrap();
        assert_eq!(owner(&build, &ws).unwrap(), Owner::Nobody);
        std::fs::create_dir(build.join("dev")).unwrap();
        assert!(matches!(
            owner(&build, &ws),
            Err(OwnerError::Unrecorded { .. })
        ));
        assert!(matches!(
            claim(&build, &ws),
            Err(OwnerError::Unrecorded { .. })
        ));
        assert!(!build.join(OWNER_FILENAME).exists());
    }

    #[test]
    fn an_unreadable_record_refuses() {
        let tmp = TempDir::new().unwrap();
        let ws = mkdir(tmp.path().join("ws"));
        let build = mkdir(ws.join("build"));
        for record in [
            "not json",
            r#"{"version":2,"workspace":".."}"#,
            r#"{"version":1,"workspace":"../ws"}"#,
        ] {
            std::fs::write(build.join(OWNER_FILENAME), record).unwrap();
            assert!(
                matches!(owner(&build, &ws), Err(OwnerError::Unreadable { .. })),
                "{record}"
            );
        }
    }
}
