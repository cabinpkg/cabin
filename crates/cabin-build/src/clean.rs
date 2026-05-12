//! Plan and execute the deletion list for `cabin clean`.
//!
//! The on-disk layout this module operates against must stay in
//! sync with [`crate::planner`]:
//!
//! ```text
//! <build_dir>/<profile>/build.ninja
//! <build_dir>/<profile>/compile_commands.json
//! <build_dir>/<profile>/packages/<pkg>/...
//! <build_dir>/<profile>/cargo/<pkg>/<target>/...
//! ```
//!
//! Safety contract:
//!
//! - every safety check runs before the deletion plan is even
//!   computed, so an unsafe build directory never reaches the
//!   filesystem step;
//! - the plan only contains paths inside the resolved
//!   `build_dir`;
//! - paths are sorted so dry-run output is deterministic;
//! - `remove_dir_all` does not follow symlinks for entries
//!   inside the tree — it removes the link, not the target — and
//!   the build directory itself is rejected up-front when it is a
//!   symlink, so this module never traverses through a symlink.

use std::path::{Path, PathBuf};

use thiserror::Error;

use cabin_core::{PackageName, ProfileName};

/// What `cabin clean` should remove.
#[derive(Debug, Clone)]
pub enum CleanScope {
    /// Remove the entire build directory.  Used by the no-flag
    /// invocation `cabin clean`.
    Whole,
    /// Remove a single profile sub-tree
    /// (`<build_dir>/<profile>/`).
    Profile(ProfileName),
    /// Remove the per-package output for one or more packages
    /// across one or more profiles.
    Packages {
        profiles: Vec<ProfileName>,
        packages: Vec<PackageName>,
    },
}

/// Inputs to [`plan_clean`].
#[derive(Debug, Clone)]
pub struct CleanRequest<'a> {
    /// Resolved absolute build directory.
    pub build_dir: &'a Path,
    /// Workspace root directory (manifest's parent).  Used by
    /// the safety check that refuses to clean the workspace
    /// itself.
    pub workspace_root: &'a Path,
    /// Manifest directories of every loaded package — single
    /// package or every workspace member.  Used to refuse a
    /// build directory that points at a package source tree.
    pub package_roots: &'a [PathBuf],
    /// Source files and source-owned directories that must not
    /// be contained by the build directory. This lets in-tree
    /// build dirs like `<pkg>/build` keep working while rejecting
    /// dangerous settings such as `--build-dir src`.
    pub protected_source_paths: &'a [PathBuf],
    /// What to clean.
    pub scope: CleanScope,
}

/// Deterministic deletion plan: a sorted, deduplicated list of
/// existing paths inside `build_dir` that the executor will
/// remove.
#[derive(Debug, Clone)]
pub struct CleanPlan {
    /// Resolved build directory the plan operates against.
    pub build_dir: PathBuf,
    /// Sorted, existing paths to remove.  Each entry lives
    /// inside `build_dir` (see [`plan_clean`]'s contract).
    pub removals: Vec<PathBuf>,
}

/// Result of an [`execute_clean`] call.
#[derive(Debug, Clone, Default)]
pub struct CleanReport {
    /// Paths the executor actually removed.  May be a strict
    /// subset of [`CleanPlan::removals`] if a concurrent process
    /// removed an entry between planning and execution.
    pub removed: Vec<PathBuf>,
}

/// Errors produced while validating a clean request, planning
/// the deletion, or removing files.
#[derive(Debug, Error)]
pub enum CleanError {
    #[error("build directory path is empty")]
    EmptyBuildDir,

    #[error("refusing to clean root path {}", .0.display())]
    RootBuildDir(PathBuf),

    #[error("refusing to clean home directory {}", .0.display())]
    HomeBuildDir(PathBuf),

    #[error("refusing to clean workspace root {}; the build directory must point at a separate output directory", .0.display())]
    WorkspaceRootBuildDir(PathBuf),

    #[error("refusing to clean package source directory {}; the build directory must point at a separate output directory", .0.display())]
    PackageRootBuildDir(PathBuf),

    #[error("refusing to clean build directory {} because it overlaps source file or directory {}", build_dir.display(), source_path.display())]
    SourcePathBuildDir {
        build_dir: PathBuf,
        source_path: PathBuf,
    },

    #[error("refusing to clean symlink {}; replace it with a real directory before re-running `cabin clean`", .0.display())]
    SymlinkBuildDir(PathBuf),

    #[error("computed deletion path {} is not inside build directory {}", path.display(), build_dir.display())]
    PathEscapesBuildDir { path: PathBuf, build_dir: PathBuf },

    #[error("failed to remove {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Validate the request's safety guards and return a sorted,
/// deduplicated deletion plan.
///
/// Every path in the returned plan is an existing entry inside
/// `req.build_dir`.  The function never touches the filesystem
/// beyond `symlink_metadata` (safety check) and `Path::exists`
/// (filtering candidates to those that actually live on disk).
pub fn plan_clean(req: &CleanRequest<'_>) -> Result<CleanPlan, CleanError> {
    validate_safe_build_dir(
        req.build_dir,
        req.workspace_root,
        req.package_roots,
        req.protected_source_paths,
    )?;

    let candidates = match &req.scope {
        CleanScope::Whole => vec![req.build_dir.to_path_buf()],
        CleanScope::Profile(profile) => vec![req.build_dir.join(profile.as_str())],
        CleanScope::Packages { profiles, packages } => {
            let mut out = Vec::with_capacity(profiles.len().saturating_mul(packages.len()) * 2);
            for profile in profiles {
                let profile_root = req.build_dir.join(profile.as_str());
                for pkg in packages {
                    out.push(profile_root.join("packages").join(pkg.as_str()));
                    out.push(profile_root.join("cargo").join(pkg.as_str()));
                }
            }
            out
        }
    };

    for candidate in &candidates {
        if !is_within(candidate, req.build_dir) {
            return Err(CleanError::PathEscapesBuildDir {
                path: candidate.clone(),
                build_dir: req.build_dir.to_path_buf(),
            });
        }
    }

    let mut existing: Vec<PathBuf> = candidates.into_iter().filter(|p| p.exists()).collect();
    existing.sort();
    existing.dedup();

    Ok(CleanPlan {
        build_dir: req.build_dir.to_path_buf(),
        removals: existing,
    })
}

/// Remove every path in `plan.removals`.
///
/// Paths that disappeared between planning and execution
/// (concurrent removal by another process) are silently skipped:
/// the goal state — the path no longer existing — is already
/// satisfied.  Symbolic links inside the build tree are removed
/// as links rather than recursively followed.
pub fn execute_clean(plan: &CleanPlan) -> Result<CleanReport, CleanError> {
    let mut removed = Vec::with_capacity(plan.removals.len());
    for path in &plan.removals {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(CleanError::Io {
                    path: path.clone(),
                    source,
                });
            }
        };
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            std::fs::remove_dir_all(path).map_err(|source| CleanError::Io {
                path: path.clone(),
                source,
            })?;
        } else {
            std::fs::remove_file(path).map_err(|source| CleanError::Io {
                path: path.clone(),
                source,
            })?;
        }
        removed.push(path.clone());
    }
    Ok(CleanReport { removed })
}

fn validate_safe_build_dir(
    build_dir: &Path,
    workspace_root: &Path,
    package_roots: &[PathBuf],
    protected_source_paths: &[PathBuf],
) -> Result<(), CleanError> {
    if build_dir.as_os_str().is_empty() {
        return Err(CleanError::EmptyBuildDir);
    }
    if build_dir.parent().is_none() {
        return Err(CleanError::RootBuildDir(build_dir.to_path_buf()));
    }
    if let Some(home) = home_dir()
        && same_path(build_dir, &home)
    {
        return Err(CleanError::HomeBuildDir(build_dir.to_path_buf()));
    }
    if same_path(build_dir, workspace_root) {
        return Err(CleanError::WorkspaceRootBuildDir(build_dir.to_path_buf()));
    }
    for root in package_roots {
        if same_path(build_dir, root) {
            return Err(CleanError::PackageRootBuildDir(build_dir.to_path_buf()));
        }
    }
    for source_path in protected_source_paths {
        if overlaps_source_path(build_dir, source_path) {
            return Err(CleanError::SourcePathBuildDir {
                build_dir: build_dir.to_path_buf(),
                source_path: source_path.clone(),
            });
        }
    }
    if let Ok(meta) = std::fs::symlink_metadata(build_dir)
        && meta.file_type().is_symlink()
    {
        return Err(CleanError::SymlinkBuildDir(build_dir.to_path_buf()));
    }
    Ok(())
}

/// Equality test for paths that tolerates symlink-only spelling
/// differences (e.g. macOS exposes `/tmp/foo` as `/private/tmp/foo`).
/// Falls back to literal equality when canonicalisation fails so a
/// non-existent build dir still matches a non-existent workspace
/// root entered by the same path string.
fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// Whether `candidate` is `base` itself or lives underneath
/// `base`.  Performed by component-wise matching so a sibling
/// directory whose name is a string prefix of `base` does not
/// accidentally pass the check.
fn is_within(candidate: &Path, base: &Path) -> bool {
    candidate.starts_with(base)
}

fn overlaps_source_path(build_dir: &Path, source_path: &Path) -> bool {
    build_dir.starts_with(source_path) || source_path.starts_with(build_dir)
}

fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn profile(name: &str) -> ProfileName {
        ProfileName::new(name.to_owned()).unwrap()
    }

    fn package(name: &str) -> PackageName {
        PackageName::new(name.to_owned()).unwrap()
    }

    fn write(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    fn populate_layout(build_dir: &Path) {
        // dev profile.
        write(&build_dir.join("dev").join("build.ninja"));
        write(
            &build_dir
                .join("dev")
                .join("packages")
                .join("hello")
                .join("hello"),
        );
        write(
            &build_dir
                .join("dev")
                .join("packages")
                .join("util")
                .join("libutil.a"),
        );
        write(
            &build_dir
                .join("dev")
                .join("cargo")
                .join("hello")
                .join("rust")
                .join("artifact"),
        );
        // release profile.
        write(&build_dir.join("release").join("build.ninja"));
        write(
            &build_dir
                .join("release")
                .join("packages")
                .join("hello")
                .join("hello"),
        );
    }

    fn req<'a>(
        build_dir: &'a Path,
        workspace_root: &'a Path,
        scope: CleanScope,
    ) -> CleanRequest<'a> {
        CleanRequest {
            build_dir,
            workspace_root,
            package_roots: &[],
            protected_source_paths: &[],
            scope,
        }
    }

    #[test]
    fn plan_whole_lists_build_dir() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.path().join("build");
        populate_layout(&build_dir);
        let workspace = tmp.path().to_path_buf();
        let plan = plan_clean(&req(&build_dir, &workspace, CleanScope::Whole)).unwrap();
        assert_eq!(plan.removals, vec![build_dir]);
    }

    #[test]
    fn plan_profile_lists_only_that_profile() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.path().join("build");
        populate_layout(&build_dir);
        let workspace = tmp.path().to_path_buf();
        let plan = plan_clean(&req(
            &build_dir,
            &workspace,
            CleanScope::Profile(profile("dev")),
        ))
        .unwrap();
        assert_eq!(plan.removals, vec![build_dir.join("dev")]);
    }

    #[test]
    fn plan_packages_includes_each_existing_path() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.path().join("build");
        populate_layout(&build_dir);
        let workspace = tmp.path().to_path_buf();
        let plan = plan_clean(&req(
            &build_dir,
            &workspace,
            CleanScope::Packages {
                profiles: vec![profile("dev"), profile("release")],
                packages: vec![package("hello")],
            },
        ))
        .unwrap();
        let expected = {
            let mut v = vec![
                build_dir.join("dev").join("cargo").join("hello"),
                build_dir.join("dev").join("packages").join("hello"),
                build_dir.join("release").join("packages").join("hello"),
            ];
            v.sort();
            v
        };
        assert_eq!(plan.removals, expected);
    }

    #[test]
    fn plan_skips_missing_candidates() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.path().join("build");
        // build dir does not exist.
        let workspace = tmp.path().to_path_buf();
        let plan = plan_clean(&req(&build_dir, &workspace, CleanScope::Whole)).unwrap();
        assert!(plan.removals.is_empty());
    }

    #[test]
    fn plan_is_deterministic_and_deduplicated() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.path().join("build");
        populate_layout(&build_dir);
        let workspace = tmp.path().to_path_buf();
        let plan = plan_clean(&req(
            &build_dir,
            &workspace,
            CleanScope::Packages {
                profiles: vec![profile("release"), profile("dev"), profile("dev")],
                packages: vec![package("hello"), package("hello")],
            },
        ))
        .unwrap();
        let mut sorted = plan.removals.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(plan.removals, sorted);
    }

    #[test]
    fn execute_removes_planned_paths() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.path().join("build");
        populate_layout(&build_dir);
        let workspace = tmp.path().to_path_buf();
        let plan = plan_clean(&req(&build_dir, &workspace, CleanScope::Whole)).unwrap();
        let report = execute_clean(&plan).unwrap();
        assert_eq!(report.removed, vec![build_dir.clone()]);
        assert!(!build_dir.exists());
    }

    #[test]
    fn execute_tolerates_concurrent_removal() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.path().join("build");
        populate_layout(&build_dir);
        let workspace = tmp.path().to_path_buf();
        let plan = plan_clean(&req(&build_dir, &workspace, CleanScope::Whole)).unwrap();
        std::fs::remove_dir_all(&build_dir).unwrap();
        let report = execute_clean(&plan).unwrap();
        assert!(report.removed.is_empty());
    }

    #[test]
    fn rejects_root_build_dir() {
        let workspace = PathBuf::from("/tmp/x");
        let err = plan_clean(&req(Path::new("/"), &workspace, CleanScope::Whole)).unwrap_err();
        assert!(matches!(err, CleanError::RootBuildDir(_)));
    }

    #[test]
    fn rejects_empty_build_dir() {
        let workspace = PathBuf::from("/tmp/x");
        let err = plan_clean(&req(Path::new(""), &workspace, CleanScope::Whole)).unwrap_err();
        assert!(matches!(err, CleanError::EmptyBuildDir));
    }

    #[test]
    fn rejects_workspace_root_build_dir() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();
        let err = plan_clean(&req(&workspace, &workspace, CleanScope::Whole)).unwrap_err();
        assert!(matches!(err, CleanError::WorkspaceRootBuildDir(_)));
    }

    #[test]
    fn rejects_package_root_build_dir() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();
        let pkg = tmp.path().join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        let request = CleanRequest {
            build_dir: &pkg,
            workspace_root: &workspace,
            package_roots: std::slice::from_ref(&pkg),
            protected_source_paths: &[],
            scope: CleanScope::Whole,
        };
        let err = plan_clean(&request).unwrap_err();
        assert!(matches!(err, CleanError::PackageRootBuildDir(_)));
    }

    #[test]
    fn rejects_build_dir_that_contains_source_path() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();
        let build_dir = tmp.path().join("pkg").join("src");
        let source = build_dir.join("main.cc");
        std::fs::create_dir_all(&build_dir).unwrap();
        std::fs::write(&source, "int main(){return 0;}").unwrap();
        let request = CleanRequest {
            build_dir: &build_dir,
            workspace_root: &workspace,
            package_roots: &[],
            protected_source_paths: std::slice::from_ref(&source),
            scope: CleanScope::Whole,
        };
        let err = plan_clean(&request).unwrap_err();
        assert!(matches!(err, CleanError::SourcePathBuildDir { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_build_dir() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("real");
        std::fs::create_dir(&target).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let workspace = tmp.path().to_path_buf();
        let err = plan_clean(&req(&link, &workspace, CleanScope::Whole)).unwrap_err();
        assert!(matches!(err, CleanError::SymlinkBuildDir(_)));
    }
}
