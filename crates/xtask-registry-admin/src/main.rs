//! Command-line shim for the registry operator commands; the commands
//! themselves live in the library.

use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use xtask_registry_admin::launch_guard::Mode;

/// Operator commands against the hosted registry, run from the
/// repository root through their Cargo aliases.
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// `--remote` or `--local`, exactly one.
#[derive(clap::Args)]
#[group(required = true, multiple = false)]
struct Target {
    /// The deployed registry.
    #[arg(long)]
    remote: bool,
    /// The emulated .wrangler/ state.
    #[arg(long)]
    local: bool,
}

impl Target {
    fn mode(&self) -> Mode {
        if self.remote {
            Mode::Remote
        } else {
            Mode::Local
        }
    }
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
        #[command(flatten)]
        target: Target,
    },
    /// Apply the D1 migrations; --remote refreshes the
    /// migrations-applied stamp the deploy gate reads
    /// (`cargo registry-migrate`).
    Migrate {
        #[command(flatten)]
        target: Target,
    },
    /// Restore the latest dump into a scratch database and compare it
    /// against the live one (`cargo registry-restore-drill`).
    RestoreDrill,
    /// Inspect pending versions and PATCH the verdicts
    /// (`cargo registry-verify`).
    Verify,
    /// Drop and recreate the registry's data from zero, pre-launch
    /// only (`cargo registry-wipe`).
    Wipe {
        /// Reset the emulated .wrangler/ state instead of the deployed
        /// registry.
        #[arg(long)]
        local: bool,
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
        Command::LaunchGuard { target } => xtask_registry_admin::launch_guard::run(target.mode()),
        Command::Migrate { target } => xtask_registry_admin::migrate::run(target.mode()),
        Command::RestoreDrill => xtask_registry_admin::restore_drill::run(),
        Command::Verify => xtask_registry_admin::verify::run(),
        Command::Wipe { local } => {
            xtask_registry_admin::wipe::run(if local { Mode::Local } else { Mode::Remote })
        }
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
