//! Command-line shim for the workflow guards; the guards themselves
//! live in the library.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Guards over a GitHub Actions run's own context, run from a workflow
/// step through their Cargo aliases.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Record `superseded=true` in `$GITHUB_OUTPUT` when origin/main
    /// carries a commit after `$GITHUB_SHA` - scoped, with
    /// `--relevant-to`, to commits matching that filter's list in
    /// `.github/path-filters.yml` (`cargo workflow-superseded`).
    Superseded {
        /// Path filter a newer commit must match to supersede this run
        #[arg(long, value_name = "FILTER")]
        relevant_to: Option<String>,
    },
    /// Fail, with an error annotation naming the by-hand recovery, unless
    /// the committed D1 migrations match the stamp in
    /// registry/migrations-applied (`cargo workflow-migrations-pending`).
    MigrationsPending,
    /// Wait for a successful main Registry deploy whose head contains
    /// `$GITHUB_SHA`, or for one of the answers that ends the wait early
    /// (`cargo workflow-await-deploy`).
    AwaitDeploy,
    /// Fail unless each crate-scoped list in `.github/path-filters.yml`
    /// covers every crate and non-Cargo input its jobs build from
    /// (`cargo workflow-filter-coverage`).
    FilterCoverage,
}

fn main() -> ExitCode {
    match run(&Cli::parse().command) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// `superseded` answers in `$GITHUB_OUTPUT`; the other guards answer
/// with the exit status, which is the step's verdict.
fn run(command: &Command) -> Result<ExitCode> {
    match command {
        Command::Superseded { relevant_to } => {
            xtask_workflow_guard::superseded::run(relevant_to.as_deref())
                .map(|()| ExitCode::SUCCESS)
        }
        Command::MigrationsPending => {
            xtask_workflow_guard::migrations_pending::run().map(|()| ExitCode::SUCCESS)
        }
        Command::AwaitDeploy => Ok(xtask_workflow_guard::await_deploy::run()),
        Command::FilterCoverage => {
            xtask_workflow_guard::filter_coverage::run().map(|()| ExitCode::SUCCESS)
        }
    }
}
