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
  restore-drill    restore the latest dump into a scratch database and
                   compare it against the live one
                   (`cargo registry-restore-drill`)

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
    let rest: Vec<String> = arguments.collect();
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    match (command.as_str(), rest.as_slice()) {
        ("-h" | "--help", []) => {
            print!("{USAGE}");
            Ok(())
        }
        ("backup-audit", []) => xtask_registry_admin::audit::run(false),
        ("backup-audit", ["--keys"]) => xtask_registry_admin::audit::run(true),
        ("backup-backfill", []) => xtask_registry_admin::backfill::run(),
        ("diagnose", []) => xtask_registry_admin::diagnose::run(),
        ("governor", rest) => governor(rest),
        ("launch-guard", [mode]) => {
            let Some(mode) = xtask_registry_admin::launch_guard::Mode::parse(mode) else {
                bail!("launch guard: unknown mode: {mode} (expected --remote or --local)");
            };
            xtask_registry_admin::launch_guard::run(mode)
        }
        ("launch-guard", []) => bail!("usage: cargo registry-launch-guard <--remote|--local>"),
        ("restore-drill", []) => xtask_registry_admin::restore_drill::run(),
        (other, []) => bail!("unknown command: {other}\n\n{USAGE}"),
        (_, [extra, ..]) => bail!("unexpected argument: {extra}\n\n{USAGE}"),
    }
}

/// The governor's own argument surface, which the shell spelled as a
/// `case` over `$2` and `$3`.
fn governor(rest: &[&str]) -> Result<()> {
    use xtask_registry_admin::governor::{Action, Pool};

    let action = match rest {
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
