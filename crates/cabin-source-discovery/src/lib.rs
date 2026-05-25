//! Reusable C and C++ source / header discovery for Cabin
//! developer tools.
//!
//! The current consumers are `cabin fmt` and `cabin tidy`.
//! The interface stays narrow so each command can share the
//! same walker, exclusion policy, and deterministic ordering
//! without re-implementing any of it.
//!
//! The walker:
//!
//! - honours VCS ignore rules (`.gitignore`, `.ignore`) by default
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
//! Only files whose extension matches the recognised
//! C / C++ source or header set are returned. The accepted set
//! mirrors the existing classifier in `cabin-core` for sources
//! (`.c`, `.cc`, `.cpp`, `.cxx`, `.c++`, `.C`) and adds the
//! conventional header extensions (`.h`, `.hh`, `.hpp`, `.hxx`).

#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

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
    /// honours `.gitignore`, `.ignore`, parent-directory
    /// ignore files, and global git excludes.  When `false`
    /// (the `--no-ignore-vcs` flag) every VCS ignore rule is
    /// disabled but the hard-coded excludes (`.git`, build /
    /// vendor / cache directories, `excluded_paths`) remain in
    /// force.
    pub respect_vcs_ignore: bool,
}

/// A file the walker identified as a C / C++ source or header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSourceFile {
    /// Absolute path of the file.
    pub absolute_path: PathBuf,
}

/// Errors surfaced by the walker.
///
/// The walker bails on the first hard error — e.g. an invalid
/// `excluded_paths` entry — so the orchestration layer can
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

/// Discover every recognised C / C++ source or header file
/// under each root, applying ignore / build / cache / vendor /
/// exclusion rules and returning the result sorted by absolute
/// path.
///
/// The walker never traverses symbolic links and never crosses
/// directories named in `BUILTIN_EXCLUDED_DIR_NAMES` (cache,
/// build-system, and `.git` state directories that no
/// developer-tool consumer ever wants to walk).
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

    let excluded_paths: BTreeSet<PathBuf> = request.excluded_paths.iter().cloned().collect();
    let excluded_dirs: BTreeSet<PathBuf> = request.excluded_directories.iter().cloned().collect();

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
        // sub-package was just removed.  The walker's contract
        // is "return every C/C++ file we can see", not "verify
        // every root exists" — that lives at the orchestration
        // layer where a clearer diagnostic is available.
        return Ok(());
    }

    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(false)
        // Respect hidden-file rules unconditionally — hidden
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

    for entry in builder.build() {
        let entry = entry?;
        let path = entry.path();
        // No file type means the entry refers to the start
        // path itself in some `ignore` configurations.
        let Some(file_type) = entry.file_type() else {
            continue;
        };

        if !file_type.is_file() || !has_recognised_extension(path) {
            continue;
        }
        if excluded_paths.contains(path)
            || path_under_any(path, excluded_dirs)
            || path_under_any_builtin_name(path)
        {
            continue;
        }

        found.insert(path.to_path_buf());
    }
    Ok(())
}

fn path_under_any(path: &Path, dirs: &BTreeSet<PathBuf>) -> bool {
    dirs.iter().any(|dir| {
        // Direct match (the path *is* the excluded entry) or
        // strict-prefix match (the path is a descendant).
        path == dir.as_path() || path.starts_with(dir)
    })
}

fn path_under_any_builtin_name(path: &Path) -> bool {
    path.ancestors().any(|ancestor| {
        ancestor
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| BUILTIN_EXCLUDED_DIR_NAMES.contains(&n))
    })
}

/// Recognised C / C++ source and header extensions.
///
/// - C source: `.c`
/// - C++ source: `.cc`, `.cpp`, `.cxx`, `.c++`, `.C`
/// - C / C++ headers: `.h`, `.hh`, `.hpp`, `.hxx`
///
/// Sources mirror `cabin_core::classify_source` plus the
/// conventional `.c++` / `.C` aliases.  Headers cover the
/// extensions the toolchain treats as C/C++ headers.  The set
/// is deliberately small: unrecognised extensions are *not*
/// formatted, which is the conservative default.
pub(crate) const RECOGNISED_EXTENSIONS: &[&str] =
    &["c", "cc", "cpp", "cxx", "c++", "C", "h", "hh", "hpp", "hxx"];

fn has_recognised_extension(path: &Path) -> bool {
    // Case-sensitive on the lower-case forms, with the
    // upper-case `.C` accepted for parity with
    // `cabin_core::classify_source` — `.C` is the POSIX
    // convention for a C++ translation unit.
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| RECOGNISED_EXTENSIONS.contains(&ext))
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
        // Write in a deliberately scrambled order — the walker
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
