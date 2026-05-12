//! Typed coverage-mode model for C/C++ test builds.
//!
//! Coverage support is intentionally narrow.  Cabin recognises a
//! single typed [`CoverageMode`] (off / on) and selects compiler
//! flags from a closed set keyed on [`CompilerKind`].  The
//! GCC-style `--coverage` shortcut is used for every recognised
//! GCC-compatible and Clang-compatible compiler because:
//!
//! 1. Both compiler families expand `--coverage` to the same
//!    underlying instrumentation (`-fprofile-arcs
//!    -ftest-coverage` on compile, `-lgcov` / profile runtime on
//!    link), so callers do not need to branch.
//! 2. The runtime emits `.gcno` / `.gcda` files next to the
//!    matching object file.  Because the planner already writes
//!    object files under `<build_dir>/<profile>/packages/<pkg>/
//!    <target>/`, coverage data inherits that location and stays
//!    inside the Cabin build directory.
//!
//! No flag is selected for `Msvc` or `Unknown` compiler kinds —
//! callers surface an actionable error instead.  Reporting (HTML
//! / LCOV / llvm-cov UI) is deliberately out of scope.

use serde::{Deserialize, Serialize};

use crate::compiler::CompilerKind;

/// Whether the current build should generate compiler coverage
/// instrumentation.
///
/// `Off` is the default and matches every existing Cabin build /
/// run / test invocation.  `On` is selected by `cabin test
/// --coverage`; it changes the per-package
/// [`crate::ResolvedBuildFlags`] and participates in the
/// [`crate::BuildConfiguration`] fingerprint so coverage-enabled
/// and coverage-disabled builds never share object identity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageMode {
    /// No coverage instrumentation.  Normal `cabin build` /
    /// `cabin run` / `cabin test` behaviour.
    #[default]
    Off,
    /// Coverage instrumentation enabled.
    On,
}

impl CoverageMode {
    /// `true` when coverage instrumentation is enabled.
    pub const fn is_on(self) -> bool {
        matches!(self, CoverageMode::On)
    }

    /// Stable lower-case identifier used in metadata output and
    /// fingerprint bytes.
    pub const fn as_key(self) -> &'static str {
        match self {
            CoverageMode::Off => "off",
            CoverageMode::On => "on",
        }
    }
}

/// Coverage compile- and link-side flag pair selected for a
/// compiler family.
///
/// `compile_args` are appended to every C / C++ compile command
/// in coverage-enabled test builds; `link_args` are appended to
/// the matching link command.  The `--coverage` shortcut populates
/// both sides identically because GCC and Clang accept it at both
/// stages.  The split exists so the build pipeline keeps the two
/// flag spaces separate, matching the existing
/// `extra_compile_args` / `extra_link_args` partition in
/// [`crate::ResolvedBuildFlags`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageFlags {
    /// Compile-side flags.  Appended to both C and C++ compile
    /// commands; this is intentionally language-neutral because
    /// `--coverage` is valid for both translation-unit kinds.
    pub compile_args: Vec<String>,
    /// Link-side flags.  Appended to the link command for any
    /// executable in the coverage-enabled build closure.
    pub link_args: Vec<String>,
}

impl CoverageFlags {
    /// `true` when no flags would be emitted.  Used by call sites
    /// that short-circuit "coverage is on but produced no flags"
    /// branches before merging into a build-flag map.
    pub fn is_empty(&self) -> bool {
        self.compile_args.is_empty() && self.link_args.is_empty()
    }
}

/// Result of picking coverage flags for one resolved compiler
/// kind.  `Unsupported` carries the recognised compiler family so
/// the call site can produce an actionable error without
/// re-deriving the kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageSupport {
    /// The compiler family is recognised and Cabin emits a
    /// well-defined coverage flag set.
    Supported(CoverageFlags),
    /// The compiler family is detected but Cabin does not emit
    /// coverage flags for it.  Includes `Msvc` (the GCC-style
    /// `--coverage` shortcut does not apply) and `Unknown`
    /// (no recognised version output — refuse to guess).
    Unsupported { kind: CompilerKind },
}

/// Pick the coverage flag set for one compiler family.
///
/// GCC, Clang, and Apple-shipped Clang all accept `--coverage`
/// at both compile and link time and route it to the gcov-style
/// runtime; Cabin uses that single shortcut for them.  MSVC
/// (`cl.exe`) does not implement the GCC-style `--coverage`
/// flag, and `CompilerKind::Unknown` represents a compiler whose
/// `--version` output Cabin did not recognise — for either kind
/// the function returns [`CoverageSupport::Unsupported`].
pub fn coverage_flags_for_compiler(kind: CompilerKind) -> CoverageSupport {
    match kind {
        CompilerKind::Clang | CompilerKind::AppleClang | CompilerKind::Gcc => {
            CoverageSupport::Supported(CoverageFlags {
                compile_args: vec!["--coverage".to_owned()],
                link_args: vec!["--coverage".to_owned()],
            })
        }
        CompilerKind::Msvc | CompilerKind::Unknown => CoverageSupport::Unsupported { kind },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_is_the_default() {
        assert_eq!(CoverageMode::default(), CoverageMode::Off);
        assert!(!CoverageMode::default().is_on());
        assert!(CoverageMode::On.is_on());
    }

    #[test]
    fn as_key_is_stable() {
        assert_eq!(CoverageMode::Off.as_key(), "off");
        assert_eq!(CoverageMode::On.as_key(), "on");
    }

    #[test]
    fn gcc_is_supported_with_dash_dash_coverage() {
        match coverage_flags_for_compiler(CompilerKind::Gcc) {
            CoverageSupport::Supported(flags) => {
                assert_eq!(flags.compile_args, vec!["--coverage".to_owned()]);
                assert_eq!(flags.link_args, vec!["--coverage".to_owned()]);
                assert!(!flags.is_empty());
            }
            other => panic!("expected Supported, got {other:?}"),
        }
    }

    #[test]
    fn clang_is_supported_with_dash_dash_coverage() {
        match coverage_flags_for_compiler(CompilerKind::Clang) {
            CoverageSupport::Supported(flags) => {
                assert_eq!(flags.compile_args, vec!["--coverage".to_owned()]);
                assert_eq!(flags.link_args, vec!["--coverage".to_owned()]);
            }
            other => panic!("expected Supported, got {other:?}"),
        }
    }

    #[test]
    fn apple_clang_is_supported_with_dash_dash_coverage() {
        match coverage_flags_for_compiler(CompilerKind::AppleClang) {
            CoverageSupport::Supported(flags) => {
                assert_eq!(flags.compile_args, vec!["--coverage".to_owned()]);
                assert_eq!(flags.link_args, vec!["--coverage".to_owned()]);
            }
            other => panic!("expected Supported, got {other:?}"),
        }
    }

    #[test]
    fn msvc_is_unsupported() {
        match coverage_flags_for_compiler(CompilerKind::Msvc) {
            CoverageSupport::Unsupported { kind } => {
                assert_eq!(kind, CompilerKind::Msvc);
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn unknown_is_unsupported() {
        match coverage_flags_for_compiler(CompilerKind::Unknown) {
            CoverageSupport::Unsupported { kind } => {
                assert_eq!(kind, CompilerKind::Unknown);
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn empty_coverage_flags_is_empty() {
        let f = CoverageFlags::default();
        assert!(f.is_empty());
    }
}
