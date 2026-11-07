//! Workspace root discovery.
//!
//! `cabin` commands are usually run from inside a workspace.  When
//! `--manifest-path` is absent, the CLI walks upward from the current
//! directory looking for a `cabin.toml` that contains a `[workspace]`
//! table.  When one is found, commands behave as if they had been
//! invoked against that root manifest; otherwise they use
//! `./cabin.toml`.
//!
//! This module is filesystem-only - it never touches the network.  The
//! caller decides whether to honor the discovered workspace or fall
//! back to a directly-supplied manifest.

use std::path::{Path, PathBuf};

use cabin_manifest::{ParsedManifest, WorkspaceTable};

use crate::error::WorkspaceError;
use crate::loader::{canonicalize, expand_workspace_members};

/// Result of [`discover_workspace_root`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredManifest {
    /// Absolute path to the workspace's `cabin.toml`.
    pub manifest_path: PathBuf,
    /// Directory containing `manifest_path`.
    pub workspace_dir: PathBuf,
}

/// Walk upward from `start` looking for a `cabin.toml` whose root
/// `[workspace]` table is present.  Returns `Ok(Some(_))` when a
/// single workspace root is found and honored, `Ok(None)` when the
/// walk reaches the filesystem root without finding one (or the only
/// root `exclude`s the package the walk started in), and a
/// [`WorkspaceError::NestedWorkspaceDiscovery`] error when the
/// walk finds two or more `[workspace]`-bearing manifests
/// stacked above `start`.  Manifest parse errors are surfaced as
/// [`WorkspaceError::Manifest`] so the user sees the bad file.
///
/// Stacked roots force the user to disambiguate (typically with
/// `--manifest-path`) so a command run inside a nested workspace never
/// silently operates on the outer one, or vice versa.
///
/// A root found above `start` is the trust boundary for every setting
/// discovery applies on the user's behalf (`[toolchain]`, `[patch]`,
/// the root's `.cabin/config.toml`).  Unless `start` is the root's own
/// directory, the root is honored only when it passes
/// `root_trusted` against the nearest package manifest (or `start` when
/// no package manifest lies between them) and, when such a package
/// exists, lists it as a member (`root_claims_package`).  Otherwise an
/// ancestor another user can write to - `/tmp`, a shared mount - could
/// plant a root whose toolchain runs for every package below it.
///
/// Discovery stays a pure filesystem walk: no network, no
/// symlink follow beyond what the OS already does for
/// `Path::is_file`, and a stop at the filesystem root.
///
/// # Errors
/// Returns [`WorkspaceError::NestedWorkspaceDiscovery`] when two or
/// more `[workspace]`-bearing manifests are stacked above `start`,
/// [`WorkspaceError::UntrustedWorkspaceRoot`] when the only root fails
/// the owner, mode, link-count, or symlink check,
/// [`WorkspaceError::UnclaimedPackage`] when it does not list the
/// nearest package as a member, and propagates [`WorkspaceError::Manifest`] from
/// `parse_manifest` when an encountered `cabin.toml` fails to parse.
///
/// # Panics
/// Panics only if the slice-pattern invariant were violated: the
/// `.unwrap()`/`.first()`/`.last()` calls run inside match arms that
/// have already proved `found` holds exactly one element (the
/// `[_only]` arm) or two-or-more elements (the `_` arm), so the
/// `Option`s are always `Some`, and `nearest` is set whenever `found`
/// is non-empty.
#[tracing::instrument(level = "debug", skip_all)]
pub fn discover_workspace_root(start: &Path) -> Result<Option<DiscoveredManifest>, WorkspaceError> {
    let mut current = start.to_path_buf();
    if current.is_relative()
        && let Ok(abs) = std::env::current_dir().map(|cwd| cwd.join(&current))
    {
        current = abs;
    }
    let start = current.clone();
    let mut found: Vec<(DiscoveredManifest, WorkspaceTable)> = Vec::new();
    // The manifest nearest to `start`, root or not: the package the
    // invocation belongs to when it is not a root itself.
    let mut nearest: Option<PathBuf> = None;
    loop {
        let candidate = current.join("cabin.toml");
        if candidate.is_file() {
            let parsed = parse_manifest(&candidate)?;
            nearest.get_or_insert_with(|| candidate.clone());
            if let Some(workspace) = parsed.workspace {
                found.push((
                    DiscoveredManifest {
                        manifest_path: candidate,
                        workspace_dir: current.clone(),
                    },
                    workspace,
                ));
            }
        }
        match current.parent() {
            Some(parent) if parent != current => {
                current = parent.to_path_buf();
            }
            _ => break,
        }
    }
    match found.as_slice() {
        [] => Ok(None),
        [_only] => {
            let (root, workspace) = found.into_iter().next().unwrap();
            let nearest = nearest.unwrap();
            if root.workspace_dir == start {
                return Ok(Some(root));
            }
            let nearest_is_root = nearest == root.manifest_path;
            // The package the invocation belongs to, or the start
            // directory itself when no package manifest lies between
            // it and the root.
            let anchor = if nearest_is_root { &start } else { &nearest };
            if !root_trusted(&root.manifest_path, anchor)? {
                return Err(WorkspaceError::UntrustedWorkspaceRoot {
                    path: anchor.clone(),
                    root: root.manifest_path,
                });
            }
            if nearest_is_root || root_claims_package(&root, &workspace, &nearest)? {
                Ok(Some(root))
            } else {
                Ok(None)
            }
        }
        _ => {
            // Refuse to pick among stacked roots.  `found` is
            // ordered nearest-first because the walk goes upward from
            // `start`.
            let nearest = &found.first().unwrap().0;
            let outer = &found.last().unwrap().0;
            Err(WorkspaceError::NestedWorkspaceDiscovery {
                nearest: nearest.manifest_path.clone(),
                outer: outer.manifest_path.clone(),
            })
        }
    }
}

/// Whether `root`, found above the package manifest `package`, claims
/// it: `Ok(true)` when the root lists the package as a member,
/// `Ok(false)` when the root `exclude`s it (the package builds
/// standalone, as if no root were above it).  A package the root
/// neither lists nor excludes is an error - the user adds it,
/// excludes it, or picks a manifest explicitly.
fn root_claims_package(
    root: &DiscoveredManifest,
    workspace: &WorkspaceTable,
    package: &Path,
) -> Result<bool, WorkspaceError> {
    let members =
        expand_workspace_members(&root.workspace_dir, &workspace.members, &workspace.exclude)?;
    let canonical_package = canonicalize(package)?;
    if members.included.contains(&canonical_package) {
        return Ok(true);
    }
    let root_dir = canonicalize(&root.workspace_dir)?;
    let excluded = members
        .excluded
        .iter()
        .filter_map(|rel| canonicalize(&root_dir.join(rel).join("cabin.toml")).ok())
        .any(|manifest| manifest == canonical_package);
    if excluded {
        return Ok(false);
    }
    Err(WorkspaceError::UnclaimedPackage {
        package: package.to_path_buf(),
        root: root.manifest_path.clone(),
    })
}

/// Whether the root manifest `root_manifest` may be trusted on behalf
/// of `anchor` (the nearest package manifest, or the start directory):
/// the manifest itself - not a symlink target, which another user could
/// point at a file the anchor's owner does own - and its directory have
/// the anchor's owner, the manifest has a single link (a hard link is
/// the same borrowing trick on hosts that allow linking to another
/// user's file), and neither is world-writable, so no other user could
/// have planted, edited or swapped the entry (`/tmp` and shared drop
/// directories fail this; group-writable team checkouts pass).  The root's `.cabin/config.toml`
/// is read for every package below the root too, so it and its directory
/// pass the same checks when present.  None of the four may be a symlink:
/// the link's own metadata says nothing about who controls the target
/// (a link into a shared directory would pass every check above while
/// another user swaps the file it resolves to).
#[cfg(unix)]
fn root_trusted(root_manifest: &Path, anchor: &Path) -> Result<bool, WorkspaceError> {
    use std::fs::Metadata;
    use std::os::unix::fs::MetadataExt;
    let lstat = |path: &Path| match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(WorkspaceError::Io {
            path: path.to_path_buf(),
            source,
        }),
    };
    let Some(anchor) = lstat(anchor)? else {
        return Ok(false);
    };
    let private = |meta: &Metadata, file: bool| {
        !meta.file_type().is_symlink()
            && meta.uid() == anchor.uid()
            && meta.mode() & 0o002 == 0
            && (!file || meta.nlink() == 1)
    };
    let root_dir = root_manifest.parent().unwrap_or(root_manifest);
    let config_dir = root_dir.join(".cabin");
    let config = config_dir.join("config.toml");
    // The manifest and its directory were just read, so their absence
    // now means the entry changed underneath us: refuse rather than
    // skip.  The config file and its directory are optional.
    let mut trusted = true;
    for (path, file, required) in [
        (root_manifest, true, true),
        (root_dir, false, true),
        (config_dir.as_path(), false, false),
        (config.as_path(), true, false),
    ] {
        trusted &= match lstat(path)? {
            Some(meta) => private(&meta, file),
            None => !required,
        };
    }
    Ok(trusted)
}

/// The Windows form of the Unix check, the comparison Git's
/// `safe.directory` makes: the root manifest, its directory, and the
/// root's `.cabin/` and `.cabin/config.toml` when present must have the
/// anchor's owner SID and must not be symlinks or junctions.  A non-admin
/// can only make itself the owner of what it creates, so this refuses a
/// root another user planted in a directory everyone can create files in.
/// There is no mode-bit or link-count analogue: a file the anchor's owner
/// owns but whose DACL lets other users write it is still trusted.
#[cfg(windows)]
fn root_trusted(root_manifest: &Path, anchor: &Path) -> Result<bool, WorkspaceError> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{GetLengthSid, OWNER_SECURITY_INFORMATION};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, READ_CONTROL,
    };
    // Whether `path` itself (never a reparse target) is a symlink or
    // junction, and its owner SID's bytes, empty when it has none.
    // One handle answers both, so the entry cannot be swapped in between.
    let lstat = |path: &Path| {
        let io = |source| WorkspaceError::Io {
            path: path.to_path_buf(),
            source,
        };
        let file = match std::fs::OpenOptions::new()
            .access_mode(READ_CONTROL)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
        {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(io(source)),
        };
        let symlink = file.metadata().map_err(io)?.file_type().is_symlink();
        let mut owner = null_mut();
        let mut descriptor = null_mut();
        // SAFETY: the handle is open with READ_CONTROL, which reading the
        // owner needs.  On success `owner` (when non-null) points into
        // `descriptor`, which the call allocated; the SID is copied out
        // before `LocalFree` releases it.
        let sid = unsafe {
            let status = GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &raw mut owner,
                null_mut(),
                null_mut(),
                null_mut(),
                &raw mut descriptor,
            );
            if status != ERROR_SUCCESS {
                return Err(io(std::io::Error::from_raw_os_error(status.cast_signed())));
            }
            let sid = if owner.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(owner.cast::<u8>(), GetLengthSid(owner) as usize)
                    .to_vec()
            };
            LocalFree(descriptor);
            sid
        };
        Ok(Some((symlink, sid)))
    };
    let Some((_, anchor_owner)) = lstat(anchor)? else {
        return Ok(false);
    };
    let root_dir = root_manifest.parent().unwrap_or(root_manifest);
    let config_dir = root_dir.join(".cabin");
    let config = config_dir.join("config.toml");
    // As on Unix, the manifest and its directory are required; the
    // config file and its directory are optional.
    let mut entries = Vec::new();
    for (path, required) in [
        (root_manifest, true),
        (root_dir, true),
        (config_dir.as_path(), false),
        (config.as_path(), false),
    ] {
        entries.push((lstat(path)?, required));
    }
    Ok(owners_trusted(&anchor_owner, &entries))
}

/// The Everyone SID, S-1-1-0: revision 1, one subauthority, the world
/// authority (big-endian 1), subauthority 0.
#[cfg(any(windows, test))]
const EVERYONE_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];

/// `(is_symlink, owner SID bytes)` of an entry, `None` when it is missing.
#[cfg(any(windows, test))]
type OwnerEntry = Option<(bool, Vec<u8>)>;

/// The decision behind the Windows `root_trusted`: every present entry
/// is no symlink and has the anchor's owner, and every missing entry is
/// optional.  An empty or Everyone anchor owner never matches: FAT and
/// exFAT volumes report every file as owned by Everyone, so equal owners
/// there say nothing about who created the file.
#[cfg(any(windows, test))]
fn owners_trusted(anchor_owner: &[u8], entries: &[(OwnerEntry, bool)]) -> bool {
    !anchor_owner.is_empty()
        && anchor_owner != EVERYONE_SID
        && entries.iter().all(|(entry, required)| match entry {
            Some((symlink, owner)) => !symlink && owner == anchor_owner,
            None => !required,
        })
}

fn parse_manifest(path: &Path) -> Result<ParsedManifest, WorkspaceError> {
    cabin_manifest::load_manifest(path).map_err(|source| WorkspaceError::Manifest {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn finds_workspace_root_from_member_dir() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"
"#,
            )
            .unwrap();
        let found = discover_workspace_root(&dir.path().join("packages/app"))
            .unwrap()
            .expect("workspace root should be discovered");
        assert_eq!(found.workspace_dir, dir.path());
        assert_eq!(found.manifest_path, dir.path().join("cabin.toml"));
    }

    #[test]
    fn finds_workspace_root_from_root_dir() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r"[workspace]
members = []
",
            )
            .unwrap();
        let found = discover_workspace_root(dir.path()).unwrap().unwrap();
        assert_eq!(found.workspace_dir, dir.path());
    }

    #[test]
    fn returns_none_when_no_workspace_root() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "only"
version = "0.1.0"
"#,
            )
            .unwrap();
        // Assumes no ancestor of the temporary directory holds a
        // workspace root.
        assert!(discover_workspace_root(dir.path()).unwrap().is_none());
    }

    #[test]
    fn skips_non_workspace_manifests_and_keeps_walking() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"
"#,
            )
            .unwrap();
        let found = discover_workspace_root(&dir.path().join("packages/app"))
            .unwrap()
            .expect("workspace root should be discovered");
        assert_eq!(found.manifest_path, dir.path().join("cabin.toml"));
    }

    /// A root found above a package is honored only for the members
    /// it lists: a package it neither lists nor excludes errors
    /// instead of silently adopting the root's toolchain and patches,
    /// while a subdirectory of a listed member still finds the root.
    #[test]
    fn unlisted_package_under_a_workspace_root_is_refused() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("packages/app/src").create_dir_all().unwrap();
        dir.child("scratch/other/cabin.toml")
            .write_str(
                r#"[package]
name = "other"
version = "0.1.0"
"#,
            )
            .unwrap();
        let found = discover_workspace_root(&dir.path().join("packages/app/src"))
            .unwrap()
            .expect("a member's subdirectory finds the root");
        assert_eq!(found.manifest_path, dir.path().join("cabin.toml"));
        let err = discover_workspace_root(&dir.path().join("scratch/other"))
            .expect_err("an unlisted package must not adopt the root");
        match err {
            WorkspaceError::UnclaimedPackage { package, root } => {
                assert_eq!(package, dir.path().join("scratch/other/cabin.toml"));
                assert_eq!(root, dir.path().join("cabin.toml"));
            }
            other => panic!("expected UnclaimedPackage, got {other:?}"),
        }
    }

    /// A package the root `exclude`s builds standalone: discovery
    /// reports no root rather than an error.
    #[test]
    fn excluded_package_under_a_workspace_root_builds_standalone() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
exclude = ["packages/vendored"]
"#,
            )
            .unwrap();
        for name in ["app", "vendored"] {
            dir.child(format!("packages/{name}/cabin.toml"))
                .write_str(&format!(
                    r#"[package]
name = "{name}"
version = "0.1.0"
"#
                ))
                .unwrap();
        }
        assert!(
            discover_workspace_root(&dir.path().join("packages/vendored"))
                .unwrap()
                .is_none()
        );
        assert!(
            discover_workspace_root(&dir.path().join("packages/app"))
                .unwrap()
                .is_some()
        );
        // An excluded member that is a symlink to a directory outside
        // the workspace (so its canonical path shares no prefix with the
        // root) is still recognized as excluded.
        #[cfg(unix)]
        {
            let elsewhere = TempDir::new().unwrap();
            elsewhere
                .child("vendored/cabin.toml")
                .write_str(
                    r#"[package]
name = "vendored"
version = "0.1.0"
"#,
                )
                .unwrap();
            std::fs::remove_dir_all(dir.path().join("packages/vendored")).unwrap();
            std::os::unix::fs::symlink(
                elsewhere.path().join("vendored"),
                dir.path().join("packages/vendored"),
            )
            .unwrap();
            assert!(
                discover_workspace_root(&dir.path().join("packages/vendored"))
                    .unwrap()
                    .is_none()
            );
        }
    }

    /// A root in a world-writable directory could have been planted
    /// by anyone, a world-writable root manifest or `.cabin/config.toml`
    /// edited by anyone, and a symlinked one swapped underneath by whoever
    /// controls the target's directory, so all are refused - from a member
    /// package and from a directory without a package of its own alike -
    /// while the same layout with private modes is honored.
    #[cfg(unix)]
    #[test]
    fn root_in_a_world_writable_directory_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["app"]
"#,
            )
            .unwrap();
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("docs").create_dir_all().unwrap();
        dir.child(".cabin/config.toml").write_str("").unwrap();
        assert!(
            discover_workspace_root(&dir.path().join("docs"))
                .unwrap()
                .is_some()
        );
        let manifest = dir.path().join("cabin.toml");
        let config = dir.path().join(".cabin/config.toml");
        for (path, mode) in [
            (dir.path(), 0o1777),
            (manifest.as_path(), 0o666),
            (config.parent().unwrap(), 0o1777),
            (config.as_path(), 0o666),
        ] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
            for below in ["app", "docs"] {
                let err = discover_workspace_root(&dir.path().join(below))
                    .expect_err("a world-writable root is untrusted");
                match err {
                    WorkspaceError::UntrustedWorkspaceRoot { path, root } => {
                        let anchor = if below == "app" {
                            "app/cabin.toml"
                        } else {
                            "docs"
                        };
                        assert_eq!(path, dir.path().join(anchor));
                        assert_eq!(root, manifest);
                    }
                    other => panic!("expected UntrustedWorkspaceRoot, got {other:?}"),
                }
            }
            assert!(discover_workspace_root(dir.path()).unwrap().is_some());
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o775)).unwrap();
        }
        // The same files reached through a symlink are refused even
        // when the link and its target are private.
        let elsewhere = TempDir::new().unwrap();
        elsewhere.child("config.toml").write_str("").unwrap();
        std::fs::remove_file(&config).unwrap();
        std::os::unix::fs::symlink(elsewhere.path().join("config.toml"), &config).unwrap();
        assert!(matches!(
            discover_workspace_root(&dir.path().join("app")),
            Err(WorkspaceError::UntrustedWorkspaceRoot { .. })
        ));
    }

    /// A root and `.cabin/config.toml` the current user created are
    /// honored from a member package and from a directory without a
    /// package, so the owner-SID check does not refuse every workspace;
    /// a symlinked config is refused where the host lets us create one.
    /// Refusing another user's root needs a second account.
    #[cfg(windows)]
    #[test]
    fn root_owned_by_the_current_user_is_trusted() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["app"]
"#,
            )
            .unwrap();
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("docs").create_dir_all().unwrap();
        dir.child(".cabin/config.toml").write_str("").unwrap();
        for below in ["app", "docs"] {
            assert!(
                discover_workspace_root(&dir.path().join(below))
                    .unwrap()
                    .is_some()
            );
        }
        let elsewhere = TempDir::new().unwrap();
        elsewhere.child("config.toml").write_str("").unwrap();
        let config = dir.path().join(".cabin/config.toml");
        std::fs::remove_file(&config).unwrap();
        // Creating a symlink needs Developer Mode or elevation.
        if std::os::windows::fs::symlink_file(elsewhere.path().join("config.toml"), &config).is_ok()
        {
            assert!(matches!(
                discover_workspace_root(&dir.path().join("app")),
                Err(WorkspaceError::UntrustedWorkspaceRoot { .. })
            ));
        }
    }

    /// The Windows owner decision: equal owners pass; another owner on
    /// any present entry, a symlink, a missing required entry, or an
    /// Everyone (FAT/exFAT) owner refuses; a missing optional entry is
    /// skipped.
    #[test]
    fn owners_trusted_requires_the_anchor_owner() {
        let user = vec![1, 5, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0, 7, 0, 0, 0];
        let other = vec![1, 5, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0, 8, 0, 0, 0];
        let owned = |owner: &Vec<u8>| Some((false, owner.clone()));
        let entries = |manifest, dir, config_dir, config| {
            vec![
                (manifest, true),
                (dir, true),
                (config_dir, false),
                (config, false),
            ]
        };
        let all_user = entries(owned(&user), owned(&user), owned(&user), owned(&user));
        assert!(owners_trusted(&user, &all_user));
        for i in 0..4 {
            let mut mismatched = all_user.clone();
            mismatched[i].0 = owned(&other);
            assert!(
                !owners_trusted(&user, &mismatched),
                "entry {i} owned by another user"
            );
            let mut linked = all_user.clone();
            linked[i].0 = Some((true, user.clone()));
            assert!(!owners_trusted(&user, &linked), "entry {i} a symlink");
        }
        assert!(owners_trusted(
            &user,
            &entries(owned(&user), owned(&user), None, None)
        ));
        assert!(!owners_trusted(
            &user,
            &entries(None, owned(&user), None, None)
        ));
        assert!(!owners_trusted(
            &user,
            &entries(owned(&user), None, None, None)
        ));
        let everyone = EVERYONE_SID.to_vec();
        let fat = entries(owned(&everyone), owned(&everyone), None, None);
        assert!(!owners_trusted(&everyone, &fat));
        let ownerless = entries(
            Some((false, Vec::new())),
            Some((false, Vec::new())),
            None,
            None,
        );
        assert!(!owners_trusted(&[], &ownerless));
    }

    /// A root manifest that vanished between parsing and the trust
    /// check is refused instead of being treated like an absent
    /// optional file.
    #[test]
    fn root_trusted_refuses_a_missing_manifest() {
        let dir = TempDir::new().unwrap();
        dir.child("app").create_dir_all().unwrap();
        assert!(!root_trusted(&dir.path().join("cabin.toml"), &dir.path().join("app")).unwrap());
    }

    #[test]
    fn nested_workspace_errors_when_starting_inside_nested() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["nested"]
"#,
            )
            .unwrap();
        dir.child("nested/cabin.toml")
            .write_str(
                r"[workspace]
members = []
",
            )
            .unwrap();
        let err = discover_workspace_root(&dir.path().join("nested"))
            .expect_err("expected NestedWorkspaceDiscovery");
        match err {
            WorkspaceError::NestedWorkspaceDiscovery { nearest, outer } => {
                assert_eq!(nearest, dir.path().join("nested/cabin.toml"));
                assert_eq!(outer, dir.path().join("cabin.toml"));
            }
            other => panic!("expected NestedWorkspaceDiscovery, got {other:?}"),
        }
    }

    #[test]
    fn nested_workspace_errors_even_when_outer_does_not_list_nested() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r"[workspace]
members = []
",
            )
            .unwrap();
        dir.child("nested/cabin.toml")
            .write_str(
                r"[workspace]
members = []
",
            )
            .unwrap();
        let err = discover_workspace_root(&dir.path().join("nested"))
            .expect_err("expected NestedWorkspaceDiscovery");
        assert!(matches!(
            err,
            WorkspaceError::NestedWorkspaceDiscovery { .. }
        ));
    }
}
