//! Lexical path-safety predicates used before joining user- or
//! archive-provided relative paths under a trusted root.
//!
//! These helpers do not touch the filesystem and do not
//! canonicalize.  They reason only about the lexical components of
//! the input, so they are safe to call on paths that do not yet
//! exist.  Callers that need archive- or context-specific behavior
//! (skipping GNU/PAX metadata, matching a declared `strip_prefix`,
//! enforcing byte caps) still own that policy themselves; archive
//! extraction in particular continues to own its own rules.

use std::path::{Component, Path};

/// Returns true when `path` is relative and every component is
/// `Normal` or `CurDir`.
///
/// Rejects absolute paths, `..` components, root components, and
/// Windows path prefixes.  The empty path is accepted: a path that
/// has decayed to nothing (for example, an archive entry whose only
/// component was a `strip_prefix` and is now empty) is lexically
/// safe to skip, and callers that distinguish "empty" from "unsafe"
/// can do so explicitly.  Use [`is_non_empty_safe_relative_path`]
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
/// file, such as a port overlay manifest path.  Rejects the empty
/// path in addition to the rejections in [`is_safe_relative_path`].
pub fn is_non_empty_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty() && is_safe_relative_path(path)
}

/// Returns true when `value` is a single, non-empty path component
/// suitable for matching against one segment of a relative path.
///
/// Rejects the empty string, `.`, `..`, and any value containing a
/// `/` or `\`.  Intended for fields like an archive `strip_prefix`
/// that must name exactly one directory level.
pub fn is_safe_single_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

/// Reserved DOS device names.  Windows resolves any path component
/// whose stem (the text before the first `.`) equals one of these,
/// case-insensitively and with or without an extension, to a
/// character device rather than a file under the target: an entry
/// named `NUL` writes to the null device instead of materializing a
/// regular file.  The `COM`/`LPT` families additionally reserve their
/// Unicode superscript-digit forms, listed in
/// [`SUPERSCRIPT_DEVICE_STEMS`].
const DOS_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Superscript digit stems Windows also reserves for `COM`/`LPT`.
const SUPERSCRIPT_DEVICE_STEMS: &[&str] = &[
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// A way a single path component names a different file on some
/// platform Cabin targets than it does lexically here.
///
/// These are the shapes that pass [`is_safe_relative_path`] (they stay
/// inside the destination root) yet alias to a different Win32
/// destination, route to a device, or cannot be materialized on a
/// common filesystem.  Enforcing them on every platform lets `cabin
/// package`, the client extractor, and the hosted-registry verifier
/// judge portability from one definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortabilityViolation {
    /// Contains `\`, a Windows path separator.
    Backslash,
    /// Contains `:`, an NTFS alternate-data-stream or drive separator.
    Colon,
    /// Contains a control character (`U+0000`..=`U+001F` or `U+007F`).
    ControlCharacter,
    /// Contains one of the Win32-forbidden characters `< > " | ? *`.
    WindowsForbiddenCharacter,
    /// Ends with `.`, which Win32 strips (so `a.` and `a` collide).
    TrailingDot,
    /// Ends with a space, which Win32 strips (so `a ` and `a` collide).
    TrailingSpace,
    /// Begins with a space, refused for the same aliasing reason.
    LeadingSpace,
    /// Stem matches a reserved DOS device name (`CON`, `NUL`, `COM1`, …).
    WindowsDeviceName,
}

impl PortabilityViolation {
    /// Short, lower-case, fixed diagnostic text naming the violated
    /// rule.  Callers surface it verbatim - `cabin package` in a
    /// pack-time error, the registry verifier as the parenthesized
    /// reason detail - so the wording is a stable contract.
    pub fn detail(&self) -> &'static str {
        match self {
            PortabilityViolation::Backslash => "backslash",
            PortabilityViolation::Colon => "colon",
            PortabilityViolation::ControlCharacter => "control character",
            PortabilityViolation::WindowsForbiddenCharacter => "windows-forbidden character",
            PortabilityViolation::TrailingDot => "trailing dot",
            PortabilityViolation::TrailingSpace => "trailing space",
            PortabilityViolation::LeadingSpace => "leading space",
            PortabilityViolation::WindowsDeviceName => "windows device name",
        }
    }
}

/// Returns the way `component` names a different file across the
/// platforms Cabin targets, or `None` when it is portable.
///
/// `component` must be a single path segment (no `/` separators): the
/// caller has already split the relative path.  This judges only
/// cross-platform aliasing and reserved names; emptiness, `.`, `..`,
/// separators, and absoluteness stay the caller's concern (see
/// [`is_safe_relative_path`]).
pub fn component_portability(component: &str) -> Option<PortabilityViolation> {
    for ch in component.chars() {
        if ch == '\\' {
            return Some(PortabilityViolation::Backslash);
        }
        if ch == ':' {
            return Some(PortabilityViolation::Colon);
        }
        // `is_ascii_control` is exactly `U+0000`..=`U+001F` and
        // `U+007F` (NUL through unit separator, plus delete).
        if ch.is_ascii_control() {
            return Some(PortabilityViolation::ControlCharacter);
        }
        if matches!(ch, '<' | '>' | '"' | '|' | '?' | '*') {
            return Some(PortabilityViolation::WindowsForbiddenCharacter);
        }
    }
    if component.ends_with('.') {
        return Some(PortabilityViolation::TrailingDot);
    }
    if component.ends_with(' ') {
        return Some(PortabilityViolation::TrailingSpace);
    }
    if component.starts_with(' ') {
        return Some(PortabilityViolation::LeadingSpace);
    }
    // Reserved names match on the stem before the first dot.
    let stem = component.split('.').next().unwrap_or(component);
    if DOS_DEVICE_NAMES
        .iter()
        .chain(SUPERSCRIPT_DEVICE_STEMS)
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return Some(PortabilityViolation::WindowsDeviceName);
    }
    None
}

/// Returns the first portability violation among the `/`-separated
/// components of `path`, or `None` when every component is portable.
///
/// A convenience over [`component_portability`] for callers holding a
/// whole forward-slash relative path (an archive entry name).  Empty
/// segments, `.`, and `..` are not judged here - absoluteness and
/// traversal stay the caller's concern.
pub fn relative_path_portability(path: &str) -> Option<PortabilityViolation> {
    path.split('/').find_map(component_portability)
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
        // `is_non_empty_safe_relative_path` instead.  Archive
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

    #[test]
    fn component_portability_accepts_ordinary_names() {
        for ok in [
            "main.cc",
            "example.h",
            "zlib-1.3.1",
            "a.b.c",
            "CONcept",
            "COMET",
        ] {
            assert_eq!(component_portability(ok), None, "{ok} should be portable");
        }
    }

    #[test]
    fn component_portability_flags_backslash() {
        assert_eq!(
            component_portability(r"a\b"),
            Some(PortabilityViolation::Backslash)
        );
    }

    #[test]
    fn component_portability_flags_colon() {
        assert_eq!(
            component_portability("a:b.h"),
            Some(PortabilityViolation::Colon)
        );
    }

    #[test]
    fn component_portability_flags_control_characters() {
        // NUL, unit separator (0x1F), and DEL (0x7F) all count; a
        // sample of the range stands in for the whole set.
        for ctrl in ['\u{0}', '\u{9}', '\u{1f}', '\u{7f}'] {
            let name = format!("a{ctrl}b");
            assert_eq!(
                component_portability(&name),
                Some(PortabilityViolation::ControlCharacter),
                "{:#x} should be rejected",
                ctrl as u32
            );
        }
    }

    #[test]
    fn component_portability_flags_windows_forbidden_characters() {
        for ch in ['<', '>', '"', '|', '?', '*'] {
            let name = format!("a{ch}b");
            assert_eq!(
                component_portability(&name),
                Some(PortabilityViolation::WindowsForbiddenCharacter),
                "{ch} should be rejected"
            );
        }
    }

    #[test]
    fn component_portability_flags_trailing_dot_and_spaces() {
        assert_eq!(
            component_portability("file."),
            Some(PortabilityViolation::TrailingDot)
        );
        assert_eq!(
            component_portability("file "),
            Some(PortabilityViolation::TrailingSpace)
        );
        assert_eq!(
            component_portability(" file"),
            Some(PortabilityViolation::LeadingSpace)
        );
    }

    #[test]
    fn component_portability_flags_reserved_device_names() {
        // Case-insensitive, with or without an extension, and the
        // superscript `COM`/`LPT` forms.
        for name in ["CON", "nul", "Com1.txt", "LPT9", "COM\u{b9}", "aux.h"] {
            assert_eq!(
                component_portability(name),
                Some(PortabilityViolation::WindowsDeviceName),
                "{name} should be a reserved device name"
            );
        }
    }

    #[test]
    fn detail_texts_are_the_fixed_contract() {
        use PortabilityViolation::*;
        assert_eq!(Backslash.detail(), "backslash");
        assert_eq!(Colon.detail(), "colon");
        assert_eq!(ControlCharacter.detail(), "control character");
        assert_eq!(
            WindowsForbiddenCharacter.detail(),
            "windows-forbidden character"
        );
        assert_eq!(TrailingDot.detail(), "trailing dot");
        assert_eq!(TrailingSpace.detail(), "trailing space");
        assert_eq!(LeadingSpace.detail(), "leading space");
        assert_eq!(WindowsDeviceName.detail(), "windows device name");
    }

    #[test]
    fn relative_path_portability_checks_every_component() {
        assert_eq!(relative_path_portability("src/main.cc"), None);
        assert_eq!(
            relative_path_portability("src/a:b.h"),
            Some(PortabilityViolation::Colon)
        );
        // The first offending component wins.
        assert_eq!(
            relative_path_portability("ok/CON/x.h"),
            Some(PortabilityViolation::WindowsDeviceName)
        );
    }
}
