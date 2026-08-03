//! Command-line shim for the repository-policy checks.  Argument parsing
//! is hand-rolled (`clap` stays in the `cabin` crate, per
//! `crates/AGENTS.md`); the checks themselves live in the library.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use xtask_ci::{repo_root, scripts};

const USAGE: &str = "\
usage: xtask-ci <check-scripts> [--repo-root <PATH>]

Checks that keep this repository's own automation honest.  Each prints
every violation it finds and exits non-zero when there is one.

checks:
  check-scripts  no non-Rust repository automation, the xtask crates keep
                 their shape, and this guard still runs in CI

options:
  --repo-root <PATH>  the checkout to inspect (default: the repository
                      this tool was built from)
  -h, --help          show this help
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

/// `Ok(true)` when the check accepted the tree.
fn run() -> Result<bool> {
    let mut arguments = std::env::args().skip(1);
    let Some(check) = arguments.next() else {
        bail!("no check named\n\n{USAGE}");
    };
    if check == "-h" || check == "--help" {
        print!("{USAGE}");
        return Ok(true);
    }
    let mut root = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(true);
            }
            "--repo-root" => {
                root =
                    Some(PathBuf::from(arguments.next().ok_or_else(|| {
                        anyhow::anyhow!("--repo-root needs a path")
                    })?));
            }
            other => bail!("unexpected argument: {other}\n\n{USAGE}"),
        }
    }
    let root = root.unwrap_or_else(repo_root);

    match check.as_str() {
        "check-scripts" => {
            let violations = scripts::check(&root)?;
            for violation in &violations {
                println!("{violation}");
            }
            if violations.is_empty() {
                let pending = scripts::pending();
                println!(
                    "repository automation OK ({} legacy script(s) still awaiting migration)",
                    pending.len()
                );
            } else {
                eprintln!(
                    "error: repository automation must be a crates/xtask-* command \
                     reached through a cargo alias (AGENTS.md, \"Repository automation\")"
                );
            }
            Ok(violations.is_empty())
        }
        other => bail!("unknown check: {other}\n\n{USAGE}"),
    }
}
