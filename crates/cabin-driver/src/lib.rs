//! Compiler-dialect drivers for Cabin's build backend.
//!
//! Cabin's planner ([`cabin_build`](../cabin_build/index.html))
//! produces a *toolchain-independent* semantic IR — a list of
//! [`BuildAction`]s describing what to compile, archive, and link,
//! with no compiler flags spelled out. This crate is the single
//! boundary where that intent becomes a concrete command line.
//!
//! A [`Dialect`] names a compiler command-line family. Two are
//! supported:
//!
//! - [`Dialect::GnuLike`] — the GCC / Clang driver (`-std=c++17`,
//!   `-O2`, `-D` / `-I` / `-c` / `-o`, `-MMD -MF` depfiles).
//! - [`Dialect::Msvc`] — the Microsoft `cl.exe` / `lib.exe` driver
//!   (`/std:c++17`, `/O2`, `/D` / `/I` / `/c` / `/Fo`,
//!   `/showIncludes` dependency tracking).
//!
//! The dialect owns every platform- and toolchain-specific
//! decision: how artifacts are named ([`Dialect::object_extension`],
//! [`Dialect::static_library_name`], [`Dialect::executable_name`]),
//! how Ninja discovers header dependencies ([`Dialect::ninja_deps`]),
//! and how each [`BuildAction`] is spelled ([`lower()`]). The planner
//! and the Ninja writer stay dialect-agnostic: they ask the dialect.
//!
//! This split mirrors the way LLVM's `clang` Driver constructs
//! per-toolchain jobs and the way `rustc` selects a `LinkerFlavor`:
//! one abstract description, many concrete spellings. Everything
//! here is pure, deterministic, and free of I/O, so both dialects
//! are fully unit-testable on any host.

pub mod action;
pub mod dialect;
pub mod lower;

pub use action::{
    ArchiveAction, BuildAction, CompileAction, CompileArguments, CompileMode, LinkAction,
};
pub use dialect::{Dialect, NinjaDeps};
pub use lower::{LoweredAction, LoweredActionKind, compile_argv, lower};
