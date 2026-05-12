//! Ninja serialisation backend for Cabin.
//!
//! This crate consumes a [`cabin_build::BuildGraph`] and writes:
//!
//! - `build.ninja` describing the same actions in Ninja's syntax;
//! - `compile_commands.json`, the Clang JSON Compilation Database.
//!
//! Ninja-specific concerns (rule layout, escaping, depfile wiring) live
//! here. The build planner stays Ninja-agnostic.

#![allow(clippy::missing_errors_doc)]

pub mod compile_commands;
pub mod error;
pub mod writer;

pub use compile_commands::write_compile_commands;
pub use error::NinjaError;
pub use writer::write_build_ninja;
