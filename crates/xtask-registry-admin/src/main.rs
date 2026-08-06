//! Command-line shim for the registry operator commands; the commands
//! themselves live in the library.

use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{ArgAction, Parser, Subcommand};

const USAGE: &str = "\
usage: xtask-registry-admin <COMMAND>

Operator commands against the hosted registry, run from the repository
root through their Cargo aliases.

commands:
  backup-audit [--keys]
                   read-only audit of the backup bucket, listing the
                   divergent keys with --keys
                   (`cargo registry-backup-audit`)
  backup-backfill  copy verified blobs missing from the backup bucket
                   (`cargo registry-backup-backfill`)
  diagnose         safe diagnostics bundle (`cargo registry-diagnose`)
  governor <usage|compare|reconcile [--keys]|release <pool> <key>|wipe>
                   the cost governor's ledger: inspect, compare against
                   D1, rebuild increase-only, release an entry against
                   evidence, or reset it pre-launch
                   (`cargo registry-governor`)
  launch-guard <--remote|--local>
                   refuse unless meta.launched is 'false'; destructive
                   maintenance runs this first
                   (`cargo registry-launch-guard`)
  migrate <--remote|--local>
                   apply the D1 migrations; --remote refreshes the
                   migrations-applied stamp the deploy gate reads
                   (`cargo registry-migrate`)
  restore-drill    restore the latest dump into a scratch database and
                   compare it against the live one
                   (`cargo registry-restore-drill`)
  verify           inspect pending versions and PATCH the verdicts
                   (`cargo registry-verify`)
  wipe [--local]   drop and recreate the registry's data from zero,
                   pre-launch only; --local resets the emulated
                   .wrangler/ state instead (`cargo registry-wipe`)

options:
  -h, --help  show this help
";

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// No command takes `--help`, as none of the shell scripts did: the
/// drill must not answer about an argument where it once took one (see
/// `the_restore_drill_takes_no_arguments`), and the mode-taking
/// commands have to see the argument they were given rather than
/// answer about it.  The binary's own `-h`/`--help` stays clap's,
/// answered with [`USAGE`].
#[derive(Subcommand)]
enum Command {
    #[command(disable_help_flag = true)]
    BackupAudit {
        /// Counted rather than a flag, so that a repeat is refused
        /// instead of silently meaning the same thing.
        #[arg(long, action = ArgAction::Count)]
        keys: u8,
    },
    #[command(disable_help_flag = true)]
    BackupBackfill,
    #[command(disable_help_flag = true)]
    Diagnose,
    #[command(disable_help_flag = true)]
    Governor {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    #[command(disable_help_flag = true)]
    LaunchGuard {
        #[arg(allow_hyphen_values = true)]
        mode: Option<String>,
    },
    #[command(disable_help_flag = true)]
    Migrate {
        #[arg(allow_hyphen_values = true)]
        mode: Option<String>,
    },
    #[command(disable_help_flag = true)]
    RestoreDrill,
    #[command(disable_help_flag = true)]
    Verify,
    #[command(disable_help_flag = true)]
    Wipe {
        #[arg(allow_hyphen_values = true)]
        mode: Option<String>,
    },
}

fn main() -> ExitCode {
    // Clap swallows a bare `--` as its option delimiter, so every
    // command here would run with one appended and the argument-taking
    // ones would read as if nothing was given - `wipe --` reaching the
    // remote wipe's prompt. The shell these ports replace passed `--`
    // through as an argument, and each command refused it.
    if std::env::args_os().any(|argument| argument == "--") {
        eprintln!("error: unexpected argument: --\n\n{USAGE}");
        return ExitCode::FAILURE;
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return refused(&err),
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// A parse failure, in this shim's own terms: `--help` is an answer on
/// stdout, an unknown command keeps the wording the runbooks quote, and
/// a refusal exits 1 - the status every other refusal here carries, and
/// the one the differentials compare against the shell's - rather than
/// clap's 2.
fn refused(err: &clap::Error) -> ExitCode {
    match (err.kind(), err.get(ContextKind::InvalidSubcommand)) {
        (ErrorKind::DisplayHelp, _) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        (ErrorKind::InvalidSubcommand, Some(ContextValue::String(name))) => {
            eprintln!("error: unknown command: {name}\n\n{USAGE}");
        }
        _ => {
            let _ = err.print();
        }
    }
    ExitCode::FAILURE
}

fn run(cli: Cli) -> Result<()> {
    let Some(command) = cli.command else {
        bail!("no command named\n\n{USAGE}");
    };
    match command {
        Command::BackupAudit { keys } => {
            if keys > 1 {
                bail!("unexpected argument: --keys\n\n{USAGE}");
            }
            xtask_registry_admin::audit::run(keys == 1)
        }
        Command::BackupBackfill => xtask_registry_admin::backfill::run(),
        Command::Diagnose => xtask_registry_admin::diagnose::run(),
        Command::Governor { rest } => governor(&rest),
        Command::LaunchGuard { mode: Some(mode) } => {
            let Some(mode) = xtask_registry_admin::launch_guard::Mode::parse(&mode) else {
                bail!("launch guard: unknown mode: {mode} (expected --remote or --local)");
            };
            xtask_registry_admin::launch_guard::run(mode)
        }
        Command::LaunchGuard { mode: None } => {
            bail!("usage: cargo registry-launch-guard <--remote|--local>")
        }
        Command::Migrate { mode: Some(mode) } => xtask_registry_admin::migrate::run(&mode),
        Command::Migrate { mode: None } => bail!("{}", xtask_registry_admin::migrate::USAGE),
        Command::RestoreDrill => xtask_registry_admin::restore_drill::run(),
        Command::Verify => xtask_registry_admin::verify::run(),
        // No argument is the REMOTE wipe, as it was for the script.
        Command::Wipe { mode } => xtask_registry_admin::wipe::run(mode.as_deref()),
    }
}

/// The governor's own argument surface, which the shell spelled as a
/// `case` over `$2` and `$3`.  Its arguments reach it raw, so that the
/// one usage line answers every malformed spelling, as the `case` did.
fn governor(rest: &[String]) -> Result<()> {
    use xtask_registry_admin::governor::{Action, Pool};

    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    let action = match rest.as_slice() {
        ["usage"] => Action::Usage,
        ["compare"] => Action::Compare,
        ["reconcile"] => Action::Reconcile { keys: false },
        ["reconcile", "--keys"] => Action::Reconcile { keys: true },
        ["release", pool, key] => {
            let Some(pool) = Pool::parse(pool) else {
                bail!("usage: cargo registry-governor release <primary|backup|dump> <key>");
            };
            Action::Release {
                pool,
                key: (*key).to_owned(),
            }
        }
        ["wipe"] => Action::Wipe,
        _ => bail!(
            "usage: cargo registry-governor <usage|compare|reconcile [--keys]|\
             release <pool> <key>|wipe>"
        ),
    };
    xtask_registry_admin::governor::run(&action)
}
