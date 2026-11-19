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

use serde::Deserialize;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
