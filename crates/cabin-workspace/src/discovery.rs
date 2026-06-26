//! Workspace root discovery.
//!
//! `cabin` commands are usually run from inside a workspace.  To make
//! that ergonomic, the CLI walks upward from the current directory
//! looking for a `cabin.toml` that contains a `[workspace]` table.
//! When one is found, commands behave as if they had been invoked
//! against that root manifest.  When none is found we keep the legacy
//! single-package behavior rooted at whatever `--manifest-path` was
//! requested (default: `./cabin.toml`).
//!
//! This module is filesystem-only - it never touches the network.
//! It is also conservative: it stops at the filesystem root and never
//! descends through symlinks it has not been asked to traverse.  The
//! caller decides whether to honor the discovered workspace or fall
//! back to a directly-supplied manifest.

use std::path::{Path, PathBuf};

use cabin_manifest::ParsedManifest;

use crate::error::WorkspaceError;

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
/// single workspace root is found, `Ok(None)` when the walk
/// reaches the filesystem root without finding one, and a
/// [`WorkspaceError::NestedWorkspaceDiscovery`] error when the
/// walk finds two or more `[workspace]`-bearing manifests
/// stacked above `start`.  Manifest parse errors are surfaced as
/// [`WorkspaceError::Manifest`] so the user sees the bad file.
///
/// The rule is strict: discovery walks all the way to the
/// filesystem root and refuses to choose a workspace at all
/// when it finds **two or more** workspace roots stacked above
/// the start path.  The user is forced to disambiguate
/// (typically by passing `--manifest-path` explicitly), which
/// avoids surprises where running from inside a nested
/// workspace silently operated on an outer one - or vice versa.
///
/// Discovery stays a pure filesystem walk: no network, no
/// symlink follow beyond what the OS already does for
/// `Path::is_file`, and a stop at the filesystem root.
///
/// # Errors
/// Returns [`WorkspaceError::NestedWorkspaceDiscovery`] when two or
/// more `[workspace]`-bearing manifests are stacked above `start`,
/// and propagates [`WorkspaceError::Manifest`] from `parse_manifest`
/// when an encountered `cabin.toml` fails to parse.
///
/// # Panics
/// Panics only if the slice-pattern invariant were violated: the
/// `.unwrap()`/`.first()`/`.last()` calls run inside match arms that
/// have already proved `found` holds exactly one element (the
/// `[_only]` arm) or two-or-more elements (the `_` arm), so the
/// `Option`s are always `Some`.
pub fn discover_workspace_root(start: &Path) -> Result<Option<DiscoveredManifest>, WorkspaceError> {
    let mut current = start.to_path_buf();
    if current.is_relative()
        && let Ok(abs) = std::env::current_dir().map(|cwd| cwd.join(&current))
    {
        current = abs;
    }
    let mut found: Vec<DiscoveredManifest> = Vec::new();
    loop {
        let candidate = current.join("cabin.toml");
        if candidate.is_file() {
            let parsed = parse_manifest(&candidate)?;
            if parsed.workspace.is_some() {
                found.push(DiscoveredManifest {
                    manifest_path: candidate,
                    workspace_dir: current.clone(),
                });
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
        [_only] => Ok(Some(found.into_iter().next().unwrap())),
        _ => {
            // refuse to silently pick one when the
            // user is sandwiched between two workspace roots.
            // `found` is ordered nearest-first because the walk
            // started at the user's cwd.
            let nearest = found.first().unwrap();
            let outer = found.last().unwrap();
            Err(WorkspaceError::NestedWorkspaceDiscovery {
                nearest: nearest.manifest_path.clone(),
                outer: outer.manifest_path.clone(),
            })
        }
    }
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
        // The cabin.toml is not a workspace root, and the parent
        // directory is some tempdir that also lacks one.  We must not
        // mistakenly identify the [package]-only manifest as a
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
        // A nested non-workspace manifest must not stop the walk -
        // discovery returns the (only) workspace root.
        dir.child("packages/app/cabin.toml")
            .write_str("[package]\nname = \"app\"\nversion = \"0.1.0\"\n")
            .unwrap();
        let found = discover_workspace_root(&dir.path().join("packages/app"))
            .unwrap()
            .expect("workspace root should be discovered");
        assert_eq!(found.manifest_path, dir.path().join("cabin.toml"));
    }

    #[test]
    fn nested_workspace_errors_when_starting_inside_nested() {
        // Two stacked workspace roots - the outer at `dir`, the
        // nested at `dir/nested`.  Discovery from inside the nested
        // workspace must error rather than silently picking one.
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
        // Outer omits `nested` from its members.  `nested` itself
        // is still a workspace.  Discovery still
        // detects two roots and refuses to silently pick.
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
