//! Command-line shim for the workflow guards; the guards themselves
//! live in the library.

use std::process::ExitCode;

use anyhow::Result;
use clap::error::ErrorKind;
use clap::{Parser, Subcommand};

const USAGE: &str = "\
usage: xtask-workflow-guard <COMMAND>

Guards over a GitHub Actions run's own context, run from a workflow step
through their Cargo aliases.

commands:
  superseded --path <PATH>...
                   record `superseded=true` in $GITHUB_OUTPUT when
                   origin/main carries a commit after $GITHUB_SHA
                   touching any given path (`cargo workflow-superseded`)
  migrations-pending
                   record `pending=true` in $GITHUB_OUTPUT when the
                   committed D1 migrations no longer match the stamp in
                   registry/migrations-applied
                   (`cargo workflow-migrations-pending`)
  await-deploy     wait for a successful main Registry deploy whose head
                   contains $GITHUB_SHA, or for one of the answers that
                   ends the wait early (`cargo workflow-await-deploy`)

options:
  -h, --help  show this help
";

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Superseded {
        // Deliberately not `required`: the empty-pathspec refusal is
        // `superseded::run`'s own, with its own unit test, and a
        // required argument would answer for it here instead.
        #[arg(long = "path")]
        paths: Vec<String>,
    },
    MigrationsPending,
    AwaitDeploy,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return refuse(&error),
    };
    match run(&cli.command) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// `-h`/`--help` answers with the binary's own usage on stdout, which
/// is what the aliases document, for the subcommands too. Every other
/// parse failure is the exit 1 the step took under `set -e`, where
/// clap's own default is 2.
fn refuse(error: &clap::Error) -> ExitCode {
    match error.kind() {
        ErrorKind::DisplayHelp => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        ErrorKind::DisplayVersion => {
            let _ = error.print();
            ExitCode::SUCCESS
        }
        _ => {
            let _ = error.print();
            ExitCode::FAILURE
        }
    }
}

/// The guards answer in `$GITHUB_OUTPUT` and exit 0; `await-deploy`
/// carries an exit status of its own, which is the step's answer.
fn run(command: &Command) -> Result<ExitCode> {
    match command {
        Command::Superseded { paths } => {
            xtask_workflow_guard::superseded::run(paths).map(|()| ExitCode::SUCCESS)
        }
        Command::MigrationsPending => {
            xtask_workflow_guard::migrations_pending::run().map(|()| ExitCode::SUCCESS)
        }
        Command::AwaitDeploy => Ok(xtask_workflow_guard::await_deploy::run()),
    }
}
