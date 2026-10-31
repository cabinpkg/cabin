//! Thin entry-point for the `cabin` binary.
//!
//! The library half (the `cabin` crate's `lib.rs`) owns parsing,
//! dispatch, and error rendering.  This shim hands off to
//! [`cabin::run`] with the process's own argv and
//! propagates its exit code.

use std::process::ExitCode;

fn main() -> ExitCode {
    cabin::run(std::env::args_os())
}
