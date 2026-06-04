//! Backend-independent build planning for Cabin.
//!
//! This crate consumes a validated [`cabin_core::Package`] plus a
//! resolved C/C++ toolchain (from `cabin-toolchain`) and produces a
//! [`BuildGraph`] — a list of compile, archive, and link actions
//! plus the metadata needed to write a Clang-style compilation
//! database.
//!
//! The graph is intentionally backend-agnostic. `cabin-ninja` knows how to
//! serialize it as `build.ninja` + `compile_commands.json`; future backends
//! (e.g. a Bazel exporter, or a direct in-process executor) could consume
//! the same structure.

#![allow(
    clippy::too_many_lines,
    // Remaining hits are `field: Default::default()` in test graph
    // fixtures, where clippy's typed-default suggestion is MaybeIncorrect.
    clippy::default_trait_access
)]

pub mod action;
pub mod check;
pub mod clean;
pub mod error;
pub mod graph;
pub mod link_diagnostics;
pub mod lower;
pub mod planner;
pub mod validate;

pub use action::{
    ArchiveAction, BuildAction, CompileAction, CompileArguments, CompileMode, LinkAction,
};
pub use check::into_check_graph;
pub use error::BuildError;
pub use graph::{BuildGraph, CompileCommand};
pub use lower::{LoweredAction, LoweredActionKind, lower_gnu_like};
pub use planner::{ManifestTargetSelector, PlanRequest, plan, select_targets_of_kind};
pub use validate::validate_toolchain_for_backend;
