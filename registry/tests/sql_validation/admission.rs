//! Executed-semantics tests for `src/sql/admission.rs`: the admission
//! gates and the `changes()` chain behind them, the reservation
//! lifecycle, and the cron sweeps. `prepare` sees none of it.

use cabin_registry_worker::{quota, sql, upload};
use rusqlite::{Connection, params};

use crate::common::{
    Limits, Op, admit_new, admit_resumed, admit_revival, confirm, footprint, migrated_connection,
    reject, release, rows, sweep,
};

const EARLIER: &str = "2026-10-25T08:00:00.000Z";
const DAY: &str = "2026-10-26";
const NOW: &str = "2026-10-26T08:00:00.000Z";

/// User 2 partway through a UTC day in scope `alpha`: package `lib`
/// created today, holding a verified revision (`r1`), a stored pending
/// one (`p1`), a rejected one (`rj`), and a released reservation
/// (`rl`), the last three admitted today at 10 bytes each. User 3 owns
/// `foo-bar` and has uploaded nothing.
fn seeded() -> Connection {
    let conn = migrated_connection();
    conn.execute_batch(&format!(
        r"
        INSERT INTO users (id, created_at, package_count, stored_bytes)
          VALUES (2, '{EARLIER}', 1, 20), (3, '{EARLIER}', 1, 0), (4, '{EARLIER}', 0, 0);
        INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
          VALUES ('alpha', 'github', '2', '{EARLIER}');
        INSERT INTO packages (scope, name, created_at, created_by)
          VALUES ('alpha', 'lib', '{DAY}T05:00:00.000Z', 2), ('alpha', 'foo-bar', '{EARLIER}', 3);
        INSERT INTO versions (scope, name, version)
          VALUES ('alpha', 'lib', '1.0.0'), ('alpha', 'lib', '2.0.0');
        INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                               published_at, archive_size, published_by, verification,
                               verification_reason, verified_at, rejected_at, upload_id,
                               charged_at, charged_class, stored_at)
          VALUES
            ('alpha', 'lib', '1.0.0', 'r1', 'r1', '{{}}', '{EARLIER}', 10, 2, 'verified',
             NULL, '{EARLIER}', NULL, 'u-r1', '{EARLIER}', 'default', '{EARLIER}'),
            ('alpha', 'lib', '2.0.0', 'p1', 'p1', '{{}}', '{DAY}T07:00:00.000Z', 10, 2, 'pending',
             NULL, NULL, NULL, 'u-p1', '{DAY}T07:00:00.000Z', 'default', '{DAY}T07:00:01.000Z'),
            ('alpha', 'lib', '2.0.0', 'rj', 'rj', '{{}}', '{DAY}T06:00:00.000Z', 10, 2, 'rejected',
             'bad', NULL, '{DAY}T06:10:00.000Z', 'u-rj', '{DAY}T06:00:00.000Z', 'default',
             '{DAY}T06:00:01.000Z'),
            ('alpha', 'lib', '1.0.0', 'rl', 'rl', '{{}}', '{DAY}T06:30:00.000Z', 10, 2, 'rejected',
             'upload_not_stored', NULL, '{DAY}T06:45:00.000Z', 'u-rl', '{DAY}T06:30:00.000Z',
             'default', NULL);
        INSERT INTO user_daily_upload_usage (usage_date, user_id, bytes, uploads)
          VALUES ('{DAY}', 2, 30, 3);
        INSERT INTO shared_daily_upload_usage (usage_date, quota_class, bytes, uploads)
          VALUES ('{DAY}', 'default', 30, 3);
        "
    ))
    .expect("seed the busy day");
    conn
}

/// The three ways an operation takes its reservation.
#[derive(Clone, Copy, Debug)]
enum Gate {
    New,
    Revival,
    Resumed,
}

impl Gate {
    /// The operation [`seeded`] admits through this gate: a new package
    /// `fresh`, the rejected `rj` under a new upload id, or the released
    /// `rl` under its own. The last two sit beside live different bytes,
    /// so they carry the opt-in.
    fn admissible(self) -> Op<'static> {
        let op = Op {
            user: 2.0,
            class: "default",
            scope: "alpha",
            name: "lib",
            version: "2.0.0",
            revision: "rj",
            metadata: "{}",
            size: 10.0,
            opt_in: true,
            stamp: NOW,
            upload_id: "u-new",
        };
        match self {
            Gate::New => Op {
                name: "fresh",
                version: "1.0.0",
                revision: "f1",
                size: 5.0,
                opt_in: false,
                ..op
            },
            Gate::Revival => op,
            Gate::Resumed => Op {
                version: "1.0.0",
                revision: "rl",
                upload_id: "u-rl",
                ..op
            },
        }
    }

    /// The gate's batch; each statement's changed rows.
    fn run(self, conn: &Connection, op: &Op, limits: &Limits) -> Vec<usize> {
        let results = match self {
            Gate::New => admit_new(conn, op, limits),
            Gate::Revival => admit_revival(conn, op, limits),
            Gate::Resumed => admit_resumed(conn, op, limits),
        };
        results.into_iter().map(|(changed, _)| changed).collect()
    }
}

/// A batch whose gate refused changed no row in any statement and left
/// every table as it was.
fn assert_refused(gate: Gate, conn: &Connection, op: &Op, limits: &Limits, case: &str) {
    let before = footprint(conn);
    let changed = gate.run(conn, op, limits);
    assert!(
        changed.iter().all(|&n| n == 0),
        "{gate:?} {case}: {changed:?}"
    );
    assert_eq!(footprint(conn), before, "{gate:?} {case}");
}

/// A batch whose gate held changed exactly one row in every statement.
fn assert_admitted(gate: Gate, conn: &Connection, op: &Op, limits: &Limits, case: &str) {
    let changed = gate.run(conn, op, limits);
    assert!(
        changed.iter().all(|&n| n == 1),
        "{gate:?} {case}: {changed:?}"
    );
}

/// Each limit a gate enforces is exact, and it alone decides: one below
/// what the operation needs refuses the whole batch - no statement
/// after the gate applies on its own conditions - and exactly what it
/// needs admits it, with every statement of the batch applying.
#[test]
fn every_limit_is_exact_and_refuses_the_whole_batch() {
    type Field = fn(&mut Limits) -> &mut f64;
    let bytes: Field = |limits| &mut limits.bytes;
    let versions_daily: Field = |limits| &mut limits.versions_daily;
    let packages_daily: Field = |limits| &mut limits.packages_daily;
    let packages_total: Field = |limits| &mut limits.packages_total;
    let upload_bytes_daily: Field = |limits| &mut limits.upload_bytes_daily;
    let uploads_daily: Field = |limits| &mut limits.uploads_daily;
    let shared_bytes_daily: Field = |limits| &mut limits.shared_bytes_daily;
    let shared_uploads_daily: Field = |limits| &mut limits.shared_uploads_daily;
    let pending: Field = |limits| &mut limits.pending;
    // The limit, and the least value that admits the seeded operation:
    // the usage plus the archive for a byte limit, the count plus one
    // for a count limit.
    let cases: [(Gate, &str, Field, f64); 19] = [
        (Gate::New, "storage", bytes, 25.0),
        (Gate::New, "package revisions per day", versions_daily, 1.0),
        (Gate::New, "new packages per day", packages_daily, 2.0),
        (Gate::New, "packages in total", packages_total, 2.0),
        (Gate::New, "user bytes per day", upload_bytes_daily, 35.0),
        (Gate::New, "user uploads per day", uploads_daily, 4.0),
        (Gate::New, "shared bytes per day", shared_bytes_daily, 35.0),
        (
            Gate::New,
            "shared uploads per day",
            shared_uploads_daily,
            4.0,
        ),
        (Gate::New, "pending uploads", pending, 2.0),
        (Gate::Revival, "storage", bytes, 30.0),
        (
            Gate::Revival,
            "package revisions per day",
            versions_daily,
            4.0,
        ),
        (
            Gate::Revival,
            "user bytes per day",
            upload_bytes_daily,
            40.0,
        ),
        (Gate::Revival, "user uploads per day", uploads_daily, 4.0),
        (
            Gate::Revival,
            "shared bytes per day",
            shared_bytes_daily,
            40.0,
        ),
        (
            Gate::Revival,
            "shared uploads per day",
            shared_uploads_daily,
            4.0,
        ),
        (Gate::Revival, "pending uploads", pending, 2.0),
        (Gate::Resumed, "storage", bytes, 30.0),
        (
            Gate::Resumed,
            "package revisions per day",
            versions_daily,
            4.0,
        ),
        (Gate::Resumed, "pending uploads", pending, 2.0),
    ];
    for (gate, case, field, least) in cases {
        let conn = seeded();
        let op = gate.admissible();
        let mut limits = Limits::of("default");
        *field(&mut limits) = least - 1.0;
        assert_refused(gate, &conn, &op, &limits, case);
        *field(&mut limits) = least;
        assert_admitted(gate, &conn, &op, &limits, case);
    }
}

/// The conditions that are not limits refuse the whole batch the same
/// way; where a condition has an admissible side, that side is
/// admitted.
#[test]
#[allow(clippy::too_many_lines)] // one independent case per condition
fn every_other_condition_refuses_the_whole_batch() {
    type Change = fn(&mut Op<'static>);
    let cases: [(Gate, &str, Change, Option<Change>); 15] = [
        (
            Gate::New,
            "the revision exists",
            |op| (op.name, op.version, op.revision) = ("lib", "2.0.0", "p1"),
            None,
        ),
        (
            Gate::New,
            "the upload id names another row",
            |op| op.upload_id = "u-p1",
            None,
        ),
        (
            Gate::New,
            "live different bytes without the opt-in",
            |op| (op.name, op.version, op.revision) = ("lib", "2.0.0", "x1"),
            Some(|op| {
                (op.name, op.version, op.revision, op.opt_in) = ("lib", "2.0.0", "x1", true);
            }),
        ),
        (
            Gate::New,
            "a -/_ twin exists",
            |op| op.name = "foo_bar",
            None,
        ),
        (
            Gate::Revival,
            "live different bytes without the opt-in",
            |op| op.opt_in = false,
            None,
        ),
        (
            Gate::Revival,
            "the upload id names another row",
            |op| op.upload_id = "u-p1",
            None,
        ),
        (
            Gate::Revival,
            "the upload id is the rejected operation's own",
            |op| op.upload_id = "u-rj",
            None,
        ),
        (
            Gate::Revival,
            "the stamp does not advance the row's",
            |op| op.stamp = "2026-10-26T06:00:00.000Z",
            Some(|op| op.stamp = "2026-10-26T06:00:00.001Z"),
        ),
        (
            Gate::Revival,
            "the row is not rejected",
            |op| op.revision = "p1",
            None,
        ),
        (
            Gate::Resumed,
            "live different bytes without the opt-in",
            |op| op.opt_in = false,
            None,
        ),
        (Gate::Resumed, "another publisher", |op| op.user = 3.0, None),
        (
            Gate::Resumed,
            "another upload id",
            |op| op.upload_id = "u-new",
            None,
        ),
        (
            Gate::Resumed,
            "the rejected upload was stored",
            |op| (op.version, op.revision, op.upload_id) = ("2.0.0", "rj", "u-rj"),
            None,
        ),
        (
            Gate::Resumed,
            "the stamp does not advance the row's",
            |op| op.stamp = "2026-10-26T06:30:00.000Z",
            Some(|op| op.stamp = "2026-10-26T06:30:00.001Z"),
        ),
        (
            Gate::Resumed,
            "the retry window has passed",
            |op| op.stamp = "2026-11-09T06:30:00.001Z",
            Some(|op| op.stamp = "2026-11-09T06:30:00.000Z"),
        ),
    ];
    let limits = Limits::of("default");
    for (gate, case, refuse, admit) in cases {
        let conn = seeded();
        let mut op = gate.admissible();
        refuse(&mut op);
        assert_refused(gate, &conn, &op, &limits, case);
        let mut op = gate.admissible();
        if let Some(admit) = admit {
            admit(&mut op);
        }
        assert_admitted(gate, &conn, &op, &limits, case);
    }
}

fn revision(conn: &Connection, revision: &str) -> String {
    rows(
        conn,
        &format!(
            "SELECT verification, verification_reason, published_at, published_by, upload_id, \
             charged_at, charged_class, stored_at, rejected_at FROM revisions \
             WHERE revision = '{revision}'"
        ),
    )
    .join("\n")
}

/// `(package_count, stored_bytes)` of a user, then the `(bytes,
/// uploads)` of their and of the default class's allowance rows on
/// `day`.
fn charges(conn: &Connection, user: i64, day: &str) -> Vec<String> {
    rows(
        conn,
        &format!(
            "SELECT package_count, stored_bytes FROM users WHERE id = {user} UNION ALL \
             SELECT bytes, uploads FROM user_daily_upload_usage \
               WHERE user_id = {user} AND usage_date = '{day}' UNION ALL \
             SELECT bytes, uploads FROM shared_daily_upload_usage \
               WHERE quota_class = 'default' AND usage_date = '{day}'"
        ),
    )
}

/// What each admission writes: a reservation bound to its upload id,
/// the storage charge, and - unless it resumes an operation that was
/// already charged - both daily allowances. A package counts toward its
/// creator once, when the admission creates it.
#[test]
fn each_admission_charges_what_it_takes_and_nothing_twice() {
    let limits = Limits::of("default");
    let reservation = |upload_id: &str, charged_at: &str| {
        format!(
            "Text(\"pending\") | Null | Text(\"{NOW}\") | Integer(2) | Text(\"{upload_id}\") | \
             Text(\"{charged_at}\") | Text(\"default\") | Null | Null"
        )
    };

    let conn = seeded();
    Gate::New.run(&conn, &Gate::New.admissible(), &limits);
    assert_eq!(revision(&conn, "f1"), reservation("u-new", NOW));
    assert_eq!(
        charges(&conn, 2, DAY),
        [
            "Integer(2) | Integer(25)",
            "Integer(35) | Integer(4)",
            "Integer(35) | Integer(4)"
        ]
    );
    // A second revision into the package just created, and one into
    // another account's package: neither counts a package again, and
    // the package rows keep their creator and creation time.
    let roomy = Limits {
        pending: 10.0,
        ..limits
    };
    let packages = rows(&conn, "SELECT * FROM packages");
    for (name, version, revision, upload_id) in [
        ("fresh", "2.0.0", "f2", "u-f2"),
        ("foo-bar", "1.0.0", "b1", "u-b1"),
    ] {
        let op = Op {
            name,
            version,
            revision,
            upload_id,
            ..Gate::New.admissible()
        };
        assert_admitted(Gate::New, &conn, &op, &roomy, name);
    }
    assert_eq!(rows(&conn, "SELECT * FROM packages"), packages);
    assert_eq!(
        charges(&conn, 2, DAY),
        [
            "Integer(2) | Integer(35)",
            "Integer(45) | Integer(6)",
            "Integer(45) | Integer(6)"
        ]
    );
    assert_eq!(
        charges(&conn, 3, DAY),
        ["Integer(1) | Integer(0)", "Integer(45) | Integer(6)"]
    );

    let conn = seeded();
    Gate::Revival.run(&conn, &Gate::Revival.admissible(), &limits);
    assert_eq!(revision(&conn, "rj"), reservation("u-new", NOW));
    assert_eq!(
        charges(&conn, 2, DAY),
        [
            "Integer(1) | Integer(30)",
            "Integer(40) | Integer(4)",
            "Integer(40) | Integer(4)"
        ]
    );

    // A resumed operation keeps its first charge: its allowances are
    // neither conditions nor charged again, even when they are spent.
    let conn = seeded();
    let spent = Limits {
        upload_bytes_daily: 0.0,
        uploads_daily: 0.0,
        shared_bytes_daily: 0.0,
        shared_uploads_daily: 0.0,
        ..limits
    };
    assert_admitted(
        Gate::Resumed,
        &conn,
        &Gate::Resumed.admissible(),
        &spent,
        "spent",
    );
    assert_eq!(
        revision(&conn, "rl"),
        reservation("u-rl", "2026-10-26T06:30:00.000Z")
    );
    assert_eq!(
        charges(&conn, 2, DAY),
        [
            "Integer(1) | Integer(30)",
            "Integer(30) | Integer(3)",
            "Integer(30) | Integer(3)"
        ]
    );
}

/// What the admission preflight reads for `op`, as the glue's
/// `UploadUsageRecord` names the columns.
fn usage(conn: &Connection, op: &Op) -> (quota::UploadUsage, bool, String, i64) {
    let count = |row: &rusqlite::Row, column: &str| -> rusqlite::Result<u64> {
        Ok(u64::try_from(row.get::<_, i64>(column)?).expect("non-negative"))
    };
    conn.query_row(
        sql::UPLOAD_USAGE,
        params![
            op.stamp,
            op.user,
            quota::shared_class(op.class),
            op.scope,
            op.name,
            op.version
        ],
        |row| {
            Ok((
                quota::UploadUsage {
                    user_bytes_today: count(row, "user_bytes")?,
                    user_uploads_today: count(row, "user_uploads")?,
                    shared_bytes_today: count(row, "shared_bytes")?,
                    shared_uploads_today: count(row, "shared_uploads")?,
                    pending: count(row, "pending")?,
                },
                row.get("version_has_revisions")?,
                row.get("reset_at")?,
                row.get("retry_after_secs")?,
            ))
        },
    )
    .expect("UPLOAD_USAGE")
}

/// The glue's allowance preflight of a newly charged upload.
fn preflight(conn: &Connection, op: &Op) -> Result<(), quota::UploadDenial> {
    let (usage, ..) = usage(conn, op);
    let quotas = quota::quotas_for_class(op.class);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small test sizes
    quota::check_daily_uploads(op.size as u64, &usage, &quotas)
        .and_then(|()| quota::check_pending_uploads(&usage, &quotas))
}

/// The usage read's columns are the glue's deserialization contract,
/// and its clock answers come from the request's own sample: the
/// allowances of the sample's UTC day, reopening at the next midnight.
/// The pending count includes reservations.
#[test]
fn the_usage_read_reports_the_samples_own_day() {
    let conn = seeded();
    let limits = Limits::of("default");
    let op = Gate::New.admissible();
    let (read, version_has_revisions, reset_at, retry_after_secs) = usage(&conn, &op);
    assert_eq!((read.user_bytes_today, read.user_uploads_today), (30, 3));
    assert_eq!(
        (read.shared_bytes_today, read.shared_uploads_today),
        (30, 3)
    );
    assert_eq!(read.pending, 1);
    assert!(!version_has_revisions);
    assert_eq!(reset_at, "2026-10-27T00:00:00Z");
    assert_eq!(retry_after_secs, 16 * 60 * 60);

    assert_admitted(Gate::New, &conn, &op, &limits, "fresh");
    let (read, version_has_revisions, ..) = usage(&conn, &op);
    assert_eq!(read.pending, 2, "a reservation takes a pending slot");
    assert!(version_has_revisions);

    // The last millisecond of the day and the first of the next.
    let late = Op {
        stamp: "2026-10-26T23:59:59.999Z",
        ..op
    };
    let (read, _, reset_at, retry_after_secs) = usage(&conn, &late);
    assert_eq!(
        (read.user_uploads_today, reset_at.as_str()),
        (4, "2026-10-27T00:00:00Z")
    );
    assert_eq!(retry_after_secs, 1);
    let next = Op {
        stamp: "2026-10-27T00:00:00.000Z",
        ..op
    };
    let (read, _, reset_at, _) = usage(&conn, &next);
    assert_eq!(
        (read.user_uploads_today, reset_at.as_str()),
        (0, "2026-10-28T00:00:00Z")
    );
    assert_eq!(read.shared_uploads_today, 0);
}

/// One upload by `user` of a new package named after its revision.
fn upload<'a>(user: f64, class: &'a str, revision: &'a str, size: f64, stamp: &'a str) -> Op<'a> {
    Op {
        user,
        class,
        scope: "alpha",
        name: revision,
        version: "1.0.0",
        revision,
        metadata: "{}",
        size,
        opt_in: false,
        stamp,
        upload_id: revision,
    }
}

/// The default class's configured allowances, each raced for by two
/// uploads whose preflights both pass on the same headroom - two
/// accounts for a shared allowance, two tokens of one account for its
/// own. D1 runs the batches one after the other: the first lands
/// exactly on the limit, the second writes nothing, and its re-run
/// preflight is what answers. No other limit is near enough to answer
/// first.
#[test]
fn racing_uploads_cannot_overshoot_a_configured_allowance() {
    let quotas = quota::quotas_for_class("default");
    assert_eq!(quotas.shared_upload_bytes_per_day, 5_368_709_120);
    let limits = Limits::of("default");
    let seeded_with = |usage: &str| {
        let conn = seeded();
        conn.execute_batch(usage).expect("seed the usage");
        conn
    };
    let race = |conn: &Connection, winner: &Op, loser: &Op, kind: Option<quota::LimitKind>| {
        assert_eq!(preflight(conn, winner), Ok(()));
        assert_eq!(preflight(conn, loser), Ok(()));
        assert_admitted(Gate::New, conn, winner, &limits, winner.revision);
        assert_refused(Gate::New, conn, loser, &limits, loser.revision);
        match (preflight(conn, loser), kind) {
            (Err(quota::UploadDenial::Daily { kind: denied, .. }), Some(kind)) => {
                assert_eq!(denied, kind);
            }
            (Err(quota::UploadDenial::Pending { .. }), None) => {}
            (answer, _) => panic!("{}: {answer:?}", loser.revision),
        }
    };

    // The shared bytes: one byte short of 5 GiB, then exactly 5 GiB,
    // then one byte over.
    let conn = seeded_with(&format!(
        "UPDATE shared_daily_upload_usage SET bytes = {} - 10",
        quotas.shared_upload_bytes_per_day
    ));
    let short = upload(3.0, "default", "short", 9.0, NOW);
    assert_admitted(Gate::New, &conn, &short, &limits, "one byte short");
    race(
        &conn,
        &upload(3.0, "default", "exact", 1.0, NOW),
        &upload(4.0, "default", "over", 1.0, NOW),
        Some(quota::LimitKind::SharedBytes),
    );
    // The shared count.
    let conn = seeded_with(&format!(
        "UPDATE shared_daily_upload_usage SET uploads = {} - 1",
        quotas.shared_uploads_per_day
    ));
    race(
        &conn,
        &upload(3.0, "default", "last", 1.0, NOW),
        &upload(4.0, "default", "next", 1.0, NOW),
        Some(quota::LimitKind::SharedCount),
    );
    // One account's own bytes and count, through two of its tokens.
    let conn = seeded_with(&format!(
        "INSERT INTO user_daily_upload_usage VALUES ('{DAY}', 3, {} - 7, 1)",
        quotas.max_upload_bytes_per_day
    ));
    race(
        &conn,
        &upload(3.0, "default", "fits", 7.0, NOW),
        &upload(3.0, "default", "spills", 1.0, NOW),
        Some(quota::LimitKind::UserBytes),
    );
    let conn = seeded_with(&format!(
        "INSERT INTO user_daily_upload_usage VALUES ('{DAY}', 3, 0, {} - 1)",
        quotas.max_uploads_per_day
    ));
    race(
        &conn,
        &upload(3.0, "default", "last", 1.0, NOW),
        &upload(3.0, "default", "next", 1.0, NOW),
        Some(quota::LimitKind::UserCount),
    );
    // The pending slots: user 2 holds one; reservations take the rest.
    let conn = seeded();
    assert_eq!(quotas.max_pending_uploads, 3);
    assert_admitted(
        Gate::New,
        &conn,
        &upload(2.0, "default", "second", 1.0, NOW),
        &limits,
        "second",
    );
    race(
        &conn,
        &upload(2.0, "default", "third", 1.0, NOW),
        &upload(2.0, "default", "fourth", 1.0, NOW),
        None,
    );
}

/// The operator's uploads are charged to the operator's own shared
/// allowance and measured against its own limits: a spent default
/// allowance does not refuse them, and they spend none of it.
#[test]
fn the_operator_class_shares_nothing_with_the_default_class() {
    let conn = seeded();
    // Past both classes' limits, so the row would refuse either class
    // that read it.
    let (ordinary, operator) = (
        quota::quotas_for_class("default"),
        quota::quotas_for_class("operator"),
    );
    conn.execute_batch(&format!(
        "UPDATE shared_daily_upload_usage SET bytes = {}, uploads = {}",
        ordinary
            .shared_upload_bytes_per_day
            .max(operator.shared_upload_bytes_per_day),
        ordinary
            .shared_uploads_per_day
            .max(operator.shared_uploads_per_day)
    ))
    .expect("spend the default allowance");
    let default_row = rows(&conn, "SELECT * FROM shared_daily_upload_usage");

    let ordinary = upload(3.0, "default", "ordinary", 1.0, NOW);
    assert_refused(
        Gate::New,
        &conn,
        &ordinary,
        &Limits::of("default"),
        "default",
    );
    let port = upload(1.0, "operator", "port", 100.0, NOW);
    assert_eq!(preflight(&conn, &port), Ok(()));
    assert_admitted(Gate::New, &conn, &port, &Limits::of("operator"), "operator");
    assert_eq!(
        rows(
            &conn,
            "SELECT * FROM shared_daily_upload_usage ORDER BY quota_class"
        ),
        [
            default_row[0].clone(),
            format!("Text(\"{DAY}\") | Text(\"operator\") | Integer(100) | Integer(1)")
        ]
    );
    // A revival is charged to the class of whoever revives, whatever
    // class the rejected operation was charged to.
    let revival = Op {
        user: 1.0,
        class: "operator",
        ..Gate::Revival.admissible()
    };
    assert_admitted(
        Gate::Revival,
        &conn,
        &revival,
        &Limits::of("operator"),
        "operator",
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT * FROM shared_daily_upload_usage ORDER BY quota_class"
        ),
        [
            default_row[0].clone(),
            format!("Text(\"{DAY}\") | Text(\"operator\") | Integer(110) | Integer(2)")
        ]
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT revision, charged_class FROM revisions \
             WHERE revision IN ('port', 'rj') ORDER BY revision"
        ),
        [
            "Text(\"port\") | Text(\"operator\")",
            "Text(\"rj\") | Text(\"operator\")"
        ]
    );
}

/// A revival and a resumed operation are measured against their own
/// row, their own account, and the version's live siblings: the row's
/// recorded bytes and state, the publisher's allowance and pending
/// uploads and nobody else's, the package's revisions of the sample's
/// UTC day. A rejected sibling is not live; a pending one is, stored or
/// not.
#[test]
#[allow(clippy::too_many_lines)] // one seeded scenario, one case per measured row
fn a_returning_operation_is_measured_against_its_own_rows() {
    // On top of `seeded`: version 3.0.0 of `lib` holds only rejected
    // revisions - `ra`, stored, `rb`, a released reservation, and `tm`,
    // stamped tomorrow by a faster clock. Version 4.0.0 holds a
    // released reservation `rc` beside a live one, `lv`. User 3 has a
    // pending upload `q1` of its own.
    let scenario = || {
        let conn = seeded();
        conn.execute_batch(&format!(
            r"
            INSERT INTO versions (scope, name, version)
              VALUES ('alpha', 'lib', '3.0.0'), ('alpha', 'lib', '4.0.0'),
                     ('alpha', 'foo-bar', '1.0.0');
            INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                   published_at, archive_size, published_by, verification,
                                   verification_reason, rejected_at, upload_id, charged_at,
                                   charged_class, stored_at)
              VALUES
                ('alpha', 'lib', '3.0.0', 'ra', 'ra', '{{}}', '{EARLIER}', 10, 2, 'rejected',
                 'bad', '{EARLIER}', 'u-ra', '{EARLIER}', 'default', '{EARLIER}'),
                ('alpha', 'lib', '3.0.0', 'rb', 'rb', '{{}}', '{EARLIER}', 10, 2, 'rejected',
                 'upload_not_stored', '{EARLIER}', 'u-rb', '{EARLIER}', 'default', NULL),
                ('alpha', 'lib', '3.0.0', 'tm', 'tm', '{{}}', '2026-10-27T00:00:00.000Z', 10, 2,
                 'rejected', 'bad', '2026-10-27T00:00:00.000Z', 'u-tm',
                 '2026-10-27T00:00:00.000Z', 'default', '2026-10-27T00:00:00.000Z'),
                ('alpha', 'lib', '4.0.0', 'rc', 'rc', '{{}}', '{EARLIER}', 10, 2, 'rejected',
                 'upload_not_stored', '{EARLIER}', 'u-rc', '{EARLIER}', 'default', NULL),
                ('alpha', 'lib', '4.0.0', 'lv', 'lv', '{{}}', '{DAY}T07:30:00.000Z', 10, 2,
                 'pending', NULL, NULL, 'u-lv', '{DAY}T07:30:00.000Z', 'default', NULL),
                ('alpha', 'foo-bar', '1.0.0', 'q1', 'q1', '{{}}', '{EARLIER}', 10, 3, 'pending',
                 NULL, NULL, 'u-q1', '{EARLIER}', 'default', '{EARLIER}');
            UPDATE users SET stored_bytes = 30 WHERE id = 2;
            UPDATE users SET stored_bytes = 10 WHERE id = 3;
            "
        ))
        .expect("seed the returning operations");
        conn
    };
    let limits = Limits::of("default");
    let revival = |revision: &'static str| Op {
        version: "3.0.0",
        revision,
        opt_in: false,
        ..Gate::Revival.admissible()
    };
    let resumed = |version: &'static str, revision: &'static str, upload_id: &'static str| Op {
        version,
        revision,
        upload_id,
        opt_in: false,
        ..Gate::Resumed.admissible()
    };

    // Beside rejected siblings only, neither needs the opt-in. User 2
    // has two pending uploads and user 3 one: the limit of three counts
    // user 2's own.
    assert_admitted(
        Gate::Revival,
        &scenario(),
        &revival("ra"),
        &limits,
        "rejected siblings",
    );
    assert_admitted(
        Gate::Resumed,
        &scenario(),
        &resumed("3.0.0", "rb", "u-rb"),
        &limits,
        "rejected siblings",
    );
    // A reservation with other bytes is live: resuming beside it needs
    // the opt-in.
    let beside_live = resumed("4.0.0", "rc", "u-rc");
    assert_refused(
        Gate::Resumed,
        &scenario(),
        &beside_live,
        &limits,
        "a live reservation",
    );
    let opted_in = Op {
        opt_in: true,
        ..beside_live
    };
    assert_admitted(
        Gate::Resumed,
        &scenario(),
        &opted_in,
        &limits,
        "a live reservation",
    );
    // A reservation that was never released has nothing to resume.
    let never_released = Op {
        opt_in: true,
        ..resumed("4.0.0", "lv", "u-lv")
    };
    assert_refused(
        Gate::Resumed,
        &scenario(),
        &never_released,
        &limits,
        "still reserved",
    );

    // The bytes are the row's: a row recorded with another checksum is
    // not this operation's to take.
    let conn = scenario();
    conn.execute_batch("UPDATE revisions SET checksum = 'other' WHERE revision IN ('ra', 'rb')")
        .expect("record other bytes");
    assert_refused(Gate::Revival, &conn, &revival("ra"), &limits, "other bytes");
    assert_refused(
        Gate::Resumed,
        &conn,
        &resumed("3.0.0", "rb", "u-rb"),
        &limits,
        "other bytes",
    );

    // User 3 revives on an allowance of exactly one upload of this
    // size, whatever user 2 spent today.
    let own_allowance = Limits {
        upload_bytes_daily: 10.0,
        uploads_daily: 1.0,
        ..limits
    };
    let by_user_3 = Op {
        user: 3.0,
        ..revival("ra")
    };
    assert_admitted(
        Gate::Revival,
        &scenario(),
        &by_user_3,
        &own_allowance,
        "own allowance",
    );

    // `lib` has four revisions stamped today; yesterday's and
    // tomorrow's are other days'.
    for (versions_daily, admitted) in [(4.0, false), (5.0, true)] {
        let conn = scenario();
        let op = resumed("3.0.0", "rb", "u-rb");
        let limits = Limits {
            versions_daily,
            ..limits
        };
        if admitted {
            assert_admitted(Gate::Resumed, &conn, &op, &limits, "the package's day");
        } else {
            assert_refused(Gate::Resumed, &conn, &op, &limits, "the package's day");
        }
    }
}

/// A day's allowances are that day's rows: an account refused at its
/// limit in the last millisecond of a day is admitted in the first of
/// the next with no sweep in between, and the sweep keeps today's and
/// yesterday's rows, whatever it deletes.
#[test]
fn allowances_reopen_at_utc_midnight_without_a_sweep() {
    let conn = seeded();
    let limits = Limits {
        uploads_daily: 3.0,
        ..Limits::of("default")
    };
    let late = upload(2.0, "default", "late", 1.0, "2026-10-26T23:59:59.999Z");
    assert_refused(Gate::New, &conn, &late, &limits, "the day is spent");
    let next = upload(2.0, "default", "next", 1.0, "2026-10-27T00:00:00.000Z");
    assert_admitted(Gate::New, &conn, &next, &limits, "the next day");
    let user_rows = "SELECT usage_date, bytes, uploads FROM user_daily_upload_usage ORDER BY 1";
    let both_days = [
        format!("Text(\"{DAY}\") | Integer(30) | Integer(3)"),
        "Text(\"2026-10-27\") | Integer(1) | Integer(1)".to_owned(),
    ];
    assert_eq!(rows(&conn, user_rows), both_days);

    sweep(&conn, "2026-10-27T23:59:59.999Z");
    assert_eq!(rows(&conn, user_rows), both_days);
    assert_eq!(crate::common::count(&conn, "shared_daily_upload_usage"), 2);
    sweep(&conn, "2026-10-28T00:00:00.000Z");
    assert_eq!(rows(&conn, user_rows), both_days[1..]);
    assert_eq!(crate::common::count(&conn, "shared_daily_upload_usage"), 1);
}

fn expired_reservations(conn: &Connection, now: &str) -> Vec<String> {
    let mut statement = conn.prepare(sql::EXPIRED_RESERVATIONS).expect("prepare");
    statement
        .query_map(params![now, upload::RESERVATION_TTL], |row| {
            row.get("revision")
        })
        .expect("EXPIRED_RESERVATIONS")
        .collect::<Result<_, _>>()
        .expect("rows")
}

/// A reservation is invisible to the verifier and unreadable until its
/// store is confirmed, and each generation settles once: a confirmation
/// or a release naming a generation that is gone changes nothing, a
/// release refunds the storage and frees the pending slot but never the
/// daily allowances, and a resumed operation is a new generation that
/// the old one's late confirmation cannot touch.
#[test]
#[allow(clippy::too_many_lines)] // one reservation walked through its lifecycle
fn a_reservation_settles_once_per_generation() {
    let conn = seeded();
    let limits = Limits::of("default");
    let op = Gate::New.admissible();
    assert_admitted(Gate::New, &conn, &op, &limits, "fresh");
    let pending_work = |conn: &Connection| -> Vec<String> {
        let mut statement = conn
            .prepare(sql::REVISIONS_BY_VERIFICATION_STATUS)
            .expect("prepare");
        statement
            .query_map(["pending"], |row| row.get("revision"))
            .expect("pending work")
            .collect::<Result<_, _>>()
            .expect("rows")
    };
    let readable = |conn: &Connection, statement: &str| -> bool {
        conn.prepare(statement)
            .expect("prepare")
            .exists(params!["alpha", "fresh", "1.0.0", "f1"])
            .expect("lookup")
    };
    assert_eq!(pending_work(&conn), ["p1"]);
    assert!(!readable(&conn, sql::VERDICT_TARGET));
    assert!(!readable(&conn, sql::ARTIFACT_BY_REVISION));
    assert_eq!(
        rows(&conn, "SELECT * FROM revisions WHERE upload_id = 'u-new'").len(),
        1
    );
    let bound: (String, String, String, String) = conn
        .query_row(sql::REVISION_BY_UPLOAD_ID, ["u-new"], |row| {
            Ok((
                row.get("scope")?,
                row.get("name")?,
                row.get("version")?,
                row.get("revision")?,
            ))
        })
        .expect("REVISION_BY_UPLOAD_ID");
    assert_eq!(
        bound,
        ("alpha".into(), "fresh".into(), "1.0.0".into(), "f1".into())
    );

    // Fifteen minutes is not yet expired; a millisecond more is. A
    // stored upload is never a reservation, however old.
    assert!(expired_reservations(&conn, "2026-10-26T08:15:00.000Z").is_empty());
    let expired = "2026-10-26T08:15:00.001Z";
    assert_eq!(expired_reservations(&conn, expired), ["f1"]);
    assert_eq!(
        expired_reservations(&conn, "2026-12-01T00:00:00.000Z"),
        ["f1"]
    );

    // The release names the listed generation.
    let charged = charges(&conn, 2, DAY);
    assert_eq!(charged[0], "Integer(2) | Integer(25)");
    let before = footprint(&conn);
    assert_eq!(release(&conn, &op, "2026-10-26T07:59:59.999Z", expired), 0);
    assert_eq!(footprint(&conn), before, "a stale release refunds nothing");
    assert_eq!(release(&conn, &op, NOW, expired), 1);
    assert_eq!(
        revision(&conn, "f1"),
        format!(
            "Text(\"rejected\") | Text(\"upload_not_stored\") | Text(\"{NOW}\") | Integer(2) | \
             Text(\"u-new\") | Text(\"{NOW}\") | Text(\"default\") | Null | Text(\"{expired}\")"
        )
    );
    assert_eq!(
        charges(&conn, 2, DAY),
        ["Integer(2) | Integer(20)", &charged[1], &charged[2]],
        "the storage is refunded, the daily allowances are not"
    );
    assert_eq!(usage(&conn, &op).0.pending, 1, "the pending slot is free");
    let before = footprint(&conn);
    assert_eq!(release(&conn, &op, NOW, expired), 0);
    assert_eq!(confirm(&conn, &op, NOW, expired), 0, "a late store");
    assert_eq!(footprint(&conn), before);

    // The same operation returns as a new generation.
    let resumed = Op {
        stamp: "2026-10-26T09:00:00.000Z",
        ..op
    };
    assert_admitted(Gate::Resumed, &conn, &resumed, &limits, "resumed");
    assert_eq!(confirm(&conn, &resumed, NOW, "2026-10-26T09:00:01.000Z"), 0);
    assert!(!readable(&conn, sql::ARTIFACT_BY_REVISION));
    assert_eq!(
        confirm(&conn, &resumed, resumed.stamp, "2026-10-26T09:00:01.000Z"),
        1
    );
    // A racing confirmation of the same generation still finds its
    // row, and the first one's time stands.
    assert_eq!(
        confirm(&conn, &resumed, resumed.stamp, "2026-10-26T09:00:02.000Z"),
        1
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT stored_at FROM revisions WHERE revision = 'f1'"
        ),
        ["Text(\"2026-10-26T09:00:01.000Z\")"]
    );
    assert_eq!(pending_work(&conn), ["f1", "p1"]);
    assert!(readable(&conn, sql::VERDICT_TARGET));
    assert!(readable(&conn, sql::ARTIFACT_BY_REVISION));
    assert!(expired_reservations(&conn, "2026-12-01T00:00:00.000Z").is_empty());
    assert_eq!(
        release(&conn, &resumed, resumed.stamp, "2026-12-01T00:00:00.000Z"),
        0
    );

    // A verdict's rejection refunds the storage once and keeps the
    // object's upload id spent: the same id cannot return.
    assert_eq!(
        reject(&conn, &resumed, resumed.stamp, "2026-10-26T10:00:00.000Z"),
        1
    );
    assert_eq!(
        reject(&conn, &resumed, resumed.stamp, "2026-10-26T10:00:00.000Z"),
        0
    );
    assert_eq!(charges(&conn, 2, DAY)[0], "Integer(2) | Integer(20)");
    let again = Op {
        stamp: "2026-10-26T11:00:00.000Z",
        ..op
    };
    assert_refused(Gate::Resumed, &conn, &again, &limits, "a stored rejection");
    assert_refused(Gate::Revival, &conn, &again, &limits, "a spent upload id");
}

/// The existing-revisions read is the disposition's input: its columns
/// are the glue's deserialization contract, `stored` tells a
/// reservation from an upload, and `resumable` is the retry window
/// measured from the operation's first charge.
#[test]
fn the_existing_revisions_read_reports_stored_and_resumable() {
    let conn = seeded();
    let read = |now: &str| -> Vec<(String, Option<String>, bool, bool)> {
        let mut statement = conn.prepare(sql::EXISTING_REVISIONS).expect("prepare");
        assert_eq!(
            statement.column_names().collect::<Vec<_>>(),
            [
                "revision",
                "checksum",
                "verification",
                "upload_id",
                "published_at",
                "published_by",
                "stored",
                "resumable"
            ]
        );
        statement
            .query_map(
                params!["alpha", "lib", "1.0.0", now, upload::RETRY_WINDOW],
                |row| {
                    Ok((
                        row.get("revision")?,
                        row.get("upload_id")?,
                        row.get("stored")?,
                        row.get("resumable")?,
                    ))
                },
            )
            .expect("EXISTING_REVISIONS")
            .collect::<Result<_, _>>()
            .expect("rows")
    };
    let at = |now: &str, resumable: bool| {
        assert_eq!(
            read(now),
            [
                ("r1".to_owned(), Some("u-r1".to_owned()), true, false),
                ("rl".to_owned(), Some("u-rl".to_owned()), false, resumable),
            ],
            "{now}"
        );
    };
    // `r1` was charged the day before `rl`: fourteen days after `rl`'s
    // charge to the millisecond, only `rl` is still inside the window.
    at("2026-11-09T06:30:00.000Z", true);
    at("2026-11-09T06:30:00.001Z", false);
    // A row whose operation was forgotten is not resumable.
    conn.execute(
        "UPDATE revisions SET upload_id = NULL, charged_at = NULL",
        [],
    )
    .expect("forget the operations");
    assert!(read(NOW).iter().all(|row| row.1.is_none() && !row.3));
}

/// The sweep times out exactly the uploads stored longer ago than the
/// verification deadline and refunds each publisher the sum of theirs;
/// a reservation is not its business. It forgets the upload operation
/// of rows terminal for longer than the retention, and never a pending
/// row's.
#[test]
fn the_sweep_times_out_stored_uploads_and_forgets_settled_operations() {
    let conn = seeded();
    conn.execute_batch(
        r"
        INSERT INTO versions (scope, name, version) VALUES ('alpha', 'lib', '3.0.0');
        INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                               published_at, archive_size, published_by, verification,
                               upload_id, charged_at, charged_class, stored_at)
          VALUES
            ('alpha', 'lib', '3.0.0', 't1', 't1', '{}', '2026-10-01T00:00:00.000Z', 7, 2,
             'pending', 'u-t1', '2026-10-01T00:00:00.000Z', 'default', '2026-10-12T07:59:59.999Z'),
            ('alpha', 'lib', '3.0.0', 't2', 't2', '{}', '2026-10-01T00:00:00.000Z', 6, 3,
             'pending', 'u-t2', '2026-10-01T00:00:00.000Z', 'default', '2026-10-01T00:00:01.000Z'),
            ('alpha', 'lib', '3.0.0', 't3', 't3', '{}', '2026-10-01T00:00:00.000Z', 4, 3,
             'pending', 'u-t3', '2026-10-01T00:00:00.000Z', 'default', '2026-10-12T08:00:00.000Z'),
            ('alpha', 'lib', '3.0.0', 't4', 't4', '{}', '2026-10-01T00:00:00.000Z', 3, 3,
             'pending', 'u-t4', '2026-10-01T00:00:00.000Z', 'default', NULL);
        UPDATE users SET stored_bytes = 27 WHERE id = 2;
        UPDATE users SET stored_bytes = 13 WHERE id = 3;
        ",
    )
    .expect("seed uploads around the deadline");
    let states = |conn: &Connection| {
        rows(
            conn,
            "SELECT revision, verification, verification_reason, rejected_at, upload_id \
             FROM revisions WHERE version = '3.0.0' OR revision IN ('r1', 'rj') ORDER BY revision",
        )
    };
    let stored_bytes =
        |conn: &Connection| rows(conn, "SELECT stored_bytes FROM users WHERE id IN (2, 3)");

    // Fourteen days after `t3` was stored, to the millisecond: `t1` and
    // `t2` are past the deadline, `t3` is on it. Users 2 and 3 are each
    // refunded their own.
    sweep(&conn, "2026-10-26T08:00:00.000Z");
    let timed_out = |revision: &str| {
        format!(
            "Text(\"{revision}\") | Text(\"rejected\") | Text(\"verification_timeout\") | \
             Text(\"2026-10-26T08:00:00.000Z\") | Text(\"u-{revision}\")"
        )
    };
    let pending = |revision: &str| {
        format!("Text(\"{revision}\") | Text(\"pending\") | Null | Null | Text(\"u-{revision}\")")
    };
    assert_eq!(
        states(&conn),
        [
            "Text(\"r1\") | Text(\"verified\") | Null | Null | Text(\"u-r1\")".to_owned(),
            "Text(\"rj\") | Text(\"rejected\") | Text(\"bad\") | \
             Text(\"2026-10-26T06:10:00.000Z\") | Text(\"u-rj\")"
                .to_owned(),
            timed_out("t1"),
            timed_out("t2"),
            pending("t3"),
            pending("t4"),
        ]
    );
    assert_eq!(stored_bytes(&conn), ["Integer(20)", "Integer(7)"]);
    // A second pass finds nothing left to refund.
    sweep(&conn, "2026-10-26T08:00:00.000Z");
    assert_eq!(stored_bytes(&conn), ["Integer(20)", "Integer(7)"]);

    // Thirty days after `rj` was rejected it still has its upload id; a
    // millisecond later it does not. `r1`, verified a day earlier, lost
    // its a day earlier. The uploads these passes time out keep theirs
    // for thirty days more.
    let forgotten = |conn: &Connection| {
        rows(
            conn,
            "SELECT revision FROM revisions WHERE upload_id IS NULL \
             AND charged_at IS NULL AND charged_class IS NULL ORDER BY revision",
        )
    };
    sweep(&conn, "2026-11-25T06:10:00.000Z");
    assert_eq!(forgotten(&conn), ["Text(\"r1\")"]);
    sweep(&conn, "2026-11-25T06:10:00.001Z");
    assert_eq!(forgotten(&conn), ["Text(\"r1\")", "Text(\"rj\")"]);
    // Long after, every operation is forgotten but the reservation's,
    // which no sweep pass settles.
    sweep(&conn, "2027-01-01T00:00:00.000Z");
    assert_eq!(
        rows(
            &conn,
            "SELECT revision FROM revisions WHERE upload_id IS NOT NULL"
        ),
        ["Text(\"t4\")"]
    );
}
