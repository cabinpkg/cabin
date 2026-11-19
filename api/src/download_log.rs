//! Download counting from request logs: which records of a Logpush
//! `http_requests` file are downloads of an archive by the Cabin
//! client, and what one file adds to each version on each UTC day.
//!
//! A file is newline-delimited JSON, read here as its decompressed
//! bytes arrive. It is tallied whole or not at all: a record of
//! another shape than the expected one, or a file past a bound, yields
//! an error and no tally. A record too long to read is the exception:
//! it is passed over unread, as no download.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::alerts::{CRITICAL, WARNING};
use crate::routes::{Route, match_route};
use crate::signup::civil_from_days;

/// What the Cabin client's `User-Agent` starts with.
const CLIENT: &str = "cabin/";

const NANOS_PER_MILLI: u64 = 1_000_000;
const NANOS_PER_DAY: u64 = 86_400_000_000_000;

/// A day of 1973, in nanoseconds. The seconds, milliseconds and
/// microseconds of any request are below it, so a start below it is in
/// another unit than `unixnano`.
const EARLIEST_NS: u64 = 100_000_000_000_000_000;

/// The fields of a record that the rule reads. The Logpush job has to
/// send every one of them, and its timestamps as `unixnano`: a record
/// that lacks one, or holds one of another type or unit, fails its
/// file ([`FileError::Record`]). Nothing is counted on a guess at what
/// the job sends.
#[derive(Deserialize)]
struct Record<'a> {
    #[serde(rename = "ClientRequestHost", borrow)]
    host: Cow<'a, str>,
    #[serde(rename = "ClientRequestMethod", borrow)]
    method: Cow<'a, str>,
    #[serde(rename = "ClientRequestSource", borrow)]
    source: Cow<'a, str>,
    #[serde(rename = "ClientRequestURI", borrow)]
    uri: Cow<'a, str>,
    #[serde(rename = "ClientRequestUserAgent", borrow)]
    user_agent: Cow<'a, str>,
    #[serde(rename = "EdgeResponseStatus")]
    status: u16,
    #[serde(rename = "EdgeStartTimestamp")]
    start_ns: u64,
}

/// What decides whether a record is a download.
#[derive(Debug, Clone, Copy)]
pub struct Rule<'a> {
    /// The host that serves the archives.
    pub host: &'a str,
    /// When the statistics start, in milliseconds since the epoch. A
    /// request that started before it is not counted.
    pub cutoff_ms: u64,
}

/// Why a record is not a download. A record that fails several of the
/// rule's conditions is counted under one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Excluded {
    /// Longer than [`Limits::record_bytes`], and not read.
    Oversized,
    /// Another host than [`Rule::host`].
    Host,
    /// Not the path of an archive in its canonical form
    /// (`publication::archive_key`), or one with a query.
    Path,
    /// Not `GET`.
    Method,
    /// Not answered `200`.
    Status,
    /// Not the Cabin client.
    UserAgent,
    /// Not a visitor's request (`eyeball`): a Worker's own fetch, for
    /// one.
    Source,
    /// Started before [`Rule::cutoff_ms`].
    BeforeCutoff,
}

/// The version and UTC day a download is counted under. A version's
/// revisions are counted together.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Counted {
    pub scope: String,
    pub name: String,
    pub version: String,
    /// Days since 1970-01-01 of the UTC day the request started on.
    pub day: u64,
}

impl Counted {
    /// The day as `YYYY-MM-DD`.
    #[must_use]
    pub fn date(&self) -> String {
        let (year, month, day) = civil_from_days(self.day);
        format!("{year:04}-{month:02}-{day:02}")
    }
}

impl Rule<'_> {
    /// A download is a visitor's `GET` of an archive's canonical path
    /// on the archives' host, with no query, answered `200` to the
    /// Cabin client, started at or after the cutoff. A cache hit and a
    /// miss count alike. The start is compared in nanoseconds with the
    /// first nanosecond of the cutoff's millisecond, and a request that
    /// started on it counts.
    fn judge(&self, record: &Record<'_>) -> Result<Counted, Excluded> {
        if record.host != self.host {
            return Err(Excluded::Host);
        }
        let Some(Route::Artifact {
            scope,
            name,
            version,
            ..
        }) = match_route(&record.uri)
        else {
            return Err(Excluded::Path);
        };
        if record.method != "GET" {
            return Err(Excluded::Method);
        }
        if record.status != 200 {
            return Err(Excluded::Status);
        }
        if !record.user_agent.starts_with(CLIENT) {
            return Err(Excluded::UserAgent);
        }
        if record.source != "eyeball" {
            return Err(Excluded::Source);
        }
        if record.start_ns < self.cutoff_ms.saturating_mul(NANOS_PER_MILLI) {
            return Err(Excluded::BeforeCutoff);
        }
        Ok(Counted {
            scope: scope.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
            day: record.start_ns / NANOS_PER_DAY,
        })
    }
}

/// What a file of the job may hold (`api/docs/runbook.md`, "Download
/// statistics"): twice what the job is set to write, so that no file
/// it writes is refused.
pub const LIMITS: Limits = Limits {
    record_bytes: 65_536,
    bytes: 10_000_000,
    records: 20_000,
};

/// The bytes of one payload ([`Tally::payloads`]): far under the 2 MB
/// D1 takes in one bound value.
pub const PAYLOAD_BYTES: usize = 1_000_000;

/// How long after its upload a file is read: its lifetime in R2. Its
/// row is kept longer (`sql::DROP_OLD_LOG_FILES`), so a file that has
/// a row is never read again, however late R2 deletes it.
pub const READ_WINDOW_MS: u64 = 14 * 86_400_000;

/// How long a file may wait unread before it is a WARNING.
pub const UNREAD_WARNING_MS: u64 = 2 * 3_600_000;

/// How long before the end of its lifetime a file that is not counted
/// is CRITICAL.
pub const EXPIRY_CRITICAL_MS: u64 = 48 * 3_600_000;

/// The `download_logs` alert (`docs/runbook.md`, "Alerts") from a
/// pass's listing: `unread`, the uploads of the files that have no
/// row; `errors`, those of the files that cannot be counted; and `cut`,
/// a listing that stopped before the bucket's end, past which files go
/// unseen. A file is unresolved until it is counted, and CRITICAL once
/// it is within [`EXPIRY_CRITICAL_MS`] of the end of its
/// [`READ_WINDOW_MS`], or past it.
#[must_use]
pub fn backlog_alert(
    now_ms: u64,
    unread: &[u64],
    errors: &[u64],
    cut: bool,
) -> Option<(u8, String)> {
    let late = unread
        .iter()
        .filter(|&&uploaded| now_ms.saturating_sub(uploaded) > UNREAD_WARNING_MS)
        .count();
    let expiring_by = (now_ms + EXPIRY_CRITICAL_MS).saturating_sub(READ_WINDOW_MS);
    let expiring = unread
        .iter()
        .chain(errors)
        .filter(|&&uploaded| uploaded <= expiring_by)
        .count();
    let mut parts = Vec::new();
    if late > 0 {
        parts.push(format!("{late} unread for over 2 hours"));
    }
    if !errors.is_empty() {
        parts.push(format!("{} that cannot be counted", errors.len()));
    }
    if expiring > 0 {
        parts.push(format!(
            "{expiring} not counted with 48 hours or less of their 14 days left"
        ));
    }
    if cut {
        parts.push("the listing stopped before the bucket's end".to_owned());
    }
    let level = match (expiring, parts.is_empty()) {
        (0, true) => return None,
        (0, false) => WARNING,
        _ => CRITICAL,
    };
    Some((level, format!("download log files: {}", parts.join("; "))))
}

/// Whether a pass records the `download_logs` alert it worked out
/// ([`backlog_alert`]) over the level the kind is recorded at, `held`.
/// A listing that stopped before the bucket's end saw part of it: it may
/// raise the alert, never lower it.
#[must_use]
pub fn records(held: i64, firing: Option<&(u8, String)>, whole: bool) -> bool {
    whole || firing.map_or(0, |(level, _)| i64::from(*level)) >= held
}

/// Whether a file uploaded at `uploaded_ms` is read at `now_ms`.
#[must_use]
pub fn readable(uploaded_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(uploaded_ms) < READ_WINDOW_MS
}

/// The decompressed size a gzip file states in its last four bytes
/// (RFC 1952 `ISIZE`: the size modulo 2^32), if it is long enough to
/// have them. The job's files state it truly.
#[must_use]
pub fn stated_size(gzip: &[u8]) -> Option<u64> {
    let trailer = gzip.get(gzip.len().checked_sub(4)?..)?;
    Some(u64::from(u32::from_le_bytes(trailer.try_into().ok()?)))
}

/// What one file may hold. Past `bytes` or `records` the file fails
/// ([`FileError`]): its tally is not cut short. The records bound the
/// tally as well, since each version and day in it took a record.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The longest record that is read. A longer one is counted as
    /// [`Excluded::Oversized`] and the file goes on: a visitor can make
    /// a record long, and one request is not to cost a file its tally.
    /// A limit below the longest record that a download can have loses
    /// downloads.
    pub record_bytes: usize,
    /// Decompressed bytes.
    pub bytes: u64,
    /// Records.
    pub records: u64,
}

/// Why a file has no tally. The same bytes fail again when they are
/// read again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    /// More decompressed bytes than [`Limits::bytes`].
    Bytes,
    /// More records than [`Limits::records`].
    Records,
    /// The record on `line` (from 1) is not a JSON object with the
    /// fields the rule reads, in their types and unit.
    Record { line: u64, error: String },
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes => write!(f, "more than {} decompressed bytes", LIMITS.bytes),
            Self::Records => write!(f, "more than {} records", LIMITS.records),
            Self::Record { line, error } => write!(f, "record {line}: {error}"),
        }
    }
}

/// What one file adds.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Records in the file, downloads or not.
    pub records: u64,
    /// Downloads by version and UTC day.
    pub downloads: BTreeMap<Counted, u64>,
    /// Records that are not downloads, by reason.
    pub excluded: BTreeMap<Excluded, u64>,
}

impl Tally {
    /// The downloads as JSON arrays of `[scope, name, version, day,
    /// downloads]`, the day as `YYYY-MM-DD`: each download in one
    /// array, and no array longer than `bytes` unless one download
    /// alone is. One array is what one statement that adds a file's
    /// downloads is bound (`sql::ADD_LOG_DOWNLOADS`), which D1 limits
    /// in size; a tally with no download has none.
    #[must_use]
    pub fn payloads(&self, bytes: usize) -> Vec<String> {
        let mut payloads = Vec::new();
        let mut payload = String::new();
        for (counted, downloads) in &self.downloads {
            let entry = json!([
                counted.scope,
                counted.name,
                counted.version,
                counted.date(),
                downloads
            ])
            .to_string();
            // The entry would follow a comma, and a bracket ends the array.
            if !payload.is_empty() && payload.len() + entry.len() + 2 > bytes {
                payload.push(']');
                payloads.push(std::mem::take(&mut payload));
            }
            payload.push(if payload.is_empty() { '[' } else { ',' });
            payload.push_str(&entry);
        }
        if !payload.is_empty() {
            payload.push(']');
            payloads.push(payload);
        }
        payloads
    }
}

/// One file being read: [`File::feed`] it the decompressed bytes in
/// the order they arrive, split anywhere, then [`File::finish`]. It
/// holds one record at a time. A file that fails is gone with its
/// error, so nothing of it can be taken for a tally.
pub struct File<'a> {
    rule: Rule<'a>,
    limits: Limits,
    record: Vec<u8>,
    oversized: bool,
    bytes: u64,
    tally: Tally,
}

impl<'a> File<'a> {
    #[must_use]
    pub fn new(rule: Rule<'a>, limits: Limits) -> Self {
        Self {
            rule,
            limits,
            record: Vec::new(),
            oversized: false,
            bytes: 0,
            tally: Tally::default(),
        }
    }

    /// The file with its next bytes read.
    ///
    /// # Errors
    ///
    /// A [`FileError`]: the file has no tally.
    pub fn feed(mut self, mut chunk: &[u8]) -> Result<Self, FileError> {
        self.bytes += chunk.len() as u64;
        if self.bytes > self.limits.bytes {
            return Err(FileError::Bytes);
        }
        while let Some(end) = chunk.iter().position(|&byte| byte == b'\n') {
            self.extend(&chunk[..end]);
            self.end_record()?;
            chunk = &chunk[end + 1..];
        }
        self.extend(chunk);
        Ok(self)
    }

    /// The tally of the whole file, once its last byte has been fed. A
    /// last record need not end in a newline.
    ///
    /// # Errors
    ///
    /// A [`FileError`] of the last record.
    pub fn finish(mut self) -> Result<Tally, FileError> {
        if self.oversized || !self.record.is_empty() {
            self.end_record()?;
        }
        Ok(self.tally)
    }

    fn extend(&mut self, bytes: &[u8]) {
        if self.oversized {
            return;
        }
        if self.record.len() + bytes.len() > self.limits.record_bytes {
            self.oversized = true;
            self.record.clear();
        } else {
            self.record.extend_from_slice(bytes);
        }
    }

    fn end_record(&mut self) -> Result<(), FileError> {
        self.tally.records += 1;
        if self.tally.records > self.limits.records {
            return Err(FileError::Records);
        }
        if std::mem::take(&mut self.oversized) {
            *self.tally.excluded.entry(Excluded::Oversized).or_default() += 1;
            return Ok(());
        }
        let verdict = self.read().map_err(|error| FileError::Record {
            line: self.tally.records,
            error,
        })?;
        self.record.clear();
        match verdict {
            Ok(counted) => *self.tally.downloads.entry(counted).or_default() += 1,
            Err(reason) => *self.tally.excluded.entry(reason).or_default() += 1,
        }
        Ok(())
    }

    /// The buffered record, judged; or what is wrong with its shape.
    fn read(&self) -> Result<Result<Counted, Excluded>, String> {
        // The record holds what a visitor sent, and nothing documents
        // that it arrives as UTF-8: bytes that are not are read as
        // U+FFFD, not as the file's failure.
        let text = String::from_utf8_lossy(&self.record);
        // serde would read an array as the fields in their order.
        if !text.starts_with('{') {
            return Err("not a JSON object".to_owned());
        }
        let record: Record<'_> = serde_json::from_str(&text).map_err(|error| error.to_string())?;
        if record.start_ns < EARLIEST_NS {
            return Err("EdgeStartTimestamp is not in nanoseconds".to_owned());
        }
        Ok(self.rule.judge(&record))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    const HOST: &str = "static.example.com";
    const FMT: &str = "/archives/fmtlib/fmt/fmtlib-fmt-10.2.1-00112233aabbccdd.zip";
    /// 2027-01-05T00:00:00.123Z, the cutoff: in milliseconds, and its
    /// first nanosecond.
    const CUTOFF_MS: u64 = 1_799_107_200_123;
    const CUTOFF_NS: u64 = 1_799_107_200_123_000_000;
    /// 2027-01-05, in days since the epoch.
    const DAY: u64 = 20_823;
    /// 2027-01-05T12:00:00Z.
    const NOON: u64 = 1_799_150_400_000_000_000;
    /// The first nanosecond of 2027-01-06.
    const NEXT_DAY: u64 = 1_799_193_600_000_000_000;

    const RULE: Rule<'_> = Rule {
        host: HOST,
        cutoff_ms: CUTOFF_MS,
    };
    const ROOMY: Limits = Limits {
        record_bytes: 4_096,
        bytes: 1 << 20,
        records: 1_000,
    };

    /// A download's record, as the job sends it: the fields the rule
    /// reads and one it does not.
    fn download() -> Value {
        json!({
            "CacheCacheStatus": "hit",
            "ClientRequestHost": HOST,
            "ClientRequestMethod": "GET",
            "ClientRequestSource": "eyeball",
            "ClientRequestURI": FMT,
            "ClientRequestUserAgent": "cabin/0.18.0",
            "EdgeResponseStatus": 200,
            "EdgeStartTimestamp": NOON,
        })
    }

    /// The download's record with one field replaced.
    fn with(field: &str, value: Value) -> Value {
        let mut record = download();
        record[field] = value;
        record
    }

    fn lines(records: &[Value]) -> String {
        let mut text = String::new();
        for record in records {
            text.push_str(&record.to_string());
            text.push('\n');
        }
        text
    }

    /// The tally of a file fed in chunks of `size` bytes.
    fn tally_in(file: &[u8], limits: Limits, size: usize) -> Result<Tally, FileError> {
        let mut reader = File::new(RULE, limits);
        for chunk in file.chunks(size) {
            reader = reader.feed(chunk)?;
        }
        reader.finish()
    }

    fn tally(file: &str, limits: Limits) -> Result<Tally, FileError> {
        tally_in(file.as_bytes(), limits, usize::MAX)
    }

    fn fmt(day: u64) -> Counted {
        Counted {
            scope: "fmtlib".to_owned(),
            name: "fmt".to_owned(),
            version: "10.2.1".to_owned(),
            day,
        }
    }

    #[test]
    fn a_download_is_counted_under_its_version_and_utc_day() {
        let text = lines(&[
            download(),
            with("CacheCacheStatus", json!("miss")),
            with("ClientRequestUserAgent", json!("cabin/0.19.0 (linux)")),
            with("EdgeStartTimestamp", json!(CUTOFF_NS)),
            with("EdgeStartTimestamp", json!(NEXT_DAY - 1)),
            with("EdgeStartTimestamp", json!(NEXT_DAY)),
            with(
                "ClientRequestURI",
                json!("/archives/my-org/my_pkg-2/my-org-my_pkg-2-1.0.0-RC.1-00112233aabbccdd.zip"),
            ),
        ]);
        let found = tally(&text, ROOMY).expect("a tally");
        assert_eq!(found.records, 7);
        assert_eq!(found.excluded, BTreeMap::new());
        let rc = Counted {
            scope: "my-org".to_owned(),
            name: "my_pkg-2".to_owned(),
            version: "1.0.0-RC.1".to_owned(),
            day: DAY,
        };
        assert_eq!(
            found.downloads,
            BTreeMap::from([(fmt(DAY), 5), (fmt(DAY + 1), 1), (rc, 1)])
        );
        assert_eq!(fmt(DAY).date(), "2027-01-05");
        assert_eq!(fmt(DAY + 1).date(), "2027-01-06");
    }

    #[test]
    fn a_record_that_fails_one_condition_is_excluded_for_it() {
        let uri = |uri: &str| with("ClientRequestURI", json!(uri));
        let agent = |agent: &str| with("ClientRequestUserAgent", json!(agent));
        let cases = [
            (
                with("ClientRequestHost", json!("example.com")),
                Excluded::Host,
            ),
            (
                with("ClientRequestHost", json!(format!("x{HOST}"))),
                Excluded::Host,
            ),
            (
                with("ClientRequestHost", json!("STATIC.EXAMPLE.COM")),
                Excluded::Host,
            ),
            (uri(&format!("{FMT}?a=1")), Excluded::Path),
            (uri(&format!("/{FMT}")), Excluded::Path),
            (uri("/packages/fmtlib/fmt.json"), Excluded::Path),
            (
                uri("/fmtlib/fmt/fmtlib-fmt-10.2.1-00112233aabbccdd.zip"),
                Excluded::Path,
            ),
            (
                uri("/archives/fmtlib/fmt/foobar-fmt-10.2.1-00112233aabbccdd.zip"),
                Excluded::Path,
            ),
            (
                uri("/archives/Fmtlib/fmt/Fmtlib-fmt-10.2.1-00112233aabbccdd.zip"),
                Excluded::Path,
            ),
            (
                uri("/archives/fmtlib/fmt/fmtlib-fmt-10.2.1-00112233AABBCCDD.zip"),
                Excluded::Path,
            ),
            (with("ClientRequestMethod", json!("HEAD")), Excluded::Method),
            (with("ClientRequestMethod", json!("POST")), Excluded::Method),
            (with("EdgeResponseStatus", json!(206)), Excluded::Status),
            (with("EdgeResponseStatus", json!(404)), Excluded::Status),
            (agent("curl/8.7.1"), Excluded::UserAgent),
            (agent(""), Excluded::UserAgent),
            (agent("curl cabin/0.18.0"), Excluded::UserAgent),
            (agent("cabinet/1"), Excluded::UserAgent),
            (agent(r#"a "quoted" \ agent"#), Excluded::UserAgent),
            (
                with("ClientRequestSource", json!("edgeWorkerFetch")),
                Excluded::Source,
            ),
            (with("ClientRequestSource", json!("")), Excluded::Source),
            (
                with("EdgeStartTimestamp", json!(CUTOFF_NS - 1)),
                Excluded::BeforeCutoff,
            ),
        ];
        for (record, reason) in cases {
            let found = tally(&lines(std::slice::from_ref(&record)), ROOMY).expect("a tally");
            assert_eq!(found.downloads, BTreeMap::new(), "{record}");
            assert_eq!(found.excluded, BTreeMap::from([(reason, 1)]), "{record}");
        }
    }

    #[test]
    fn a_file_with_no_download_and_an_empty_file_have_a_tally() {
        assert_eq!(tally("", ROOMY), Ok(Tally::default()));
        let found = tally(&lines(&[with("ClientRequestMethod", json!("HEAD"))]), ROOMY);
        assert_eq!(
            found.map(|tally| (tally.records, tally.downloads.len())),
            Ok((1, 0))
        );
    }

    #[test]
    fn a_record_of_another_shape_fails_its_file() {
        let mut without_source = download();
        without_source
            .as_object_mut()
            .expect("an object")
            .shift_remove("ClientRequestSource");
        let in_order = json!([HOST, "GET", "eyeball", FMT, "cabin/0.18.0", 200, NOON]);
        let cases = [
            without_source.to_string(),
            with("ClientRequestSource", Value::Null).to_string(),
            with("EdgeStartTimestamp", json!("2027-01-05T12:00:00Z")).to_string(),
            with("EdgeStartTimestamp", json!(NOON / 1_000_000_000)).to_string(),
            with("EdgeResponseStatus", json!("200")).to_string(),
            in_order.to_string(),
            "fmtlib,fmt,10.2.1".to_owned(),
            String::new(),
            download().to_string()[..40].to_owned(),
        ];
        for broken in cases {
            let text = format!("{}\n{broken}\n{}\n", download(), download());
            match tally(&text, ROOMY) {
                Err(FileError::Record { line: 2, .. }) => {}
                other => panic!("{broken:?}: {other:?}"),
            }
        }
        // A last record cut short, with no newline after it.
        let text = format!("{}\n{}", download(), &download().to_string()[..40]);
        assert!(matches!(
            tally(&text, ROOMY),
            Err(FileError::Record { line: 2, .. })
        ));
    }

    #[test]
    fn bytes_that_are_not_utf8_do_not_fail_the_file() {
        let text = lines(&[download()]);
        let (head, tail) = text.split_once("cabin/0.18.0").expect("the agent");
        let file = [head.as_bytes(), b"curl \xff", tail.as_bytes()].concat();
        let found = tally_in(&file, ROOMY, usize::MAX).expect("a tally");
        assert_eq!(found.excluded, BTreeMap::from([(Excluded::UserAgent, 1)]));
    }

    #[test]
    fn the_tally_does_not_depend_on_where_the_bytes_are_split() {
        let text = lines(&[
            download(),
            with("ClientRequestMethod", json!("HEAD")),
            with(
                "ClientRequestUserAgent",
                json!("x".repeat(ROOMY.record_bytes)),
            ),
            with("EdgeStartTimestamp", json!(NEXT_DAY)),
        ]);
        let text = text.trim_end();
        let whole = tally(text, ROOMY).expect("a tally");
        assert_eq!(whole.records, 4);
        assert_eq!(
            whole.downloads,
            BTreeMap::from([(fmt(DAY), 1), (fmt(DAY + 1), 1)])
        );
        assert_eq!(
            whole.excluded,
            BTreeMap::from([(Excluded::Oversized, 1), (Excluded::Method, 1)])
        );
        for size in [1, 7, 4_096] {
            let found = tally_in(text.as_bytes(), ROOMY, size);
            assert_eq!(found.as_ref(), Ok(&whole), "chunks of {size}");
        }
    }

    #[test]
    fn the_downloads_are_split_into_payloads_by_their_bytes() {
        let found = Tally {
            records: 9,
            downloads: BTreeMap::from([(fmt(DAY), 3), (fmt(DAY + 1), 1), (fmt(DAY + 2), 5)]),
            excluded: BTreeMap::new(),
        };
        let entries = [
            r#"["fmtlib","fmt","10.2.1","2027-01-05",3]"#,
            r#"["fmtlib","fmt","10.2.1","2027-01-06",1]"#,
            r#"["fmtlib","fmt","10.2.1","2027-01-07",5]"#,
        ];
        let [first, second, third] = entries;
        let alone = entries.map(|entry| format!("[{entry}]"));
        let pair = format!("[{first},{second}]");
        assert_eq!(
            found.payloads(usize::MAX),
            [format!("[{first},{second},{third}]")]
        );
        assert_eq!(found.payloads(pair.len()), [pair.clone(), alone[2].clone()]);
        assert_eq!(found.payloads(pair.len() - 1), alone);
        assert_eq!(found.payloads(1), alone, "one download alone may be longer");
        assert_eq!(Tally::default().payloads(1), [""; 0]);
    }

    #[test]
    fn a_gzip_file_states_its_size_in_its_last_four_bytes() {
        assert_eq!(stated_size(&[0x1f, 0x8b, 0x10, 0x27, 0, 0]), Some(10_000));
        assert_eq!(stated_size(&[0xff; 4]), Some(4_294_967_295));
        assert_eq!(stated_size(&[1, 2, 3]), None);
    }

    #[test]
    fn the_backlog_alert_rises_with_the_files_age_and_errors() {
        const HOUR: u64 = 3_600_000;
        let now = 1_799_150_400_000;
        let critical = |summary: &str| Some((CRITICAL, format!("download log files: {summary}")));
        let warning = |summary: &str| Some((WARNING, format!("download log files: {summary}")));

        assert_eq!(backlog_alert(now, &[], &[], false), None);
        assert_eq!(backlog_alert(now, &[now, now - 2 * HOUR], &[], false), None);
        assert_eq!(
            backlog_alert(now, &[now, now - 2 * HOUR - 1], &[], false),
            warning("1 unread for over 2 hours")
        );
        assert_eq!(
            backlog_alert(now, &[], &[now, now], false),
            warning("2 that cannot be counted")
        );
        assert_eq!(
            backlog_alert(now, &[], &[], true),
            warning("the listing stopped before the bucket's end")
        );

        // 48 hours or less of the 14 days left, or none.
        let by = now - 12 * 24 * HOUR;
        assert_eq!(
            backlog_alert(now, &[by + 1], &[by + 1], false),
            warning("1 unread for over 2 hours; 1 that cannot be counted")
        );
        assert_eq!(
            backlog_alert(now, &[by, now - 20 * 24 * HOUR], &[], false),
            critical(
                "2 unread for over 2 hours; 2 not counted with 48 hours or less of their 14 days left"
            )
        );
        assert_eq!(
            backlog_alert(now, &[], &[by, now, now], true),
            critical(
                "3 that cannot be counted; 1 not counted with 48 hours or less of their 14 days left; \
                 the listing stopped before the bucket's end"
            )
        );
    }

    #[test]
    fn a_listing_stopped_short_never_lowers_the_alert() {
        let warning = Some((WARNING, String::new()));
        let critical = Some((CRITICAL, String::new()));
        assert!(!records(2, warning.as_ref(), false));
        assert!(records(2, critical.as_ref(), false));
        assert!(records(1, critical.as_ref(), false));
        assert!(records(2, warning.as_ref(), true));
        assert!(records(2, None, true));
    }

    #[test]
    fn a_file_is_read_while_it_is_younger_than_its_lifetime() {
        let uploaded = 1_799_150_400_000;
        assert!(readable(uploaded, uploaded));
        assert!(readable(uploaded, uploaded + READ_WINDOW_MS - 1));
        assert!(!readable(uploaded, uploaded + READ_WINDOW_MS));
        assert!(readable(uploaded + 1, uploaded), "a clock behind R2's");
        assert_eq!(READ_WINDOW_MS, 1_209_600_000, "fourteen days");
    }

    #[test]
    fn a_file_past_a_limit_fails_whole() {
        let text = lines(&[download(), download(), download()]);
        let bytes = text.len() as u64;
        assert!(tally(&text, Limits { bytes, ..ROOMY }).is_ok());
        let short = Limits {
            bytes: bytes - 1,
            ..ROOMY
        };
        assert_eq!(tally(&text, short), Err(FileError::Bytes));
        assert_eq!(tally_in(text.as_bytes(), short, 7), Err(FileError::Bytes));
        assert!(
            tally(
                &text,
                Limits {
                    records: 3,
                    ..ROOMY
                }
            )
            .is_ok()
        );
        assert_eq!(
            tally(
                &text,
                Limits {
                    records: 2,
                    ..ROOMY
                }
            ),
            Err(FileError::Records)
        );
        // An oversized record is a record, and its bytes are the file's.
        let long = lines(&[with("ClientRequestUserAgent", json!("x".repeat(8_192)))]);
        assert_eq!(
            tally(
                &long,
                Limits {
                    bytes: 8_192,
                    ..ROOMY
                }
            ),
            Err(FileError::Bytes)
        );
        assert_eq!(
            tally(
                &long.repeat(2),
                Limits {
                    records: 1,
                    ..ROOMY
                }
            ),
            Err(FileError::Records)
        );
        let exact = download().to_string();
        let fits = Limits {
            record_bytes: exact.len(),
            ..ROOMY
        };
        assert_eq!(
            tally(&exact, fits).map(|tally| tally.downloads.len()),
            Ok(1)
        );
        let short = Limits {
            record_bytes: exact.len() - 1,
            ..ROOMY
        };
        assert_eq!(
            tally(&exact, short).map(|tally| tally.excluded),
            Ok(BTreeMap::from([(Excluded::Oversized, 1)]))
        );
    }
}
