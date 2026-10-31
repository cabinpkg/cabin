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

use cabin_core::{InstructionSet, LanguageStandard, OptLevel, Sysroot};

/// A single semantic build step.
///
/// Backend- and toolchain-independent: the concrete command argv is
/// produced later by [`crate::lower()`], not stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildAction {
    /// Compile (or syntax-check) one C/C++ translation unit.
    Compile(CompileAction),
    /// Assemble one assembly source.
    Assemble(AssembleAction),
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
    /// Syntax/semantic check only (`cabin check`): emit no object.
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
    /// Whether this translation unit compiles with GNU language
    /// extensions.  Strictly per-target: two compiles in one build
    /// may differ in both level and this flag.  The GNU/Clang
    /// lowering spells the standard `-std=gnu…` instead of
    /// `-std=c…`; the planner rejects the flag on the MSVC dialect
    /// before lowering (`cl.exe` has no GNU dialect mode).
    pub gnu_extensions: bool,
    /// Absolute path of the translation unit to compile.
    pub source: Utf8PathBuf,
    /// Object file the normal build produces.  Retained even in
    /// [`CompileMode::SyntaxOnly`] so the stamp lives beside it and
    /// the workspace-scope filter in `cabin check` can match on it.
    pub object: Utf8PathBuf,
    /// Object vs. syntax-only.
    pub mode: CompileMode,
    /// Header-dependency tracking file.  `Some` requests dependency
    /// discovery for this compile: the GNU/Clang lowering writes a
    /// Makefile depfile here (`-MD -MF <depfile>`); the MSVC lowering
    /// ignores the path and emits `/showIncludes`.
    pub depfile: Option<Utf8PathBuf>,
    /// Compiler driver executable.
    pub compiler: Utf8PathBuf,
    /// Platform triple a Clang-family driver builds for, spelled
    /// `--target=<triple>` right after the driver; `None` keeps the
    /// driver's default.  Set only on the GNU/Clang dialect.
    pub target: Option<String>,
    /// Sysroot spelled right after the target; `None` keeps the
    /// driver's default.  Set only on the GNU/Clang dialect.
    pub sysroot: Option<Sysroot>,
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
    /// Emit debug information (`-g` / `/Z7`).
    pub debug_info: bool,
    /// Define `NDEBUG` (assertions disabled in the active profile).
    pub define_ndebug: bool,
    /// Include search directories.  Spelled `-I <dir>` / `/I <dir>`.
    pub include_dirs: Vec<Utf8PathBuf>,
    /// Include search directories marked as *system* search paths,
    /// so diagnostics inside their headers are suppressed.  Searched
    /// after [`Self::include_dirs`].  The planner routes third-party
    /// contributions here (registry packages, pkg-config system
    /// dependencies).  Spelled `-isystem <dir>` for
    /// GNU/Clang; `/external:W0 /external:I <dir>` for MSVC (the
    /// planner only populates this on MSVC-dialect builds when the
    /// detected compiler supports the `/external:` block).
    pub system_include_dirs: Vec<Utf8PathBuf>,
    /// Preprocessor defines, without any prefix.  Spelled `-D<define>`
    /// / `/D<define>`.
    pub defines: Vec<String>,
    /// Instruction sets this source may use beyond the platform
    /// baseline.  Spelled `-m<name>` by both dialects: the planner
    /// leaves it empty for `cl.exe`, which exposes every intrinsic
    /// without a flag, so on the MSVC dialect only `clang-cl` gets it.
    pub instruction_sets: Vec<InstructionSet>,
    /// Escape-hatch compile flags appended verbatim after the
    /// include block.  The user writes these in the active dialect
    /// (language-neutral first, then language-specific).
    pub extra_flags: Vec<String>,
}

/// Assemble one assembly source into an object file: GNU syntax
/// (`.S`) through the GCC/Clang driver, which preprocesses it first,
/// and MASM (`.asm`) through `ml64.exe`.  Only defines and include
/// dirs apply; the standard, optimization, and escape-hatch flags are
/// C/C++ compiler arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembleAction {
    /// Assembler executable: the C++ compiler driver on the GNU/Clang
    /// dialect, `ml64.exe` on MSVC.
    pub assembler: Utf8PathBuf,
    /// Platform triple a Clang-family driver builds for, spelled
    /// `--target=<triple>` right after the driver; `None` keeps the
    /// driver's default.  Set only on the GNU/Clang dialect.
    pub target: Option<String>,
    /// Sysroot spelled right after the target; `None` keeps the
    /// driver's default.  Set only on the GNU/Clang dialect.
    pub sysroot: Option<Sysroot>,
    /// Absolute path of the assembly source.
    pub source: Utf8PathBuf,
    /// Object file to produce.
    pub object: Utf8PathBuf,
    /// Header-dependency file the GNU/Clang driver's preprocessing
    /// pass writes; the MSVC lowering ignores it.
    pub depfile: Option<Utf8PathBuf>,
    /// Include search directories (`-I` / `/I`).
    pub include_dirs: Vec<Utf8PathBuf>,
    /// Preprocessor defines, without any prefix.
    pub defines: Vec<String>,
    /// Human-readable description (`AS foo.o`).
    pub description: String,
}

/// Archive object files into a static library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveAction {
    /// Archiver executable (`ar` / `lib`).
    pub archiver: Utf8PathBuf,
    /// Whether the archiver splits a response file like a command line
    /// when it runs on Windows
    /// ([`cabin_core::ArchiverIdentity::windows_rsp_quoting`]).
    pub windows_rsp_quoting: bool,
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
    /// Platform triple a Clang-family driver builds for, spelled
    /// `--target=<triple>` right after the driver; `None` keeps the
    /// driver's default.  Set only on the GNU/Clang dialect.
    pub target: Option<String>,
    /// Sysroot spelled right after the target; `None` keeps the
    /// driver's default.  Set only on the GNU/Clang dialect.
    pub sysroot: Option<Sysroot>,
    /// Executable to produce.
    pub output: Utf8PathBuf,
    /// Link inputs (objects then static archives), in link order.
    pub inputs: Vec<Utf8PathBuf>,
    /// Extra linker flags (`ldflags`), inserted after the inputs.  On
    /// MSVC they follow `/link`, so they are `link.exe` options.
    pub arguments: Vec<String>,
    /// System libraries to link, as bare names (e.g. `pthread`, `m`).
    /// Lowered per-dialect - `-l<name>` for GNU-like, `<name>.lib` for
    /// MSVC - and placed after the archive inputs so a static library's
    /// required system libraries resolve left-to-right on the GNU link
    /// line.  Kept separate from `arguments` (raw `ldflags`) so the
    /// dialect layer owns the spelling.
    pub link_libs: Vec<String>,
    /// Apple frameworks to link, as bare names, lowered to
    /// `-framework <name>` after `link_libs`.  The planner fills it
    /// only for Apple platforms, which MSVC never targets.
    pub frameworks: Vec<String>,
    /// Human-readable description (`LINK app`).
    pub description: String,
}
