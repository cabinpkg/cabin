//! Compiler command-line dialects and their artifact conventions.

use cabin_core::CompilerKind;

/// A compiler command-line family. Selected from the detected C++
/// compiler and threaded through planning and Ninja generation so
/// every artifact name and command line speaks one dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// GCC / Clang. Object files end in `.o`, static libraries are
    /// `lib<name>.a`, executables have no extension, and header
    /// dependencies are tracked with GCC-style depfiles.
    GnuLike,
    /// Microsoft Visual C++ (`cl.exe` / `lib.exe`). Object files end
    /// in `.obj`, static libraries are `<name>.lib`, executables end
    /// in `.exe`, and header dependencies are tracked with
    /// `/showIncludes`.
    Msvc,
}

impl Dialect {
    /// Pick the dialect a compiler family speaks. MSVC drives the
    /// `cl.exe` dialect; every other recognized (or unrecognized)
    /// compiler drives the GCC/Clang dialect, which is also the
    /// safe default for hosts where detection has not run.
    #[must_use]
    pub fn from_compiler_kind(kind: CompilerKind) -> Self {
        match kind {
            CompilerKind::Msvc => Dialect::Msvc,
            CompilerKind::Clang
            | CompilerKind::AppleClang
            | CompilerKind::Gcc
            | CompilerKind::Unknown => Dialect::GnuLike,
        }
    }

    /// Extension (without the leading dot) for compiled object files.
    #[must_use]
    pub fn object_extension(self) -> &'static str {
        match self {
            Dialect::GnuLike => "o",
            Dialect::Msvc => "obj",
        }
    }

    /// File name of the static library built from target `stem`.
    #[must_use]
    pub fn static_library_name(self, stem: &str) -> String {
        match self {
            Dialect::GnuLike => format!("lib{stem}.a"),
            Dialect::Msvc => format!("{stem}.lib"),
        }
    }

    /// File name of the executable built from target `stem`.
    #[must_use]
    pub fn executable_name(self, stem: &str) -> String {
        match self {
            Dialect::GnuLike => stem.to_owned(),
            Dialect::Msvc => format!("{stem}.exe"),
        }
    }

    /// How Ninja should discover header dependencies for compiles in
    /// this dialect.
    #[must_use]
    pub fn ninja_deps(self) -> NinjaDeps {
        match self {
            Dialect::GnuLike => NinjaDeps::Gcc,
            // cl.exe prints each included header to stdout prefixed by
            // this string under `/showIncludes`. The prefix is
            // localized by the compiler UI language; the value below
            // matches an English `cl`, which is what Cabin's CI uses.
            // A mismatch only degrades incremental rebuild precision,
            // never correctness of a clean build.
            Dialect::Msvc => NinjaDeps::Msvc {
                prefix: "Note: including file:",
            },
        }
    }
}

/// Ninja's header-dependency discovery mode for a dialect's compile
/// rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NinjaDeps {
    /// `deps = gcc`, paired with a Makefile `depfile`.
    Gcc,
    /// `deps = msvc`, paired with the `/showIncludes` stdout `prefix`.
    Msvc {
        /// Value for Ninja's `msvc_deps_prefix`.
        prefix: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msvc_compiler_selects_msvc_dialect() {
        assert_eq!(
            Dialect::from_compiler_kind(CompilerKind::Msvc),
            Dialect::Msvc
        );
    }

    #[test]
    fn gcc_clang_unknown_select_gnu_dialect() {
        for kind in [
            CompilerKind::Clang,
            CompilerKind::AppleClang,
            CompilerKind::Gcc,
            CompilerKind::Unknown,
        ] {
            assert_eq!(Dialect::from_compiler_kind(kind), Dialect::GnuLike);
        }
    }

    #[test]
    fn artifact_names_follow_the_dialect() {
        assert_eq!(Dialect::GnuLike.object_extension(), "o");
        assert_eq!(Dialect::Msvc.object_extension(), "obj");

        assert_eq!(Dialect::GnuLike.static_library_name("greet"), "libgreet.a");
        assert_eq!(Dialect::Msvc.static_library_name("greet"), "greet.lib");

        assert_eq!(Dialect::GnuLike.executable_name("app"), "app");
        assert_eq!(Dialect::Msvc.executable_name("app"), "app.exe");
    }

    #[test]
    fn ninja_deps_mode_follows_the_dialect() {
        assert_eq!(Dialect::GnuLike.ninja_deps(), NinjaDeps::Gcc);
        assert_eq!(
            Dialect::Msvc.ninja_deps(),
            NinjaDeps::Msvc {
                prefix: "Note: including file:"
            }
        );
    }
}
