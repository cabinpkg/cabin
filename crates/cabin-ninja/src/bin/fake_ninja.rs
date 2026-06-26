//! Test-only stand-in for `ninja`.
//!
//! Cabin's CLI integration tests need to assert the exact
//! argv `cabin build` / `cabin run` / `cabin test` passes to
//! the build backend.  Real `ninja` isn't guaranteed to be on
//! every developer's machine, and even when it is, asserting
//! its observable effect (timing, file outputs, parallelism)
//! is brittle.  This binary records the invocation argv to a
//! file specified by `CABIN_FAKE_NINJA_RECORD` and exits with
//! status 0 - the test then reads the file and asserts the
//! flags it expects.
//!
//! The contract:
//!
//! - When `CABIN_FAKE_NINJA_RECORD` is set, append a single
//!   line listing `argv[1..]` separated by `\u{001f}` (unit
//!   separator) so tests can split unambiguously.  When the
//!   env var is unset, do nothing - invocations without it
//!   are silent successes.
//! - Always exit `0`.  Cabin's pipeline treats anything else
//!   as a backend failure; tests that need failures wire that
//!   in directly.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const FIELD_SEP: char = '\u{001f}';

fn main() -> ExitCode {
    if let Some(record_path) = std::env::var_os("CABIN_FAKE_NINJA_RECORD") {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        let line = argv.join(&FIELD_SEP.to_string());
        let path = PathBuf::from(record_path);
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(mut file) => {
                if let Err(err) = writeln!(file, "{line}") {
                    eprintln!(
                        "cabin-ninja-fake-ninja: failed to write record to {}: {}",
                        path.display(),
                        err,
                    );
                    return ExitCode::from(2);
                }
            }
            Err(err) => {
                eprintln!(
                    "cabin-ninja-fake-ninja: failed to open record at {}: {}",
                    path.display(),
                    err,
                );
                return ExitCode::from(2);
            }
        }
    }
    ExitCode::SUCCESS
}
