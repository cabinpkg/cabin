//! Reusable C/C++ source / header discovery for Cabin
//! developer tools.
//!
//! The current consumers are `cabin fmt` and `cabin tidy`.
//! The interface stays narrow so each command can share the
//! same walker, exclusion policy, and deterministic ordering
//! without re-implementing any of it.
//!
//! The walker:
//!
//! - honors VCS ignore rules (`.gitignore`, `.ignore`) by default
//!   via the `ignore` crate; callers may disable this with
//!   [`SourceDiscoveryRequest::respect_vcs_ignore`];
//! - excludes a fixed set of well-known build / cache / tooling
//!   directories (see `BUILTIN_EXCLUDED_DIR_NAMES`);
//! - accepts caller-supplied extra excluded directories (the
//!   resolved build directory, vendor directory, and the manifest
//!   directories of *other* Cabin packages on the workspace so a
//!   walk from package A does not pick up package B's sources);
//! - accepts caller-supplied per-path excludes (the `--exclude`
//!   CLI flag);
//! - returns [`DiscoveredSourceFile`]s sorted by their absolute
//!   path so output is byte-stable across platforms and walks.
//!
//! Only files whose extension matches the recognized C/C++
//! source or header set (`RECOGNIZED_EXTENSIONS`) are returned.

#![deny(missing_docs)]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use thiserror::Error;

/// Input shape for [`discover_sources`].
///
/// The request mirrors the shared CLI surface for `cabin fmt`
/// and `cabin tidy`, but is intentionally agnostic to any one
/// command's semantics.  Callers translate
/// their domain-specific selection into a list of `roots`,
/// resolve their own build / vendor / cache directories into
/// [`SourceDiscoveryRequest::excluded_directories`], and pass
/// any per-path `--exclude` flags through verbatim.
#[derive(Debug, Clone)]
pub struct SourceDiscoveryRequest {
    /// Absolute directories to walk.  Each root is walked
    /// independently and their results are merged and
    /// deduplicated by absolute path.  Empty `roots` returns
    /// an empty result without error.
    pub roots: Vec<PathBuf>,

    /// Absolute paths the caller explicitly asked to exclude.
    /// A directory entry excludes every descendant; a file
    /// entry excludes only that file.  Each entry must be
    /// absolute; a relative entry yields
    /// [`SourceDiscoveryError::ExcludeNotAbsolute`].
    pub excluded_paths: Vec<PathBuf>,

    /// Absolute directories that should be skipped wholesale
    /// (resolved build directory, vendor directory, the
    /// manifest directories of *other* selected packages, …).
    /// Same absolute-path rule as
    /// [`SourceDiscoveryRequest::excluded_paths`].
    pub excluded_directories: Vec<PathBuf>,

    /// When `true` (the default for `cabin fmt`) the walker
    /// honors `.gitignore`, `.ignore`, parent-directory
    /// ignore files, and global git excludes.  When `false`
    /// (the `--no-ignore-vcs` flag) every VCS ignore rule is
    /// disabled but the hard-coded excludes (`.git`, build /
    /// vendor / cache directories, `excluded_paths`) remain in
    /// force.
    pub respect_vcs_ignore: bool,
}

/// A file the walker identified as a C/C++ source or header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSourceFile {
    /// Absolute path of the file.
    pub absolute_path: PathBuf,
}

/// Errors surfaced by the walker.
///
/// The walker bails on the first hard error - e.g. an invalid
/// `excluded_paths` entry - so the orchestration layer can
/// render a single actionable diagnostic instead of a noisy
/// per-entry list.
#[derive(Debug, Error)]
pub enum SourceDiscoveryError {
    /// `excluded_paths` / `excluded_directories` contained a
    /// relative path.  The caller is expected to absolutise
    /// excludes against the package root before invoking the
    /// walker; this error catches a bypass of that rule.
    #[error("exclude path must be absolute: {path}")]
    ExcludeNotAbsolute {
        /// The offending exclude entry, rendered as the caller
        /// supplied it.
        path: String,
    },

    /// `ignore` returned an I/O error walking the tree.  The
    /// underlying error is preserved so callers can render it
    /// verbatim.
    #[error("source discovery failed: {0}")]
    Walk(#[from] ignore::Error),
}

/// Canonicalize exclusion paths into a set, collapsing the
/// platform-specific spellings so a `PathBuf` identity / prefix
/// test matches the walked entries.
fn canonicalize_paths(paths: &[PathBuf]) -> BTreeSet<PathBuf> {
    paths.iter().map(cabin_fs::canonicalize_or_input).collect()
}

/// Discover every recognized C/C++ source or header file
/// under each root, applying ignore / build / cache / vendor /
/// exclusion rules and returning the result sorted by absolute
/// path.
///
/// The walker never traverses symbolic links and never crosses
/// directories named in `BUILTIN_EXCLUDED_DIR_NAMES` (cache,
/// build-system, and `.git` state directories that no
/// developer-tool consumer ever wants to walk).
///
/// # Errors
/// Returns [`SourceDiscoveryError::ExcludeNotAbsolute`] if any
/// `excluded_paths` or `excluded_directories` entry is relative,
/// and propagates [`SourceDiscoveryError::Walk`] (wrapping an
/// [`ignore::Error`]) from the underlying tree walk.
pub fn discover_sources(
    request: &SourceDiscoveryRequest,
) -> Result<Vec<DiscoveredSourceFile>, SourceDiscoveryError> {
    for path in request
        .excluded_paths
        .iter()
        .chain(request.excluded_directories.iter())
    {
        if !path.is_absolute() {
            return Err(SourceDiscoveryError::ExcludeNotAbsolute {
                path: path.display().to_string(),
            });
        }
    }

    // Compare exclusions against a canonical spelling of every path.
    // The walker yields entries under the (already canonical) roots,
    // but caller-supplied excludes are absolutized against the process
    // working directory, which on Windows can carry an 8.3 short name
    // (`RUNNER~1`), a `\\?\` verbatim prefix, or `/` separators that
    // the walked path does not.  Canonicalizing both sides collapses
    // those spellings so a `PathBuf` identity / prefix test matches.
    let excluded_paths = canonicalize_paths(&request.excluded_paths);
    let excluded_dirs = canonicalize_paths(&request.excluded_directories);

    let mut found: BTreeSet<PathBuf> = BTreeSet::new();
    for root in &request.roots {
        walk_root(
            root,
            request.respect_vcs_ignore,
            &excluded_paths,
            &excluded_dirs,
            &mut found,
        )?;
    }

    Ok(found
        .into_iter()
        .map(|absolute_path| DiscoveredSourceFile { absolute_path })
        .collect())
}

fn walk_root(
    root: &Path,
    respect_vcs_ignore: bool,
    excluded_paths: &BTreeSet<PathBuf>,
    excluded_dirs: &BTreeSet<PathBuf>,
    found: &mut BTreeSet<PathBuf>,
) -> Result<(), SourceDiscoveryError> {
    if !root.exists() {
        // A non-existent root is not an error: a workspace
        // member directory may not exist if it was excluded
        // from `[workspace.members]` glob expansion or if a
        // sub-package was removed.  The walker's contract
        // is "return every C/C++ file we can see", not "verify
        // every root exists" - that lives at the orchestration
        // layer where a clearer diagnostic is available.
        return Ok(());
    }

    // The walker's `filter_entry` below is never consulted for the
    // root itself (`ignore` exempts depth 0), so an exclusion that
    // names the root or an ancestor of it must be resolved here or
    // the root's direct-child files would leak into the result.
    let canonical_root = cabin_fs::canonicalize_or_input(root);
    if path_under_any(&canonical_root, excluded_paths)
        || path_under_any(&canonical_root, excluded_dirs)
    {
        return Ok(());
    }

    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(false)
        // Respect hidden-file rules unconditionally - hidden
        // directories like `.git` and `.cache` never carry
        // developer-edited C/C++ source we want to format.
        .hidden(true)
        // Wire in VCS ignore handling only when the caller
        // asked for it.  When disabled, the walker still
        // skips the builtin directory name list below.
        .git_ignore(respect_vcs_ignore)
        .git_exclude(respect_vcs_ignore)
        .git_global(respect_vcs_ignore)
        .ignore(respect_vcs_ignore)
        .parents(respect_vcs_ignore)
        // Deterministic order makes the walk's filter
        // decisions reproducible across platforms.  The final
        // result is sorted by absolute path in `found` regardless,
        // but a stable filter order also keeps diagnostics
        // deterministic.
        .sort_by_file_name(std::ffi::OsStr::cmp);

    // Prune excluded directories instead of walking them and
    // filtering their files afterwards: descending into `build/`,
    // `node_modules/`, or an excluded vendor tree can be arbitrarily
    // expensive, and an unreadable entry inside one would fail the
    // whole walk for files the caller never asked about.  Exclusions
    // are matched against the canonical spelling (see the set
    // construction in `discover_sources`).
    let excluded_paths_filter = excluded_paths.clone();
    let excluded_dirs_filter = excluded_dirs.clone();
    builder.filter_entry(move |entry| {
        if !entry.file_type().is_some_and(|t| t.is_dir()) {
            return true;
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| BUILTIN_EXCLUDED_DIR_NAMES.contains(&name))
        {
            return false;
        }
        if excluded_paths_filter.is_empty() && excluded_dirs_filter.is_empty() {
            return true;
        }
        let canonical = cabin_fs::canonicalize_or_input(entry.path());
        !path_under_any(&canonical, &excluded_paths_filter)
            && !path_under_any(&canonical, &excluded_dirs_filter)
    });

    for entry in builder.build() {
        let entry = entry?;
        let path = entry.path();
        // No file type means the entry refers to the start
        // path itself in some `ignore` configurations.
        let Some(file_type) = entry.file_type() else {
            continue;
        };

        if !file_type.is_file() || !has_recognized_extension(path) {
            continue;
        }
        // Directory exclusions were pruned above; only per-file
        // excludes remain.  Store the raw walked path so returned
        // paths keep the walker's spelling, not the canonical one.
        let canonical = cabin_fs::canonicalize_or_input(path);
        if excluded_paths.contains(&canonical) || excluded_dirs.contains(&canonical) {
            continue;
        }

        found.insert(path.to_path_buf());
    }
    Ok(())
}

fn path_under_any(path: &Path, dirs: &BTreeSet<PathBuf>) -> bool {
    dirs.iter()
        .any(|dir| path == dir.as_path() || path.starts_with(dir))
}

/// Recognized C/C++ source and header extensions.
///
/// - C source: `.c`
/// - C++ source: `.cc`, `.cpp`, `.cxx`, `.c++`, `.C`
/// - C/C++ headers: `.h`, `.hh`, `.hpp`, `.hxx`
///
/// Sources mirror `cabin_core::classify_source` plus the
/// conventional `.c++` / `.C` aliases.  Headers cover the
/// extensions the toolchain treats as C/C++ headers.  The set
/// is deliberately small: unrecognized extensions are *not*
/// formatted, which is the conservative default.
pub(crate) const RECOGNIZED_EXTENSIONS: &[&str] =
    &["c", "cc", "cpp", "cxx", "c++", "C", "h", "hh", "hpp", "hxx"];

fn has_recognized_extension(path: &Path) -> bool {
    // Case-sensitive on the lower-case forms, with the
    // upper-case `.C` accepted for parity with
    // `cabin_core::classify_source` - `.C` is the POSIX
    // convention for a C++ translation unit.
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| RECOGNIZED_EXTENSIONS.contains(&ext))
}

/// Directory names whose contents are *always* excluded from
/// source discovery.  The names are well-known build / cache /
/// VCS state and have no developer-edited C/C++ source we want
/// to format.
///
/// Three groups, all flattened into a single list:
/// - VCS state: `.git`, `.hg`, `.svn`, `.jj`, `.pijul`
/// - Build / output: `build`, `target`, `dist`, `out`, `.cabin`
/// - Third-party caches: `node_modules`, `.venv`, `__pycache__`
///
/// Callers do not need to repeat these names in
/// [`SourceDiscoveryRequest::excluded_directories`]; the walker
/// applies them unconditionally.
pub(crate) const BUILTIN_EXCLUDED_DIR_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".jj",
    ".pijul",
    "build",
    "target",
    "dist",
    "out",
    ".cabin",
    "node_modules",
    ".venv",
    "__pycache__",
];

#[cfg(test)]
mod tests {
    use super::*;

    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    fn relative(root: &Path, files: &[DiscoveredSourceFile]) -> Vec<String> {
        files
            .iter()
            .map(|f| {
                f.absolute_path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    fn request(root: &Path) -> SourceDiscoveryRequest {
        SourceDiscoveryRequest {
            roots: vec![root.to_path_buf()],
            excluded_paths: Vec::new(),
            excluded_directories: Vec::new(),
            respect_vcs_ignore: true,
        }
    }

    #[test]
    fn finds_c_and_cpp_sources_and_headers() {
        let dir = TempDir::new().unwrap();
        for f in [
            "src/main.cc",
            "src/util.cpp",
            "src/legacy.cxx",
            "src/posix.C",
            "src/c_compat.c",
            "include/cabin/api.h",
            "include/cabin/api.hh",
            "include/cabin/api.hpp",
            "include/cabin/api.hxx",
        ] {
            dir.child(f).touch().unwrap();
        }

        let found = discover_sources(&request(dir.path())).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(
            names,
            vec![
                "include/cabin/api.h",
                "include/cabin/api.hh",
                "include/cabin/api.hpp",
                "include/cabin/api.hxx",
                "src/c_compat.c",
                "src/legacy.cxx",
                "src/main.cc",
                "src/posix.C",
                "src/util.cpp",
            ]
        );
    }

    #[test]
    fn ignores_unknown_extensions() {
        let dir = TempDir::new().unwrap();
        dir.child("README.md").touch().unwrap();
        dir.child("src/main.rs").touch().unwrap();
        dir.child("src/data.txt").touch().unwrap();
        dir.child("src/main.cc").touch().unwrap();

        let found = discover_sources(&request(dir.path())).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(names, vec!["src/main.cc"]);
    }

    #[test]
    fn excludes_builtin_directories() {
        let dir = TempDir::new().unwrap();
        dir.child("src/main.cc").touch().unwrap();
        dir.child("build/cache.cc").touch().unwrap();
        dir.child("target/old.cc").touch().unwrap();
        dir.child("dist/staging.cc").touch().unwrap();
        dir.child("node_modules/dep.cc").touch().unwrap();
        dir.child(".git/oid.cc").touch().unwrap();
        dir.child(".cabin/state.cc").touch().unwrap();

        let found = discover_sources(&request(dir.path())).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(names, vec!["src/main.cc"]);
    }

    #[test]
    fn excludes_caller_supplied_directories() {
        let dir = TempDir::new().unwrap();
        dir.child("src/main.cc").touch().unwrap();
        dir.child("vendor/dep/main.cc").touch().unwrap();
        dir.child("third_party/lib/main.cc").touch().unwrap();

        let mut req = request(dir.path());
        req.excluded_directories.push(dir.path().join("vendor"));
        req.excluded_directories
            .push(dir.path().join("third_party"));

        let found = discover_sources(&req).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(names, vec!["src/main.cc"]);
    }

    #[test]
    fn excluded_paths_directory_excludes_descendants() {
        // The documented `--exclude <dir>` contract: a directory
        // entry skips every descendant.
        let dir = TempDir::new().unwrap();
        dir.child("src/main.cc").touch().unwrap();
        dir.child("vendored/dep.cc").touch().unwrap();

        let mut req = request(dir.path());
        req.excluded_paths.push(dir.path().join("vendored"));

        let found = discover_sources(&req).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(names, vec!["src/main.cc"]);
    }

    #[test]
    fn root_equal_to_excluded_directory_yields_nothing() {
        // The walker's `filter_entry` is never consulted for the
        // root itself, so the root-boundary guard must catch an
        // exclusion naming the root - direct-child files must not
        // leak while subdirectories are pruned.
        let dir = TempDir::new().unwrap();
        dir.child("main.cc").touch().unwrap();
        dir.child("sub/other.cc").touch().unwrap();

        let mut req = request(dir.path());
        req.excluded_directories.push(dir.path().to_path_buf());
        assert!(discover_sources(&req).unwrap().is_empty());
    }

    #[test]
    fn root_inside_excluded_paths_directory_yields_nothing() {
        // Same boundary through the `--exclude <dir>` channel, with
        // the root strictly below the excluded directory.
        let dir = TempDir::new().unwrap();
        dir.child("vendored/pkg/main.cc").touch().unwrap();

        let root = dir.path().join("vendored").join("pkg");
        let req = SourceDiscoveryRequest {
            roots: vec![root],
            excluded_paths: vec![dir.path().join("vendored")],
            excluded_directories: Vec::new(),
            respect_vcs_ignore: true,
        };
        assert!(discover_sources(&req).unwrap().is_empty());
    }

    #[test]
    fn root_beneath_builtin_named_directory_is_walked() {
        // Only directories inside the walk are pruned by name: a
        // package that happens to be checked out under a directory
        // named `out` (or `dist`, `build`, ...) must still have its
        // sources discovered.
        let dir = TempDir::new().unwrap();
        dir.child("out/proj/src/main.cc").touch().unwrap();

        let root = dir.path().join("out").join("proj");
        let req = SourceDiscoveryRequest {
            roots: vec![root.clone()],
            excluded_paths: Vec::new(),
            excluded_directories: Vec::new(),
            respect_vcs_ignore: true,
        };
        let found = discover_sources(&req).unwrap();
        let names = relative(&root, &found);
        assert_eq!(names, vec!["src/main.cc"]);
    }

    #[cfg(unix)]
    #[test]
    fn excluded_trees_are_pruned_not_traversed() {
        use std::os::unix::fs::PermissionsExt;

        // Pruning must skip the excluded tree entirely: an
        // unreadable directory inside `build/` would otherwise fail
        // the whole walk for files the caller never asked about.
        let dir = TempDir::new().unwrap();
        dir.child("src/main.cc").touch().unwrap();
        dir.child("build/unreadable/trap.cc").touch().unwrap();
        let unreadable = dir.path().join("build").join("unreadable");
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        let restore =
            || std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o755));
        // Root reads through 0o000; the assertion is only
        // meaningful when the directory really is unreadable.
        if std::fs::read_dir(&unreadable).is_ok() {
            restore().unwrap();
            return;
        }

        let found = discover_sources(&request(dir.path()));
        restore().unwrap();
        let names = relative(dir.path(), &found.unwrap());
        assert_eq!(names, vec!["src/main.cc"]);
    }

    #[test]
    fn excludes_caller_supplied_files() {
        let dir = TempDir::new().unwrap();
        dir.child("src/main.cc").touch().unwrap();
        dir.child("src/skip.cc").touch().unwrap();

        let mut req = request(dir.path());
        req.excluded_paths.push(dir.path().join("src/skip.cc"));

        let found = discover_sources(&req).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(names, vec!["src/main.cc"]);
    }

    #[test]
    fn respects_gitignore_by_default() {
        let dir = TempDir::new().unwrap();
        dir.child(".gitignore")
            .write_str("src/generated.cc\n")
            .unwrap();
        // Make this a git-ish tree so `ignore`'s git-aware
        // search activates without a real `.git` directory.
        dir.child(".git/HEAD")
            .write_str("ref: refs/heads/main\n")
            .unwrap();
        dir.child("src/main.cc").touch().unwrap();
        dir.child("src/generated.cc").touch().unwrap();

        let found = discover_sources(&request(dir.path())).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(names, vec!["src/main.cc"]);
    }

    #[test]
    fn no_ignore_vcs_includes_gitignored_files() {
        let dir = TempDir::new().unwrap();
        dir.child(".gitignore")
            .write_str("src/generated.cc\n")
            .unwrap();
        dir.child(".git/HEAD")
            .write_str("ref: refs/heads/main\n")
            .unwrap();
        dir.child("src/main.cc").touch().unwrap();
        dir.child("src/generated.cc").touch().unwrap();

        let mut req = request(dir.path());
        req.respect_vcs_ignore = false;
        let found = discover_sources(&req).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(names, vec!["src/generated.cc", "src/main.cc"]);
    }

    #[test]
    fn output_is_deterministically_sorted() {
        let dir = TempDir::new().unwrap();
        // Write in a deliberately scrambled order - the walker
        // must still emit ascending paths.
        for f in ["z/last.cc", "a/first.cc", "m/middle.cc"] {
            dir.child(f).touch().unwrap();
        }
        let found = discover_sources(&request(dir.path())).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(names, vec!["a/first.cc", "m/middle.cc", "z/last.cc"]);
    }

    #[test]
    fn relative_exclude_path_is_rejected() {
        let dir = TempDir::new().unwrap();
        let mut req = request(dir.path());
        req.excluded_paths.push(PathBuf::from("src/main.cc"));
        let err = discover_sources(&req).unwrap_err();
        assert!(matches!(
            err,
            SourceDiscoveryError::ExcludeNotAbsolute { .. }
        ));
    }

    #[test]
    fn missing_root_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        let req = SourceDiscoveryRequest {
            roots: vec![dir.path().join("does-not-exist")],
            excluded_paths: Vec::new(),
            excluded_directories: Vec::new(),
            respect_vcs_ignore: true,
        };
        let found = discover_sources(&req).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn multiple_roots_merge_and_dedup() {
        let dir = TempDir::new().unwrap();
        dir.child("a/main.cc").touch().unwrap();
        dir.child("b/main.cc").touch().unwrap();
        let req = SourceDiscoveryRequest {
            roots: vec![
                dir.path().join("a"),
                dir.path().join("b"),
                dir.path().join("a"),
            ],
            excluded_paths: Vec::new(),
            excluded_directories: Vec::new(),
            respect_vcs_ignore: true,
        };
        let found = discover_sources(&req).unwrap();
        let names = relative(dir.path(), &found);
        assert_eq!(names, vec!["a/main.cc", "b/main.cc"]);
    }
}
