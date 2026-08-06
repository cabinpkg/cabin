//! Command-line shim for the registry source guards; the guards
//! themselves live in the library.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use xtask_registry_guard::{deploy, r2, registry_dir, sql};

const USAGE: &str = "\
usage: xtask-registry-guard <GUARD> [options]

Static guards over the hosted registry Worker's sources.  Each prints
every violation it finds and exits non-zero when there is one.

guards:
  check-sql   executed SQL must stay inside src/sql.rs
  check-r2    R2 bucket handles may only be acquired in the pinned,
              governor-admitting functions
  check-deploy  wrangler.jsonc still declares what the code deploys
              against (and, when built, the bundle exports every bound
              Durable Object class)

options:
  --require-bundle       check-deploy only: a missing build/index.js is
                         a failure rather than a skipped check
  --registry-dir <PATH>  the registry checkout to inspect (default: the
                         `registry/` of this tool's own checkout)
  -h, --help             show this help
";

/// The guards are subcommands; both options are global, because
/// `--help` and the options are accepted after the guard name as well
/// as before it.
// `args_override_self`: a repeated flag keeps its last value, as the
// parser this replaces did, so a wrapper may supply a default and
// override it.
#[derive(Parser)]
#[command(disable_help_subcommand = true, args_override_self = true)]
struct Cli {
    #[command(subcommand)]
    guard: Option<Guard>,
    #[arg(long, global = true)]
    registry_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    require_bundle: bool,
}

#[derive(Subcommand)]
enum Guard {
    CheckSql,
    CheckR2,
    CheckDeploy,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // One help text for the binary, whichever guard it was asked
        // after; every other parse failure is a refusal, which exits 1
        // rather than clap's 2.
        Err(err) if err.kind() == ErrorKind::DisplayHelp => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            let _ = err.print();
            return ExitCode::FAILURE;
        }
    };
    match run(&cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// `Ok(true)` when the guard accepted the tree.
fn run(cli: &Cli) -> Result<bool> {
    let Some(guard) = &cli.guard else {
        bail!("no guard named\n\n{USAGE}");
    };
    let directory = cli.registry_dir.clone().unwrap_or_else(registry_dir);
    if cli.require_bundle && !matches!(guard, Guard::CheckDeploy) {
        bail!("--require-bundle is only meaningful for check-deploy\n\n{USAGE}");
    }

    let (violations, remedy) = match guard {
        Guard::CheckDeploy => {
            let report = deploy::check(&directory, cli.require_bundle);
            for note in &report.notes {
                println!("{note}");
            }
            for failure in &report.failures {
                eprintln!("{failure}");
            }
            if let Some(summary) = report.summary {
                eprintln!("FAIL: {summary}");
                return Ok(false);
            }
            println!("deploy config OK");
            return Ok(true);
        }
        Guard::CheckSql => (
            sql::check(&directory)?,
            "error: executed SQL outside src/sql.rs; \
             route the statements above through sql:: consts",
        ),
        Guard::CheckR2 => (
            r2::check(&directory)?,
            "error: R2 bucket acquisition outside the pinned \
             governor-admitting functions",
        ),
    };
    for violation in &violations {
        println!("{violation}");
    }
    if !violations.is_empty() {
        eprintln!("{remedy}");
    }
    Ok(violations.is_empty())
}
