//! Command-line shim for the registry operator commands.  Argument
//! parsing is hand-rolled (`clap` stays in the `cabin` crate, per
//! `crates/AGENTS.md`); the commands themselves live in the library.

use std::process::ExitCode;

use anyhow::{Result, bail};

const USAGE: &str = "\
usage: xtask-registry-admin <COMMAND>

Operator commands against the hosted registry, run from the repository
root through their Cargo aliases.

commands:
  backup-backfill  copy verified blobs missing from the backup bucket
                   (`cargo registry-backup-backfill`)
  diagnose         safe diagnostics bundle (`cargo registry-diagnose`)

options:
  -h, --help  show this help
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let Some(command) = arguments.next() else {
        bail!("no command named\n\n{USAGE}");
    };
    if let Some(extra) = arguments.next() {
        bail!("unexpected argument: {extra}\n\n{USAGE}");
    }
    match command.as_str() {
        "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        "backup-backfill" => xtask_registry_admin::backfill::run(),
        "diagnose" => xtask_registry_admin::diagnose::run(),
        other => bail!("unknown command: {other}\n\n{USAGE}"),
    }
}
