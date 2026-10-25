//! Command-line shim for the registry smoke test.

use std::process::ExitCode;

use xtask_registry_smoke::legs::finale;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((mode, rest)) if mode == finale::DRIVER => finale::driver(rest),
        _ => xtask_registry_smoke::run(),
    };
    match result {
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
