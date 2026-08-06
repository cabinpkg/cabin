//! Command-line shim for the fixture generator; the generator itself
//! lives in the library.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Build the in-tree `cabin` binary and package the
/// publish-conformance fixtures into the output directory
/// (`cargo gen-fixtures <out-dir>`).
#[derive(Parser)]
struct Cli {
    // Clap's `PathBuf` parser also refuses an empty value, which would
    // otherwise package into the working directory - the repository
    // root, when run through the alias.
    out: PathBuf,
}

fn main() -> ExitCode {
    match xtask_registry_fixtures::generate(&Cli::parse().out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
