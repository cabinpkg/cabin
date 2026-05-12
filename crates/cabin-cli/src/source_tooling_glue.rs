//! Shared helpers for the source-tooling commands.
//!
//! `cabin fmt` and `cabin tidy` each translate a
//! CLI flag bundle into typed inputs for `cabin-source-discovery`
//! and a downstream runner.  Their selection plumbing, exclude
//! handling, and reporter-side rendering are identical; this
//! module owns the shared pieces so the glue files can stay
//! focused on the parts that genuinely differ per command.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cabin_workspace::{PackageGraph, PackageSelection, SelectionMode};

/// Translate the standard `--workspace` / `--package` /
/// `--default-members` trio into a [`PackageSelection`].  The
/// three source-tooling commands all expose the same trio with
/// the same precedence; `--exclude` on each is a *path*
/// exclusion handled by source discovery, so the typed selection
/// carries an empty package-name exclude list.
pub(crate) fn package_selection_from_flags(
    workspace: bool,
    packages: &[String],
    default_members: bool,
) -> PackageSelection {
    let mode = if workspace {
        SelectionMode::WholeWorkspace
    } else if !packages.is_empty() {
        SelectionMode::ExplicitPackages(packages.to_vec())
    } else if default_members {
        SelectionMode::DefaultMembers
    } else {
        SelectionMode::CurrentPackage
    };
    PackageSelection {
        mode,
        exclude: Vec::new(),
    }
}

/// Manifest dirs that the source walker should skip when
/// invoked from a selected package's root.  The walker emits one
/// entry per root; when a root is package A and a sibling
/// package B's manifest dir lives under it (or the workspace
/// root contains every member), walking would otherwise visit
/// B's sources too.  Excluding every non-selected manifest dir
/// prevents that; selected packages remain reachable because
/// their own root is the start point of an independent walk.
///
/// Only local packages have meaningful manifest dirs on the
/// host filesystem.  Extracted-registry entries live in the
/// artifact cache and the walker never reaches them anyway.
pub(crate) fn nested_package_excludes(
    graph: &PackageGraph,
    selected: &BTreeSet<usize>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for (idx, pkg) in graph.packages.iter().enumerate() {
        if selected.contains(&idx) {
            continue;
        }
        out.push(pkg.manifest_dir.clone());
    }
    out
}

/// Render a list of package names for reporter status / verbose
/// output.  Single-package selections emit `package `name``;
/// multi-package selections emit `packages `a`, `b`, `c``.
pub(crate) fn describe_packages(names: &[String]) -> String {
    match names.len() {
        0 => "<no package selected>".to_owned(),
        1 => format!("package `{}`", names[0]),
        _ => {
            let joined = names
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("packages {joined}")
        }
    }
}

/// Resolve `path` against `base` when relative; leave absolute
/// paths untouched.
pub(crate) fn absolutize(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

/// Render `path` relative to the workspace root for reporter
/// output, falling back to the absolute path when it is not a
/// descendant of the workspace.
pub(crate) fn display_workspace_relative(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .map(|rel| rel.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}
