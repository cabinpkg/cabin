//! One-shot backup reconciliation (`registry/docs/runbook.md`,
//! "Disaster recovery"): copies every **verified** archive blob that is
//! missing from the BACKUP bucket.  The deployed Worker replicates
//! through the durable `backup_pending` queue on its own; this is the
//! manual recovery path for a drain that keeps failing, or for seeding
//! backups over pre-existing data.
//!
//! It UPSERTS one `backup_pending` row per verified checksum and never
//! deletes any: the Worker's drain retires each row itself (its
//! existence head finds the copy, settles the governor's backup ledger
//! at the observed size, and deletes the row), so every copy made
//! here - and every pre-queue verified blob - is absorbed, ledger
//! included, within one breaker cron pass.  Deleting or skipping rows
//! here would leave the governor's backup ledger understating reality.
//!
//! Requires `CLOUDFLARE_API_TOKEN` in the environment.  Idempotent:
//! re-running skips blobs the backup already holds.  The copies run
//! outside the Worker, so they are not charged to the governor's
//! operation pools (they bill as ordinary R2 usage on the operator's
//! account activity).
//!
//! Ceilings, where this deliberately stops short of the shell it
//! replaces.  All three need a broken host or an interrupted run to
//! reach, and no caller reads anything but zero/non-zero:
//!
//! - a failing wrangler ends the run with status 1, where `set -e`
//!   ended it with wrangler's own status;
//! - the staging file is removed when the run returns, but not when a
//!   signal kills it, where the shell's `trap ... EXIT` removed it
//!   either way;
//! - a `stat` that fails on the blob wrangler just wrote ends the run,
//!   where the shell printed the copy line with a blank byte count and
//!   carried on.  The copy itself has already succeeded in both.

use std::process::Stdio;

use anyhow::{Context, Result, bail};

use crate::{output, results, step, wrangler};

const PRIMARY: &str = "cabin-registry-blobs";
const BACKUP: &str = "cabin-registry-backup";

// The queue rows make the drain visit (and ledger) every verified blob,
// whether this run copies it or an earlier out-of-band copy already
// exists. MAX(archive_size) is the conservative expected size; the
// drain settles at the size its head observes.
const ENQUEUE: &str = "
  INSERT INTO backup_pending (key, bytes, enqueued_at)
    SELECT 'blobs/sha256/' || checksum, MAX(archive_size),
           strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    FROM revisions WHERE verification = 'verified' GROUP BY checksum
  ON CONFLICT (key) DO NOTHING";

// Verified only: the backup set holds exactly the content the registry
// serves as verified (`docs/architecture.md`, "Backups"); pending
// uploads are not backed up until their verdict, and rejected blobs are
// reclaimed.
const VERIFIED: &str = "SELECT DISTINCT checksum FROM revisions
  WHERE verification = 'verified'";

/// True for exactly what the shell's `^[0-9a-f]{64}$` matched.  A
/// checksum that is not one means the enumeration answered something
/// other than the query asked for, and copying blobs under it would
/// write to an attacker-shaped key.
#[must_use]
pub fn is_checksum(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The lines `d1_column` fed the copy loop: one `console.log` per row,
/// captured by `$(...)` and split by `while IFS= read -r`.
#[must_use]
pub fn column_lines(
    rows: &[serde_json::Map<String, serde_json::Value>],
    column: &str,
) -> Vec<String> {
    let mut text = String::new();
    for row in rows {
        // `console.log` is not the `${...}` the diagnostics bundle
        // used: it prints a string verbatim and renders anything else
        // through Node's inspect, which the checksum grammar then
        // refuses. This is not inspect's exact text - an array renders
        // `["b"]` where Node wrote `[ 'b' ]` - but nothing it can
        // produce matches the grammar, and rendering rather than
        // refusing here keeps the refusal where the shell had it:
        // inside the loop, after the rows before it were copied.
        match row.get(column) {
            Some(serde_json::Value::String(value)) => text.push_str(value),
            Some(other) => text.push_str(&other.to_string()),
            None => text.push_str("undefined"),
        }
        text.push('\n');
    }
    // Bash cannot hold a NUL in a variable: command substitution drops
    // it, so `<32 hex>\0<32 hex>` reached the loop as 64 hex digits and
    // passed the grammar below.
    text.retain(|character| character != '\0');
    // `$(...)` strips trailing newlines and nothing else, and the loop
    // splits on those that remain - so an empty enumeration still
    // yields the single blank line the here-string fed it, and a value
    // carrying a newline still becomes two iterations.
    text.trim_end_matches('\n')
        .split('\n')
        .map(str::to_owned)
        .collect()
}

/// Runs the backfill.
///
/// # Errors
///
/// If any wrangler invocation fails, if the enumeration answers a
/// checksum that is not one, or if a copy cannot be made.
pub fn run() -> Result<()> {
    step("enqueueing every verified blob for the worker's drain");
    let enqueued = wrangler(&["d1", "execute", "DB", "--remote", "--command", ENQUEUE])
        // The shell discarded stdout here and kept wrangler's
        // diagnostics on the terminal.
        .stdout(Stdio::null())
        .status()
        .context("run wrangler d1 execute")?;
    if !enqueued.success() {
        bail!("enqueueing the verified blobs failed: {enqueued}");
    }

    step(&format!("copying verified blobs missing from {BACKUP}"));
    let blob = tempfile::NamedTempFile::new().context("create the staging file")?;
    let path = blob
        .path()
        .to_str()
        .context("the staging file has a non-UTF-8 path")?
        .to_owned();

    let answer = output(&mut wrangler(&[
        "d1",
        "execute",
        "DB",
        "--remote",
        "--json",
        "--command",
        VERIFIED,
    ]))?;
    let mut copied = 0_u32;
    let mut present = 0_u32;
    for checksum in column_lines(&results(&answer)?, "checksum") {
        // The shell skipped the blank line its here-string produced for
        // an empty enumeration, then refused anything else unexpected.
        if checksum.is_empty() {
            continue;
        }
        if !is_checksum(&checksum) {
            bail!("unexpected checksum: {checksum}");
        }
        let key = format!("blobs/sha256/{checksum}");

        // `r2 object` commands default to local state; this command
        // only ever targets deployed environments.
        let held = wrangler(&[
            "r2",
            "object",
            "get",
            &format!("{BACKUP}/{key}"),
            "--file",
            &path,
            "--remote",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run wrangler r2 object get")?;
        if held.success() {
            present += 1;
            continue;
        }

        for arguments in [
            [
                "r2",
                "object",
                "get",
                &format!("{PRIMARY}/{key}"),
                "--file",
                &path,
                "--remote",
            ],
            [
                "r2",
                "object",
                "put",
                &format!("{BACKUP}/{key}"),
                "--file",
                &path,
                "--remote",
            ],
        ] {
            let status = wrangler(&arguments)
                .status()
                .context("run wrangler r2 object")?;
            if !status.success() {
                bail!("{} {key}: {status}", arguments[2]);
            }
        }
        let bytes = std::fs::metadata(&path)
            .with_context(|| format!("stat {path}"))?
            .len();
        println!("    copied {key} ({bytes} bytes)");
        copied += 1;
    }

    println!("backup backfill OK (copied {copied}, already present {present})");
    Ok(())
}
