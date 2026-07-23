//! Cloudflare glue for the two backup jobs (`docs/runbook.md`,
//! "Disaster recovery"): the nightly D1 dump - drive the D1 export
//! REST endpoint, stream the returned dump into the BACKUP bucket
//! while validating it (`crate::backup`), verify the stored object by
//! re-reading it, prune dumps beyond retention, and record success in
//! `meta` - and the verified-artifact backup-queue drain that
//! replicates blobs from BLOBS to BACKUP. Like the rest of the wasm
//! glue this file is thin I/O wiring; every decision lives in the
//! host-testable [`crate::backup`].

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt};
use serde::Deserialize;
use worker::{
    Bucket, D1Database, Data, Delay, Env, FixedLengthStream, Method, Request, console_error,
    console_log,
};

use crate::backup::{self, DumpCheck, DumpScanner, ExportPoll};
use crate::glue::{
    CountRecord, commit_object, consume_one, now_iso8601, post_json, read_meta, upsert_meta,
};
use crate::governor::{Consume, Decision, OpPool, Reserve, StoragePool};
use crate::governor_client::{self, Gate};
use crate::sql;

const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";

/// The export endpoint auto-cancels when polling stops, so poll at a
/// steady ~1 s cadence and give up after ~2 minutes - far beyond what
/// this database needs, far below the scheduled handler's wall-clock
/// allowance.
const EXPORT_POLL_ATTEMPTS: u32 = 120;

fn err(message: String) -> worker::Error {
    worker::Error::RustError(message)
}

/// One governed decision for the dump job; a refusal (or an
/// unreachable governor) fails the job - the freshness alert is the
/// operator's summons - rather than initiating unpaid R2 work.
async fn governed(env: &Env, decision: &Decision) -> worker::Result<()> {
    match governor_client::decide(env, decision).await {
        Gate::Allowed => Ok(()),
        Gate::Refused(refusal) => Err(err(format!(
            "the governor refused a dump-job operation: {refusal:?}"
        ))),
    }
}

fn consume_a_infra() -> Consume {
    Consume {
        pool: OpPool::AInfra,
        n: 1,
        principal: None,
        principal_cap: None,
    }
}

/// Admission for one dump-object write: one infrastructure Class A op
/// plus a storage reservation. A same-size retry is the idempotent
/// reservation; a retry whose export size CHANGED answers the
/// key-conflict refusal and fails the job - committing past admission
/// here would be spending past the hard cap, and the freshness alert
/// plus the next day's fresh key (or an operator release of the stale
/// entry) are the recovery paths.
async fn admit_dump_object(env: &Env, key: &str, bytes: u64) -> worker::Result<()> {
    let admit = Decision {
        consume: vec![consume_a_infra()],
        reserve: vec![Reserve {
            pool: StoragePool::Dump,
            key: key.to_owned(),
            bytes,
        }],
        ..Decision::default()
    };
    governed(env, &admit).await
}

/// Best-effort release after an affirmative delete of a dump object.
async fn release_dump_object(env: &Env, key: &str) {
    governor_client::settle(
        env,
        &Decision {
            release: vec![crate::governor::Release {
                pool: StoragePool::Dump,
                key: key.to_owned(),
            }],
            ..Decision::default()
        },
    )
    .await;
}

/// One nightly backup pass. Any failure leaves `meta.last_backup_at`
/// untouched, which the breaker cron turns into a freshness alert
/// within [`backup::STALE_AFTER_HOURS`].
pub async fn run_nightly_dump(env: &Env) -> worker::Result<()> {
    let db = env.d1("DB")?;
    let bucket = env.bucket("BACKUP")?;
    let now = now_iso8601();
    let Some(date) = crate::analytics::utc_date(&now).map(str::to_owned) else {
        return Err(err(format!("clock produced a non-ISO timestamp: {now}")));
    };
    let key = backup::dump_object_key(&date);

    // One validated dump per date: a same-day re-run (an ops
    // rehearsal's temporary schedule, most likely) must not stream over
    // the date's verified copy - a failed re-export would clobber it
    // while `meta.last_backup_key` still points there. A failed attempt
    // never updates the meta row, so a retry after a failure still
    // overwrites the bad object.
    if read_meta(&db, "last_backup_key").await? == Some(key.clone()) {
        console_log!("backup dump {key} is already stored and verified; skipping");
        return Ok(());
    }

    let signed_url = export_signed_url(env).await?;
    let check = stream_dump_into(env, &bucket, &key, &signed_url).await?;
    if let Some(error) = check.error() {
        remove_invalid_dump(env, &db, &bucket, &key).await;
        return Err(err(format!("dump {key} failed validation: {error}")));
    }
    if let Err(error) = verify_reread(env, &bucket, &key, &check.sha256_hex).await {
        remove_invalid_dump(env, &db, &bucket, &key).await;
        return Err(error);
    }
    // The dump's reservation settles into committed usage now that the
    // stored object is validated; best-effort, like every settle.
    governor_client::settle(
        env,
        &Decision {
            commit: vec![crate::governor::Commit {
                pool: StoragePool::Dump,
                key: key.clone(),
                bytes: check.bytes,
            }],
            ..Decision::default()
        },
    )
    .await;
    let sidecar_key = format!("{key}.sha256");
    let sidecar = format!("{}  {date}.sql\n", check.sha256_hex);
    admit_dump_object(env, &sidecar_key, sidecar.len() as u64).await?;
    bucket.put(&sidecar_key, sidecar.clone()).execute().await?;
    governor_client::settle(
        env,
        &Decision {
            commit: vec![crate::governor::Commit {
                pool: StoragePool::Dump,
                key: sidecar_key,
                bytes: sidecar.len() as u64,
            }],
            ..Decision::default()
        },
    )
    .await;
    console_log!(
        "backup dump {key} stored and verified: {} bytes, sha256 {}",
        check.bytes,
        check.sha256_hex
    );

    prune_dumps(env, &bucket, &date).await;

    // Recorded last: `last_backup_at` must never claim success for a
    // dump that was not stored, validated, and re-read above.
    db.batch(vec![
        upsert_meta(&db, "last_backup_at", &now)?,
        upsert_meta(&db, "last_backup_key", &key)?,
    ])
    .await?;
    Ok(())
}

/// Runs the export on the D1 REST API (the same endpoint `wrangler d1
/// export --remote` drives) and polls it to completion. The token is
/// `D1_EXPORT_API_TOKEN`, scoped to D1 alone; the database id comes
/// from the `D1_DATABASE_ID` var because bindings do not expose it.
async fn export_signed_url(env: &Env) -> worker::Result<String> {
    let account = env.var("CF_ACCOUNT_ID")?.to_string();
    let database = env.var("D1_DATABASE_ID")?.to_string();
    let token = env.secret("D1_EXPORT_API_TOKEN")?.to_string();
    // Overridable for the local smoke test only; deployed environments
    // use the real API.
    let base = env
        .var("CF_API_BASE")
        .map_or_else(|_| DEFAULT_API_BASE.to_owned(), |var| var.to_string());
    let url = format!("{base}/accounts/{account}/d1/database/{database}/export");

    let mut bookmark: Option<String> = None;
    for attempt in 0..EXPORT_POLL_ATTEMPTS {
        if attempt > 0 {
            Delay::from(Duration::from_secs(1)).await;
        }
        let body = backup::export_request_body(bookmark.as_deref());
        let mut response = post_json(&url, &body, Some(&token)).await?;
        let status = response.status_code();
        let text = response.text().await?;
        match backup::parse_export_poll(&text) {
            ExportPoll::Complete { signed_url } => return Ok(signed_url),
            ExportPoll::Failed(detail) => {
                return Err(err(format!("d1 export failed ({status}): {detail}")));
            }
            ExportPoll::Continue { bookmark: next } if status == 200 => {
                bookmark = next.or(bookmark);
            }
            ExportPoll::Continue { .. } => {
                return Err(err(format!("d1 export answered {status}")));
            }
        }
    }
    Err(err("d1 export did not complete in time".to_owned()))
}

/// Downloads the signed URL and streams it into the bucket at `key`,
/// hashing and validating on the way through ([`DumpScanner`]); the
/// dump never needs to fit in Worker memory. A response without a
/// declared length is refused because it cannot ride an R2 fixed-length
/// streaming put safely.
async fn stream_dump_into(
    env: &Env,
    bucket: &Bucket,
    key: &str,
    signed_url: &str,
) -> worker::Result<DumpCheck> {
    let mut response = worker::Fetch::Request(Request::new(signed_url, Method::Get)?)
        .send()
        .await?;
    if response.status_code() != 200 {
        return Err(err(format!(
            "the export download answered {}",
            response.status_code()
        )));
    }
    let length = response
        .headers()
        .get("content-length")?
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| err("the export download omitted a valid content-length".to_owned()))?;
    // Capacity is taken before the write starts; the declared length is
    // authoritative because the fixed-length put below fails on any
    // disagreement.
    admit_dump_object(env, key, length).await?;

    let scanner = Rc::new(RefCell::new(Some(DumpScanner::new())));
    let tap = Rc::clone(&scanner);
    let stream = response.stream()?.map(move |chunk| {
        if let (Ok(chunk), Some(scanner)) = (&chunk, tap.borrow_mut().as_mut()) {
            scanner.update(chunk);
        }
        chunk
    });
    bucket
        .put(key, Data::Stream(FixedLengthStream::wrap(stream, length)))
        .execute()
        .await?;
    let taken = scanner.borrow_mut().take();
    taken
        .map(DumpScanner::finish)
        .ok_or_else(|| err("dump scanner state was lost".to_owned()))
}

/// The R2 bindings have no rename, so the dump streams to its final key
/// and an invalid result is deleted again rather than staged-and-copied
/// (a copy would need its own re-verification). An unverified object
/// therefore sits at the dump key only between the put and this delete
/// (or after a crash inside that window), and recovery selects dumps
/// by their `.sha256` sidecar, which is written strictly after
/// validation (docs/runbook.md), so a failed attempt cannot masquerade
/// as a good dump.
///
/// Two same-date runs (rehearsal schedules aligning on one minute) can
/// race on the shared key; a run whose re-read mismatched because a
/// parallel run overwrote and recorded the key must not delete that
/// validated dump, so the meta row is consulted first - and an
/// unreadable meta keeps the object too, deletion being the
/// destructive branch. Runs that all fail leave no meta row; the next
/// nightly pass re-exports and the freshness alert covers the gap.
/// ponytail: a sub-second window remains between a parallel run's
/// re-read and its meta write - a D1 lock would close it, if
/// simultaneous rehearsal schedules ever become a real pattern.
async fn remove_invalid_dump(env: &Env, db: &D1Database, bucket: &Bucket, key: &str) {
    match read_meta(db, "last_backup_key").await {
        Ok(recorded) if recorded.as_deref() != Some(key) => {
            match bucket.delete(key).await {
                // The affirmative delete is the proof that releases
                // the dump's reservation.
                Ok(()) => release_dump_object(env, key).await,
                Err(error) => {
                    worker::console_error!("failed to delete the invalid dump {key}: {error}");
                }
            }
        }
        Ok(_) => {
            console_log!("dump {key} was recorded by a parallel run; keeping it");
        }
        Err(_) => {
            worker::console_error!("could not confirm {key} is unrecorded; keeping it");
        }
    }
}

/// Validation is only real once the stored object itself checks out:
/// re-read `key` from the bucket and compare its digest with the one
/// computed while streaming in.
async fn verify_reread(
    env: &Env,
    bucket: &Bucket,
    key: &str,
    expected_hex: &str,
) -> worker::Result<()> {
    governed(
        env,
        &Decision {
            consume: vec![Consume {
                pool: OpPool::BInfra,
                n: 1,
                principal: None,
                principal_cap: None,
            }],
            ..Decision::default()
        },
    )
    .await?;
    let Some(object) = bucket.get(key).execute().await? else {
        return Err(err(format!("dump {key} is missing on re-read")));
    };
    let Some(body) = object.body() else {
        return Err(err(format!("dump {key} has no body on re-read")));
    };
    let mut stream = body.stream()?;
    let mut scanner = DumpScanner::new();
    while let Some(chunk) = stream.try_next().await? {
        scanner.update(&chunk);
    }
    let reread_hex = scanner.finish().sha256_hex;
    if reread_hex == expected_hex {
        Ok(())
    } else {
        Err(err(format!(
            "dump {key} re-read digest {reread_hex} does not match streamed digest {expected_hex}"
        )))
    }
}

/// Deletes dumps (and their sidecars) beyond the retention policy.
/// Best-effort: a failed delete logs and stays for the next nightly
/// pass. Steady state is ~42 dumps plus sidecars, so one unpaginated
/// list page (R2 default 1000) covers it with a wide margin.
async fn prune_dumps(env: &Env, bucket: &Bucket, today: &str) {
    // The list is a billable Class A operation; a governor refusal
    // skips this pass's optional maintenance instead of running it
    // unpaid.
    let paid = governed(
        env,
        &Decision {
            consume: vec![consume_a_infra()],
            ..Decision::default()
        },
    )
    .await;
    if let Err(error) = paid {
        worker::console_error!("dump prune skipped: {error}");
        return;
    }
    let listing = match bucket.list().prefix(backup::DUMP_PREFIX).execute().await {
        Ok(listing) => listing,
        Err(error) => {
            worker::console_error!("dump prune could not list the backup bucket: {error}");
            return;
        }
    };
    let dates: Vec<String> = listing
        .objects()
        .iter()
        .filter_map(|object| backup::date_of_dump_key(&object.key()).map(str::to_owned))
        .collect();
    for date in backup::dates_to_prune(&dates, today) {
        let key = backup::dump_object_key(&date);
        for target in [key.clone(), format!("{key}.sha256")] {
            match bucket.delete(&target).await {
                Ok(()) => {
                    release_dump_object(env, &target).await;
                    console_log!("dump prune deleted {target}");
                }
                Err(error) => {
                    worker::console_error!("dump prune failed to delete {target}: {error}");
                }
            }
        }
    }
}

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
    // Bounded per call: keyset-paginated pages of 10, so rows a pass
    // keeps (missing primary blob, unconfirmed commit) are walked
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
    let Some(checksum) = row.key.strip_prefix("blobs/sha256/") else {
        console_error!("backup drain: malformed queue key {}", row.key);
        delete_row(row.key.clone()).await;
        return true;
    };
    // Only blobs the registry still serves as verified content are
    // worth a backup copy: a rejection (or replacement) that landed
    // after the enqueue retires the row instead.
    let live: Result<Option<CountRecord>, _> = match db
        .prepare(sql::COUNT_LIVE_VERIFIED_BLOB_REFERENCES)
        .bind(&[checksum.into()])
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
                .bind(&[row.key.as_str().into(), checksum.into()])
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
    // put needs (the same shape as `stream_dump_into`), so the archive
    // never has to fit in Worker memory; the put fails on any
    // disagreement with the actual body.
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
