//! Toolchain detection helpers used by the Cabin build pipeline.
//!
//! This crate owns toolchain resolution, subprocess-based tool detection,
//! compiler-cache wrapper resolution, and Ninja lookup. It does not parse
//! manifests or write build plans; downstream crates consume the typed
//! resolved values and detection reports exposed here.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::return_self_not_must_use,
    clippy::doc_markdown
)]

pub mod cpp;
pub mod detect;
pub mod error;
mod path_search;
pub mod resolve;
pub mod wrapper;

pub use cpp::locate_ninja;
pub use detect::{
    DetectionError as ToolchainDetectionFailure, ProcessRunner, RunError, RunOutput, ToolRunner,
    detect_toolchain,
};
pub use error::ToolchainError;
pub use resolve::{
    ConfigToolEntry, ConfigToolchainLayer, Inputs as ResolveInputs, resolve_toolchain,
};
pub use wrapper::{
    CompilerWrapperResolutionError, ConfigWrapperLayer, WrapperInputs, resolve_compiler_wrapper,
};
