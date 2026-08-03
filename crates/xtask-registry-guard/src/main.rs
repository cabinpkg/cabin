//! Command-line shim for the registry source guards.  Argument parsing
//! is hand-rolled (`clap` stays in the `cabin` crate, per
//! `crates/AGENTS.md`); the guards themselves live in the library.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use xtask_registry_guard::{r2, registry_dir, sql};

const USAGE: &str = "\
usage: xtask-registry-guard <check-sql|check-r2> [--registry-dir <PATH>]

Static guards over the hosted registry Worker's sources.  Each prints
every violation it finds and exits non-zero when there is one.

guards:
  check-sql   executed SQL must stay inside src/sql.rs
  check-r2    R2 bucket handles may only be acquired in the pinned,
              governor-admitting functions

options:
  --registry-dir <PATH>  the registry checkout to inspect (default: the
                         `registry/` of this tool's own checkout)
  -h, --help             show this help
";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// `Ok(true)` when the guard accepted the tree.
fn run() -> Result<bool> {
    let mut arguments = std::env::args().skip(1);
    let Some(guard) = arguments.next() else {
        bail!("no guard named\n\n{USAGE}");
    };
    if guard == "-h" || guard == "--help" {
        print!("{USAGE}");
        return Ok(true);
    }
    let mut directory = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(true);
            }
            "--registry-dir" => {
                directory =
                    Some(PathBuf::from(arguments.next().ok_or_else(|| {
                        anyhow::anyhow!("--registry-dir needs a path")
                    })?));
            }
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }
    let directory = directory.unwrap_or_else(registry_dir);

    let (violations, remedy) = match guard.as_str() {
        "check-sql" => (
            sql::check(&directory)?,
            "error: executed SQL outside src/sql.rs; \
             route the statements above through sql:: consts",
        ),
        "check-r2" => (
            r2::check(&directory)?,
            "error: R2 bucket acquisition outside the pinned \
             governor-admitting functions",
        ),
        other => bail!("unknown guard: {other}\n\n{USAGE}"),
    };
    for violation in &violations {
        println!("{violation}");
    }
    if !violations.is_empty() {
        eprintln!("{remedy}");
    }
    Ok(violations.is_empty())
}
