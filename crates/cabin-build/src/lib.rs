//! Backend-independent build planning for Cabin.
//!
//! This crate consumes a workspace package graph plus a resolved C/C++
//! toolchain (from `cabin-toolchain`) and produces a [`BuildGraph`] - a
//! list of compile, assemble, archive, and link actions plus the
//! metadata needed to write a Clang-style compilation database.
//!
//! The graph is backend-agnostic: `cabin-ninja` serializes it as
//! `build.ninja` + `compile_commands.json`.

#![allow(
    clippy::too_many_lines,
    // Remaining hits are `field: Default::default()` in test graph
    // fixtures, where clippy's typed-default suggestion is MaybeIncorrect.
    clippy::default_trait_access
)]

pub mod check;
pub mod clean;
pub mod error;
pub mod gc;
pub mod graph;
pub mod layout;
pub mod link_diagnostics;
pub mod owner;
pub mod planner;
pub mod standard_compat;
pub mod validate;

// The toolchain-independent build IR and its dialect selector live in
// `cabin-driver`; re-export the pieces that appear in this crate's
// public surface (`BuildGraph`, `PlanRequest`) so consumers keep one
// import path.  The lowering itself (`cabin_driver::lower`) is a
// backend concern, consumed directly by `cabin-ninja`.
pub use cabin_driver::{
    ArchiveAction, BuildAction, CompileAction, CompileArguments, CompileMode, Dialect, LinkAction,
};
pub use check::into_check_graph;
pub use error::{BuildError, FeatureGateFix, PlannedStandardError};
pub use graph::{BuildGraph, CompileCommand, InterfaceViolationKind, StandardViolation};
pub use planner::{
    ManifestTargetSelector, PlanRequest, SystemTargetInterface, plan, select_targets_of_kind,
    selector_required_features_met,
};
pub use standard_compat::{
    ClashingBound, DeclScope, DeclSite, EdgeRequirement, RequirementOrigin, StandardCompatViolation,
};
pub use validate::{
    RequestedStandards, clang_cl_languages, collect_requested_standards,
    msvc_external_includes_supported, requested_standards_of, validate_output_paths,
    validate_planned_standards, validate_toolchain_for_backend, validate_toolchain_standards,
};
