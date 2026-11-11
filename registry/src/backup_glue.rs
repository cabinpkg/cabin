//! Cloudflare glue for the verified-archive backup (`docs/runbook.md`,
//! "Disaster recovery"): the backup-queue drain that replicates blobs from
//! BLOBS to BACKUP. Like the rest of the wasm glue this file is thin I/O
//! wiring.

use serde::Deserialize;
use worker::{D1Database, Data, Env, FixedLengthStream, console_error};

use crate::glue::{CountRecord, commit_object, consume_one};
use crate::governor::{Consume, Decision, OpPool, Reserve, StoragePool};
use crate::governor_client::{self, Gate};
use crate::sql;

#[derive(Deserialize)]
struct BackupPendingRecord {
    key: String,
}

/// Drains the verified-artifact backup queue (`docs/runbook.md`,
/// "Disaster recovery"): for each due row, replicate the blob from
/// BLOBS to the append-only BACKUP bucket and delete the row on
/// success. Runs off the verdict response path and on every breaker
/// cron pass; the queue row is the durable record, so any failure
/// here just leaves the work for the next pass (and the stale-row
/// alert if it keeps failing). Every billable call is charged to the
/// infrastructure pools first, and a governor refusal stops the drain
/// - fail closed, retry next pass.
pub(crate) async fn drain_backup_queue(env: &Env) {
    let (Ok(db), Ok(blobs), Ok(backup)) = (env.d1("DB"), env.bucket("BLOBS"), env.bucket("BACKUP"))
    else {
        console_error!("backup drain: a binding is missing");
        return;
    };
    // Bounded per call: at most five keyset-paginated pages of 10, so
    // rows a pass keeps (a missing primary blob or body) are walked
    // past instead of re-read forever.
    let mut cursor = String::new();
    for _ in 0..5 {
        let rows: Vec<BackupPendingRecord> = match db
            .prepare(sql::LIST_BACKUP_PENDING)
            .bind(&[cursor.as_str().into()])
        {
            Ok(statement) => match statement.all().await {
                Ok(result) => match result.results() {
                    Ok(rows) => rows,
                    Err(err) => {
                        console_error!("backup drain: queue rows did not parse: {err}");
                        return;
                    }
                },
                Err(err) => {
                    console_error!("backup drain: queue listing failed: {err}");
                    return;
                }
            },
            Err(err) => {
                console_error!("backup drain: queue listing failed to bind: {err}");
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        let page = rows.len();
        for row in &rows {
            if !backup_one(env, &db, &blobs, &backup, row).await {
                return;
            }
        }
        if page < 10 {
            return;
        }
        if let Some(last) = rows.last() {
            cursor.clone_from(&last.key);
        }
    }
}

/// Replicates one queue row; `false` stops the drain pass (governor
/// refusal or a storage error worth backing off from).
async fn backup_one(
    env: &Env,
    db: &D1Database,
    blobs: &worker::Bucket,
    backup: &worker::Bucket,
    row: &BackupPendingRecord,
) -> bool {
    let delete_row = |key: String| async move {
        if let Ok(statement) = db.prepare(sql::DELETE_BACKUP_PENDING).bind(&[key.into()])
            && statement.run().await.is_err()
        {
            console_error!("backup drain: deleting a queue row failed");
        }
    };
    let Some(hex) = row.key.strip_prefix(crate::checksum::BLOB_KEY_PREFIX) else {
        console_error!("backup drain: malformed queue key {}", row.key);
        delete_row(row.key.clone()).await;
        return true;
    };
    // The queue key keeps the OCI-style bare-hex layout; the column
    // comparison needs the canonical stored spelling.
    let checksum = crate::checksum::from_hex(hex);
    // Only blobs the registry still serves as verified content are
    // worth a backup copy: a rejection (or replacement) that landed
    // after the enqueue retires the row instead.
    let live: Result<Option<CountRecord>, _> = match db
        .prepare(sql::COUNT_LIVE_VERIFIED_BLOB_REFERENCES)
        .bind(&[checksum.as_str().into()])
    {
        Ok(statement) => statement.first(None).await,
        Err(err) => Err(err),
    };
    match live {
        Ok(Some(count)) if count.n > 0 => {}
        Ok(_) => {
            // Dead row: retire under the in-statement liveness
            // re-check, so a verdict racing this pass cannot lose its
            // just-enqueued work.
            if let Ok(statement) = db
                .prepare(sql::RETIRE_DEAD_BACKUP_PENDING)
                .bind(&[row.key.as_str().into(), checksum.as_str().into()])
                && statement.run().await.is_err()
            {
                console_error!("backup drain: retiring a dead queue row failed");
            }
            return true;
        }
        Err(err) => {
            console_error!("backup drain: liveness check for {} failed: {err}", row.key);
            return false;
        }
    }

    match governor_client::decide(env, &consume_one(OpPool::BInfra)).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_error!("backup drain: governor refused the backup head; stopping");
            return false;
        }
    }
    match backup.head(&row.key).await {
        Ok(Some(object)) => {
            // Already replicated (a shared checksum, a retry after a
            // lost acknowledgement, or an out-of-band backfill copy):
            // settle the ledger at the size the head OBSERVED - the
            // object is reality, the queue row only expected it. The
            // row is retired only once the commit is CONFIRMED: for an
            // out-of-band copy there is no reservation behind it, so
            // deleting the row on an unacknowledged commit would leave
            // a real backup object unledgered with nothing left to
            // retry.
            let commit = commit_object(StoragePool::Backup, &row.key, object.size());
            match governor_client::decide(env, &commit).await {
                Gate::Allowed => {
                    delete_row(row.key.clone()).await;
                    true
                }
                Gate::Refused(_) => {
                    console_error!(
                        "backup drain: ledger commit for {} unconfirmed; keeping the row",
                        row.key
                    );
                    false
                }
            }
        }
        Ok(None) => match copy_blob_to_backup(env, blobs, backup, &row.key).await {
            CopyOutcome::Copied(len) => {
                // Same confirmed-commit rule as the head arm. A lost
                // commit here would still be covered by the copy's
                // reservation, but keeping the row costs one retry and
                // keeps the settled/reserved distinction honest.
                let commit = commit_object(StoragePool::Backup, &row.key, len);
                match governor_client::decide(env, &commit).await {
                    Gate::Allowed => {
                        delete_row(row.key.clone()).await;
                        true
                    }
                    Gate::Refused(_) => {
                        console_error!(
                            "backup drain: ledger commit for {} unconfirmed; keeping the row",
                            row.key
                        );
                        false
                    }
                }
            }
            CopyOutcome::KeepRow => true,
            CopyOutcome::Stop => false,
        },
        Err(err) => {
            console_error!("backup drain: head for {} failed: {err}", row.key);
            false
        }
    }
}

enum CopyOutcome {
    /// The copy landed; the value is the object's byte length.
    Copied(u64),
    /// This row cannot be copied right now (missing primary blob);
    /// keep it so the stale-row alert keeps firing.
    KeepRow,
    /// Back off: governor refusal or a storage error.
    Stop,
}

/// The charged copy itself: one infrastructure Class B read of the
/// primary blob, then one Class A put with a backup-storage
/// reservation, both taken before their calls.
async fn copy_blob_to_backup(
    env: &Env,
    blobs: &worker::Bucket,
    backup: &worker::Bucket,
    key: &str,
) -> CopyOutcome {
    match governor_client::decide(env, &consume_one(OpPool::BInfra)).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_error!("backup drain: governor refused the source read; stopping");
            return CopyOutcome::Stop;
        }
    }
    let object = match blobs.get(key).execute().await {
        Ok(Some(object)) => object,
        Ok(None) => {
            // A verified version's primary blob is missing: loud, and
            // the row stays so the alert keeps firing until an
            // operator intervenes.
            console_error!("backup drain: primary blob {key} is missing");
            return CopyOutcome::KeepRow;
        }
        Err(err) => {
            console_error!("backup drain: reading {key} failed: {err}");
            return CopyOutcome::Stop;
        }
    };
    // The stored size is the known length the fixed-length streaming
    // put needs, so the archive never has to fit in Worker memory; the
    // put fails on any disagreement with the actual body.
    let len = object.size();
    let Some(body) = object.body() else {
        console_error!("backup drain: blob {key} has no body");
        return CopyOutcome::KeepRow;
    };
    let stream = match body.stream() {
        Ok(stream) => stream,
        Err(err) => {
            console_error!("backup drain: streaming {key} failed: {err}");
            return CopyOutcome::Stop;
        }
    };

    let admit = Decision {
        consume: vec![Consume {
            pool: OpPool::AInfra,
            n: 1,
            principal: None,
            principal_cap: None,
        }],
        reserve: vec![Reserve {
            pool: StoragePool::Backup,
            key: key.to_owned(),
            bytes: len,
        }],
        ..Decision::default()
    };
    match governor_client::decide(env, &admit).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_error!("backup drain: governor refused the backup put; stopping");
            return CopyOutcome::Stop;
        }
    }
    if let Err(err) = backup
        .put(key, Data::Stream(FixedLengthStream::wrap(stream, len)))
        .execute()
        .await
    {
        // The put's outcome is uncertain: the reservation stays held
        // (conservative) and the queue row retries next pass.
        console_error!("backup drain: replicating {key} failed: {err}");
        return CopyOutcome::Stop;
    }
    CopyOutcome::Copied(len)
}
