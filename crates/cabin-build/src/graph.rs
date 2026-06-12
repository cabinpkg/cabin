use cabin_core::StandardFlagConflict;
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
    /// Standards problems recorded against *planned* compiles. The
    /// planner records these instead of failing eagerly: the
    /// `cabin check` rewrite prunes dependency compiles after
    /// planning, and a violation that does not survive into the
    /// final graph must not gate the command. The CLI surfaces
    /// survivors through [`crate::validate_planned_standards`]
    /// before anything is lowered or written.
    pub standard_violations: Vec<StandardViolation>,
}

/// One standards problem recorded against a planned compile. Each
/// variant carries the offending compile's object path so the
/// `cabin check` rewrite can prune violations with the same path
/// filter as the compiles they belong to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandardViolation {
    /// An MSVC-dialect compile whose standard `cl.exe` has no
    /// stable `/std:` flag — the planner cannot lower it (its
    /// compile-commands entry is omitted).
    MsvcSpelling {
        /// `package:target` of the offending compile.
        target: String,
        /// Human label of the source language (`C` / `C++`).
        language: &'static str,
        /// Canonical spelling of the offending standard.
        standard: &'static str,
        /// Object path of the offending compile.
        object: Utf8PathBuf,
    },
    /// A compile that carries both a first-class standard
    /// declaration and an explicit `-std=` / `/std:` token in its
    /// manifest-derived flag list — the documented escape-hatch
    /// ambiguity, scoped to compiles the declaration covers.
    FlagConflict {
        conflict: StandardFlagConflict,
        /// Object path of the offending compile.
        object: Utf8PathBuf,
    },
    /// A consuming compile whose effective implementation standard
    /// is below a reachable library-like dependency's interface
    /// requirement for the same language. Recorded against the
    /// *consumer's* compile so the `cabin check` rewrite prunes the
    /// incompatibility together with the compiles it protects — a
    /// dependency-internal incompatibility never gates a check that
    /// only compiles the selected packages' own translation units.
    InterfaceIncompatibility {
        consumer: String,
        dependency: String,
        language: &'static str,
        consumer_standard: &'static str,
        required: &'static str,
        requirement_source: &'static str,
        /// Object path of one of the consumer's compiles of the
        /// language (every object of a target shares the same
        /// per-package prefix the check filter tests).
        object: Utf8PathBuf,
    },
}

impl StandardViolation {
    /// Object path of the offending compile, for the check
    /// rewrite's path filter.
    #[must_use]
    pub fn object(&self) -> &Utf8PathBuf {
        match self {
            Self::MsvcSpelling { object, .. }
            | Self::FlagConflict { object, .. }
            | Self::InterfaceIncompatibility { object, .. } => object,
        }
    }
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
