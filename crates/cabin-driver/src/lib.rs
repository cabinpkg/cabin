//! Compiler-dialect drivers for Cabin's build backend.
//!
//! Cabin's planner ([`cabin_build`](../cabin_build/index.html))
//! produces a *toolchain-independent* semantic IR - a list of
//! [`BuildAction`]s describing what to compile, assemble, archive, and link,
//! with no compiler flags spelled out.  This crate is the single
//! boundary where that intent becomes a concrete command line.
//!
//! A [`Dialect`] names a compiler command-line family.  Two are
//! supported:
//!
//! - [`Dialect::GnuLike`] - the GCC / Clang driver (`-std=c++17`,
//!   `-O2`, `-D` / `-I` / `-isystem` / `-c` / `-o`, `-MD -MF`
//!   depfiles).
//! - [`Dialect::Msvc`] - the Microsoft `cl.exe` / `lib.exe` driver
//!   (`/std:c++17`, `/O2`, `/D` / `/I` / `/external:I` / `/c` /
//!   `/Fo`, `/showIncludes` dependency tracking).
//!
//! The dialect owns every platform- and toolchain-specific
//! decision: how artifacts are named ([`Dialect::object_extension`],
//! [`Dialect::static_library_name`], [`Dialect::executable_name`]),
//! how Ninja discovers header dependencies ([`Dialect::ninja_deps`]),
//! how its tools read a response file
//! ([`Dialect::response_file_args`]), and how each [`BuildAction`]
//! is spelled ([`lower()`]).  The planner
//! and the Ninja writer stay dialect-agnostic: they ask the dialect.
//!
//! Everything here is pure, deterministic, and free of I/O, so both
//! dialects are unit-testable on any host.

pub mod action;
pub mod dialect;
pub mod lower;

pub use action::{
    ArchiveAction, AssembleAction, BuildAction, CompileAction, CompileArguments, CompileMode,
    LinkAction,
};
pub use dialect::{Dialect, NinjaDeps, windows_quote_arg};
pub use lower::{LoweredAction, LoweredActionKind, compile_argv, lower};
