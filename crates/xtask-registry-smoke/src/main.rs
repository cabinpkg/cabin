//! Command-line shim for the registry smoke test.

use std::process::ExitCode;

fn main() -> ExitCode {
    match xtask_registry_smoke::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // The shell's `fail`: `FAIL: <msg>` on stderr, exit 1 -
            // never the `error:` shape the other xtask binaries use,
            // because the prefix is observable output the original
            // defined.
            eprintln!("FAIL: {err:#}");
            ExitCode::FAILURE
        }
    }
}
