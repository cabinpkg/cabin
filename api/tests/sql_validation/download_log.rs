//! Executed-semantics tests for `src/sql/download_log.rs`: a file's
//! downloads are added once, to the totals and the days that are kept,
//! and nothing is added for a file that is settled or was read against
//! another start.

use std::collections::BTreeMap;

use cabin_api_worker::download_log::{Counted, Excluded, Tally};
use cabin_api_worker::sql;
use rusqlite::Connection;
use rusqlite::types::Value;
use serde_json::json;

use crate::common::{batch, migrated_connection, rows, seed_scope_collision};

/// 2027-01-05, in days since the epoch, and a time on it.
const TODAY: u64 = 20_823;
const NOW: &str = "2027-01-05T12:00:00.000Z";
const UPLOADED: &str = "2027-01-05T11:58:00.000Z";

/// Small enough that a tally of a few downloads takes several payloads.
const PAYLOAD: usize = 120;

fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

fn tally(records: u64, downloads: &[(&str, &str, &str, u64, u64)]) -> Tally {
    Tally {
        records,
        downloads: downloads
            .iter()
            .map(|&(scope, name, version, day, downloads)| {
                let counted = Counted {
                    scope: scope.to_owned(),
                    name: name.to_owned(),
                    version: version.to_owned(),
                    day,
                };
                (counted, downloads)
            })
            .collect(),
        excluded: BTreeMap::from([(Excluded::UserAgent, 2)]),
    }
}

fn start(conn: &Connection) -> String {
    conn.query_row(sql::META_VALUE, ["downloads_cutoff"], |row| row.get(0))
        .expect("the start")
}

/// One file's batch, as a pass sends it; how many rows its statements
/// changed, summed.
fn add(conn: &Connection, key: &str, start: &str, tally: &Tally) -> usize {
    let excluded = json!(tally.excluded).to_string();
    let records = Value::Integer(tally.records.try_into().expect("records"));
    let registration = vec![
        text(key),
        text(NOW),
        text(UPLOADED),
        records,
        text(&excluded),
        text(start),
    ];
    let mut statements = vec![(sql::REGISTER_LOG_FILE, registration)];
    for payload in tally.payloads(PAYLOAD) {
        let bound = vec![text(key), text(&payload)];
        statements.push((sql::ADD_LOG_DOWNLOADS, bound.clone()));
        let mut daily = bound.clone();
        daily.push(text(NOW));
        statements.push((sql::ADD_LOG_DAILY_DOWNLOADS, daily));
        statements.push((sql::COUNT_UNKNOWN_LOG_DOWNLOADS, bound));
    }
    statements.push((sql::FINISH_LOG_FILE, vec![text(key)]));
    batch(conn, statements)
        .into_iter()
        .map(|(changed, _)| changed)
        .sum()
}

fn totals(conn: &Connection) -> Vec<String> {
    rows(
        conn,
        "SELECT scope, name, version, downloads FROM versions ORDER BY 1, 2, 3",
    )
}

fn days(conn: &Connection) -> Vec<String> {
    rows(
        conn,
        "SELECT scope, name, version, day, downloads FROM version_daily_downloads \
         ORDER BY 1, 2, 3, 4",
    )
}

fn files(conn: &Connection) -> Vec<String> {
    rows(
        conn,
        "SELECT key, status, at, uploaded, records, excluded, unknown_version, error \
         FROM processed_log_files ORDER BY key",
    )
}

fn done(key: &str, records: u64, unknown: u64) -> String {
    let (key, at, uploaded, excluded) = (
        text(key),
        text(NOW),
        text(UPLOADED),
        text(r#"{"user_agent":2}"#),
    );
    format!(
        r#"{key:?} | Text("done") | {at:?} | {uploaded:?} | Integer({records}) | {excluded:?} | Integer({unknown}) | Null"#
    )
}

#[test]
fn a_files_downloads_are_added_once_to_the_totals_and_the_kept_days() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let start = start(&conn);
    assert_eq!(
        rows(
            &conn,
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', value) IS value \
               AND value BETWEEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 minute') \
                             AND strftime('%Y-%m-%dT%H:%M:%fZ', 'now') \
             FROM meta WHERE key = 'downloads_cutoff'"
        ),
        ["Integer(1)"],
        "the start is when the schema was made, in the form of the other times"
    );

    conn.execute_batch(
        "INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'lib', '2026-07-15T00:00:00.000Z', 2);
         INSERT INTO versions (scope, name, version)
           VALUES ('alpha', 'pkg', '2.0.0'), ('alpha', 'lib', '1.0.0');
         UPDATE versions SET yanked = 1 WHERE scope = 'beta';",
    )
    .expect("more versions, one of them yanked");

    // Four days of one version, the oldest of them the day before the
    // 90 that are kept; another version of its package, another package
    // of its scope and another scope's version of the same name, which
    // is yanked; and three versions that have no row, each matching a
    // version that has one in two of scope, name and version, in the
    // first payload and the last.
    let first = tally(
        20,
        &[
            ("alpha", "lib", "1.0.0", TODAY, 2),
            ("alpha", "pkg", "0.9.0", TODAY, 8),
            ("alpha", "pkg", "1.0.0", TODAY, 3),
            ("alpha", "pkg", "1.0.0", TODAY - 1, 2),
            ("alpha", "pkg", "1.0.0", TODAY - 89, 1),
            ("alpha", "pkg", "1.0.0", TODAY - 90, 4),
            ("alpha", "pkg", "2.0.0", TODAY, 9),
            ("alpha", "zzz", "1.0.0", TODAY, 30),
            ("beta", "pkg", "1.0.0", TODAY, 7),
            ("gamma", "pkg", "1.0.0", TODAY, 50),
        ],
    );
    let payloads = first.payloads(PAYLOAD);
    assert!(payloads.len() > 2, "the tally takes several payloads");
    assert!(payloads[0].contains("0.9.0") && payloads.last().unwrap().contains("gamma"));
    assert!(add(&conn, "20270105/a.log.gz", &start, &first) > 0);
    let after_first = [
        r#"Text("alpha") | Text("lib") | Text("1.0.0") | Integer(2)"#,
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Integer(10)"#,
        r#"Text("alpha") | Text("pkg") | Text("2.0.0") | Integer(9)"#,
        r#"Text("beta") | Text("pkg") | Text("1.0.0") | Integer(7)"#,
    ];
    assert_eq!(totals(&conn), after_first);
    let kept = [
        r#"Text("alpha") | Text("lib") | Text("1.0.0") | Text("2027-01-05") | Integer(2)"#,
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Text("2026-10-08") | Integer(1)"#,
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Text("2027-01-04") | Integer(2)"#,
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Text("2027-01-05") | Integer(3)"#,
        r#"Text("alpha") | Text("pkg") | Text("2.0.0") | Text("2027-01-05") | Integer(9)"#,
        r#"Text("beta") | Text("pkg") | Text("1.0.0") | Text("2027-01-05") | Integer(7)"#,
    ];
    assert_eq!(days(&conn), kept);
    assert_eq!(files(&conn), [done("20270105/a.log.gz", 20, 88)]);

    // The same file again, its batch as it was and as another pass
    // would send it: nothing changes.
    let other = tally(99, &[("alpha", "pkg", "1.0.0", TODAY, 100)]);
    for again in [&first, &other] {
        assert_eq!(add(&conn, "20270105/a.log.gz", &start, again), 0);
        assert_eq!(totals(&conn), after_first);
        assert_eq!(days(&conn), kept);
        assert_eq!(files(&conn), [done("20270105/a.log.gz", 20, 88)]);
    }

    // Another file adds to the same total and the same day.
    let second = tally(1, &[("alpha", "pkg", "1.0.0", TODAY, 1)]);
    assert!(add(&conn, "20270105/b.log.gz", &start, &second) > 0);
    assert_eq!(
        totals(&conn)[1],
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Integer(11)"#
    );
    assert_eq!(
        days(&conn)[3],
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Text("2027-01-05") | Integer(4)"#
    );

    // A file with no download is done too.
    assert_eq!(add(&conn, "20270105/c.log.gz", &start, &tally(0, &[])), 2);
    assert_eq!(files(&conn)[2], done("20270105/c.log.gz", 0, 0));
    assert_eq!(files(&conn).len(), 3);
}

#[test]
fn a_file_read_against_another_start_adds_nothing() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let start = start(&conn);
    let file = tally(3, &[("alpha", "pkg", "1.0.0", TODAY, 3)]);

    let earlier = "2020-01-01T00:00:00.000Z";
    assert_eq!(add(&conn, "20270105/a.log.gz", earlier, &file), 0);
    assert_eq!(
        totals(&conn)[0],
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Integer(0)"#
    );
    assert_eq!(days(&conn), [""; 0]);
    assert_eq!(
        files(&conn),
        [""; 0],
        "it is left to be read against the new start"
    );

    assert!(add(&conn, "20270105/a.log.gz", &start, &file) > 0);
    assert_eq!(
        totals(&conn)[0],
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Integer(3)"#
    );
}

#[test]
fn a_refused_file_adds_nothing_until_its_row_is_deleted() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let start = start(&conn);
    let refuse = |key: &str| {
        conn.execute(
            sql::REFUSE_LOG_FILE,
            [key, NOW, UPLOADED, "record 3: not a JSON object"],
        )
        .expect("refuse")
    };

    assert_eq!(refuse("20270105/a.log.gz"), 1);
    let (at, uploaded) = (text(NOW), text(UPLOADED));
    let refused = format!(
        r#"Text("20270105/a.log.gz") | Text("error") | {at:?} | {uploaded:?} | Integer(0) | Text("{{}}") | Integer(0) | Text("record 3: not a JSON object")"#
    );
    assert_eq!(files(&conn), std::slice::from_ref(&refused));

    // It is not read again; if it were, it would add nothing.
    let file = tally(3, &[("alpha", "pkg", "1.0.0", TODAY, 3)]);
    assert_eq!(add(&conn, "20270105/a.log.gz", &start, &file), 0);
    assert_eq!(
        totals(&conn)[0],
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Integer(0)"#
    );
    assert_eq!(files(&conn), [refused]);

    // A file that is done is not refused afterwards.
    assert!(add(&conn, "20270105/b.log.gz", &start, &file) > 0);
    assert_eq!(refuse("20270105/b.log.gz"), 0);
    assert_eq!(
        rows(
            &conn,
            "SELECT key, status FROM processed_log_files ORDER BY key"
        ),
        [
            r#"Text("20270105/a.log.gz") | Text("error")"#,
            r#"Text("20270105/b.log.gz") | Text("done")"#
        ]
    );
    // The keys asked about that have a row, and no other.
    assert_eq!(refuse("20270105/d.log.gz"), 1);
    let asked = json!([
        "20270105/a.log.gz",
        "20270105/b.log.gz",
        "20270105/c.log.gz"
    ]);
    let mut settled = conn.prepare(sql::SETTLED_LOG_FILES).expect("prepare");
    let settled: Vec<String> = settled
        .query_map([asked.to_string()], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("keys");
    assert_eq!(settled, ["20270105/a.log.gz", "20270105/b.log.gz"]);

    // With its row deleted the file is read again, and counted once.
    conn.execute(
        "DELETE FROM processed_log_files WHERE key = '20270105/a.log.gz'",
        [],
    )
    .expect("delete the row");
    assert!(add(&conn, "20270105/a.log.gz", &start, &file) > 0);
    assert_eq!(
        totals(&conn)[0],
        r#"Text("alpha") | Text("pkg") | Text("1.0.0") | Integer(6)"#
    );
}

#[test]
fn the_days_and_the_rows_are_dropped_at_their_ages() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    conn.execute_batch(
        "INSERT INTO version_daily_downloads (scope, name, version, day, downloads)
           VALUES ('alpha', 'pkg', '1.0.0', '2026-10-07', 1),
                  ('alpha', 'pkg', '1.0.0', '2026-10-08', 2),
                  ('alpha', 'pkg', '1.0.0', '2027-01-05', 3);
         INSERT INTO processed_log_files (key, status, at, uploaded, error)
           VALUES ('a', 'done', '2026-12-06T11:59:59.999Z', '2026-12-06T11:00:00.000Z', NULL),
                  ('b', 'error', '2026-12-06T11:59:59.999Z', '2026-12-06T11:00:00.000Z', 'e'),
                  ('c', 'done', '2026-12-06T12:00:00.000Z', '2026-12-06T11:00:00.000Z', NULL),
                  ('d', 'error', '2026-12-06T12:00:00.000Z', '2026-11-01T00:00:00.000Z', 'e');",
    )
    .expect("seed");

    assert_eq!(
        conn.execute(sql::DROP_OLD_DAILY_DOWNLOADS, [NOW])
            .expect("drop days"),
        1
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT day FROM version_daily_downloads ORDER BY day"
        ),
        [r#"Text("2026-10-08")"#, r#"Text("2027-01-05")"#],
        "the 90 days that end today are kept"
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT downloads FROM versions WHERE scope = 'alpha'"
        ),
        ["Integer(0)"]
    );

    assert_eq!(
        conn.execute(sql::DROP_OLD_LOG_FILES, [NOW])
            .expect("drop rows"),
        2
    );
    assert_eq!(
        rows(&conn, "SELECT key FROM processed_log_files ORDER BY key"),
        [r#"Text("c")"#, r#"Text("d")"#],
        "a row is kept 30 days from when it was written, whatever its file's age"
    );
}
