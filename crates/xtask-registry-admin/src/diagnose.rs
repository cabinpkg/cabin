//! Safe diagnostics bundle (`registry/docs/runbook.md`, "Logs"): the
//! aggregate state an incident report or a bug thread needs, and
//! nothing that must not leave the operator's terminal - no tokens or
//! token hashes, no object keys or content checksums, no package names,
//! no user data.  Counts, modes, timestamps and version identifiers
//! only; every section names its source so a reader can go deeper with
//! the runbook.
//!
//! Read-only.  Requires wrangler auth; with `REGISTRY_VERIFY_TOKEN` set
//! it also includes the governor's usage snapshot (aggregates only).

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use sha2::{Digest as _, Sha256};

use crate::{display, key_value, output, registry_dir, repo_root, results, step, wrangler};

// `last_backup_key` is the one object key the disclosure rule in
// `registry/docs/runbook.md` allows out. What makes it safe is the
// write path, not a check here: `backup_glue` is the only writer and
// stores `backup::dump_object_key`'s `d1/<date>.sql`, built from the
// same timestamp `last_backup_at` already prints. Re-deriving it to
// compare would only restate what a D1 writer could have edited
// anyway, and withholding it would cost the operator a cross-check.
const SERVICE_STATE: &str = "
  SELECT key, value FROM meta WHERE key IN
    ('service_mode', 'service_mode_reason', 'registry_generation',
     'launched', 'last_backup_at', 'last_backup_key', 'total_stored_bytes')
  ORDER BY key";

const COUNTS: &str = "
  SELECT
    (SELECT COUNT(*) FROM users) AS users,
    (SELECT COUNT(*) FROM scopes) AS scopes,
    (SELECT COUNT(*) FROM packages) AS packages,
    (SELECT COUNT(*) FROM versions) AS versions,
    (SELECT COUNT(*) FROM revisions) AS revisions,
    (SELECT COUNT(*) FROM revisions WHERE verification = 'pending') AS pending,
    (SELECT COUNT(*) FROM revisions WHERE verification = 'verified') AS verified,
    (SELECT COUNT(*) FROM revisions WHERE verification = 'rejected') AS rejected,
    (SELECT COUNT(*) FROM versions WHERE yanked = 1) AS yanked,
    (SELECT COUNT(*) FROM tokens) AS tokens,
    (SELECT COUNT(*) FROM backup_pending) AS backup_pending";

/// # Errors
///
/// If a D1 read fails or answers in an unexpected shape.  A failing
/// deploy-configuration check, a pending migrations stamp and an
/// unavailable deployments listing are reported, not errors: the point
/// of the bundle is to describe a service that may be unwell.
pub fn run() -> Result<()> {
    step("deploy configuration (cargo check-deploy)");
    let checked = Command::new("cargo")
        .arg("check-deploy")
        .current_dir(repo_root())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if checked {
        println!("    config OK");
    } else {
        println!(
            "    CONFIG CHECK FAILED - run cargo check-deploy from the repository root for detail"
        );
    }
    migrations_stamp()?;

    step("service state (meta; docs/runbook.md \"Budget breaker and service mode\")");
    let state = output(&mut wrangler(&[
        "d1",
        "execute",
        "DB",
        "--remote",
        "--json",
        "--command",
        SERVICE_STATE,
    ]))?;
    for row in results(&state)? {
        let (key, value) = key_value(&row)?;
        println!("    {key}: {value}");
    }

    step("corpus and queue counts (D1)");
    let counts = output(&mut wrangler(&[
        "d1",
        "execute",
        "DB",
        "--remote",
        "--json",
        "--command",
        COUNTS,
    ]))?;
    let counts = results(&counts)?;
    // One row, always: the SELECT is a list of scalar sub-selects. No
    // row means the answer was not the one this asks for, which the
    // shell hit as `Object.entries(undefined)`.
    let row = counts.first().context("the counts query answered no row")?;
    for (key, value) in row {
        println!("    {key}: {}", display(value));
    }

    governor_ledger()?;

    step("worker deployments (wrangler deployments list)");
    match output(wrangler(&["deployments", "list"]).stderr(Stdio::null())) {
        Ok(listing) => {
            for line in listing.lines().take(30) {
                println!("    {line}");
            }
        }
        // The shell wrote this fallback too, but behind a `||` on a
        // pipeline ending in `sed`, which succeeds whatever wrangler
        // did - so the section silently vanished instead. A
        // diagnostics bundle that drops a section without saying so is
        // worse than one that says the token could not read it.
        Err(_) => println!("    (deployments list unavailable with this token)"),
    }

    println!("diagnose OK");
    Ok(())
}

/// Whether the applied-migrations stamp still matches the migrations,
/// which is what gates deploys (`registry.yml`, "Skip until changed D1
/// migrations are applied by hand").
/// The files `migrations/*.sql` expands to, in the order the shell
/// concatenated them.
///
/// # Errors
///
/// If the directory cannot be read.
pub fn migration_files(directory: &std::path::Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(directory)
        .with_context(|| format!("read {}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .with_context(|| format!("read {}", directory.display()))?;
    files.retain(|path| {
        let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
            return false;
        };
        // A bash glob skips dotfiles unless `dotglob` is set, so an
        // operator's `.draft.sql` scratch file is outside the stamp.
        // `registry.yml`'s deploy gate and `scripts/migrate.sh` hash
        // the same glob; counting one here would report PENDING while
        // deploys stay unblocked.
        !name.starts_with('.') && path.extension().is_some_and(|kind| kind == "sql")
    });
    // A glob expands sorted; `read_dir` does not.
    files.sort();
    Ok(files)
}

fn migrations_stamp() -> Result<()> {
    let registry = registry_dir();
    let files = migration_files(&registry.join("migrations"))?;
    let mut hasher = Sha256::new();
    for file in &files {
        hasher.update(std::fs::read(file).with_context(|| format!("read {}", file.display()))?);
    }
    let stamp = cabin_core::hash::hex_digest(&hasher.finalize());

    // An unreadable stamp file is a mismatch, not an abort: the shell
    // compared against an empty command substitution and carried on
    // reporting. `$(cat ...)` strips trailing newlines and nothing
    // else, and `registry.yml`'s deploy gate compares the same way, so
    // trimming any wider would claim "current" while deploys stay
    // blocked.
    let applied = registry.join("migrations-applied");
    let recorded = std::fs::read_to_string(&applied).unwrap_or_default();
    if stamp == recorded.trim_end_matches('\n') {
        println!("    migrations stamp: current (deploys unblocked)");
    } else {
        println!("    migrations stamp: PENDING - deploys stay skipped until");
        println!("    scripts/migrate.sh --remote (or a wipe) lands and is committed");
    }
    Ok(())
}

/// The governor's usage snapshot, which needs a token the operator may
/// not have to hand.  Its own `==>` heading is dropped and the rest
/// indented, so the snapshot reads as a section of this bundle.
///
/// A token that is present but does not work is an error, as the
/// shell's `pipefail` made it: a bundle that says `diagnose OK` after a
/// section failed is worse than no bundle.
fn governor_ledger() -> Result<()> {
    let Some(token) = std::env::var_os("REGISTRY_VERIFY_TOKEN").filter(|t| !t.is_empty()) else {
        step("governor ledger: skipped (REGISTRY_VERIFY_TOKEN unset)");
        return Ok(());
    };
    step("governor ledger (scripts/governor.sh usage)");
    let snapshot = output(
        Command::new("bash")
            .args(["scripts/governor.sh", "usage"])
            .env("REGISTRY_VERIFY_TOKEN", token)
            .stderr(Stdio::inherit())
            .current_dir(registry_dir()),
    );
    for line in snapshot
        .context("read the governor usage snapshot")?
        .lines()
        .skip(1)
    {
        println!("    {line}");
    }
    Ok(())
}
