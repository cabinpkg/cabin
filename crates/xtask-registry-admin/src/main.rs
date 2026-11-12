//! Command-line shim for the registry operator commands; the commands
//! themselves live in the library.

use std::process::ExitCode;

use anyhow::Result;
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
    /// Safe diagnostics bundle (`cargo registry-diagnose`).
    Diagnose,
    /// The cost governor's usage snapshot
    /// (`cargo registry-governor usage`).
    Governor {
        #[command(subcommand)]
        action: Governor,
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
    /// Inspect pending versions and PATCH the verdicts
    /// (`cargo registry-verify`).
    Verify {
        /// Mint two OIDC tokens and print the first one's claims,
        /// then walk the trusted-publishing exchange round trip,
        /// instead of verifying: proves the workflow's `id-token`
        /// grant, the per-mint `jti` freshness, and the exchange
        /// without a verdict.  The `VERIFY_CHECK_OIDC=true` env form
        /// is the workflow-dispatch input's spelling of the same flag
        /// (the `run:` block stays a plain invocation).
        #[arg(long)]
        check_oidc: bool,
    },
    /// Drop and recreate the registry's data from zero, pre-launch
    /// only (`cargo registry-wipe`).
    Wipe {
        /// Reset the emulated .wrangler/ state instead of the deployed
        /// registry.
        #[arg(long)]
        local: bool,
    },
}

#[derive(Subcommand)]
enum Governor {
    /// Print the operation windows' usage.
    Usage,
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
        Command::Diagnose => xtask_registry_admin::diagnose::run(),
        Command::Governor {
            action: Governor::Usage,
        } => xtask_registry_admin::governor::run(),
        Command::LaunchGuard { target } => xtask_registry_admin::launch_guard::run(target.mode()),
        Command::Migrate { target } => xtask_registry_admin::migrate::run(target.mode()),
        Command::Verify { check_oidc } => {
            // GitHub renders a boolean dispatch input as the string
            // "true"/"false", so only the exact "true" opts in.
            if check_oidc || std::env::var("VERIFY_CHECK_OIDC").as_deref() == Ok("true") {
                xtask_registry_admin::verify::check_oidc()
            } else {
                xtask_registry_admin::verify::run()
            }
        }
        Command::Wipe { local } => {
            xtask_registry_admin::wipe::run(if local { Mode::Local } else { Mode::Remote })
        }
    }
}
