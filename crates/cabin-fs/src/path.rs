//! Lexical path-safety predicates used before joining user- or
//! archive-provided relative paths under a trusted root.
//!
//! These helpers do not touch the filesystem and do not
//! canonicalize. They reason only about the lexical components of
//! the input, so they are safe to call on paths that do not yet
//! exist. Callers that need archive- or context-specific behaviour
//! (skipping GNU/PAX metadata, matching a declared `strip_prefix`,
//! enforcing byte caps) still own that policy themselves; archive
//! extraction in particular continues to own its own rules.

use std::path::{Component, Path};

/// Returns true when `path` is relative and every component is
/// `Normal` or `CurDir`.
///
/// Rejects absolute paths, `..` components, root components, and
/// Windows path prefixes. The empty path is accepted: a path that
/// has decayed to nothing (for example, an archive entry whose only
/// component was a `strip_prefix` and is now empty) is lexically
/// safe to skip, and callers that distinguish "empty" from "unsafe"
/// can do so explicitly. Use [`is_non_empty_safe_relative_path`]
/// when the caller requires the path to name an actual file.
pub fn is_safe_relative_path(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    path.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Returns true when `path` is non-empty, relative, and every
/// component is `Normal` or `CurDir`.
///
/// Intended for user-authored relative paths that must name a
/// file, such as a port overlay manifest path. Rejects the empty
/// path in addition to the rejections in [`is_safe_relative_path`].
pub fn is_non_empty_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty() && is_safe_relative_path(path)
}

/// Returns true when `value` is a single, non-empty path component
/// suitable for matching against one segment of a relative path.
///
/// Rejects the empty string, `.`, `..`, and any value containing a
/// `/` or `\`. Intended for fields like an archive `strip_prefix`
/// that must name exactly one directory level.
pub fn is_safe_single_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_relative_path_accepts_simple_name() {
        assert!(is_safe_relative_path(Path::new("foo")));
    }

    #[test]
    fn safe_relative_path_accepts_nested_name() {
        assert!(is_safe_relative_path(Path::new("foo/bar")));
    }

    #[test]
    fn safe_relative_path_accepts_leading_curdir() {
        assert!(is_safe_relative_path(Path::new("./foo")));
    }

    #[test]
    fn safe_relative_path_accepts_empty_path() {
        // The empty path carries no escape risk on its own; callers
        // that need a named file should use
        // `is_non_empty_safe_relative_path` instead. Archive
        // extraction relies on the empty case being safe so that an
        // entry that decays to nothing after `strip_prefix` can be
        // skipped rather than rejected.
        assert!(is_safe_relative_path(Path::new("")));
    }

    #[test]
    fn safe_relative_path_rejects_unix_absolute() {
        assert!(!is_safe_relative_path(Path::new("/etc/passwd")));
    }

    #[test]
    fn safe_relative_path_rejects_parent_component() {
        assert!(!is_safe_relative_path(Path::new("../escape")));
    }

    #[test]
    fn safe_relative_path_rejects_embedded_parent_component() {
        assert!(!is_safe_relative_path(Path::new("foo/../escape")));
    }

    #[cfg(windows)]
    #[test]
    fn safe_relative_path_rejects_windows_drive_prefix() {
        assert!(!is_safe_relative_path(Path::new(r"C:\foo")));
    }

    #[cfg(windows)]
    #[test]
    fn safe_relative_path_rejects_windows_unc_prefix() {
        assert!(!is_safe_relative_path(Path::new(r"\\server\share\foo")));
    }

    #[test]
    fn non_empty_safe_relative_path_accepts_simple_name() {
        assert!(is_non_empty_safe_relative_path(Path::new("foo")));
    }

    #[test]
    fn non_empty_safe_relative_path_accepts_nested_name() {
        assert!(is_non_empty_safe_relative_path(Path::new("foo/bar")));
    }

    #[test]
    fn non_empty_safe_relative_path_rejects_empty_path() {
        assert!(!is_non_empty_safe_relative_path(Path::new("")));
    }

    #[test]
    fn non_empty_safe_relative_path_rejects_unix_absolute() {
        assert!(!is_non_empty_safe_relative_path(Path::new("/etc/passwd")));
    }

    #[test]
    fn non_empty_safe_relative_path_rejects_parent_component() {
        assert!(!is_non_empty_safe_relative_path(Path::new("../escape")));
    }

    #[test]
    fn safe_single_component_accepts_simple_name() {
        assert!(is_safe_single_component("prefix"));
    }

    #[test]
    fn safe_single_component_accepts_name_with_dashes_and_digits() {
        assert!(is_safe_single_component("zlib-1.3.1"));
    }

    #[test]
    fn safe_single_component_rejects_empty() {
        assert!(!is_safe_single_component(""));
    }

    #[test]
    fn safe_single_component_rejects_curdir() {
        assert!(!is_safe_single_component("."));
    }

    #[test]
    fn safe_single_component_rejects_parent() {
        assert!(!is_safe_single_component(".."));
    }

    #[test]
    fn safe_single_component_rejects_forward_slash() {
        assert!(!is_safe_single_component("a/b"));
    }

    #[test]
    fn safe_single_component_rejects_backslash() {
        assert!(!is_safe_single_component(r"a\b"));
    }

    #[test]
    fn safe_single_component_rejects_trailing_slash() {
        assert!(!is_safe_single_component("prefix/"));
    }
}
