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
