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

use crate::{
    BACKUP_BUCKET as BACKUP, BLOBS_BUCKET as PRIMARY, Nullish, column_lines, output, results, step,
    wrangler,
};

// The queue rows make the drain visit (and ledger) every verified blob,
// whether this run copies it or an earlier out-of-band copy already
// exists. MAX(archive_size) is the conservative expected size; the
// drain settles at the size its head observes.
const ENQUEUE: &str = "
  INSERT INTO backup_pending (key, bytes, enqueued_at)
    SELECT 'blobs/sha256/' || substr(checksum, 8), MAX(archive_size),
           strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    FROM revisions WHERE verification = 'verified' GROUP BY checksum
  ON CONFLICT (key) DO NOTHING";

// Verified only: the backup set holds exactly the content the registry
// serves as verified (`docs/architecture.md`, "Backups"); pending
// uploads are not backed up until their verdict, and rejected blobs are
// reclaimed.
const VERIFIED: &str = "SELECT DISTINCT checksum FROM revisions
  WHERE verification = 'verified'";

/// True for exactly the canonical stored spelling
/// (`sha256:<64 lowercase hex>`, the `revisions.checksum` column
/// contract).  A checksum that is not one means the enumeration
/// answered something other than the query asked for, and copying
/// blobs under a key derived from it would write to an
/// attacker-shaped key.
#[must_use]
pub fn is_checksum(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
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
    for checksum in column_lines(&results(&answer)?, "checksum", Nullish::Printed) {
        // The shell skipped the blank line its here-string produced for
        // an empty enumeration, then refused anything else unexpected.
        if checksum.is_empty() {
            continue;
        }
        if !is_checksum(&checksum) {
            bail!("unexpected checksum: {checksum}");
        }
        let key = format!("blobs/sha256/{}", &checksum["sha256:".len()..]);

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
