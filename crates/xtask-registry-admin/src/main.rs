//! Command-line shim for the registry operator commands; the commands
//! themselves live in the library.

use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

/// Operator commands against the hosted registry, run from the
/// repository root through their Cargo aliases.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read-only audit of the backup bucket
    /// (`cargo registry-backup-audit`).
    BackupAudit {
        /// List the divergent keys.
        #[arg(long)]
        keys: bool,
    },
    /// Copy verified blobs missing from the backup bucket
    /// (`cargo registry-backup-backfill`).
    BackupBackfill,
    /// Safe diagnostics bundle (`cargo registry-diagnose`).
    Diagnose,
    /// The cost governor's ledger: usage | compare | reconcile
    /// [--keys] | release <pool> <key> | wipe
    /// (`cargo registry-governor`).
    Governor {
        // Raw, so the one usage line in `governor` below answers every
        // malformed spelling.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    /// Refuse unless meta.launched is 'false'; destructive maintenance
    /// runs this first (`cargo registry-launch-guard`).
    LaunchGuard {
        // `--remote`/`--local` reach the library raw, which parses and
        // refuses them itself.
        #[arg(allow_hyphen_values = true)]
        mode: Option<String>,
    },
    /// Apply the D1 migrations; --remote refreshes the
    /// migrations-applied stamp the deploy gate reads
    /// (`cargo registry-migrate`).
    Migrate {
        #[arg(allow_hyphen_values = true)]
        mode: Option<String>,
    },
    /// Restore the latest dump into a scratch database and compare it
    /// against the live one (`cargo registry-restore-drill`).
    RestoreDrill,
    /// Inspect pending versions and PATCH the verdicts
    /// (`cargo registry-verify`).
    Verify,
    /// Drop and recreate the registry's data from zero, pre-launch
    /// only; --local resets the emulated .wrangler/ state instead
    /// (`cargo registry-wipe`).
    Wipe {
        #[arg(allow_hyphen_values = true)]
        mode: Option<String>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::BackupAudit { keys } => xtask_registry_admin::audit::run(keys),
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
        // No argument is the REMOTE wipe.
        Command::Wipe { mode } => xtask_registry_admin::wipe::run(mode.as_deref()),
    }
}

/// The governor's own argument surface, kept raw so the one usage line
/// answers every malformed spelling.
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
