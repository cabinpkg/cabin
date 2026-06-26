//! Source-file language classification.
//!
//! Cabin treats C/C++ as related but distinct source
//! languages.  The build planner consults this module to decide
//! which compiler driver and which standard to use for each
//! source file in a `library` / `executable` / `test` / `example`
//! target.  The same target may carry both `.c` and `.cc` sources;
//! classification is per-file.
//!
//! This module is data and pure logic only.  Filesystem traversal
//! and process spawning live elsewhere.
//!
//! ## Recognized extensions
//!
//! | Extension | Language |
//! | ---------------------------------- | -------- |
//! | `.c` | [`SourceLanguage::C`] |
//! | `.cc`, `.cpp`, `.cxx`, `.c++`, `.C` | [`SourceLanguage::Cxx`] |
//!
//! Headers (`.h`, `.hh`, `.hpp`) are not classified here - they
//! are not compiled as standalone translation units.  Anything
//! outside the table above returns `None` so callers can surface
//! a clear "unrecognized source extension" diagnostic instead of
//! silently picking the wrong compiler.

use camino::Utf8Path;

/// Source-file language as observed by the build planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceLanguage {
    /// A C translation unit (`.c`).
    C,
    /// A C++ translation unit (`.cc`, `.cpp`, `.cxx`, `.c++`,
    /// `.C`).
    Cxx,
}

impl SourceLanguage {
    /// Stable lower-case identifier suitable for diagnostics,
    /// JSON output, and rule names. `c` for C and `cxx` for C++ -
    /// matching the [`crate::ToolKind`] keys.
    pub const fn as_key(self) -> &'static str {
        match self {
            Self::C => "c",
            Self::Cxx => "cxx",
        }
    }

    /// Human-readable label used in error messages so the
    /// language is unambiguous to the user.
    pub const fn human_label(self) -> &'static str {
        match self {
            Self::C => "C",
            Self::Cxx => "C++",
        }
    }
}

impl std::fmt::Display for SourceLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_key())
    }
}

/// Classify a source file by its filename extension.  Returns
/// `None` when the extension is missing or unrecognized - the
/// planner surfaces an explicit diagnostic in that case rather
/// than silently picking a default compiler.
///
/// Extension matching is case-sensitive on the lower-case forms
/// (`.c`, `.cc`, `.cpp`, `.cxx`, `.c++`) and accepts the
/// upper-case `.C` extension that traditionally indicates a C++
/// translation unit on POSIX systems.
pub fn classify_source(path: &Utf8Path) -> Option<SourceLanguage> {
    // We deliberately do not lower-case the extension: `.C` is
    // the only non-lower-case spelling Cabin recognizes (POSIX
    // C++ convention), and matching it explicitly avoids
    // collapsing `.C` and `.c` into the same bucket on
    // case-insensitive filesystems.
    let ext = path.extension()?;
    match ext {
        "c" => Some(SourceLanguage::C),
        "cc" | "cpp" | "cxx" | "c++" | "C" => Some(SourceLanguage::Cxx),
        _ => None,
    }
}

/// Pick the link-driver language for a target whose objects
/// span the supplied set of source languages.
///
/// **Rule:** if any object came from a C++ source (or any
/// transitively linked library declares any C++ object), the
/// link driver is the C++ compiler.  Otherwise the C compiler
/// drives the link.  The C++ driver pulls in the C++ runtime
/// (`libstdc++` / `libc++`), which is required for any
/// translation unit that uses C++; the C driver omits that
/// runtime, which is correct for pure-C link lines.
///
/// Returns [`SourceLanguage::C`] for an empty input - that is
/// the conservative choice for an empty link line, but in
/// practice the planner rejects executables with no objects
/// before this is consulted.
///
/// The slice form (rather than a generic `IntoIterator`) keeps
/// the predicate cheap to call on the per-target language
/// manifests the planner already collects, and lets callers
/// reason about the input by reading the call site directly.
pub fn link_driver_language(languages: &[SourceLanguage]) -> SourceLanguage {
    if languages.contains(&SourceLanguage::Cxx) {
        SourceLanguage::Cxx
    } else {
        SourceLanguage::C
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    #[test]
    fn classifies_c_extension_as_c() {
        assert_eq!(
            classify_source(&Utf8PathBuf::from("foo.c")),
            Some(SourceLanguage::C)
        );
        assert_eq!(
            classify_source(&Utf8PathBuf::from("src/lib.c")),
            Some(SourceLanguage::C)
        );
    }

    #[test]
    fn classifies_cpp_extensions_as_cxx() {
        for ext in ["cc", "cpp", "cxx", "c++", "C"] {
            let path = Utf8PathBuf::from(format!("src/file.{ext}"));
            assert_eq!(
                classify_source(&path),
                Some(SourceLanguage::Cxx),
                "extension `.{ext}` must classify as C++"
            );
        }
    }

    #[test]
    fn classification_is_case_sensitive_for_lower_case_only() {
        // `.C` is the legitimate POSIX upper-case C++ extension;
        // anything else upper-cased is unrecognized so the
        // planner can surface a clear error instead of guessing.
        assert_eq!(
            classify_source(&Utf8PathBuf::from("file.C")),
            Some(SourceLanguage::Cxx)
        );
        assert!(classify_source(&Utf8PathBuf::from("file.CPP")).is_none());
    }

    #[test]
    fn classification_returns_none_for_unknown_or_missing_extension() {
        assert!(classify_source(&Utf8PathBuf::from("file")).is_none());
        assert!(classify_source(&Utf8PathBuf::from("file.h")).is_none());
        assert!(classify_source(&Utf8PathBuf::from("file.hpp")).is_none());
        assert!(classify_source(&Utf8PathBuf::from("file.txt")).is_none());
    }

    #[test]
    fn link_driver_is_cxx_when_any_source_is_cpp() {
        assert_eq!(
            link_driver_language(&[SourceLanguage::Cxx]),
            SourceLanguage::Cxx
        );
        assert_eq!(
            link_driver_language(&[SourceLanguage::C, SourceLanguage::Cxx]),
            SourceLanguage::Cxx
        );
        assert_eq!(
            link_driver_language(&[SourceLanguage::Cxx, SourceLanguage::C]),
            SourceLanguage::Cxx
        );
    }

    #[test]
    fn link_driver_is_c_when_every_source_is_c() {
        assert_eq!(
            link_driver_language(&[SourceLanguage::C]),
            SourceLanguage::C
        );
        assert_eq!(
            link_driver_language(&[SourceLanguage::C, SourceLanguage::C]),
            SourceLanguage::C
        );
    }

    #[test]
    fn link_driver_falls_back_to_c_for_empty_input() {
        // Empty inputs do not occur in practice (the planner
        // rejects empty targets up-front); the documented
        // fallback is C so a future caller cannot accidentally
        // depend on the C++ driver being selected for an empty
        // link line.
        assert_eq!(link_driver_language(&[]), SourceLanguage::C);
    }

    #[test]
    fn keys_are_stable_across_renames() {
        // The keys land in JSON metadata and rule names; lock
        // them down so a future contributor cannot rename the
        // variant accidentally.
        assert_eq!(SourceLanguage::C.as_key(), "c");
        assert_eq!(SourceLanguage::Cxx.as_key(), "cxx");
        assert_eq!(SourceLanguage::C.to_string(), "c");
        assert_eq!(SourceLanguage::Cxx.to_string(), "cxx");
    }
}
