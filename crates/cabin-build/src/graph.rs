use std::path::PathBuf;

use crate::action::BuildAction;

/// Backend-independent description of everything that needs to happen to
/// realize a build. A backend (currently `cabin-ninja`) walks this graph,
/// lowers each semantic [`BuildAction`] to a concrete command via
/// [`crate::lower::lower_gnu_like`], and emits the equivalent
/// build-system-specific representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildGraph {
    /// All actions to execute, in topological order. Earlier actions in the
    /// vector never depend on later actions. These are *semantic*
    /// actions: compile / archive / link intent, not pre-lowered argv.
    pub actions: Vec<BuildAction>,
    /// Output paths that should be marked as default targets.
    pub default_outputs: Vec<PathBuf>,
    /// One entry per C/C++ source compile, used to emit
    /// `compile_commands.json`. Both languages contribute entries
    /// with their language-appropriate compiler driver and flags
    /// recorded in `arguments`.
    pub compile_commands: Vec<CompileCommand>,
}

/// One entry of a Clang JSON Compilation Database.
///
/// `arguments` is kept as a list so each backend / consumer can render it
/// however the format requires (LLVM accepts both `command` and
/// `arguments` keys; `cabin-ninja` emits `command`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileCommand {
    pub directory: PathBuf,
    pub file: PathBuf,
    pub arguments: Vec<String>,
    pub output: PathBuf,
}
