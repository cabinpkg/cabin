//! Command-line shim for the dist packaging steps.  Argument parsing is
//! hand-rolled (`clap` stays in the `cabin` crate, per
//! `crates/AGENTS.md`); the steps themselves live in the library.

use std::process::ExitCode;

use anyhow::{Result, bail};
use xtask_dist::package::{self, USAGE};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Every failure is the exit 1 the step took under `set -e`.
fn run() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let Some(command) = arguments.next() else {
        bail!("no command named\n\n{USAGE}");
    };
    let rest: Vec<String> = arguments.collect();
    match command.as_str() {
        "-h" | "--help" if rest.is_empty() => {
            print!("{USAGE}");
            Ok(())
        }
        // `cargo dist-package --help` arrives as `package --help`.
        "package" if rest.len() == 1 && (rest[0] == "-h" || rest[0] == "--help") => {
            print!("{USAGE}");
            Ok(())
        }
        "package" => package::run(&rest),
        other => bail!("unknown command: {other}\n\n{USAGE}"),
    }
}
