//! Test-only stand-in for `ninja`, so Cabin's CLI integration tests
//! can assert the exact argv `cabin build` / `cabin run` /
//! `cabin test` passes to the build backend without a real Ninja.
//!
//! The contract:
//!
//! - When `CABIN_FAKE_NINJA_RECORD` is set, append a single
//!   line listing `argv[1..]` separated by `\u{001f}` (unit
//!   separator) so tests can split unambiguously.  When the
//!   env var is unset, do nothing.
//! - Exit `0`, or `2` when the record file cannot be opened or
//!   written.

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
