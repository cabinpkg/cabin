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
/// directory, the root is honored only when, on Unix, it passes
/// `root_trusted` against the nearest package manifest (or `start` when
/// no package manifest lies between them) and, when such a package
/// exists, lists it as a member (`root_claims_package`).  Otherwise an
/// ancestor another user can write to - `/tmp`, a shared mount - could
/// plant a root whose toolchain runs for every package below it.
/// Windows skips the ownership checks.
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

/// Windows has no uid or mode bits to compare; the root is trusted.
#[cfg(not(unix))]
fn root_trusted(_root_manifest: &Path, _anchor: &Path) -> Result<bool, WorkspaceError> {
    Ok(true)
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
            .write_str("[package]\nname = \"app\"\nversion = \"0.1.0\"\n")
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
            .write_str("[package]\nname = \"only\"\nversion = \"0.1.0\"\n")
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
            .write_str("[package]\nname = \"app\"\nversion = \"0.1.0\"\n")
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
            .write_str("[package]\nname = \"app\"\nversion = \"0.1.0\"\n")
            .unwrap();
        dir.child("packages/app/src").create_dir_all().unwrap();
        dir.child("scratch/other/cabin.toml")
            .write_str("[package]\nname = \"other\"\nversion = \"0.1.0\"\n")
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
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"
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
                .write_str("[package]\nname = \"vendored\"\nversion = \"0.1.0\"\n")
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
            .write_str("[package]\nname = \"app\"\nversion = \"0.1.0\"\n")
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

    /// A root manifest that vanished between parsing and the trust
    /// check is refused instead of being treated like an absent
    /// optional file.
    #[cfg(unix)]
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
