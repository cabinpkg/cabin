//! Command-line shim for the dist packaging steps.  Argument parsing is
//! hand-rolled (`clap` stays in the `cabin` crate, per
//! `crates/AGENTS.md`); the steps themselves live in the library.

use std::process::ExitCode;

use anyhow::{Result, bail};
use xtask_dist::checksums;
use xtask_dist::package::{self, USAGE};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Every failure is the exit 1 the step took under `set -e`;
/// `checksums` owns its status outright, because its refusal is the
/// shell's bare sentence rather than this shim's `error:` rendering.
fn run() -> Result<ExitCode> {
    let mut arguments = std::env::args().skip(1);
    let Some(command) = arguments.next() else {
        bail!("no command named\n\n{USAGE}");
    };
    let rest: Vec<String> = arguments.collect();
    match command.as_str() {
        "-h" | "--help" if rest.is_empty() => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        // `cargo dist-package --help` arrives as `package --help`.
        "package" | "checksums" if rest.len() == 1 && (rest[0] == "-h" || rest[0] == "--help") => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "package" => package::run(&rest).map(|()| ExitCode::SUCCESS),
        "checksums" => match rest.first() {
            None => Ok(checksums::run()),
            Some(extra) => bail!("unexpected argument: {extra}\n\n{USAGE}"),
        },
        other => bail!("unknown command: {other}\n\n{USAGE}"),
    }
}
