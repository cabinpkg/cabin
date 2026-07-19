//! Cloudflare glue for the nightly D1 dump job (`docs/runbook.md`,
//! "Disaster recovery"): drive the D1 export REST endpoint, stream the
//! returned dump into the BACKUP bucket while validating it
//! (`crate::backup`), verify the stored object by re-reading it, prune
//! dumps beyond retention, and record success in `meta`. Like the rest
//! of the wasm glue this file is thin I/O wiring; every decision lives
//! in the host-testable [`crate::backup`].

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt};
use worker::{
    Bucket, D1Database, Data, Delay, Env, FixedLengthStream, Method, Request, console_log,
};

use crate::backup::{self, DumpCheck, DumpScanner, ExportPoll};
use crate::glue::{now_iso8601, post_json, read_meta, upsert_meta};

const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";

/// The export endpoint auto-cancels when polling stops, so poll at a
/// steady ~1 s cadence and give up after ~2 minutes - far beyond what
/// this database needs, far below the scheduled handler's wall-clock
/// allowance.
const EXPORT_POLL_ATTEMPTS: u32 = 120;

fn err(message: String) -> worker::Error {
    worker::Error::RustError(message)
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
    let check = stream_dump_into(&bucket, &key, &signed_url).await?;
    if let Some(error) = check.error() {
        remove_invalid_dump(&db, &bucket, &key).await;
        return Err(err(format!("dump {key} failed validation: {error}")));
    }
    if let Err(error) = verify_reread(&bucket, &key, &check.sha256_hex).await {
        remove_invalid_dump(&db, &bucket, &key).await;
        return Err(error);
    }
    bucket
        .put(
            format!("{key}.sha256"),
            format!("{}  {date}.sql\n", check.sha256_hex),
        )
        .execute()
        .await?;
    console_log!(
        "backup dump {key} stored and verified: {} bytes, sha256 {}",
        check.bytes,
        check.sha256_hex
    );

    prune_dumps(&bucket, &date).await;

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
async fn remove_invalid_dump(db: &D1Database, bucket: &Bucket, key: &str) {
    match read_meta(db, "last_backup_key").await {
        Ok(recorded) if recorded.as_deref() != Some(key) => {
            if let Err(error) = bucket.delete(key).await {
                worker::console_error!("failed to delete the invalid dump {key}: {error}");
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
async fn verify_reread(bucket: &Bucket, key: &str, expected_hex: &str) -> worker::Result<()> {
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
async fn prune_dumps(bucket: &Bucket, today: &str) {
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
            if let Err(error) = bucket.delete(&target).await {
                worker::console_error!("dump prune failed to delete {target}: {error}");
            } else {
                console_log!("dump prune deleted {target}");
            }
        }
    }
}
