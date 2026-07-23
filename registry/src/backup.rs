//! Backup logic for the nightly D1 dump and the backup-freshness alert
//! (`docs/runbook.md`, "Disaster recovery"): the D1 export polling
//! protocol, streaming dump validation, the dump retention policy, and
//! freshness evaluation. Everything here is pure and host-testable; the
//! wasm32 glue supplies the clock and performs the HTTP, R2, and D1 I/O.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Object-key prefix of the nightly dumps in the BACKUP bucket; a dump
/// lands at `d1/<YYYY-MM-DD>.sql` with a `.sha256` sidecar next to it.
pub const DUMP_PREFIX: &str = "d1/";

/// The freshness alert threshold: a nightly job gets a day and a half of
/// slack before its silence is treated as an incident.
pub const STALE_AFTER_HOURS: u64 = 36;

/// Retention: the most recent 30 daily dumps, plus the first dump of
/// each calendar month for 12 months.
pub const DAILY_KEEP: usize = 30;
pub const MONTHLY_KEEP_MONTHS: u64 = 12;

/// Tables whose `CREATE TABLE` statement a valid dump must contain.
/// This mirrors the canonical schema (`migrations/`) plus wrangler's
/// own migration-history table (without which a restored database
/// re-runs old migrations and fails); extend it when a migration adds
/// a table the registry cannot run without.
pub const EXPECTED_TABLES: &[&str] = &[
    "users",
    "identities",
    "scopes",
    "scope_members",
    "tokens",
    "packages",
    "versions",
    "meta",
    "backup_pending",
    "d1_migrations",
];

/// Tables a valid dump must carry at least one `INSERT INTO` row for.
/// Both are populated by the migrations themselves, so even a
/// brand-new, empty registry dumps rows here - which catches a
/// schema-only or data-truncated export that would otherwise validate
/// and record success while restoring an empty registry.
pub const EXPECTED_ROWS: &[&str] = &["meta", "d1_migrations"];

/// `d1/<date>.sql` for a `YYYY-MM-DD` date.
pub fn dump_object_key(date: &str) -> String {
    format!("{DUMP_PREFIX}{date}.sql")
}

/// The `YYYY-MM-DD` of a dump object key, `None` for sidecars and
/// anything else that is not exactly `d1/<date>.sql`.
pub fn date_of_dump_key(key: &str) -> Option<&str> {
    let date = key.strip_prefix(DUMP_PREFIX)?.strip_suffix(".sql")?;
    is_date(date).then_some(date)
}

// --- D1 export polling protocol -----------------------------------------

/// POST body for the D1 export REST endpoint
/// (`/accounts/<account>/d1/database/<database>/export`). The first call
/// carries no bookmark; every follow-up polls with the bookmark the
/// previous response returned.
pub fn export_request_body(bookmark: Option<&str>) -> String {
    let mut body = serde_json::json!({ "output_format": "polling" });
    if let Some(bookmark) = bookmark {
        body["current_bookmark"] = Value::String(bookmark.to_owned());
    }
    body.to_string()
}

/// One parsed D1 export polling response.
#[derive(Debug, PartialEq, Eq)]
pub enum ExportPoll {
    /// The dump is ready at this signed URL (valid for about an hour).
    Complete { signed_url: String },
    /// Still running; poll again with this bookmark.
    Continue { bookmark: Option<String> },
    /// The export failed; retrying with the same bookmark cannot help.
    Failed(String),
}

/// Parses a D1 export endpoint response. Field names are the external
/// contract of the D1 REST API - verify them against the current
/// Cloudflare API documentation if exports start failing.
pub fn parse_export_poll(body: &str) -> ExportPoll {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return ExportPoll::Failed("export response was not JSON".to_owned());
    };
    if value.get("success").and_then(Value::as_bool) != Some(true) {
        let detail = value
            .get("errors")
            .and_then(|errors| errors.as_array()?.first()?.get("message")?.as_str())
            .unwrap_or("the export request was refused");
        return ExportPoll::Failed(detail.to_owned());
    }
    let result = value.get("result").unwrap_or(&Value::Null);
    // The API reference nests the completed payload under
    // `result.result` - and that is what the live endpoint returned
    // when this shipped - but Cloudflare's own Workflows backup
    // example reads `result.signed_url`, so accept both shapes.
    if let Some(url) = result
        .get("result")
        .and_then(|inner| inner.get("signed_url")?.as_str())
        .or_else(|| result.get("signed_url").and_then(Value::as_str))
    {
        return ExportPoll::Complete {
            signed_url: url.to_owned(),
        };
    }
    if result.get("status").and_then(Value::as_str) == Some("error") {
        let detail = result
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the export reported an error");
        return ExportPoll::Failed(detail.to_owned());
    }
    ExportPoll::Continue {
        bookmark: result
            .get("at_bookmark")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

// --- Streaming dump validation -------------------------------------------

/// Incremental validation of a dump as it streams through the Worker:
/// SHA-256, total size, and presence of every expected `CREATE TABLE`
/// statement (matched across chunk boundaries via a carried tail).
pub struct DumpScanner {
    hasher: Sha256,
    bytes: u64,
    carry: Vec<u8>,
    seen: Vec<bool>,
    seen_rows: Vec<bool>,
}

/// `CREATE TABLE <name>` as the migrations write it, plus the quoted
/// and `IF NOT EXISTS` spellings `SQLite` tools and the D1 exporter
/// emit (`d1_migrations` ships as
/// `CREATE TABLE IF NOT EXISTS "d1_migrations"`).
fn table_patterns(table: &str) -> [String; 4] {
    [
        format!("CREATE TABLE {table}"),
        format!("CREATE TABLE \"{table}\""),
        format!("CREATE TABLE IF NOT EXISTS {table}"),
        format!("CREATE TABLE IF NOT EXISTS \"{table}\""),
    ]
}

/// `INSERT INTO` as the D1 exporter writes it (quoted), plus the
/// unquoted spelling.
fn row_patterns(table: &str) -> [String; 2] {
    [
        format!("INSERT INTO \"{table}\""),
        format!("INSERT INTO {table}"),
    ]
}

impl DumpScanner {
    pub fn new() -> Self {
        DumpScanner {
            hasher: Sha256::new(),
            bytes: 0,
            carry: Vec::new(),
            seen: vec![false; EXPECTED_TABLES.len()],
            seen_rows: vec![false; EXPECTED_ROWS.len()],
        }
    }

    /// Feeds the next chunk of the dump.
    pub fn update(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
        self.bytes += chunk.len() as u64;

        // Search the carried tail of the previous chunk plus this one, so
        // a statement split across the boundary still matches.
        let mut haystack = std::mem::take(&mut self.carry);
        haystack.extend_from_slice(chunk);
        let mut longest = 0;
        for (index, table) in EXPECTED_TABLES.iter().enumerate() {
            for pattern in table_patterns(table) {
                longest = longest.max(pattern.len());
                if !self.seen[index] && contains(&haystack, pattern.as_bytes()) {
                    self.seen[index] = true;
                }
            }
        }
        for (index, table) in EXPECTED_ROWS.iter().enumerate() {
            for pattern in row_patterns(table) {
                longest = longest.max(pattern.len());
                if !self.seen_rows[index] && contains(&haystack, pattern.as_bytes()) {
                    self.seen_rows[index] = true;
                }
            }
        }
        let tail = haystack.len().saturating_sub(longest - 1);
        haystack.drain(..tail);
        self.carry = haystack;
    }

    pub fn finish(self) -> DumpCheck {
        DumpCheck {
            sha256_hex: crate::auth::hex(&self.hasher.finalize()),
            bytes: self.bytes,
            missing_tables: EXPECTED_TABLES
                .iter()
                .zip(self.seen)
                .filter_map(|(table, seen)| (!seen).then_some(*table))
                .collect(),
            missing_rows: EXPECTED_ROWS
                .iter()
                .zip(self.seen_rows)
                .filter_map(|(table, seen)| (!seen).then_some(*table))
                .collect(),
        }
    }
}

impl Default for DumpScanner {
    fn default() -> Self {
        Self::new()
    }
}

/// The verdict over a fully streamed dump.
#[derive(Debug)]
pub struct DumpCheck {
    pub sha256_hex: String,
    pub bytes: u64,
    pub missing_tables: Vec<&'static str>,
    pub missing_rows: Vec<&'static str>,
}

impl DumpCheck {
    /// `None` when the dump is acceptable; otherwise what is wrong with
    /// it.
    pub fn error(&self) -> Option<String> {
        if self.bytes == 0 {
            return Some("the dump is empty".to_owned());
        }
        if !self.missing_tables.is_empty() {
            return Some(format!(
                "the dump is missing CREATE TABLE statements for: {}",
                self.missing_tables.join(", ")
            ));
        }
        if !self.missing_rows.is_empty() {
            return Some(format!(
                "the dump carries no rows for the always-populated tables: {}",
                self.missing_rows.join(", ")
            ));
        }
        None
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// --- Retention ------------------------------------------------------------

/// Which dump dates to delete: everything that is neither among the
/// [`DAILY_KEEP`] most recent dumps nor the first dump of a calendar
/// month from the last [`MONTHLY_KEEP_MONTHS`] months (`today`'s month
/// counts as the first). Inputs that do not look like `YYYY-MM-DD` are
/// never returned - unknown objects are not this function's to delete -
/// and neither are dates after `today`: the service never writes those,
/// and a stray future-dated object must not occupy a newest-30 slot and
/// push a genuine daily out of retention.
/// Returned ascending for deterministic delete order.
pub fn dates_to_prune(dates: &[String], today: &str) -> Vec<String> {
    let Some(today_month) = month_index(today) else {
        return Vec::new();
    };
    let mut valid: Vec<&str> = dates
        .iter()
        .map(String::as_str)
        .filter(|date| is_date(date) && *date <= today)
        .collect();
    valid.sort_unstable_by(|a, b| b.cmp(a));
    valid.dedup();

    let mut keep: Vec<&str> = valid.iter().take(DAILY_KEEP).copied().collect();
    // The first (oldest) dump of each recent-enough month; `valid` is
    // newest-first, so the last date seen per month wins.
    let mut monthly_first: Option<&str> = None;
    for date in &valid {
        if let Some(month) = month_index(date)
            && month <= today_month
            && today_month - month < MONTHLY_KEEP_MONTHS
        {
            match monthly_first {
                Some(first) if first[..7] == date[..7] => monthly_first = Some(date),
                _ => {
                    keep.extend(monthly_first);
                    monthly_first = Some(date);
                }
            }
        }
    }
    keep.extend(monthly_first);

    valid
        .iter()
        .rev()
        .filter(|date| !keep.contains(date))
        .map(|date| (*date).to_owned())
        .collect()
}

// --- Freshness ------------------------------------------------------------

/// How current the last successful dump is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    /// No successful dump recorded (or an unreadable record - fail
    /// closed into the alerting state).
    Never,
}

impl Freshness {
    pub fn as_str(self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::Stale => "stale",
            Freshness::Never => "never",
        }
    }
}

/// Evaluates `meta.last_backup_at` against the clock. More than
/// [`STALE_AFTER_HOURS`] old is stale; a missing or unparsable record is
/// [`Freshness::Never`]; an unparsable clock reads as stale (fail
/// closed, like every other unreadable input around the breaker).
pub fn freshness(now_iso: &str, last_backup_at: Option<&str>) -> Freshness {
    let Some(last) = last_backup_at.and_then(epoch_secs) else {
        return Freshness::Never;
    };
    let Some(now) = epoch_secs(now_iso) else {
        return Freshness::Stale;
    };
    if now.saturating_sub(last) > (STALE_AFTER_HOURS * 3600).cast_signed() {
        Freshness::Stale
    } else {
        Freshness::Fresh
    }
}

/// The backup-health alert for one breaker pass, `None` when healthy.
/// Raised while the last dump is missing or stale, or while any
/// verified-artifact backup has sat in the replication queue for over
/// an hour - a backup system's classic failure mode is stopping
/// silently, so unhealthy states alert on every pass until resolved.
pub fn alert(freshness: Freshness, stale_backups: u64) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match freshness {
        Freshness::Fresh => {}
        Freshness::Stale => parts.push(format!(
            "the last successful D1 dump is older than {STALE_AFTER_HOURS} h"
        )),
        Freshness::Never => parts.push("no successful D1 dump has been recorded".to_owned()),
    }
    match stale_backups {
        0 => {}
        1 => parts.push(
            "1 verified archive blob is overdue for backup replication \
             (check the drain; scripts/backup-backfill.sh recovers by hand)"
                .to_owned(),
        ),
        n => parts.push(format!(
            "{n} verified archive blobs are overdue for backup replication \
             (check the drain; scripts/backup-backfill.sh recovers by hand)"
        )),
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

// --- Small date helpers ----------------------------------------------------

fn is_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && [0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&i| bytes[i].is_ascii_digit())
}

/// Months since year zero of a `YYYY-MM-...` prefix.
fn month_index(date: &str) -> Option<u64> {
    if !is_date(date.get(..10)?) {
        return None;
    }
    let year: u64 = date[..4].parse().ok()?;
    let month: u64 = date[5..7].parse().ok()?;
    (1..=12).contains(&month).then_some(year * 12 + month - 1)
}

/// Unix seconds of a `YYYY-MM-DDTHH:MM:SS[.frac]Z` timestamp, the shape
/// the Worker clock produces. Fractional seconds are truncated.
fn epoch_secs(iso: &str) -> Option<i64> {
    let bytes = iso.as_bytes();
    if bytes.len() < 19 || !is_date(iso.get(..10)?) || bytes[10] != b'T' {
        return None;
    }
    let (hour, minute, second) = (
        two_digits(iso, 11)?,
        two_digits(iso, 14)?,
        two_digits(iso, 17)?,
    );
    if bytes[13] != b':' || bytes[16] != b':' || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let year: i64 = iso[..4].parse().ok()?;
    let month: i64 = iso[5..7].parse().ok()?;
    let day: i64 = iso[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

fn two_digits(value: &str, at: usize) -> Option<i64> {
    let digits = value.get(at..at + 2)?;
    digits
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| digits.parse().ok())?
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- dump keys ---

    #[test]
    fn dump_keys_round_trip_and_reject_sidecars() {
        assert_eq!(dump_object_key("2026-07-10"), "d1/2026-07-10.sql");
        assert_eq!(date_of_dump_key("d1/2026-07-10.sql"), Some("2026-07-10"));
        for bad in [
            "d1/2026-07-10.sql.sha256",
            "d1/garbage.sql",
            "blobs/sha256/abc",
            "2026-07-10.sql",
        ] {
            assert_eq!(date_of_dump_key(bad), None, "key: {bad}");
        }
    }

    // --- export protocol ---

    #[test]
    fn export_request_bodies_match_the_polling_protocol() {
        assert_eq!(export_request_body(None), r#"{"output_format":"polling"}"#);
        assert_eq!(
            export_request_body(Some("bm")),
            r#"{"output_format":"polling","current_bookmark":"bm"}"#
        );
    }

    #[test]
    fn export_poll_parses_the_three_outcomes() {
        let complete = r#"{"success":true,"result":{"status":"complete",
            "at_bookmark":"bm","result":{"signed_url":"https://x/y.sql","filename":"y.sql"}}}"#;
        assert_eq!(
            parse_export_poll(complete),
            ExportPoll::Complete {
                signed_url: "https://x/y.sql".to_owned()
            }
        );

        // The flat spelling from Cloudflare's Workflows backup example.
        let flat =
            r#"{"success":true,"result":{"status":"complete","signed_url":"https://x/z.sql"}}"#;
        assert_eq!(
            parse_export_poll(flat),
            ExportPoll::Complete {
                signed_url: "https://x/z.sql".to_owned()
            }
        );

        let running = r#"{"success":true,"result":{"status":"active","at_bookmark":"bm2"}}"#;
        assert_eq!(
            parse_export_poll(running),
            ExportPoll::Continue {
                bookmark: Some("bm2".to_owned())
            }
        );

        let failed = r#"{"success":true,"result":{"status":"error","error":"boom"}}"#;
        assert_eq!(
            parse_export_poll(failed),
            ExportPoll::Failed("boom".to_owned())
        );

        let refused = r#"{"success":false,"errors":[{"code":10000,"message":"denied"}]}"#;
        assert_eq!(
            parse_export_poll(refused),
            ExportPoll::Failed("denied".to_owned())
        );

        assert!(matches!(parse_export_poll("<html>"), ExportPoll::Failed(_)));
    }

    // --- dump validation ---

    fn schema_text() -> String {
        use std::fmt::Write as _;
        EXPECTED_TABLES
            .iter()
            .fold(String::new(), |mut out, table| {
                let _ = writeln!(out, "CREATE TABLE {table} (x TEXT);");
                out
            })
    }

    fn dump_text() -> String {
        use std::fmt::Write as _;
        EXPECTED_ROWS.iter().fold(schema_text(), |mut out, table| {
            let _ = writeln!(out, "INSERT INTO \"{table}\" VALUES('x');");
            out
        })
    }

    #[test]
    fn scanner_accepts_a_complete_dump_and_hashes_it() {
        let dump = dump_text();
        let mut scanner = DumpScanner::new();
        scanner.update(dump.as_bytes());
        let check = scanner.finish();
        assert_eq!(check.error(), None);
        assert_eq!(check.bytes, dump.len() as u64);
        assert_eq!(check.sha256_hex, crate::auth::token_hash(&dump));
    }

    #[test]
    fn scanner_matches_statements_split_across_chunks() {
        let dump = dump_text();
        // Feed one byte at a time: every pattern necessarily spans
        // chunk boundaries.
        let mut scanner = DumpScanner::new();
        for byte in dump.as_bytes() {
            scanner.update(std::slice::from_ref(byte));
        }
        let check = scanner.finish();
        assert_eq!(check.error(), None);
        assert_eq!(check.sha256_hex, crate::auth::token_hash(&dump));
    }

    #[test]
    fn scanner_accepts_quoted_table_names() {
        use std::fmt::Write as _;
        let mut dump = EXPECTED_TABLES
            .iter()
            .fold(String::new(), |mut out, table| {
                let _ = writeln!(out, "CREATE TABLE \"{table}\" (x TEXT);");
                out
            });
        for table in EXPECTED_ROWS {
            let _ = writeln!(dump, "INSERT INTO \"{table}\" VALUES('x');");
        }
        let mut scanner = DumpScanner::new();
        scanner.update(dump.as_bytes());
        assert_eq!(scanner.finish().error(), None);
    }

    #[test]
    fn scanner_accepts_the_d1_exporter_spelling() {
        // The wrangler-owned migration table appears in real exports as
        // a quoted IF NOT EXISTS statement with no space before the
        // parenthesis.
        let mut dump = dump_text();
        dump = dump.replace(
            "CREATE TABLE d1_migrations (x TEXT);",
            "CREATE TABLE IF NOT EXISTS \"d1_migrations\"(x TEXT);",
        );
        let mut scanner = DumpScanner::new();
        scanner.update(dump.as_bytes());
        assert_eq!(scanner.finish().error(), None);
    }

    #[test]
    fn scanner_rejects_schema_only_dumps() {
        // All CREATE TABLE statements but no data: an export truncated
        // or emptied of rows must not validate - meta and d1_migrations
        // are populated by the migrations themselves, so every real
        // dump has rows for them.
        let mut scanner = DumpScanner::new();
        scanner.update(schema_text().as_bytes());
        let check = scanner.finish();
        assert_eq!(check.missing_rows, vec!["meta", "d1_migrations"]);
        let error = check.error().unwrap();
        assert!(error.contains("no rows"), "{error}");
    }

    #[test]
    fn scanner_reports_empty_and_missing_tables() {
        let empty = DumpScanner::new().finish();
        assert_eq!(empty.error().as_deref(), Some("the dump is empty"));

        let mut scanner = DumpScanner::new();
        scanner.update(b"CREATE TABLE users (x);\nCREATE TABLE meta (y);\n");
        let check = scanner.finish();
        assert_eq!(
            check.missing_tables,
            vec![
                "identities",
                "scopes",
                "scope_members",
                "tokens",
                "packages",
                "versions",
                "backup_pending",
                "d1_migrations"
            ]
        );
        let error = check.error().unwrap();
        assert!(error.contains("tokens, packages, versions"), "{error}");
    }

    // --- retention ---

    fn dates(specs: &[&str]) -> Vec<String> {
        specs.iter().map(|s| (*s).to_owned()).collect()
    }

    /// `n` consecutive dates ending at `2026-07-10` (inclusive), oldest
    /// first, entirely inside 2026-06/07 to keep expectations readable.
    fn consecutive(n: u32) -> Vec<String> {
        assert!(n <= 40);
        (0..n)
            .rev()
            .map(|back| {
                let day = 10i64 - i64::from(back);
                if day >= 1 {
                    format!("2026-07-{day:02}")
                } else {
                    format!("2026-06-{:02}", 30 + day)
                }
            })
            .collect()
    }

    #[test]
    fn fewer_than_thirty_dumps_prunes_nothing() {
        assert!(dates_to_prune(&consecutive(30), "2026-07-10").is_empty());
    }

    #[test]
    fn prunes_dailies_beyond_thirty_but_keeps_monthly_firsts() {
        // 2026-06-01 .. 2026-07-10: 40 dumps. Newest 30 = 2026-06-11
        // onward; June's first dump (06-01) survives as the monthly
        // first, 06-02 .. 06-10 go.
        let all = consecutive(40);
        assert_eq!(all[0], "2026-06-01");
        let pruned = dates_to_prune(&all, "2026-07-10");
        let expected: Vec<String> = (2..=10).map(|day| format!("2026-06-{day:02}")).collect();
        assert_eq!(pruned, expected);
    }

    #[test]
    fn monthly_firsts_expire_after_twelve_months() {
        let mut all = dates(&[
            "2025-01-05", // 18 months back: prune
            "2025-06-20", // 13 months back: prune
            "2025-08-15", // 11 months back: keep
        ]);
        all.extend(consecutive(30));
        assert_eq!(
            dates_to_prune(&all, "2026-07-10"),
            dates(&["2025-01-05", "2025-06-20"])
        );
    }

    #[test]
    fn the_monthly_first_is_the_oldest_dump_of_its_month() {
        let mut all = dates(&["2025-08-15", "2025-08-02", "2025-08-20"]);
        all.extend(consecutive(30));
        assert_eq!(
            dates_to_prune(&all, "2026-07-10"),
            dates(&["2025-08-15", "2025-08-20"])
        );
    }

    #[test]
    fn future_dated_dumps_neither_displace_dailies_nor_get_pruned() {
        // 31 consecutive dailies ending today: the oldest (2026-06-10)
        // is June's monthly first, so nothing is pruned. A stray
        // future-dated key must not change that by eating a newest-30
        // slot (which would push 2026-06-11 out), and must not be
        // deleted either.
        let mut all = dates(&["2099-01-01"]);
        all.extend(consecutive(31));
        assert_eq!(dates_to_prune(&all, "2026-07-10"), Vec::<String>::new());
    }

    #[test]
    fn unrecognized_keys_and_dates_are_never_pruned() {
        let mut all = dates(&["not-a-date", "2026-13-01", "2026-07-1"]);
        all.extend(consecutive(35));
        let pruned = dates_to_prune(&all, "2026-07-10");
        assert!(pruned.iter().all(|date| is_date(date)), "{pruned:?}");
        assert!(dates_to_prune(&consecutive(40), "garbage").is_empty());
    }

    // --- freshness ---

    #[test]
    fn freshness_is_fresh_stale_or_never() {
        let now = "2026-07-10T12:00:00.123Z";
        assert_eq!(
            freshness(now, Some("2026-07-10T03:00:00Z")),
            Freshness::Fresh
        );
        // Exactly 36 h is still fresh; one second past is stale.
        assert_eq!(
            freshness(now, Some("2026-07-09T00:00:00Z")),
            Freshness::Fresh
        );
        assert_eq!(
            freshness(now, Some("2026-07-08T23:59:59Z")),
            Freshness::Stale
        );
        assert_eq!(freshness(now, None), Freshness::Never);
        assert_eq!(freshness(now, Some("corrupt")), Freshness::Never);
        // A last-backup timestamp in the future (clock skew) is fresh.
        assert_eq!(
            freshness(now, Some("2026-07-11T00:00:00Z")),
            Freshness::Fresh
        );
        // An unreadable clock fails closed.
        assert_eq!(
            freshness("garbage", Some("2026-07-10T03:00:00Z")),
            Freshness::Stale
        );
    }

    #[test]
    fn epoch_parsing_handles_calendar_arithmetic() {
        assert_eq!(epoch_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_secs("1970-01-02T03:04:05.678Z"), Some(97_445));
        // 2026 is not a leap year; 2024 is.
        let feb28_2026 = epoch_secs("2026-02-28T00:00:00Z").unwrap();
        assert_eq!(
            epoch_secs("2026-03-01T00:00:00Z").unwrap() - feb28_2026,
            86_400
        );
        let feb28_2024 = epoch_secs("2024-02-28T00:00:00Z").unwrap();
        assert_eq!(
            epoch_secs("2024-03-01T00:00:00Z").unwrap() - feb28_2024,
            2 * 86_400
        );
        for bad in ["2026-07-10", "2026-07-10T25:00:00Z", "garbage", ""] {
            assert_eq!(epoch_secs(bad), None, "input: {bad}");
        }
    }

    // --- alerting ---

    #[test]
    fn alert_covers_freshness_and_the_backup_backlog() {
        assert_eq!(alert(Freshness::Fresh, 0), None);
        let stale = alert(Freshness::Stale, 0).unwrap();
        assert!(stale.contains("older than 36 h"), "{stale}");
        let never = alert(Freshness::Never, 0).unwrap();
        assert!(never.contains("no successful D1 dump"), "{never}");
        let one = alert(Freshness::Fresh, 1).unwrap();
        assert!(one.contains("1 verified archive blob is overdue"), "{one}");
        let both = alert(Freshness::Stale, 2).unwrap();
        assert!(
            both.contains("older than 36 h")
                && both.contains("2 verified archive blobs are overdue"),
            "{both}"
        );
    }
}
