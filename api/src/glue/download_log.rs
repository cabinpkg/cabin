//! The download-log pass (`crate::download_log`; `docs/runbook.md`,
//! "Download statistics"): each cron run lists `DOWNLOAD_LOGS`, reads
//! the files that have no row, oldest upload first, and adds each
//! file's downloads in one batch.
//!
//! A failure that a later read can mend - a governor refusal, an R2 or
//! D1 error - ends the pass without an error row; the next run starts
//! over from the listing. Only a file whose bytes cannot be counted
//! gets an error row. The object is read whole before it is
//! decompressed, so a read that breaks off is told apart from a file
//! that is not gzip.
//!
//! The runtime's `DecompressionStream` has no backpressure: one write
//! decompresses all of its input into memory before a read takes any.
//! A file is therefore refused unread when its gzip trailer states more
//! than the limit. That bounds the memory of a file the job wrote: R2
//! keeps an object whole or not at all, a read shorter than the object
//! is a failed read, and only the job and the operators write to the
//! bucket. A file whose last four bytes understate its size - one cut
//! short or made so - is decompressed whole before the limit on what
//! the stream gives out stops it.

use std::collections::HashSet;

use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use worker::js_sys::{Array, Function, Reflect, Uint8Array};
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::web_sys::ReadableStream;
use worker::{Bucket, ByteStream, D1Database, Env, Object, console_error, console_log};

use crate::download_log::{self, File, LIMITS, PAYLOAD_BYTES, Rule, Tally};
use crate::governor::OpPool;
use crate::governor_client::{self, Gate};
use crate::sql;

use super::{changed_rows, consume_one, iso8601_from_ms, now_iso8601, read_meta};

/// Pages of a thousand keys a pass lists: fourteen days of two files a
/// minute, ten times what the job writes when it sends a file each five
/// minutes (`docs/runbook.md`, "Download statistics").
const LIST_PAGES: usize = 40;

/// Files a pass reads: two a minute between two passes.
const FILES_PER_PASS: usize = 30;

#[derive(Deserialize)]
struct KeyRecord {
    key: String,
}

/// Runs the pass once `ARCHIVES_ORIGIN` names the host the logs count
/// downloads on. Failures only log; the next pass retries.
pub(super) async fn count_downloads(env: &Env, db: &D1Database) {
    let Ok(origin) = env.var("ARCHIVES_ORIGIN") else {
        return;
    };
    if let Err(err) = try_count_downloads(env, db, &origin.to_string()).await {
        console_error!("download-log pass: {err}; the next pass reads again");
    }
}

async fn try_count_downloads(env: &Env, db: &D1Database, origin: &str) -> worker::Result<()> {
    let host = worker::Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .ok_or_else(|| worker::Error::RustError(format!("ARCHIVES_ORIGIN {origin} has no host")))?;
    let now = now_iso8601();
    db.batch(vec![
        db.prepare(sql::DROP_OLD_LOG_FILES)
            .bind(&[now.as_str().into()])?,
        db.prepare(sql::DROP_OLD_DAILY_DOWNLOADS)
            .bind(&[now.as_str().into()])?,
    ])
    .await?;
    let start = read_meta(db, "downloads_cutoff")
        .await?
        .ok_or_else(|| worker::Error::RustError("meta has no downloads_cutoff".to_owned()))?;
    let start_ms = worker::js_sys::Date::parse(&start);
    if start_ms.is_nan() {
        return Err(worker::Error::RustError(format!(
            "downloads_cutoff {start} is not a time"
        )));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cutoff_ms = start_ms as u64;

    let bucket = env.bucket("DOWNLOAD_LOGS")?;
    let now_ms = worker::Date::now().as_millis();
    let mut due = Vec::new();
    let mut cursor = None;
    for page in 1..=LIST_PAGES {
        if !admitted(env, OpPool::AInfra).await {
            return Ok(());
        }
        let mut list = bucket.list().limit(1000);
        if let Some(cursor) = cursor.take() {
            list = list.cursor(cursor);
        }
        let listed = list.execute().await?;
        let objects = listed.objects();
        let keys = json!(objects.iter().map(Object::key).collect::<Vec<_>>()).to_string();
        let settled: HashSet<String> = db
            .prepare(sql::SETTLED_LOG_FILES)
            .bind(&[keys.into()])?
            .all()
            .await?
            .results::<KeyRecord>()?
            .into_iter()
            .map(|record| record.key)
            .collect();
        due.extend(
            objects
                .iter()
                .filter(|object| !settled.contains(&object.key()))
                .filter(|object| download_log::readable(object.uploaded().as_millis(), now_ms))
                .map(|object| (object.uploaded().as_millis(), object.key(), object.size())),
        );
        cursor = listed.cursor();
        if !listed.truncated() || cursor.is_none() {
            break;
        }
        if page == LIST_PAGES {
            console_error!(
                "download-log pass: DOWNLOAD_LOGS holds more than {LIST_PAGES} pages; the later keys are not read"
            );
        }
    }

    due.sort_unstable();
    for (uploaded_ms, key, size) in due.into_iter().take(FILES_PER_PASS) {
        let uploaded = iso8601_from_ms(uploaded_ms);
        if size > LIMITS.bytes {
            let error = format!("the object is {size} bytes, more than {}", LIMITS.bytes);
            refuse(db, &key, &now, &uploaded, &error).await?;
            continue;
        }
        if !admitted(env, OpPool::BInfra).await {
            return Ok(());
        }
        let rule = Rule {
            host: &host,
            cutoff_ms,
        };
        match read(&bucket, &key, rule).await? {
            None => {}
            Some(Ok(tally)) => add(db, &key, &now, &uploaded, &start, &tally).await?,
            Some(Err(error)) => refuse(db, &key, &now, &uploaded, &error).await?,
        }
    }
    Ok(())
}

/// The tally of the file under `key`, or why it has none; nothing when
/// the object is gone, as R2 ends its lifetime.
async fn read(
    bucket: &Bucket,
    key: &str,
    rule: Rule<'_>,
) -> worker::Result<Option<Result<Tally, String>>> {
    let Some(object) = bucket.get(key).execute().await? else {
        return Ok(None);
    };
    let Some(body) = object.body() else {
        return Ok(None);
    };
    let gzip = body.bytes().await?;
    if gzip.len() as u64 != object.size() {
        return Err(worker::Error::RustError(format!(
            "read {} of the {} bytes of {key}",
            gzip.len(),
            object.size()
        )));
    }
    if let Some(stated) = download_log::stated_size(&gzip)
        && stated > LIMITS.bytes
    {
        return Ok(Some(Err(format!(
            "the file decompresses to {stated} bytes, more than {}",
            LIMITS.bytes
        ))));
    }
    Ok(Some(tally(gunzip(&gzip)?, rule).await))
}

/// Whether the governor admits one more operation of `pool`; a refusal
/// ends the pass.
async fn admitted(env: &Env, pool: OpPool) -> bool {
    if let Gate::Refused(_) = governor_client::decide(env, &consume_one(pool)).await {
        console_error!(
            "download-log pass: the governor refused {}; stopping",
            pool.as_str()
        );
        return false;
    }
    true
}

/// The decompressed bytes of `gzip` as the runtime's
/// `DecompressionStream` gives them out (web-sys binds it only among
/// its unstable APIs). With every byte already here, an error of the
/// stream is the file's own.
fn gunzip(gzip: &[u8]) -> worker::Result<ByteStream> {
    let global = worker::js_sys::global();
    let class = |name: &str| -> worker::Result<Function> {
        Ok(Reflect::get(&global, &JsValue::from_str(name))?.dyn_into()?)
    };
    let response = Reflect::construct(&class("Response")?, &Array::of1(&Uint8Array::from(gzip)))?;
    let body = Reflect::get(&response, &JsValue::from_str("body"))?;
    let decompressor = Reflect::construct(
        &class("DecompressionStream")?,
        &Array::of1(&JsValue::from_str("gzip")),
    )?;
    let pipe_through: Function =
        Reflect::get(&body, &JsValue::from_str("pipeThrough"))?.dyn_into()?;
    let stream: ReadableStream = pipe_through.call1(&body, &decompressor)?.dyn_into()?;
    Ok(ByteStream::from(stream))
}

/// The tally of one file, or why it has none.
async fn tally(mut chunks: ByteStream, rule: Rule<'_>) -> Result<Tally, String> {
    let mut file = File::new(rule, LIMITS);
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|err| format!("not gzip: {err}"))?;
        file = file.feed(&chunk).map_err(|err| err.to_string())?;
    }
    file.finish().map_err(|err| err.to_string())
}

async fn add(
    db: &D1Database,
    key: &str,
    now: &str,
    uploaded: &str,
    start: &str,
    tally: &Tally,
) -> worker::Result<()> {
    // At most `LIMITS.records`, far below 2^53, where f64 is exact.
    #[allow(clippy::cast_precision_loss)]
    let records = JsValue::from_f64(tally.records as f64);
    let excluded = json!(tally.excluded).to_string();
    let mut statements = vec![db.prepare(sql::REGISTER_LOG_FILE).bind(&[
        key.into(),
        now.into(),
        uploaded.into(),
        records,
        excluded.into(),
        start.into(),
    ])?];
    for payload in tally.payloads(PAYLOAD_BYTES) {
        let file = [key.into(), payload.as_str().into()];
        statements.push(db.prepare(sql::ADD_LOG_DOWNLOADS).bind(&file)?);
        statements.push(db.prepare(sql::ADD_LOG_DAILY_DOWNLOADS).bind(&[
            key.into(),
            payload.as_str().into(),
            now.into(),
        ])?);
        statements.push(db.prepare(sql::COUNT_UNKNOWN_LOG_DOWNLOADS).bind(&file)?);
    }
    // Last in every batch: a row committed `adding` would let a later
    // batch add the file again (`sql::download_log`).
    statements.push(db.prepare(sql::FINISH_LOG_FILE).bind(&[key.into()])?);
    let results = db.batch(statements).await?;
    let registered = match results.first() {
        Some(result) => changed_rows(result.meta()?),
        None => 0,
    };
    if registered == 0 {
        console_log!("download-log pass: {key} has a row, or the start moved; it added nothing");
    }
    Ok(())
}

/// Records a file that cannot be counted; it is not read again until
/// an operator deletes its row.
async fn refuse(
    db: &D1Database,
    key: &str,
    now: &str,
    uploaded: &str,
    error: &str,
) -> worker::Result<()> {
    db.prepare(sql::REFUSE_LOG_FILE)
        .bind(&[key.into(), now.into(), uploaded.into(), error.into()])?
        .run()
        .await?;
    console_error!("download-log pass: {key} cannot be counted: {error}");
    Ok(())
}
