//! Toolchain detection helpers used by the Cabin build pipeline.
//!
//! This crate owns toolchain resolution, subprocess-based tool detection,
//! sysroot selection, compiler-wrapper resolution, and Ninja lookup. It does not parse
//! manifests or write build plans; downstream crates consume the typed
//! resolved values and detection reports exposed here.

pub mod detect;
pub mod error;
pub mod msvc;
pub mod ninja;
mod path_search;
pub mod resolve;
pub mod sysroot;
pub mod target;
pub mod wrapper;

pub use detect::{
    DetectionError as ToolchainDetectionFailure, ProcessRunner, RunError, RunOutput, ToolRunner,
    detect_toolchain,
};
pub use error::ToolchainError;
pub use msvc::{msvc_environment, msvc_tool_path, path_is_discovered_msvc_cl};
pub use ninja::locate_ninja;
pub use resolve::{
    ConfigToolEntry, ConfigToolchainLayer, Inputs as ResolveInputs, resolve_toolchain,
};
pub use sysroot::{SysrootDialectError, discover_apple_sdk, explicit_sysroot};
pub use target::{
    CompilerTargetError, CompilerTargets, check_build_context_compilers, check_compiler_targets,
};
pub use wrapper::{
    CompilerWrapperResolutionError, ConfigWrapperLayer, WrapperInputs, resolve_compiler_wrapper,
};
