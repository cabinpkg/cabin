//! Command-line shim for the registry guards; the guards themselves
//! live in the library.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use xtask_registry_guard::{deploy, kani, r2, registry_dir, sql, tlc};

/// Static guards over the hosted registry Worker's sources, build and
/// proofs.  Each
/// prints every violation it finds and exits non-zero when there is
/// one.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    guard: Guard,
    /// The registry checkout to inspect (default: the `registry/` of
    /// this tool's own checkout).
    #[arg(long, global = true)]
    registry_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Guard {
    /// Executed SQL must stay inside src/sql/.
    CheckSql,
    /// R2 bucket handles may only be acquired in the pinned,
    /// governor-admitting functions.
    CheckR2,
    /// wrangler.jsonc still declares what the code deploys against.
    CheckDeploy {
        /// A missing build/index.js is a failure rather than a skipped
        /// check.
        #[arg(long)]
        require_bundle: bool,
    },
    /// A saved `cargo kani` report shows every cover satisfied.
    CheckKaniReport {
        /// The report `cargo kani` printed.
        report: PathBuf,
    },
    /// TLC reports every trap of each witness configuration violated.
    CheckTlcWitnesses {
        /// The pinned `tla2tools.jar`.
        #[arg(long)]
        jar: PathBuf,
        /// The specification the configurations check.
        spec: PathBuf,
        /// The witness configurations.
        #[arg(required = true)]
        configs: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run(&Cli::parse()) {
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
    let directory = cli.registry_dir.clone().unwrap_or_else(registry_dir);

    let (violations, remedy) = match &cli.guard {
        Guard::CheckDeploy { require_bundle } => {
            let report = deploy::check(&directory, *require_bundle);
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
        Guard::CheckKaniReport { report } => (
            kani::check(
                &std::fs::read_to_string(report)
                    .with_context(|| format!("reading {}", report.display()))?,
            ),
            "error: a Kani harness left a cover unsatisfied; \
             its assumptions may leave nothing to check",
        ),
        Guard::CheckSql => (
            sql::check(&directory)?,
            "error: executed SQL outside src/sql/; \
             route the statements above through sql:: consts",
        ),
        Guard::CheckR2 => (
            r2::check(&directory)?,
            "error: R2 bucket acquisition outside the pinned \
             governor-admitting functions",
        ),
        Guard::CheckTlcWitnesses { jar, spec, configs } => {
            let mut violations = Vec::new();
            for config in configs {
                violations.extend(tlc::check(jar, spec, config)?);
            }
            (
                violations,
                "error: a witness trap was not violated; the model may no \
                 longer reach the behavior it negates",
            )
        }
    };
    for violation in &violations {
        println!("{violation}");
    }
    if !violations.is_empty() {
        eprintln!("{remedy}");
    }
    Ok(violations.is_empty())
}
