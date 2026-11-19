//! Executed-semantics tests for `src/sql/alerts.rs`: which rows a cron
//! pass claims for its mail, pass after pass.

use cabin_api_worker::alerts::{ATTEMPTS, CRITICAL, Kind, PASS_META_KEY, RENOTIFY, WARNING};
use cabin_api_worker::sql;
use rusqlite::Connection;

use crate::common::{migrated_connection, rows};

/// The pass `minutes` after the first one, as the cron's clock writes it.
fn at(minutes: u32) -> String {
    format!(
        "2026-07-{:02}T{:02}:{:02}:00.000Z",
        16 + minutes / 1440,
        minutes % 1440 / 60,
        minutes % 60
    )
}

/// One cron pass as `glue::cron::report_alerts` runs it, whose mail
/// fails: record each kind at its level (0 is healthy), then claim.
/// The claimed rows, as `kind level notified_level`. D1 binds every
/// number as a REAL.
fn failed(conn: &Connection, minutes: u32, observed: &[(Kind, u8)]) -> Vec<String> {
    let now = at(minutes);
    for &(kind, level) in observed {
        if level == 0 {
            conn.execute(sql::CLEAR_ALERT, (kind.as_str(), &now))
                .expect("CLEAR_ALERT");
        } else {
            let summary = format!("why at {now}");
            conn.execute(
                sql::FIRE_ALERT,
                (kind.as_str(), f64::from(level), &now, summary),
            )
            .expect("FIRE_ALERT");
        }
    }
    let mut claim = conn.prepare(sql::CLAIM_ALERTS).expect("prepare");
    let mut claimed: Vec<String> = claim
        .query_map(
            (&now, RENOTIFY, f64::from(ATTEMPTS), f64::from(CRITICAL)),
            |row| {
                Ok(format!(
                    "{} {} {}",
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?
                ))
            },
        )
        .expect("CLAIM_ALERTS")
        .collect::<Result<_, _>>()
        .expect("claimed row");
    claimed.sort();
    claimed
}

/// The mail of the pass at `minutes` was accepted: each row it claimed
/// is recorded at the level the mail named.
fn deliver(conn: &Connection, minutes: u32, claimed: &[String]) {
    for row in claimed {
        let mut cells = row.split(' ');
        let kind = cells.next().expect("kind");
        let level: f64 = cells.next().expect("level").parse().expect("a level");
        conn.execute(sql::MARK_ALERT_NOTIFIED, (kind, level, at(minutes)))
            .expect("MARK_ALERT_NOTIFIED");
    }
}

/// One cron pass whose mail is accepted.
fn mailed(conn: &Connection, minutes: u32, observed: &[(Kind, u8)]) -> Vec<String> {
    let claimed = failed(conn, minutes, observed);
    deliver(conn, minutes, &claimed);
    claimed
}

const NONE: [&str; 0] = [];

/// The start of a pass for a kind of one UTC day:
/// `glue::cron::report_alerts` starts the kind's day before it records
/// the kind.
fn start_day(conn: &Connection, minutes: u32) {
    conn.execute(
        sql::START_ALERT_DAY,
        (Kind::SharedUploads.as_str(), at(minutes)),
    )
    .expect("START_ALERT_DAY");
}

/// A kind of one UTC day starts over with its day. What the new day
/// shows is claimed as a first firing even when the day before ended
/// higher, and the day that ended is claimed no recovery; a mail it
/// was still owed is dropped with its failed tries, and one accepted
/// after its day ended makes no recovery owed. A pass that could not
/// observe the kind starts its day all the same. Within a day the
/// kind is as any other: a rise is claimed, a level that stays is not,
/// and no day is long enough for a CRITICAL level's reminder. A kind
/// that is not of one day is left as it is.
#[test]
fn a_kind_of_one_day_starts_over_with_its_day() {
    let conn = migrated_connection();
    let pass = |minutes, shared, publication| {
        start_day(&conn, minutes);
        mailed(
            &conn,
            minutes,
            &[
                (Kind::SharedUploads, shared),
                (Kind::Publication, publication),
            ],
        )
    };
    // 10:00 and 23:45 of the first day.
    assert_eq!(pass(600, 2, 2), ["publication 2 0", "shared_uploads 2 0"]);
    assert_eq!(pass(1425, 2, 2), NONE);
    // 00:00 of the second day, already at half; then 01:00, at the limit.
    assert_eq!(pass(1440, 1, 2), ["shared_uploads 1 0"]);
    assert_eq!(pass(1500, 3, 2), ["shared_uploads 3 1"]);
    // 10:00, a day after the first mails: the other kind's reminder.
    assert_eq!(pass(2040, 3, 2), ["publication 2 2"]);
    assert_eq!(pass(2865, 3, 0), ["publication 0 2"]);
    // 00:00 of the third day, whose usage the pass could not read.
    start_day(&conn, 2880);
    assert_eq!(mailed(&conn, 2880, &[(Kind::Publication, 0)]), NONE);
    let state = "SELECT kind, level, notified_level, attempts, since FROM alerts ORDER BY kind";
    let healthy = |kind: &str, since: u32| {
        format!(
            "Text(\"{kind}\") | Integer(0) | Integer(0) | Integer(0) | Text(\"{}\")",
            at(since)
        )
    };
    assert_eq!(
        rows(&conn, state),
        [
            healthy("publication", 2865),
            healthy("shared_uploads", 2880)
        ]
    );

    // 23:45: half, and the mail is still on its way at 00:00 of the
    // fourth day, which is quiet. It is accepted after that pass.
    start_day(&conn, 4305);
    let late = failed(&conn, 4305, &[(Kind::SharedUploads, 1)]);
    assert_eq!(late, ["shared_uploads 1 0"]);
    start_day(&conn, 4320);
    assert_eq!(failed(&conn, 4320, &[(Kind::SharedUploads, 0)]), NONE);
    deliver(&conn, 4305, &late);
    start_day(&conn, 4335);
    assert_eq!(failed(&conn, 4335, &[(Kind::SharedUploads, 0)]), NONE);
    assert_eq!(
        rows(&conn, state),
        [
            healthy("publication", 2865),
            healthy("shared_uploads", 4320)
        ]
    );
}

/// The alert pass's state as a check from outside reads it at
/// `minutes`: `checked_seconds mail_attempts`.
fn pass_state(conn: &Connection, minutes: u32) -> String {
    conn.query_row(sql::ALERT_PASS_STATE, (at(minutes), PASS_META_KEY), |row| {
        Ok(format!(
            "{:?} {}",
            row.get::<_, Option<i64>>(0)?,
            row.get::<_, i64>(1)?
        ))
    })
    .expect("ALERT_PASS_STATE")
}

/// The time a pass records with its observations, as
/// `glue::cron::report_alerts` writes it.
fn record_pass(conn: &Connection, value: &str) {
    conn.execute(sql::UPSERT_META, (PASS_META_KEY, value))
        .expect("UPSERT_META");
}

/// A check from outside reads how long ago a pass recorded its
/// observations and how many passes in a row a mail has failed. A pass
/// that never recorded one, or a value that is no time, reads as no
/// age, never as a fresh one.
#[test]
fn the_pass_state_is_the_age_of_the_last_pass_and_its_failed_mails() {
    let conn = migrated_connection();
    assert_eq!(pass_state(&conn, 0), "None 0");
    record_pass(&conn, &at(0));
    assert_eq!(pass_state(&conn, 50), "Some(3000) 0");

    // The most failures of any kind's mail, next to a kind whose
    // mail went out.
    mailed(&conn, 0, &[(Kind::Publication, WARNING)]);
    failed(&conn, 15, &[(Kind::Verification, WARNING)]);
    failed(&conn, 30, &[(Kind::Verification, WARNING)]);
    record_pass(&conn, &at(30));
    assert_eq!(pass_state(&conn, 30), "Some(0) 2");
    mailed(&conn, 45, &[(Kind::Verification, WARNING)]);
    assert_eq!(pass_state(&conn, 45), "Some(900) 0");
    // A recovery's mail counts as a firing's does.
    failed(&conn, 60, &[(Kind::Verification, 0)]);
    assert_eq!(pass_state(&conn, 60), "Some(1800) 1");

    record_pass(&conn, "soon");
    assert_eq!(pass_state(&conn, 60), "None 1");
}

/// A kind is mailed when it first fires and once when it recovers, and
/// not while it stays as it is; one that recovers before any mail went
/// out is owed none.
#[test]
fn a_kind_is_claimed_when_it_fires_and_once_when_it_recovers() {
    let conn = migrated_connection();
    let warning = [(Kind::Verification, WARNING)];
    let healthy = [(Kind::Verification, 0)];

    assert_eq!(mailed(&conn, 0, &healthy), NONE, "nothing ever fired");
    assert_eq!(mailed(&conn, 15, &warning), ["verification 1 0"]);
    assert_eq!(mailed(&conn, 30, &warning), NONE, "it stays as it is");
    assert_eq!(mailed(&conn, 45, &healthy), ["verification 0 1"]);
    assert_eq!(mailed(&conn, 60, &healthy), NONE, "recovery is mailed once");

    assert_eq!(failed(&conn, 75, &warning), ["verification 1 0"]);
    assert_eq!(
        mailed(&conn, 90, &healthy),
        NONE,
        "its firing never reached anyone"
    );
    assert_eq!(
        rows(&conn, "SELECT since FROM alerts"),
        [format!("Text({:?})", at(90))],
        "since is when it returned to healthy"
    );
}

/// A rise is mailed, a fall is not, and the rise after a fall is.
#[test]
fn a_rise_is_claimed_and_a_fall_is_not() {
    let conn = migrated_connection();
    let mode = |level| [(Kind::ServiceMode, level)];

    assert_eq!(mailed(&conn, 0, &mode(WARNING)), ["service_mode 1 0"]);
    assert_eq!(mailed(&conn, 15, &mode(CRITICAL)), ["service_mode 2 1"]);
    assert_eq!(mailed(&conn, 30, &mode(WARNING)), NONE, "a fall");
    assert_eq!(mailed(&conn, 45, &mode(CRITICAL)), ["service_mode 2 1"]);
    assert_eq!(mailed(&conn, 60, &mode(CRITICAL + 1)), ["service_mode 3 2"]);
    assert_eq!(
        rows(&conn, "SELECT since, summary FROM alerts"),
        [format!(
            "Text({:?}) | Text({:?})",
            at(0),
            format!("why at {}", at(60))
        )],
        "since is when it left healthy; the summary is the last pass's"
    );
}

/// A kind that stays critical or above is mailed again every 24 hours;
/// a warning is not.
#[test]
fn a_critical_kind_is_claimed_again_after_a_day() {
    let conn = migrated_connection();
    let all = [
        (Kind::Publication, CRITICAL),
        (Kind::ServiceMode, CRITICAL + 1),
        (Kind::Verification, WARNING),
    ];

    assert_eq!(
        mailed(&conn, 0, &all),
        ["publication 2 0", "service_mode 3 0", "verification 1 0"]
    );
    assert_eq!(mailed(&conn, 1425, &all), NONE, "15 minutes short of a day");
    assert_eq!(
        mailed(&conn, 1440, &all),
        ["publication 2 2", "service_mode 3 3"]
    );
    assert_eq!(mailed(&conn, 1455, &all), NONE);
}

/// A mail that fails is tried by four passes, then once a day.
#[test]
fn a_failed_mail_is_tried_four_passes_and_then_daily() {
    let conn = migrated_connection();
    let warning = [(Kind::Verification, WARNING)];
    let owed = ["verification 1 0"];

    for attempt in 0..4 {
        assert_eq!(
            failed(&conn, attempt * 15, &warning),
            owed,
            "attempt {attempt}"
        );
    }
    assert_eq!(failed(&conn, 60, &warning), NONE, "the four are spent");
    assert_eq!(rows(&conn, "SELECT attempts FROM alerts"), ["Integer(4)"]);
    assert_eq!(failed(&conn, 45 + 1425, &warning), NONE);
    assert_eq!(
        failed(&conn, 45 + 1440, &warning),
        owed,
        "a day after the last"
    );
    assert_eq!(failed(&conn, 60 + 1440, &warning), NONE, "one try a day");
    assert_eq!(failed(&conn, 45 + 2865, &warning), NONE);
    assert_eq!(
        mailed(&conn, 45 + 2880, &warning),
        owed,
        "the day after that"
    );
    assert_eq!(
        rows(&conn, "SELECT attempts, notified_level FROM alerts"),
        ["Integer(0) | Integer(1)"]
    );
}

/// A fall, a rise and a recovery each start the four passes over, and
/// a recovery whose mail keeps failing is tried four passes like any
/// other.
#[test]
fn a_changed_level_starts_the_four_passes_over() {
    let conn = migrated_connection();
    let mode = |level| [(Kind::ServiceMode, level)];
    let spend = |from: u32, level: u8, owed: &str| {
        for attempt in 0..4 {
            assert_eq!(failed(&conn, from + attempt * 15, &mode(level)), [owed]);
        }
        assert_eq!(failed(&conn, from + 60, &mode(level)), NONE, "spent");
    };

    spend(0, WARNING, "service_mode 1 0");
    // A rise right after the four are spent.
    spend(75, CRITICAL, "service_mode 2 0");
    assert_eq!(
        mailed(&conn, 150, &mode(WARNING)),
        ["service_mode 1 0"],
        "a fall"
    );
    spend(165, CRITICAL, "service_mode 2 1");
    assert_eq!(
        failed(&conn, 240, &mode(0)),
        ["service_mode 0 1"],
        "a recovery"
    );
    for attempt in 1..4 {
        assert_eq!(
            failed(&conn, 240 + attempt * 15, &mode(0)),
            ["service_mode 0 1"]
        );
    }
    assert_eq!(failed(&conn, 300, &mode(0)), NONE, "the recovery's four");
    assert_eq!(
        rows(&conn, "SELECT since FROM alerts"),
        [format!("Text({:?})", at(240))],
        "since is when it returned to healthy"
    );
}

/// A kind that fires again before its recovery was mailed is mailed as
/// a first firing, whatever level was mailed before.
#[test]
fn a_firing_after_an_unmailed_recovery_is_claimed_as_a_first_firing() {
    let conn = migrated_connection();
    let mode = |level| [(Kind::ServiceMode, level)];

    assert_eq!(mailed(&conn, 0, &mode(CRITICAL)), ["service_mode 2 0"]);
    assert_eq!(failed(&conn, 15, &mode(0)), ["service_mode 0 2"]);
    assert_eq!(mailed(&conn, 30, &mode(WARNING)), ["service_mode 1 0"]);
    assert_eq!(
        rows(&conn, "SELECT since FROM alerts"),
        [format!("Text({:?})", at(30))],
        "since is when it left healthy again"
    );
}

/// Only the rows a pass claimed are recorded as mailed by it.
#[test]
fn a_delivery_marks_the_rows_its_pass_claimed_and_no_other() {
    let conn = migrated_connection();
    let warning = [(Kind::Verification, WARNING)];
    for attempt in 0..4 {
        failed(&conn, attempt * 15, &warning);
    }
    // The verification row is spent; this pass claims publication alone.
    let both = [(Kind::Verification, WARNING), (Kind::Publication, CRITICAL)];
    assert_eq!(mailed(&conn, 60, &both), ["publication 2 0"]);
    assert_eq!(
        rows(
            &conn,
            "SELECT kind, notified_level FROM alerts ORDER BY kind"
        ),
        [
            r#"Text("publication") | Integer(2)"#,
            r#"Text("verification") | Integer(0)"#
        ]
    );
}

/// A fall seen while a rise's mail was on its way keeps the lower
/// level, so the rise after it is mailed.
#[test]
fn a_delivery_after_a_fall_keeps_the_lower_level() {
    let conn = migrated_connection();
    let mode = |level| [(Kind::ServiceMode, level)];

    assert_eq!(mailed(&conn, 0, &mode(WARNING)), ["service_mode 1 0"]);
    let rise = failed(&conn, 15, &mode(CRITICAL));
    assert_eq!(rise, ["service_mode 2 1"]);
    assert_eq!(failed(&conn, 30, &mode(WARNING)), NONE, "a fall");
    deliver(&conn, 15, &rise);
    assert_eq!(mailed(&conn, 45, &mode(CRITICAL)), ["service_mode 2 1"]);
}

/// A delivery records the level its mail named. A row that recovered
/// while the mail was on its way is still owed its recovery, and a row
/// a later pass claimed is that pass's to record.
#[test]
fn a_delivery_records_the_level_its_mail_named() {
    let conn = migrated_connection();
    let warning = [(Kind::Verification, WARNING)];

    let first = failed(&conn, 0, &warning);
    conn.execute(sql::CLEAR_ALERT, (Kind::Verification.as_str(), at(1)))
        .expect("CLEAR_ALERT");
    deliver(&conn, 0, &first);
    assert_eq!(
        mailed(&conn, 15, &[(Kind::Verification, 0)]),
        ["verification 0 1"]
    );

    let earlier = failed(&conn, 30, &warning);
    let later = failed(&conn, 45, &warning);
    deliver(&conn, 30, &earlier);
    assert_eq!(
        rows(&conn, "SELECT notified_level FROM alerts"),
        ["Integer(0)"],
        "the later pass holds the claim"
    );
    deliver(&conn, 45, &later);
    assert_eq!(
        rows(&conn, "SELECT notified_level FROM alerts"),
        ["Integer(1)"]
    );
}
