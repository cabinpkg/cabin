//! Plan and execute the deletion list for `cabin clean`.
//!
//! The on-disk layout this module operates against is
//! [`crate::layout`]'s; the build directory's lock file is never
//! removed.
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
//!   inside the tree - it removes the link, not the target - and
//!   the build directory itself is rejected up-front when it is a
//!   symlink, so this module never traverses through a symlink.

use std::path::{Path, PathBuf};

use thiserror::Error;

use cabin_core::{PackageName, ProfileName};

/// What `cabin clean` should remove.
#[derive(Debug, Clone)]
pub enum CleanScope {
    /// Remove everything in the build directory except its lock file
    /// (see [`crate::gc::BuildDirLock`]).  Used by the no-flag
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
    /// Manifest directories of every loaded package - single
    /// package or every workspace member.  Used to refuse a
    /// build directory that points at a package source tree.
    pub package_roots: &'a [PathBuf],
    /// Source files and source-owned directories that must not
    /// be contained by the build directory.  This lets in-tree
    /// build dirs like `<pkg>/build` keep working while rejecting
    /// dangerous settings such as `--build-dir src`.
    pub protected_source_paths: &'a [PathBuf],
    /// What to clean.
    pub scope: CleanScope,
    /// A `--target` platform: the scope applies to its
    /// [`crate::layout::platform_dir`] instead of the build machine's.
    pub target: Option<&'a cabin_core::TargetPlatform>,
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
    /// Paths the executor removed.  May be a strict
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

    #[error(
        "refusing to clean {} as profile `{profile}`: {} is a `--target` build Cabin wrote there \
         before it kept those under `{}`; remove {} yourself, or run `cabin clean` with no \
         --profile, --package, or --target to empty the whole build directory",
        dir.display(),
        nested.display(),
        crate::layout::TARGETS_DIR,
        dir.display()
    )]
    PreviousTargetLayout {
        profile: ProfileName,
        dir: PathBuf,
        nested: PathBuf,
    },

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
/// (filtering candidates to those that live on disk).
///
/// # Errors
/// Returns a [`CleanError`] safety-guard variant when `build_dir`
/// is unsafe to delete: [`CleanError::EmptyBuildDir`],
/// [`CleanError::RootBuildDir`], [`CleanError::HomeBuildDir`],
/// [`CleanError::WorkspaceRootBuildDir`],
/// [`CleanError::PackageRootBuildDir`],
/// [`CleanError::SourcePathBuildDir`], or
/// [`CleanError::SymlinkBuildDir`].  Also returns
/// [`CleanError::PreviousTargetLayout`] when a profile directory in
/// scope holds a `--target` build from before `.targets`, and
/// [`CleanError::PathEscapesBuildDir`] if a computed deletion
/// candidate would fall outside `build_dir`.
pub fn plan_clean(req: &CleanRequest<'_>) -> Result<CleanPlan, CleanError> {
    validate_safe_build_dir(
        req.build_dir,
        req.workspace_root,
        req.package_roots,
        req.protected_source_paths,
    )?;

    let root = crate::layout::platform_dir(req.build_dir, req.target);
    // Containment below is lexical, so every directory between the build
    // directory and the platform's tree must be real too.
    for dir in root.ancestors().take_while(|dir| *dir != req.build_dir) {
        if std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(CleanError::SymlinkBuildDir(dir.to_path_buf()));
        }
    }
    // A profile named like a triple shares its directory name with the
    // tree a `--target` build wrote before `.targets`; never remove that
    // tree as the profile's.
    let profile_dir = |profile: &ProfileName| {
        let dir = crate::layout::profile_dir(&root, profile);
        match crate::layout::previous_target_tree(&dir) {
            Some(nested) => Err(CleanError::PreviousTargetLayout {
                profile: profile.clone(),
                dir,
                nested,
            }),
            None => Ok(dir),
        }
    };
    let candidates = match &req.scope {
        // The lock file outlives every clean (see
        // `gc::BuildDirLock`), so a whole clean empties the build
        // directory around it.  Under `--target` the triple's tree goes.
        CleanScope::Whole if req.target.is_none() => match std::fs::read_dir(&root) {
            Ok(entries) => entries
                .map(|entry| entry.map(|entry| entry.path()))
                .filter(|path| {
                    path.as_ref().map_or(true, |path| {
                        path.file_name() != Some(std::ffi::OsStr::new(crate::gc::LOCK_FILENAME))
                    })
                })
                .collect::<Result<_, _>>()
                .map_err(|source| CleanError::Io {
                    path: root.clone(),
                    source,
                })?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source) => return Err(CleanError::Io { path: root, source }),
        },
        CleanScope::Whole => vec![root],
        CleanScope::Profile(profile) => vec![profile_dir(profile)?],
        CleanScope::Packages { profiles, packages } => {
            let mut out = Vec::with_capacity(profiles.len().saturating_mul(packages.len()));
            for profile in profiles {
                let profile_dir = profile_dir(profile)?;
                for pkg in packages {
                    // Both contexts, so clean removes exactly the
                    // directories the planner writes.
                    for build_context in [false, true] {
                        out.push(crate::layout::package_dir(&profile_dir, build_context, pkg));
                    }
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

    // `symlink_metadata`, not `exists`: a dangling link is still removed.
    let mut existing: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|p| std::fs::symlink_metadata(p).is_ok())
        .collect();
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
/// the goal state - the path no longer existing - is already
/// satisfied.  Symbolic links inside the build tree are removed
/// as links rather than recursively followed.
///
/// # Errors
/// Returns [`CleanError::Io`] if querying a path's metadata
/// (`symlink_metadata`) fails for any reason other than the path
/// already being gone, or if removing a directory or file fails.
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

/// Refuse a build directory that `cabin clean` - or a collection - must
/// not remove from: an empty path, the root, the home directory, the
/// workspace or a package root, a directory overlapping declared
/// sources, or a symlink.
///
/// # Errors
/// The matching [`CleanError`] safety-guard variant.
pub fn validate_safe_build_dir(
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
/// Falls back to literal equality when canonicalization fails so a
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
    if build_dir.starts_with(source_path) || source_path.starts_with(build_dir) {
        return true;
    }
    // The literal check above misses when the two paths reach the same
    // location by different spellings - most visibly on Windows, where a
    // build dir taken from the cwd may carry an 8.3 short name
    // (`RUNNER~1`) while the manifest-derived source paths are long-name
    // canonical (`runneradmin`), and on macOS where `/tmp` resolves to
    // `/private/tmp`.  Canonicalize both through the project's single
    // canonical-path boundary and re-test containment, so `cabin clean`
    // still refuses a build dir that holds source files.  Falls back to
    // "no overlap" when either side cannot be canonicalized (e.g. the
    // build dir does not exist - there is nothing to protect there).
    match (
        cabin_fs::canonicalize(build_dir),
        cabin_fs::canonicalize(source_path),
    ) {
        (Ok(cb), Ok(cs)) => cb.starts_with(&cs) || cs.starts_with(&cb),
        _ => false,
    }
}

/// Platform home directory for the destructive-operation guard.
/// `std::env::home_dir` honors `HOME` / `USERPROFILE` overrides but
/// still resolves the home through platform APIs when the variable
/// is unset, so an unset variable alone does not silently disable
/// the guard.
fn home_dir() -> Option<PathBuf> {
    std::env::home_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use predicates::prelude::*;

    fn profile(name: &str) -> ProfileName {
        ProfileName::new(name.to_owned()).unwrap()
    }

    fn package(name: &str) -> PackageName {
        PackageName::new(name.to_owned()).unwrap()
    }

    fn populate_layout(build_dir: &assert_fs::fixture::ChildPath) {
        build_dir.child("dev/build.ninja").write_str("x").unwrap();
        build_dir
            .child("dev/packages/hello/hello")
            .write_str("x")
            .unwrap();
        build_dir
            .child("dev/packages/util/libutil.a")
            .write_str("x")
            .unwrap();
        build_dir
            .child("dev/build-machine/packages/hello/hello")
            .write_str("x")
            .unwrap();
        build_dir
            .child("release/build.ninja")
            .write_str("x")
            .unwrap();
        build_dir
            .child("release/packages/hello/hello")
            .write_str("x")
            .unwrap();
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
            target: None,
        }
    }

    #[test]
    fn plan_whole_lists_everything_but_the_lock_file() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.child("build");
        populate_layout(&build_dir);
        build_dir
            .child(crate::gc::LOCK_FILENAME)
            .write_str("")
            .unwrap();
        build_dir
            .child(".cabin-gc/state.json")
            .write_str("{}")
            .unwrap();
        let plan = plan_clean(&req(build_dir.path(), tmp.path(), CleanScope::Whole)).unwrap();
        assert_eq!(
            plan.removals,
            vec![
                build_dir.child(".cabin-gc").to_path_buf(),
                build_dir.child("dev").to_path_buf(),
                build_dir.child("release").to_path_buf(),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn plan_whole_lists_a_dangling_link() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.child("build");
        build_dir.create_dir_all().unwrap();
        std::os::unix::fs::symlink(tmp.child("gone"), build_dir.child("link")).unwrap();
        let plan = plan_clean(&req(build_dir.path(), tmp.path(), CleanScope::Whole)).unwrap();
        assert_eq!(plan.removals, vec![build_dir.child("link").to_path_buf()]);
    }

    #[test]
    fn plan_profile_lists_only_that_profile() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.child("build");
        populate_layout(&build_dir);
        let plan = plan_clean(&req(
            build_dir.path(),
            tmp.path(),
            CleanScope::Profile(profile("dev")),
        ))
        .unwrap();
        assert_eq!(plan.removals, vec![build_dir.path().join("dev")]);
    }

    #[test]
    fn plan_packages_includes_each_existing_path() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.child("build");
        populate_layout(&build_dir);
        let plan = plan_clean(&req(
            build_dir.path(),
            tmp.path(),
            CleanScope::Packages {
                profiles: vec![profile("dev"), profile("release")],
                packages: vec![package("hello")],
            },
        ))
        .unwrap();
        let expected = {
            let mut v = vec![
                build_dir.path().join("dev").join("packages").join("hello"),
                build_dir
                    .path()
                    .join("dev")
                    .join("build-machine")
                    .join("packages")
                    .join("hello"),
                build_dir
                    .path()
                    .join("release")
                    .join("packages")
                    .join("hello"),
            ];
            v.sort();
            v
        };
        assert_eq!(plan.removals, expected);
    }

    #[test]
    fn plan_skips_missing_candidates() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.child("build");
        // build dir does not exist.
        let plan = plan_clean(&req(build_dir.path(), tmp.path(), CleanScope::Whole)).unwrap();
        assert!(plan.removals.is_empty());
    }

    #[test]
    fn plan_is_deterministic_and_deduplicated() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.child("build");
        populate_layout(&build_dir);
        let plan = plan_clean(&req(
            build_dir.path(),
            tmp.path(),
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
        let build_dir = tmp.child("build");
        populate_layout(&build_dir);
        let plan = plan_clean(&req(build_dir.path(), tmp.path(), CleanScope::Whole)).unwrap();
        let report = execute_clean(&plan).unwrap();
        assert_eq!(report.removed.len(), 2);
        build_dir.child("dev").assert(predicate::path::missing());
        build_dir
            .child("release")
            .assert(predicate::path::missing());
    }

    #[test]
    fn execute_tolerates_concurrent_removal() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.child("build");
        populate_layout(&build_dir);
        let plan = plan_clean(&req(build_dir.path(), tmp.path(), CleanScope::Whole)).unwrap();
        std::fs::remove_dir_all(build_dir.path()).unwrap();
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
        let err = plan_clean(&req(tmp.path(), tmp.path(), CleanScope::Whole)).unwrap_err();
        assert!(matches!(err, CleanError::WorkspaceRootBuildDir(_)));
    }

    #[test]
    fn rejects_package_root_build_dir() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.child("pkg");
        pkg.create_dir_all().unwrap();
        let pkg_path = pkg.to_path_buf();
        let request = CleanRequest {
            build_dir: pkg.path(),
            workspace_root: tmp.path(),
            package_roots: std::slice::from_ref(&pkg_path),
            protected_source_paths: &[],
            scope: CleanScope::Whole,
            target: None,
        };
        let err = plan_clean(&request).unwrap_err();
        assert!(matches!(err, CleanError::PackageRootBuildDir(_)));
    }

    #[test]
    fn rejects_build_dir_that_contains_source_path() {
        let tmp = TempDir::new().unwrap();
        let build_dir = tmp.child("pkg/src");
        let source = build_dir.child("main.cc");
        source.write_str("int main(){return 0;}").unwrap();
        let source_path = source.to_path_buf();
        let request = CleanRequest {
            build_dir: build_dir.path(),
            workspace_root: tmp.path(),
            package_roots: &[],
            protected_source_paths: std::slice::from_ref(&source_path),
            scope: CleanScope::Whole,
            target: None,
        };
        let err = plan_clean(&request).unwrap_err();
        assert!(matches!(err, CleanError::SourcePathBuildDir { .. }));
    }

    #[test]
    fn rejects_build_dir_overlapping_source_by_divergent_spelling() {
        // A build dir whose *spelling* differs from the canonical source
        // path - here through a `..` segment, standing in for the 8.3
        // short-name vs long-name divergence seen on Windows (and the
        // `/tmp` vs `/private/tmp` one on macOS) - must still be
        // rejected.  The literal `starts_with` check misses it; the
        // canonicalized fallback catches it, so `cabin clean` cannot
        // delete a build dir that holds source files.
        let tmp = TempDir::new().unwrap();
        let source = tmp.child("pkg/src/main.cc");
        source.write_str("int main(){return 0;}").unwrap();
        tmp.child("pkg/extra").create_dir_all().unwrap();
        let source_path = source.to_path_buf();
        // `pkg/extra/../src` only equals `pkg/src` after canonicalization.
        let build_dir = tmp.path().join("pkg").join("extra").join("..").join("src");
        let request = CleanRequest {
            build_dir: &build_dir,
            workspace_root: tmp.path(),
            package_roots: &[],
            protected_source_paths: std::slice::from_ref(&source_path),
            scope: CleanScope::Whole,
            target: None,
        };
        let err = plan_clean(&request).unwrap_err();
        assert!(matches!(err, CleanError::SourcePathBuildDir { .. }));
    }

    #[test]
    fn a_profile_named_like_a_triple_and_that_triple_clean_apart() {
        let tmp = TempDir::new().unwrap();
        let build = tmp.child("build");
        let triple = "x86_64-unknown-linux-gnu";
        build
            .child(format!("{triple}/build.ninja"))
            .write_str("x")
            .unwrap();
        build
            .child(format!("{triple}/packages/app/app"))
            .write_str("x")
            .unwrap();
        build
            .child(format!(".targets/{triple}/{triple}/packages/app/app"))
            .write_str("x")
            .unwrap();
        build
            .child(format!(".targets/{triple}/dev/packages/app/app"))
            .write_str("x")
            .unwrap();
        let platform = cabin_core::TargetPlatform::from_triple(triple).unwrap();

        let native = plan_clean(&req(
            build.path(),
            tmp.path(),
            CleanScope::Profile(profile(triple)),
        ))
        .unwrap();
        assert_eq!(native.removals, vec![build.child(triple).to_path_buf()]);

        let mut cross = req(
            build.path(),
            tmp.path(),
            CleanScope::Profile(profile(triple)),
        );
        cross.target = Some(&platform);
        assert_eq!(
            plan_clean(&cross).unwrap().removals,
            vec![
                build
                    .child(format!(".targets/{triple}/{triple}"))
                    .to_path_buf()
            ]
        );
        let mut cross = req(build.path(), tmp.path(), CleanScope::Whole);
        cross.target = Some(&platform);
        assert_eq!(
            plan_clean(&cross).unwrap().removals,
            vec![build.child(format!(".targets/{triple}")).to_path_buf()]
        );
    }

    #[test]
    fn a_target_tree_from_before_targets_is_not_cleaned_as_a_profile() {
        let tmp = TempDir::new().unwrap();
        let build = tmp.child("build");
        let triple = "aarch64-unknown-linux-gnu";
        build
            .child(format!("{triple}/dev/build.ninja"))
            .write_str("x")
            .unwrap();
        build
            .child(format!("{triple}/dev/packages/app/app"))
            .write_str("x")
            .unwrap();
        for scope in [
            CleanScope::Profile(profile(triple)),
            CleanScope::Packages {
                profiles: vec![profile("dev"), profile(triple)],
                packages: vec![package("app")],
            },
        ] {
            let err = plan_clean(&req(build.path(), tmp.path(), scope)).unwrap_err();
            assert!(
                matches!(&err, CleanError::PreviousTargetLayout { dir, nested, .. }
                    if *dir == build.child(triple).path() && *nested == build.child(format!("{triple}/dev")).path()),
                "{err}"
            );
        }
        // Emptying the whole build directory still reaches it.
        let whole = plan_clean(&req(build.path(), tmp.path(), CleanScope::Whole)).unwrap();
        assert_eq!(whole.removals, vec![build.child(triple).to_path_buf()]);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_build_dir() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.child("real");
        target
            .child("x86_64-pc-linux-gnu")
            .create_dir_all()
            .unwrap();
        let link = tmp.child("link");
        std::os::unix::fs::symlink(target.path(), link.path()).unwrap();
        let err = plan_clean(&req(link.path(), tmp.path(), CleanScope::Whole)).unwrap_err();
        assert!(matches!(err, CleanError::SymlinkBuildDir(_)));
        // Narrowing to one triple's tree still checks the build dir,
        // `.targets`, and the triple's own directory.
        let x86 = cabin_core::TargetPlatform::from_triple("x86_64-pc-linux-gnu").unwrap();
        let mut narrowed = req(link.path(), tmp.path(), CleanScope::Whole);
        narrowed.target = Some(&x86);
        let err = plan_clean(&narrowed).unwrap_err();
        assert!(matches!(err, CleanError::SymlinkBuildDir(_)));
        let arm = cabin_core::TargetPlatform::from_triple("aarch64-unknown-linux-gnu").unwrap();
        let build = tmp.child("build");
        build.child(".targets").create_dir_all().unwrap();
        std::os::unix::fs::symlink(
            target.path(),
            build.child(".targets/aarch64-unknown-linux-gnu").path(),
        )
        .unwrap();
        let mut narrowed = req(build.path(), tmp.path(), CleanScope::Whole);
        narrowed.target = Some(&arm);
        let err = plan_clean(&narrowed).unwrap_err();
        assert!(
            matches!(&err, CleanError::SymlinkBuildDir(dir) if dir.ends_with("aarch64-unknown-linux-gnu"))
        );
        let linked = tmp.child("linked");
        linked.create_dir_all().unwrap();
        std::os::unix::fs::symlink(target.path(), linked.child(".targets").path()).unwrap();
        let mut narrowed = req(linked.path(), tmp.path(), CleanScope::Whole);
        narrowed.target = Some(&x86);
        let err = plan_clean(&narrowed).unwrap_err();
        assert!(matches!(&err, CleanError::SymlinkBuildDir(dir) if dir.ends_with(".targets")));
    }
}
