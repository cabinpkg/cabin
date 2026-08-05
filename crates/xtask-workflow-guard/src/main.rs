//! Command-line shim for the workflow guards.  Argument parsing is
//! hand-rolled (`clap` stays in the `cabin` crate, per
//! `crates/AGENTS.md`); the guards themselves live in the library.

use std::process::ExitCode;

use anyhow::{Result, bail};

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

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// The guards answer in `$GITHUB_OUTPUT` and exit 0; `await-deploy`
/// carries an exit status of its own, which is the step's answer.
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
        "superseded" => {
            xtask_workflow_guard::superseded::run(&paths(&rest)?).map(|()| ExitCode::SUCCESS)
        }
        "migrations-pending" => match rest.first() {
            None => xtask_workflow_guard::migrations_pending::run().map(|()| ExitCode::SUCCESS),
            Some(extra) => bail!("unexpected argument: {extra}\n\n{USAGE}"),
        },
        "await-deploy" => match rest.first() {
            None => Ok(xtask_workflow_guard::await_deploy::run()),
            Some(extra) => bail!("unexpected argument: {extra}\n\n{USAGE}"),
        },
        other => bail!("unknown command: {other}\n\n{USAGE}"),
    }
}

/// `superseded`'s own argument surface: `--path <p>`, repeated.
fn paths(rest: &[String]) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    let mut rest = rest.iter();
    while let Some(argument) = rest.next() {
        if argument != "--path" {
            bail!("unexpected argument: {argument}\n\n{USAGE}");
        }
        let Some(path) = rest.next() else {
            bail!("--path takes a value\n\n{USAGE}");
        };
        paths.push(path.clone());
    }
    Ok(paths)
}
