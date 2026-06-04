//! Semantic build-action IR.
//!
//! The planner emits these toolchain-independent action specs; the
//! lowering layer ([`crate::lower`]) turns them into concrete command
//! argv for a specific compiler family. Today only a GNU/Clang-like
//! lowering exists; a future toolchain driver (e.g. MSVC) lowers the
//! same actions differently — different flag spellings,
//! `/showIncludes` dependency tracking, `lib.exe` archiving — without
//! the planner or this IR changing.

use std::path::PathBuf;

use cabin_core::SourceLanguage;

/// A single semantic build step: compile a translation unit, archive
/// objects into a static library, or link an executable.
///
/// Backend- and toolchain-independent: the concrete command argv is
/// produced later by [`crate::lower::lower_gnu_like`], not stored
/// here. This is the planner's primary output, replacing the
/// previously pre-lowered argv action.
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
    /// `stamp` is `touch`ed to record a successful check. The
    /// `object` path is retained on the action so the stamp lives
    /// beside it and the workspace-scope filter in `cabin check` can
    /// match on it.
    SyntaxOnly {
        /// Stamp file written on a successful check, in place of the
        /// object.
        stamp: PathBuf,
    },
}

/// Compile one translation unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileAction {
    /// Source language; selects the standard flag and, at lowering
    /// time, the rule / lowered action kind.
    pub language: SourceLanguage,
    /// Absolute path of the translation unit to compile.
    pub source: PathBuf,
    /// Object file the normal build produces. Retained even in
    /// [`CompileMode::SyntaxOnly`] so the stamp lives beside it and
    /// the workspace-scope filter in `cabin check` can match on it.
    pub object: PathBuf,
    /// Object vs. syntax-only.
    pub mode: CompileMode,
    /// Inputs the compile depends on but that are not command
    /// arguments (e.g. a generated source produced upstream). The
    /// `source` is the sole compiled input and is not repeated here.
    pub implicit_inputs: Vec<PathBuf>,
    /// Makefile-style depfile path; `Some` for these compiles so the
    /// GNU/Clang lowering wires `-MMD -MF <depfile>` into Ninja's
    /// `deps = gcc` machinery.
    pub depfile: Option<PathBuf>,
    /// Compiler driver executable.
    pub compiler: PathBuf,
    /// Optional compiler-cache wrapper (e.g. `ccache`) prepended to
    /// the *run* command by lowering. Never affects
    /// `compile_commands.json`, which records the underlying compiler
    /// so IDE tooling sees the real driver.
    pub compiler_wrapper: Option<PathBuf>,
    /// Structured compile arguments (flags, includes, defines).
    pub arguments: CompileArguments,
    /// Human-readable description for build output (`CXX foo.o`,
    /// `CHECK foo.o`).
    pub description: String,
}

/// Structured arguments for a compile.
///
/// The two flag groups bracket the `-D`/`-I` block:
/// `std_and_profile_flags` precede it and `extra_flags` follow it,
/// mirroring the established GNU/Clang argv layout so lowering is
/// byte-for-byte stable with the historic command lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileArguments {
    /// Language standard plus language-neutral profile flags
    /// (optimization, debug info, `NDEBUG`). Emitted before the
    /// dependency / define / include block.
    pub std_and_profile_flags: Vec<String>,
    /// Include search directories. Lowered as `-I <dir>` pairs for
    /// GNU/Clang.
    pub include_dirs: Vec<PathBuf>,
    /// Preprocessor defines, without the `-D` prefix. Lowered as
    /// `-D<define>` for GNU/Clang.
    pub defines: Vec<String>,
    /// Escape-hatch compile flags (language-neutral first, then
    /// language-specific) appended after the include block.
    pub extra_flags: Vec<String>,
}

/// Archive object files into a static library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveAction {
    /// Archiver executable (`ar`).
    pub archiver: PathBuf,
    /// Static library to produce.
    pub output: PathBuf,
    /// Object files to archive, in order.
    pub inputs: Vec<PathBuf>,
    /// Human-readable description (`AR libfoo.a`).
    pub description: String,
}

/// Link objects and static archives into an executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkAction {
    /// Link-driver executable (the C or C++ compiler, chosen per
    /// target by the planner).
    pub linker: PathBuf,
    /// Executable to produce.
    pub output: PathBuf,
    /// Link inputs (objects then static archives), in link order.
    pub inputs: Vec<PathBuf>,
    /// Inputs the link depends on but that are not command arguments.
    pub implicit_inputs: Vec<PathBuf>,
    /// Extra linker flags (`ldflags`), inserted after the inputs and
    /// before the `-o <output>` pair.
    pub arguments: Vec<String>,
    /// Human-readable description (`LINK app`).
    pub description: String,
}
