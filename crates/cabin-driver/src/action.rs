//! Semantic build-action IR.
//!
//! The planner emits these toolchain-independent action specs; the
//! [`crate::lower()`] layer turns them into concrete command argv for a
//! specific [`crate::Dialect`].  Nothing here names a compiler flag:
//! the IR records *intent* (optimization level, debug info, defines,
//! include directories, the source language) and each dialect spells
//! it (`-O2` vs `/O2`, `-c` vs `/c`, …).  A new dialect is added in
//! [`crate::lower()`] without this IR changing.

use camino::Utf8PathBuf;

use cabin_core::{LanguageStandard, OptLevel};

/// A single semantic build step: compile a translation unit, archive
/// objects into a static library, or link an executable.
///
/// Backend- and toolchain-independent: the concrete command argv is
/// produced later by [`crate::lower()`], not stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildAction {
    /// Compile (or syntax-check) one C/C++ translation unit.
    Compile(CompileAction),
    /// Archive object files into a static library.
    Archive(ArchiveAction),
    /// Link object files and static archives into an executable.
    Link(LinkAction),
}

/// What a compile action should produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileMode {
    /// Normal compile: emit the object file at [`CompileAction::object`].
    Object,
    /// Syntax/semantic check only (`cabin check`): emit no object;
    /// `stamp` is `touch`ed to record a successful check.  The
    /// `object` path is retained on the action so the stamp lives
    /// beside it and the workspace-scope filter in `cabin check` can
    /// match on it.
    SyntaxOnly {
        /// Stamp file written on a successful check, in place of the
        /// object.
        stamp: Utf8PathBuf,
    },
}

/// Compile one translation unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileAction {
    /// Effective language standard for this translation unit.  Also
    /// determines the source language (`standard.language()`), which
    /// selects the rule / lowered action kind, `/EHsc`, and the
    /// `/Tp` / `/Tc` source-language flag on MSVC.
    pub standard: LanguageStandard,
    /// Absolute path of the translation unit to compile.
    pub source: Utf8PathBuf,
    /// Object file the normal build produces.  Retained even in
    /// [`CompileMode::SyntaxOnly`] so the stamp lives beside it and
    /// the workspace-scope filter in `cabin check` can match on it.
    pub object: Utf8PathBuf,
    /// Object vs. syntax-only.
    pub mode: CompileMode,
    /// Inputs the compile depends on but that are not command
    /// arguments (e.g. a generated source produced upstream).  The
    /// `source` is the sole compiled input and is not repeated here.
    pub implicit_inputs: Vec<Utf8PathBuf>,
    /// Header-dependency tracking file.  `Some` records that the
    /// dialect should wire dependency discovery for this compile -
    /// the GNU/Clang lowering emits a `-MD -MF <depfile>` Makefile
    /// depfile here (`-MD`, not `-MMD`, so headers found through a
    /// system include dir still land in the depfile and keep
    /// invalidating rebuilds), while the MSVC lowering ignores the
    /// path and relies on `/showIncludes`.
    pub depfile: Option<Utf8PathBuf>,
    /// Compiler driver executable.
    pub compiler: Utf8PathBuf,
    /// Optional compiler wrapper (e.g. `ccache`) prepended to
    /// the *run* command by lowering.  Never affects
    /// `compile_commands.json`, which records the underlying compiler
    /// so IDE tooling sees the real driver.
    pub compiler_wrapper: Option<Utf8PathBuf>,
    /// Semantic compile arguments (optimization, defines, includes).
    pub arguments: CompileArguments,
    /// Human-readable description for build output (`CXX foo.o`,
    /// `CHECK foo.o`).
    pub description: String,
}

/// Semantic arguments for a compile, with no flag spelled out.
///
/// The language standard lives on [`CompileAction::standard`]; each
/// dialect spells it (`-std=c++20` vs `/std:c++20`).  The
/// optimization / debug / assertion intent comes from the resolved
/// profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileArguments {
    /// Optimization level the active profile selected (`-O0` / `/Od`,
    /// …).
    pub opt_level: OptLevel,
    /// Emit debug information (`-g` / `/Zi`).
    pub debug_info: bool,
    /// Define `NDEBUG` (assertions disabled in the active profile).
    pub define_ndebug: bool,
    /// Include search directories.  Spelled `-I <dir>` / `/I <dir>`.
    pub include_dirs: Vec<Utf8PathBuf>,
    /// Include search directories marked as *system* search paths,
    /// so diagnostics inside their headers are suppressed.  Searched
    /// after [`Self::include_dirs`].  The planner routes third-party
    /// contributions here (registry packages, foundation ports,
    /// pkg-config system dependencies).  Spelled `-isystem <dir>` for
    /// GNU/Clang; `/external:W0 /external:I <dir>` for MSVC (the
    /// planner only populates this on MSVC-dialect builds when the
    /// detected compiler supports the `/external:` block).
    pub system_include_dirs: Vec<Utf8PathBuf>,
    /// Preprocessor defines, without any prefix.  Spelled `-D<define>`
    /// / `/D<define>`.
    pub defines: Vec<String>,
    /// Escape-hatch compile flags appended verbatim after the
    /// include block.  The user writes these in the active dialect
    /// (language-neutral first, then language-specific).
    pub extra_flags: Vec<String>,
}

/// Archive object files into a static library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveAction {
    /// Archiver executable (`ar` / `lib`).
    pub archiver: Utf8PathBuf,
    /// Static library to produce.
    pub output: Utf8PathBuf,
    /// Object files to archive, in order.
    pub inputs: Vec<Utf8PathBuf>,
    /// Human-readable description (`AR libfoo.a`).
    pub description: String,
}

/// Link objects and static archives into an executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkAction {
    /// Link-driver executable (the C or C++ compiler, chosen per
    /// target by the planner).
    pub linker: Utf8PathBuf,
    /// Executable to produce.
    pub output: Utf8PathBuf,
    /// Link inputs (objects then static archives), in link order.
    pub inputs: Vec<Utf8PathBuf>,
    /// Inputs the link depends on but that are not command arguments.
    pub implicit_inputs: Vec<Utf8PathBuf>,
    /// Extra linker flags (`ldflags`), inserted after the inputs and
    /// before the output spelling.
    pub arguments: Vec<String>,
    /// System libraries to link, as bare names (e.g. `pthread`, `m`).
    /// Lowered per-dialect - `-l<name>` for GNU-like, `<name>.lib` for
    /// MSVC - and placed after the archive inputs so a static library's
    /// required system libraries resolve left-to-right on the GNU link
    /// line.  Kept separate from `arguments` (raw `ldflags`) precisely so
    /// the dialect layer owns the spelling rather than the planner.
    pub link_libs: Vec<String>,
    /// Human-readable description (`LINK app`).
    pub description: String,
}
