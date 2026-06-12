use camino::Utf8PathBuf;

use cabin_driver::{BuildAction, Dialect};

/// Backend-independent description of everything that needs to happen to
/// realize a build. A backend (currently `cabin-ninja`) walks this graph,
/// lowers each semantic [`BuildAction`] to a concrete command via
/// [`cabin_driver::lower()`] for the graph's [`Dialect`], and emits the
/// equivalent build-system-specific representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildGraph {
    /// All actions to execute, in topological order. Earlier actions in the
    /// vector never depend on later actions. These are *semantic*
    /// actions: compile / archive / link intent, not pre-lowered argv.
    pub actions: Vec<BuildAction>,
    /// Command-line dialect every action in this graph is lowered for.
    /// The backend reads it to pick compile/archive/link spellings and
    /// the matching Ninja dependency-tracking mode.
    pub dialect: Dialect,
    /// Output paths that should be marked as default targets.
    pub default_outputs: Vec<Utf8PathBuf>,
    /// One entry per C/C++ source compile, used to emit
    /// `compile_commands.json`. Both languages contribute entries
    /// with their language-appropriate compiler driver and flags
    /// recorded in `arguments`.
    pub compile_commands: Vec<CompileCommand>,
    /// MSVC-dialect compiles whose standard has no stable `/std:`
    /// flag. The planner cannot lower such a compile (its
    /// compile-commands entry is omitted), so it records the
    /// violation instead of failing eagerly: the `cabin check`
    /// rewrite prunes dependency compiles after planning, and a
    /// violation that does not survive into the final graph must
    /// not gate the command. The CLI surfaces survivors through
    /// [`crate::validate_planned_standards`] before anything is
    /// lowered or written.
    pub msvc_standard_violations: Vec<MsvcStandardViolation>,
}

/// One planned MSVC-dialect compile whose standard `cl.exe` has no
/// stable flag for. See
/// [`BuildGraph::msvc_standard_violations`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsvcStandardViolation {
    /// `package:target` of the offending compile.
    pub target: String,
    /// Human label of the source language (`C` / `C++`).
    pub language: &'static str,
    /// Canonical spelling of the offending standard (e.g. `c++23`).
    pub standard: &'static str,
    /// Object path of the offending compile, used by the check
    /// rewrite's path filter to prune violations alongside their
    /// compiles.
    pub object: Utf8PathBuf,
}

/// One entry of a Clang JSON Compilation Database.
///
/// `arguments` is kept as a list so each backend / consumer can render it
/// however the format requires (LLVM accepts both `command` and
/// `arguments` keys; `cabin-ninja` emits `command`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileCommand {
    pub directory: Utf8PathBuf,
    pub file: Utf8PathBuf,
    pub arguments: Vec<String>,
    pub output: Utf8PathBuf,
}
