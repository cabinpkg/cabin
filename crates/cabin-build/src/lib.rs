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

pub mod check;
pub mod clean;
pub mod error;
pub mod graph;
pub mod link_diagnostics;
pub mod planner;
pub mod validate;

// The toolchain-independent build IR and its dialect selector live in
// `cabin-driver`; re-export the pieces that appear in this crate's
// public surface (`BuildGraph`, `PlanRequest`) so consumers keep one
// import path. The lowering itself (`cabin_driver::lower`) is a
// backend concern, consumed directly by `cabin-ninja`.
pub use cabin_driver::{
    ArchiveAction, BuildAction, CompileAction, CompileArguments, CompileMode, Dialect, LinkAction,
};
pub use check::into_check_graph;
pub use error::BuildError;
pub use graph::{BuildGraph, CompileCommand, MsvcStandardViolation};
pub use planner::{ManifestTargetSelector, PlanRequest, plan, select_targets_of_kind};
pub use validate::{
    RequestedStandards, collect_requested_standards, msvc_external_includes_supported,
    requested_standards_of, validate_planned_standards, validate_toolchain_for_backend,
    validate_toolchain_standards,
};
