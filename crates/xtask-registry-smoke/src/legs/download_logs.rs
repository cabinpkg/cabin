//! The download-log pass over local R2: one cron pass reads a file of
//! downloads and the files that cannot be counted, through the
//! runtime's own decompression (`api/src/glue/download_log.rs`).

use std::io::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Map, Value, json};
use xtask_registry_admin::{output, wrangler};

use crate::context::Smoke;
use crate::legs::finale::BREAKER_CRON;
use crate::servers::{d1, d1_rows};
use crate::step;

/// The host of `ARCHIVES_ORIGIN` in the run's `.dev.vars`.
const HOST: &str = "archives.smoke.invalid";

/// The decompressed bytes and the object bytes the pass reads at most
/// (`api/src/download_log.rs`, `LIMITS`).
const LIMIT: usize = 10_000_000;

/// The pass counts one version's downloads from a file, records the
/// files it cannot count as errors, and adds nothing from them: the
/// truncated file holds the same downloads as the counted one, and
/// warns of them. A rerun clears the rows of these keys, whose objects
/// it puts again, and the alert.
///
/// # Errors
///
/// The first failed check.
pub fn run(smoke: &mut Smoke, scope: &str, name: &str, work: &Path) -> Result<()> {
    step(
        "the cron counts a download log's downloads, records the files it cannot count, and warns of them",
    );
    d1("DELETE FROM processed_log_files WHERE key LIKE 'smoke/%'; \
        DELETE FROM alerts WHERE kind = 'download_logs';")?;
    let version_row = d1_rows(&format!(
        "SELECT version, downloads FROM versions \
         WHERE scope = '{scope}' AND name = '{name}' ORDER BY version LIMIT 1"
    ))?;
    let version = text(version_row.first(), "version")?;
    let total = number(version_row.first(), "downloads")?;
    let start = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("the clock is before 1970")?;
    let day = format!("date({}, 'unixepoch')", start.as_secs());
    let daily = || -> Result<i64> {
        let rows = d1_rows(&format!(
            "SELECT COALESCE(SUM(downloads), 0) AS downloads FROM version_daily_downloads \
             WHERE scope = '{scope}' AND name = '{name}' AND version = '{version}' AND day = {day}"
        ))?;
        number(rows.first(), "downloads")
    };
    let daily_before = daily()?;

    put_files(work, scope, name, &version, start.as_nanos())?;

    smoke.check(BREAKER_CRON, &[200])?;
    check_rows()?;
    // A pass records the alert from its listing, before it reads.
    smoke.check(BREAKER_CRON, &[200])?;
    let alert = d1_rows("SELECT level, summary FROM alerts WHERE kind = 'download_logs'")?;
    if number(alert.first(), "level")? != 1
        || text(alert.first(), "summary")? != "download log files: 5 that cannot be counted"
    {
        bail!(
            "the download_logs alert is {alert:?}, not a WARNING for the 5 files that cannot be counted"
        );
    }
    let after = d1_rows(&format!(
        "SELECT downloads FROM versions \
         WHERE scope = '{scope}' AND name = '{name}' AND version = '{version}'"
    ))?;
    let total_after = number(after.first(), "downloads")?;
    let daily_after = daily()?;
    if total_after != total + 2 || daily_after != daily_before + 2 {
        bail!(
            "{scope}/{name} {version} went from {total} to {total_after} downloads, \
             and from {daily_before} to {daily_after} on the day: not 2 more of each"
        );
    }
    Ok(())
}

/// A file of two downloads of `version` started at `start_ns` and six
/// other records, the same file cut short, bytes that are not gzip, a
/// file of more records than the limit, a file stating more
/// decompressed bytes than the limit, and an object past the object
/// limit.
fn put_files(work: &Path, scope: &str, name: &str, version: &str, start_ns: u128) -> Result<()> {
    let archive = |version: &str| {
        format!("/archives/{scope}/{name}/{scope}-{name}-{version}-0123456789abcdef.zip")
    };
    let download = json!({
        "CacheCacheStatus": "miss",
        "ClientRequestHost": HOST,
        "ClientRequestMethod": "GET",
        "ClientRequestSource": "eyeball",
        "ClientRequestURI": archive(version),
        "ClientRequestUserAgent": "cabin/0.18.0",
        "EdgeResponseStatus": 200,
        "EdgeStartTimestamp": u64::try_from(start_ns).context("a start past 2554")?,
    });
    let with = |field: &str, value: Value| {
        let mut record = download.clone();
        record[field] = value;
        record.to_string()
    };
    let records = [
        download.to_string(),
        with("CacheCacheStatus", json!("hit")),
        with("ClientRequestSource", json!("edgeWorkerFetch")),
        with("ClientRequestMethod", json!("HEAD")),
        with("EdgeResponseStatus", json!(206)),
        with("EdgeResponseStatus", json!(404)),
        with("ClientRequestURI", json!("/config.json")),
        with("ClientRequestURI", json!(archive("99.0.0"))),
    ];
    let good = gzip(records.join("\n").as_bytes())?;
    put(work, "smoke/good.log.gz", &good)?;
    // All the data and all but the last byte of the trailer, whose
    // other bytes state a size under the limit: only the runtime's
    // strict decompression refuses it.
    put(work, "smoke/truncated.log.gz", &good[..good.len() - 1])?;
    // Stating a size of 0, so that the decompression refuses it.
    put(work, "smoke/corrupt.log.gz", b"not gzip\0\0\0\0")?;
    let head = with("ClientRequestMethod", json!("HEAD"));
    put(
        work,
        "smoke/many.log.gz",
        &gzip(vec![head; 20_001].join("\n").as_bytes())?,
    )?;
    // Small, and stating 64 MiB: refused before it is decompressed.
    put(work, "smoke/big.log.gz", &gzip(&vec![b'x'; 64 << 20])?)?;
    put(work, "smoke/huge.log.gz", &vec![0; LIMIT + 1])?;
    Ok(())
}

/// The counted file is `done` with its records, exclusions and unknown
/// version, and each other file has an error row that says why.
fn check_rows() -> Result<()> {
    let rows = d1_rows(
        "SELECT key, status, records, excluded, unknown_version, error \
         FROM processed_log_files WHERE key LIKE 'smoke/%' ORDER BY key",
    )?;
    let found: Vec<(String, String, String)> = rows
        .iter()
        .map(|row| {
            Ok((
                text(Some(row), "key")?,
                text(Some(row), "status")?,
                row.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ))
        })
        .collect::<Result<_>>()?;
    let expected = [
        (
            "smoke/big.log.gz",
            "error",
            "the file decompresses to 67108864 bytes",
        ),
        ("smoke/corrupt.log.gz", "error", "not gzip"),
        ("smoke/good.log.gz", "done", ""),
        ("smoke/huge.log.gz", "error", "the object is 10000001 bytes"),
        ("smoke/many.log.gz", "error", "more than 20000 records"),
        ("smoke/truncated.log.gz", "error", "not gzip"),
    ];
    let matches = found.len() == expected.len()
        && found
            .iter()
            .zip(expected)
            .all(|((key, status, error), want)| {
                key == want.0 && status == want.1 && error.starts_with(want.2)
            });
    if !matches {
        bail!("the download-log rows are {found:?}, not {expected:?}");
    }
    let good_row = rows
        .iter()
        .find(|row| row.get("key") == Some(&json!("smoke/good.log.gz")));
    let excluded: Value = serde_json::from_str(&text(good_row, "excluded")?)
        .context("parse the counted file's exclusions")?;
    if number(good_row, "records")? != 8
        || number(good_row, "unknown_version")? != 1
        || excluded != json!({"path": 1, "method": 1, "status": 2, "source": 1})
    {
        bail!("the counted file's row is {good_row:?}");
    }
    Ok(())
}

fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).context("gzip a download log")?;
    encoder.finish().context("gzip a download log")
}

/// Puts `bytes` into local R2's `DOWNLOAD_LOGS` bucket (`wrangler.jsonc`)
/// under `key`, through a file in `work`.
fn put(work: &Path, key: &str, bytes: &[u8]) -> Result<()> {
    let file = work.join("download-log.gz");
    std::fs::write(&file, bytes).with_context(|| format!("write {}", file.display()))?;
    let file = file.to_str().context("the work directory is not UTF-8")?;
    output(&mut wrangler(&[
        "r2",
        "object",
        "put",
        &format!("registry-download-logs/{key}"),
        "--file",
        file,
        "--local",
    ]))?;
    Ok(())
}

fn text(row: Option<&Map<String, Value>>, column: &str) -> Result<String> {
    row.and_then(|row| row.get(column))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("no text {column} in {row:?}"))
}

fn number(row: Option<&Map<String, Value>>, column: &str) -> Result<i64> {
    row.and_then(|row| row.get(column))
        .and_then(Value::as_i64)
        .with_context(|| format!("no integer {column} in {row:?}"))
}
