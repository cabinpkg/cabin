use camino::Utf8PathBuf;

use cabin_driver::{BuildAction, Dialect};

use crate::PlannedStandardError;
use crate::standard_compat::StandardCompatViolation;

/// Backend-independent description of everything that needs to happen to
/// realize a build.  A backend (currently `cabin-ninja`) walks this graph,
/// lowers each semantic [`BuildAction`] to a concrete command via
/// [`cabin_driver::lower()`] for the graph's [`Dialect`], and emits the
/// equivalent build-system-specific representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildGraph {
    /// All actions to execute, in topological order.  Earlier actions in the
    /// vector never depend on later actions.  These are *semantic*
    /// actions: compile / archive / link intent, not pre-lowered argv.
    pub actions: Vec<BuildAction>,
    /// Command-line dialect every action in this graph is lowered for.
    /// The backend reads it to pick compile/archive/link spellings and
    /// the matching Ninja dependency-tracking mode.
    pub dialect: Dialect,
    /// Output paths that should be marked as default targets.
    pub default_outputs: Vec<Utf8PathBuf>,
    /// Full names of every package with at least one planned target
    /// in this graph's own context.  Consumers that map build paths
    /// back to packages (progress banners, link diagnostics) restrict
    /// their lookups to the planned sets, so an unplanned workspace
    /// member whose bare name equals a planned package's scope can
    /// never win the reverse parse.  Sorted for determinism.
    pub planned_packages: std::collections::BTreeSet<String>,
    /// The `planned_packages` of the build-context plan folded in by
    /// [`Self::append`].  Kept apart because each plan rejects only its
    /// own bare-name/scope collisions: one context's bare package may
    /// share its name with the other context's scope.
    pub build_context_packages: std::collections::BTreeSet<String>,
    /// One entry per C/C++ source compile, used to emit
    /// `compile_commands.json`.  Both languages contribute entries
    /// with their language-appropriate compiler driver and flags
    /// recorded in `arguments`.
    pub compile_commands: Vec<CompileCommand>,
    /// Standards problems recorded against *planned* compiles.  The
    /// planner records these instead of failing eagerly: the
    /// `cabin check` rewrite prunes dependency compiles after
    /// planning, and a violation that does not survive into the
    /// final graph must not gate the command.  The CLI surfaces
    /// survivors through [`crate::validate_planned_standards`]
    /// before anything is lowered or written.
    pub standard_violations: Vec<StandardViolation>,
    /// Violated dependency edges from the post-resolution
    /// standard-compatibility pass ([`crate::standard_compat`]).
    /// Always empty unless the pass ran
    /// ([`crate::PlanRequest::standard_compat`]).  The CLI renders
    /// non-ignored records as errors that fail the command and
    /// `ignored` records as unchecked-edge notes; the `cabin check`
    /// rewrite carries them through unchanged (they describe
    /// resolved edges, not planned compiles).
    pub standard_compat_violations: Vec<StandardCompatViolation>,
}

impl BuildGraph {
    /// Fold the build context's plan into the target context's, so
    /// one Ninja file builds both.  The two plans' outputs live under
    /// different directories, and no target action consumes a build
    /// context output yet, so concatenation keeps topological order.
    pub fn append(&mut self, other: BuildGraph) {
        debug_assert_eq!(self.dialect, other.dialect);
        self.actions.extend(other.actions);
        self.default_outputs.extend(other.default_outputs);
        self.build_context_packages.extend(other.planned_packages);
        self.compile_commands.extend(other.compile_commands);
        self.standard_violations.extend(other.standard_violations);
        self.standard_compat_violations
            .extend(other.standard_compat_violations);
    }
}

/// One standards problem recorded against a planned compile.  It
/// carries the offending compile's object path so the `cabin check`
/// rewrite can prune violations with the same path filter as the
/// compiles they belong to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardViolation {
    /// Object path of the offending compile (for an interface
    /// incompatibility or conflicting define, one of the target's
    /// compiles; for an assembly source, the object it would have
    /// produced).
    pub object: Utf8PathBuf,
    pub error: PlannedStandardError,
}

/// Which side of an interface requirement a consuming compile
/// fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceViolationKind {
    /// The consumer's level is below the requirement's minimum.
    BelowMinimum { min: &'static str },
    /// The consumer's level is above the requirement's maximum.
    AboveMaximum { max: &'static str },
}

/// One entry of a Clang JSON Compilation Database.
///
/// `arguments` is kept as a list so each backend / consumer can render it
/// however the format requires (LLVM accepts both `command` and
/// `arguments` keys; `cabin-ninja` emits `arguments`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileCommand {
    pub directory: Utf8PathBuf,
    pub file: Utf8PathBuf,
    pub arguments: Vec<String>,
    pub output: Utf8PathBuf,
}
