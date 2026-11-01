//! Compiler command-line dialects and their artifact conventions.

use cabin_core::{AssemblySyntax, CompilerKind};
use camino::{Utf8Path, Utf8PathBuf};

use crate::BuildAction;

/// A compiler command-line family.  Selected from the detected C++
/// compiler and threaded through planning and Ninja generation so
/// every artifact name and command line speaks one dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// GCC / Clang.  Object files end in `.o`, static libraries are
    /// `lib<name>.a`, executables have no extension, and header
    /// dependencies are tracked with GCC-style depfiles.
    GnuLike,
    /// Microsoft Visual C++ (`cl.exe` / `lib.exe`).  Object files end
    /// in `.obj`, static libraries are `<name>.lib`, executables end
    /// in `.exe`, and header dependencies are tracked with
    /// `/showIncludes`.
    Msvc,
}

impl Dialect {
    /// Pick the dialect a compiler family speaks.  MSVC-dialect
    /// compilers (`cl.exe` and Clang's `clang-cl` driver) drive the
    /// MSVC dialect; every other recognized (or unrecognized)
    /// compiler drives the GCC/Clang dialect.
    #[must_use]
    pub fn from_compiler_kind(kind: CompilerKind) -> Self {
        if kind.speaks_msvc_dialect() {
            Dialect::Msvc
        } else {
            Dialect::GnuLike
        }
    }

    /// The dialect to assume for the host when compiler detection has
    /// not run or failed, as in `cabin tidy`: Windows defaults to MSVC,
    /// every other host to GCC/Clang, matching the default toolchain
    /// each host resolves.
    #[must_use]
    pub fn host_default() -> Self {
        if cfg!(windows) {
            Dialect::Msvc
        } else {
            Dialect::GnuLike
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

    /// The assembler for `syntax` sources given the C++ compiler the
    /// dialect was detected from, or `None` when that toolchain cannot
    /// assemble the syntax.  The GCC/Clang driver assembles GNU `.S`
    /// itself; MASM goes through the `ml64.exe` Visual Studio installs
    /// beside `cl.exe` (and not beside `clang-cl`).
    #[must_use]
    pub fn assembler(self, syntax: AssemblySyntax, compiler: &Utf8Path) -> Option<Utf8PathBuf> {
        match (self, syntax) {
            (Dialect::GnuLike, AssemblySyntax::Gnu) => Some(compiler.to_path_buf()),
            // shortcut: x64 MSVC only - an arm64 host needs `armasm64`
            // and its own argv once Cabin builds there.
            (Dialect::Msvc, AssemblySyntax::Masm)
                if compiler
                    .file_stem()
                    .is_some_and(|stem| stem.eq_ignore_ascii_case("cl")) =>
            {
                Some(compiler.with_file_name("ml64.exe"))
            }
            _ => None,
        }
    }

    /// How Ninja should discover what an assembly source includes, or
    /// `None` when the assembler cannot report it (`ml64`).
    #[must_use]
    pub fn assemble_deps(self) -> Option<NinjaDeps> {
        match self {
            Dialect::GnuLike => Some(NinjaDeps::Gcc),
            Dialect::Msvc => None,
        }
    }

    /// Whether the archiver updates an existing library in place, so the
    /// previous build's archive must be removed first: `ar crs` adds and
    /// replaces members but never drops one, which would keep the object
    /// of a source removed or renamed since; `lib /OUT:` always writes the
    /// library from scratch.
    #[must_use]
    pub fn archive_updates_in_place(self) -> bool {
        matches!(self, Dialect::GnuLike)
    }

    /// How Ninja should discover header dependencies for compiles in
    /// this dialect.
    #[must_use]
    pub fn ninja_deps(self) -> NinjaDeps {
        match self {
            Dialect::GnuLike => NinjaDeps::Gcc,
            // cl.exe prints each included header to stdout prefixed by
            // this string under `/showIncludes`, in the compiler UI
            // language; the Ninja run sets `VSLANG=1033` to get English.
            // On a mismatch Ninja records no header dependencies, so an
            // incremental build misses header edits; a clean build is
            // unaffected.
            Dialect::Msvc => NinjaDeps::Msvc {
                prefix: "Note: including file:",
            },
        }
    }

    /// What a response file this dialect's archiver and linker read
    /// starts with: MSVC tools read one as UTF-8 only after a byte order
    /// mark.
    #[must_use]
    pub fn response_file_header(self) -> &'static str {
        match self {
            Dialect::GnuLike => "",
            Dialect::Msvc => "\u{feff}",
        }
    }

    /// Whether this dialect's archiver and linker need a response file's
    /// inputs on lines of their own: MSVC tools reject a line of 128 KiB
    /// or more (LNK1170), so a library of many objects cannot list them
    /// on one.  GCC, Clang, and `ar` read lines of any length.
    #[must_use]
    pub fn response_file_input_lines(self) -> bool {
        self == Dialect::Msvc
    }

    /// `args` spelled for a response file that `action`'s archiver or
    /// linker reads in their place, separated by spaces.  GCC, Clang,
    /// and `ar` split a response file at whitespace and honor quotes and
    /// backslash escapes, so every argument is spelled with each
    /// whitespace, quote, and backslash character backslash-escaped.
    /// MSVC tools split it like a command line, and so does `llvm-ar` 11
    /// or later on a Windows host: it follows the host's rules, where the
    /// Clang driver keeps GNU rules everywhere.  Neither form can carry a
    /// NUL.
    #[must_use]
    pub fn response_file_args(
        self,
        action: &BuildAction,
        windows_host: bool,
        args: &[String],
    ) -> String {
        let windows = self == Dialect::Msvc || self.llvm_ar_windows_quoting(action, windows_host);
        let mut out = String::new();
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            if windows {
                windows_quote_arg(arg, &mut out);
                continue;
            }
            if arg.is_empty() {
                out.push_str("''");
            }
            for c in arg.chars() {
                if c.is_ascii_whitespace() || matches!(c, '\x0b' | '\'' | '"' | '\\') {
                    out.push('\\');
                }
                out.push(c);
            }
        }
        out
    }

    /// The option that goes before `@file` when `action`'s tool reads
    /// its arguments from a response file.  An `llvm-ar` given Windows
    /// quoting is told so explicitly: one whose version Cabin could not
    /// read is assumed to be LLVM 11 or later, and an older one, which
    /// knows no `--rsp-quoting`, then fails on the option instead of
    /// splitting the file with GNU rules.
    #[must_use]
    pub fn response_file_option(
        self,
        action: &BuildAction,
        windows_host: bool,
    ) -> Option<&'static str> {
        self.llvm_ar_windows_quoting(action, windows_host)
            .then_some("--rsp-quoting=windows")
    }

    fn llvm_ar_windows_quoting(self, action: &BuildAction, windows_host: bool) -> bool {
        self == Dialect::GnuLike
            && windows_host
            && matches!(action, BuildAction::Archive(archive) if archive.windows_rsp_quoting)
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

/// Append one argument to `out` using `CommandLineToArgvW` quoting.
///
/// An argument is emitted verbatim unless it is empty or contains a
/// space, tab, or double-quote - so backslash-laden Windows paths like
/// `/FoC:\build\main.obj` pass through untouched, while
/// `C:\Program Files\…\cl.exe` is wrapped in double quotes.  Inside a
/// quoted argument, a run of N backslashes is doubled to 2N when it
/// precedes the closing quote, and becomes 2N+1 followed by the quote
/// when it precedes an embedded double-quote; elsewhere backslashes are
/// literal.
pub fn windows_quote_arg(arg: &str, out: &mut String) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // These backslashes precede a quote: escape each, then
                // the quote itself.
                for _ in 0..=backslashes * 2 {
                    out.push('\\');
                }
                out.push('"');
                backslashes = 0;
            }
            other => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push(other);
            }
        }
    }
    // Trailing backslashes precede the closing quote, so double them.
    for _ in 0..backslashes * 2 {
        out.push('\\');
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ArchiveAction;

    #[test]
    fn an_archiver_with_windows_quoting_reads_a_response_file_like_a_command_line_on_windows() {
        let archive = |windows_rsp_quoting| {
            BuildAction::Archive(ArchiveAction {
                archiver: "llvm-ar".into(),
                windows_rsp_quoting,
                output: "lib.a".into(),
                inputs: vec![],
                description: String::new(),
            })
        };
        let args = ["crs".to_owned(), r"C:\a b\x.o".to_owned()];
        let gnu = r"crs C:\\a\ b\\x.o";
        let windows = r#"crs "C:\a b\x.o""#;
        for (windows_rsp_quoting, windows_host, expected, option) in [
            (true, true, windows, Some("--rsp-quoting=windows")),
            (true, false, gnu, None),
            (false, true, gnu, None),
        ] {
            let context = format!(
                "Windows quoting: {windows_rsp_quoting}, on a Windows host: {windows_host}"
            );
            let archive = archive(windows_rsp_quoting);
            assert_eq!(
                Dialect::GnuLike.response_file_args(&archive, windows_host, &args),
                expected,
                "{context}"
            );
            assert_eq!(
                Dialect::GnuLike.response_file_option(&archive, windows_host),
                option,
                "{context}"
            );
        }
        // `lib.exe` reads its response file like a command line on its own.
        assert_eq!(
            Dialect::Msvc.response_file_option(&archive(true), true),
            None
        );
    }

    #[test]
    fn msvc_compiler_selects_msvc_dialect() {
        assert_eq!(
            Dialect::from_compiler_kind(CompilerKind::Msvc),
            Dialect::Msvc
        );
    }

    #[test]
    fn clang_cl_selects_msvc_dialect() {
        // `clang-cl` is Clang under the hood but speaks the MSVC
        // command line, so it must drive the MSVC dialect.
        assert_eq!(
            Dialect::from_compiler_kind(CompilerKind::ClangCl),
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

    #[test]
    fn windows_quote_doubles_trailing_backslashes_before_closing_quote() {
        // A trailing backslash inside a quoted arg would escape the
        // closing quote, so a run of N is doubled to 2N.
        let mut out = String::new();
        windows_quote_arg(r"C:\dir with space\", &mut out);
        assert_eq!(out, "\"C:\\dir with space\\\\\"");
    }

    #[test]
    fn windows_quote_escapes_embedded_quote_and_its_backslashes() {
        // An embedded `"` and the run of N backslashes before it become
        // 2N+1 backslashes and the quote.
        let mut zero = String::new();
        windows_quote_arg(r#"a "b"#, &mut zero); // no backslash before the quote
        assert_eq!(zero, "\"a \\\"b\"");

        let mut one = String::new();
        windows_quote_arg(r#"a\"b"#, &mut one); // one backslash before the quote
        assert_eq!(one, "\"a\\\\\\\"b\"");
    }
}
