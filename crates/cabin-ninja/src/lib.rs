//! Ninja serialization backend for Cabin.
//!
//! This crate consumes a [`cabin_build::BuildGraph`] and writes:
//!
//! - `build.ninja` describing the same actions in Ninja's syntax;
//! - `compile_commands.json`, the Clang JSON Compilation Database.
//!
//! It also reads back the GCC-style depfiles that Ninja's own parser
//! would misread (see [`depfile`]).
//!
//! Ninja-specific concerns (rule layout, escaping, depfile wiring) live
//! here.  The build planner stays Ninja-agnostic.

pub mod compile_commands;
pub mod depfile;
pub mod error;
pub mod writer;

pub use compile_commands::write_compile_commands;
pub use depfile::msvc_deps_report;
pub use error::NinjaError;
pub use writer::write_build_ninja;
