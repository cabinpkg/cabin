//! Backend-independent build planning for Cabin.
//!
//! This crate consumes a validated [`cabin_core::Package`] plus a
//! resolved C/C++ toolchain (from `cabin-toolchain`) and produces a
//! [`BuildGraph`] — a list of compile, archive, and link actions
//! plus the metadata needed to write a Clang-style compilation
//! database.
//!
//! The graph is intentionally backend-agnostic. `cabin-ninja` knows how to
//! serialise it as `build.ninja` + `compile_commands.json`; future backends
//! (e.g. a Bazel exporter, or a direct in-process executor) could consume
//! the same structure.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::single_match_else,
    clippy::items_after_statements,
    clippy::default_trait_access,
    clippy::too_many_lines,
    clippy::map_unwrap_or,
    clippy::manual_let_else,
    clippy::redundant_closure_for_method_calls
)]

pub mod clean;
pub mod error;
pub mod graph;
pub mod link_diagnostics;
pub mod planner;
pub mod validate;

pub use error::BuildError;
pub use graph::{Action, ActionKind, BuildGraph, CompileCommand};
pub use planner::{ManifestTargetSelector, PlanRequest, plan, select_targets_of_kind};
pub use validate::validate_toolchain_for_backend;
