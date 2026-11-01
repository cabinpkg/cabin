//! Executed-semantics tests for `src/sql/quota.rs`: the publish token
//! bucket's compare-and-swap and the per-user quota tallies.

use cabin_registry_worker::{quota, sql};
use rusqlite::{Connection, StatementStatus, params};

use crate::common::{Limits, migrated_connection, resolve, sign_in};

/// One `glue::bearer::package::cas_bucket` write: D1 binds every JS
/// number as a REAL (the user id too) and the timestamp as the `f64`
/// `Display` text.
fn cas(
    conn: &rusqlite::Connection,
    user_id: i64,
    prev: Option<(f64, f64)>,
    next: (f64, f64),
) -> usize {
    #[allow(clippy::cast_precision_loss)]
    let user_id = user_id as f64;
    let (prev_tokens, prev_updated_at) = match prev {
        Some((tokens, updated_at)) => (Some(tokens), Some(updated_at.to_string())),
        None => (None, None),
    };
    conn.execute(
        sql::CAS_USER_BUCKET,
        rusqlite::params![
            next.0,
            next.1.to_string(),
            user_id,
            prev_tokens,
            prev_updated_at
        ],
    )
    .expect("CAS_USER_BUCKET executes")
}

/// The bucket as `read_bucket` and `glue::bucket_from_columns` read it
/// back: by column name, as D1 hands the row to serde.
fn stored(conn: &rusqlite::Connection, user_id: i64) -> Option<(f64, f64)> {
    let (tokens, updated_at): (Option<f64>, Option<String>) = conn
        .query_row(sql::USER_BUCKET, [user_id], |row| {
            Ok((row.get("rl_tokens")?, row.get("rl_updated_at")?))
        })
        .expect("USER_BUCKET reads the user row");
    Some((tokens?, updated_at?.parse().ok()?))
}

/// The Kani bucket harness (`quota::verification`) takes a stored
/// bucket to be exactly what an allowed take wrote. That holds only if
/// the CAS persists the take bit for bit, a take computed from the read
/// back state matches the row again, and a take computed from a state
/// the row no longer holds changes nothing.
#[test]
fn the_bucket_cas_round_trips_a_take_and_refuses_a_stale_one() {
    let conn = migrated_connection();
    sign_in(
        &conn,
        "github",
        "424242",
        "mona",
        "2026-07-15T00:00:00.000Z",
    );
    let (user_id, _, _) = resolve(&conn, "github", "424242").expect("identity resolves");

    // A user who has never published holds NULL columns, which the
    // first take's NULL `prev` must match.
    let first = (4.0, 1_784_073_600_123.0);
    assert_eq!(cas(&conn, user_id, None, first), 1);
    let read = stored(&conn, user_id).expect("the first take is stored");
    assert_eq!(
        (read.0.to_bits(), read.1.to_bits()),
        (first.0.to_bits(), first.1.to_bits())
    );

    // A fractional balance, written from the read-back state (the
    // balances are synthetic; the statement only compares them).
    let second = (4.0 / 3.0, 1_784_073_660_124.0);
    assert_eq!(cas(&conn, user_id, Some(read), second), 1);
    let read = stored(&conn, user_id).expect("the second take is stored");
    assert_eq!(
        (read.0.to_bits(), read.1.to_bits()),
        (second.0.to_bits(), second.1.to_bits())
    );

    // A second take in the same millisecond spends one token from the
    // fractional read-back: the comparison must be exact, not just the
    // storage.
    let third = (second.0 - 1.0, second.1);
    assert_eq!(cas(&conn, user_id, Some(read), third), 1);

    // Requests that read an earlier state lost the race: nothing moves,
    // whether the stale state differs only in its balance (an earlier
    // take in the same millisecond), only in its timestamp, or is the
    // never-published NULL pair.
    for stale in [Some(second), Some((third.0, first.1)), None] {
        assert_eq!(cas(&conn, user_id, stale, (4.0, 1_784_073_660_300.0)), 0);
    }
    let read = stored(&conn, user_id).expect("the winner's take stands");
    assert_eq!(
        (read.0.to_bits(), read.1.to_bits()),
        (third.0.to_bits(), third.1.to_bits())
    );
}

/// Users whose tallies disagree with the rows they count.
fn tally_drift(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM users u WHERE package_count != \
           (SELECT COUNT(*) FROM packages WHERE created_by = u.id) \
         OR stored_bytes != (SELECT COALESCE(SUM(archive_size), 0) FROM revisions \
           WHERE published_by = u.id AND verification != 'rejected')",
        [],
        |row| row.get(0),
    )
    .expect("tally drift")
}

/// `(package_count, new_today, stored_bytes)` as the publish preflight
/// reads them for a prospective new package.
fn tallies(conn: &Connection, user: f64, day: &str) -> (i64, i64, i64) {
    let (packages, today) = conn
        .query_row(
            sql::USER_PACKAGE_COUNTS,
            params![user, day, "alpha", "unpublished"],
            |row| Ok((row.get("package_count")?, row.get("new_today")?)),
        )
        .expect("USER_PACKAGE_COUNTS");
    let bytes = conn
        .query_row(sql::USER_STORED_BYTES, [user], |row| {
            row.get("stored_bytes")
        })
        .expect("USER_STORED_BYTES");
    (packages, today, bytes)
}

/// Runs one batch's statements, in order, as one transaction; each
/// statement's changed rows and VM steps.
fn batch(conn: &Connection, statements: &[(&str, &[&dyn rusqlite::ToSql])]) -> Vec<(usize, i32)> {
    let tx = conn.unchecked_transaction().expect("begin");
    let results = statements
        .iter()
        .map(|&(statement, params)| {
            let mut prepared = tx.prepare(statement).expect("prepare");
            let changed = prepared.execute(params).expect(statement);
            (changed, prepared.get_status(StatementStatus::VmStep))
        })
        .collect();
    tx.commit().expect("commit");
    results
}

/// `persist_new_revision`'s batch in the glue's statement order, with
/// D1's REAL binds and the default class's limits over the stamp's UTC
/// day.
#[allow(clippy::too_many_arguments)]
fn publish_batch(
    conn: &Connection,
    user: f64,
    name: &str,
    version: &str,
    revision: &str,
    size: f64,
    opt_in: i64,
    stamp: &str,
) -> Vec<(usize, i32)> {
    let limits = Limits::of("default");
    let day = quota::utc_day_prefix(stamp).expect("ISO stamp");
    batch(
        conn,
        &[
            (
                sql::INSERT_PACKAGE,
                params![
                    "alpha",
                    name,
                    stamp,
                    user,
                    size,
                    limits.bytes,
                    day,
                    limits.packages_daily,
                    limits.packages_total
                ],
            ),
            (sql::COUNT_USER_PACKAGE, params![user]),
            (
                sql::INSERT_VERSION_ROW,
                params![
                    "alpha",
                    name,
                    version,
                    user,
                    size,
                    limits.bytes,
                    day,
                    limits.versions_daily
                ],
            ),
            (
                sql::COUNT_STORED_BYTES_ON_PUBLISH,
                params![
                    revision,
                    size,
                    "alpha",
                    name,
                    version,
                    revision,
                    opt_in,
                    user,
                    limits.bytes,
                    day,
                    limits.versions_daily
                ],
            ),
            (
                sql::INSERT_REVISION,
                params![
                    "alpha",
                    name,
                    version,
                    revision,
                    revision,
                    "{}",
                    stamp,
                    size,
                    user,
                    opt_in,
                    limits.bytes,
                    day,
                    limits.versions_daily
                ],
            ),
            (sql::COUNT_USER_STORED_BYTES, params![user, size]),
        ],
    )
}

/// [`publish_batch`]'s revision insert's changed rows.
#[allow(clippy::too_many_arguments)]
fn publish(
    conn: &Connection,
    user: f64,
    name: &str,
    version: &str,
    revision: &str,
    size: f64,
    opt_in: i64,
    stamp: &str,
) -> usize {
    publish_batch(conn, user, name, version, revision, size, opt_in, stamp)[4].0
}

/// `revive_rejected_revision`'s batch, with the binds of
/// [`publish_batch`], reviving `revision` of version `1.0.0`.
fn revive_batch(
    conn: &Connection,
    user: f64,
    name: &str,
    revision: &str,
    size: f64,
    stamp: &str,
) -> Vec<(usize, i32)> {
    let limits = Limits::of("default");
    let day = quota::utc_day_prefix(stamp).expect("ISO stamp");
    batch(
        conn,
        &[
            (
                sql::COUNT_STORED_BYTES_ON_REVIVAL,
                params![
                    "alpha",
                    name,
                    "1.0.0",
                    revision,
                    revision,
                    size,
                    revision,
                    1,
                    stamp,
                    user,
                    limits.bytes,
                    day,
                    limits.versions_daily
                ],
            ),
            (
                sql::REVIVE_REJECTED_REVISION,
                params![
                    "{}",
                    stamp,
                    user,
                    "alpha",
                    name,
                    1,
                    "1.0.0",
                    revision,
                    revision,
                    size,
                    limits.bytes,
                    day,
                    limits.versions_daily
                ],
            ),
            (sql::COUNT_USER_STORED_BYTES, params![user, size]),
        ],
    )
}

/// [`revive_batch`]'s flip's changed rows.
fn revive(
    conn: &Connection,
    user: f64,
    name: &str,
    revision: &str,
    size: f64,
    stamp: &str,
) -> usize {
    revive_batch(conn, user, name, revision, size, stamp)[1].0
}

/// `apply_rejection`'s batch against the generation stamped `stamp`;
/// the flip's changed rows.
fn reject(conn: &Connection, name: &str, revision: &str, size: f64, stamp: &str) -> usize {
    let tx = conn.unchecked_transaction().expect("begin");
    tx.execute(
        sql::REFUND_STORED_BYTES_ON_REJECTION,
        params![revision, "alpha", name, "1.0.0", size, stamp, revision],
    )
    .expect("REFUND_STORED_BYTES_ON_REJECTION");
    let flipped = tx
        .execute(
            sql::MARK_REVISION_REJECTED,
            params!["bad", "alpha", name, "1.0.0", revision, stamp, revision],
        )
        .expect("MARK_REVISION_REJECTED");
    tx.execute(
        sql::REFUND_USER_STORED_BYTES,
        params!["alpha", name, "1.0.0", revision],
    )
    .expect("REFUND_USER_STORED_BYTES");
    tx.commit().expect("commit");
    flipped
}

/// The tallies the preflight reads must equal the rows they count after
/// every write the lifecycle makes - the batches' `changes()` coupling
/// is what keeps a suppressed write from counting, and nothing else
/// can catch a miscounted path before a quota misfires in production.
#[test]
fn quota_tallies_track_every_publish_lifecycle_write() {
    let conn = migrated_connection();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (2, '2026-10-01T00:00:00.000Z'),
                                                   (3, '2026-10-01T00:00:00.000Z');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '2', '2026-10-01T00:00:00.000Z');",
    )
    .expect("seed users and scope");
    let day = "2026-10-26T";
    let check = |expected_2: (i64, i64, i64), expected_3: (i64, i64, i64)| {
        assert_eq!(tally_drift(&conn), 0);
        assert_eq!(tallies(&conn, 2.0, day), expected_2, "user 2");
        assert_eq!(tallies(&conn, 3.0, day), expected_3, "user 3");
    };
    let late = "2026-10-25T23:59:59.999Z";
    let midnight = "2026-10-26T00:00:00.000Z";

    // A first publish the UTC day before, its byte-identical retry, and
    // a second new package at midnight - the only one created today.
    assert_eq!(
        publish(&conn, 2.0, "foo-bar", "1.0.0", "a1", 40.0, 0, late),
        1
    );
    assert_eq!(
        publish(&conn, 2.0, "foo-bar", "1.0.0", "a1", 40.0, 0, midnight),
        0
    );
    check((1, 0, 40), (0, 0, 0));
    assert_eq!(
        publish(&conn, 2.0, "baz", "1.0.0", "b1", 7.0, 0, midnight),
        1
    );
    check((2, 1, 47), (0, 0, 0));

    // Another account's version of user 2's package charges its own
    // publisher, never the creator's package count; its racing
    // byte-identical retry of user 2's new package counts nothing.
    assert_eq!(
        publish(&conn, 3.0, "foo-bar", "2.0.0", "c1", 50.0, 0, midnight),
        1
    );
    assert_eq!(
        publish(&conn, 3.0, "baz", "1.0.0", "b1", 7.0, 0, midnight),
        0
    );
    check((2, 1, 47), (0, 0, 50));

    // A `-`/`_` twin and an unflagged respin are suppressed in-batch;
    // the flagged respin charges, and the superseded revision keeps
    // charging (it stays fetchable). Verifying moves nothing.
    assert_eq!(
        publish(&conn, 2.0, "foo_bar", "1.0.0", "d1", 9.0, 0, midnight),
        0
    );
    assert_eq!(
        publish(&conn, 2.0, "foo-bar", "1.0.0", "a2", 60.0, 0, midnight),
        0
    );
    check((2, 1, 47), (0, 0, 50));
    assert_eq!(
        publish(&conn, 2.0, "foo-bar", "1.0.0", "a2", 60.0, 1, midnight),
        1
    );
    let verified = conn
        .execute(
            sql::MARK_REVISION_VERIFIED,
            params![midnight, "alpha", "foo-bar", "1.0.0", "a1", late, "a1"],
        )
        .expect("MARK_REVISION_VERIFIED");
    assert_eq!(verified, 1);
    check((2, 1, 107), (0, 0, 50));

    // A rejection refunds the row's publisher once; a repeated verdict
    // lands nowhere. A revival by another account charges the reviver.
    assert_eq!(reject(&conn, "foo-bar", "a2", 60.0, midnight), 1);
    assert_eq!(reject(&conn, "foo-bar", "a2", 60.0, midnight), 0);
    check((2, 1, 47), (0, 0, 50));
    let later = "2026-10-26T01:00:00.000Z";
    assert_eq!(revive(&conn, 3.0, "foo-bar", "a2", 60.0, later), 1);
    assert_eq!(revive(&conn, 3.0, "foo-bar", "a2", 60.0, later), 0);
    check((2, 1, 47), (0, 0, 110));
    // Rejecting the revived generation refunds its current publisher,
    // not the package's creator or the row's first publisher.
    assert_eq!(reject(&conn, "foo-bar", "a2", 60.0, later), 1);
    check((2, 1, 47), (0, 0, 50));
}

/// The quota half of the glue's `Preflight::refusal` for the default
/// class: the counts `publish_counts` reads, through
/// `quota::check_publish`.
fn preflight(
    conn: &Connection,
    user: f64,
    name: &str,
    size: u64,
    day: &str,
) -> Result<(), quota::Denial> {
    let count = |statement: &str, params: &[&dyn rusqlite::ToSql], column: &str| -> u64 {
        let n: i64 = conn
            .query_row(statement, params, |row| row.get(column))
            .expect("preflight read");
        u64::try_from(n).expect("non-negative count")
    };
    let counts = quota::PublishCounts {
        user_stored_bytes: count(sql::USER_STORED_BYTES, &[&user], "stored_bytes"),
        user_package_count: count(
            sql::USER_PACKAGE_COUNTS,
            &[&user, &day, &"alpha", &name],
            "package_count",
        ),
        user_new_packages_today: count(
            sql::USER_PACKAGE_COUNTS,
            &[&user, &day, &"alpha", &name],
            "new_today",
        ),
        package_versions_today: count(
            sql::COUNT_PACKAGE_VERSIONS_ON_DAY,
            &[&"alpha", &name, &day],
            "n",
        ),
        package_exists: count(sql::PACKAGE_EXISTS, &[&"alpha", &name], "n") > 0,
    };
    quota::check_publish(size, &counts, &quota::quotas_for_class("default"))
}

/// Everything a publish or revival batch writes.
fn footprint(conn: &Connection) -> [i64; 6] {
    conn.query_row(
        "SELECT (SELECT COUNT(*) FROM packages), (SELECT COUNT(*) FROM versions), \
         (SELECT COUNT(*) FROM revisions WHERE verification != 'rejected'), \
         (SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'total_stored_bytes'), \
         (SELECT SUM(package_count) FROM users), (SELECT SUM(stored_bytes) FROM users)",
        [],
        |row| {
            Ok([
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ])
        },
    )
    .expect("footprint")
}

/// A racing loser: the publisher, the package, and the archive size its
/// preflight checked, and its batch.
type Loser<'a> = (f64, &'a str, u64, &'a dyn Fn() -> usize);

/// One race: every loser's preflight passes, the winner's batch lands,
/// then each loser's batch must write nothing and its re-run preflight
/// must refuse with `denial`.
fn race(
    conn: &Connection,
    day: &str,
    winner: &dyn Fn() -> usize,
    losers: &[Loser],
    denial: &quota::Denial,
) {
    for &(user, name, size, _) in losers {
        assert_eq!(preflight(conn, user, name, size, day), Ok(()));
    }
    assert_eq!(winner(), 1);
    for &(user, name, size, loser) in losers {
        let before = footprint(conn);
        assert_eq!(loser(), 0, "{} must suppress {name}", denial.code);
        assert_eq!(footprint(conn), before, "{} refused {name}", denial.code);
        assert_eq!(&preflight(conn, user, name, size, day).unwrap_err(), denial);
    }
}

/// Two publishes whose preflights both pass on the same headroom race
/// to their batches - one user publishing through two tokens (every
/// token of a user resolves to its one tally row), or two publishers
/// into one package. D1 runs the batches one after the other: the first
/// lands exactly on the limit, and the second's in-batch quota guards
/// must suppress every write it would make - no package, version, or
/// revision row, no tally or stored-bytes bump - so that its re-run
/// preflight is what answers, with the quota `403`. One race per quota
/// the guards enforce, plus a revival racing a new version.
#[test]
#[allow(clippy::too_many_lines)] // one seed, one race per quota guard
fn racing_publishes_cannot_overshoot_a_quota() {
    let conn = migrated_connection();
    let quotas = quota::quotas_for_class("default");
    let day = "2026-10-26T";
    let earlier = "2026-10-25T08:00:00.000Z";
    let now = "2026-10-26T08:00:00.000Z";
    // Users 3 and 4 sit one package under the total and the daily
    // package quota, package `busy` one version under the daily version
    // quota (and holds a rejected revision to revive); the tallies are
    // recomputed from the rows (the runbook's repair statement).
    conn.execute_batch(&format!(
        "INSERT INTO users (id, created_at) VALUES (2, '{earlier}'), (3, '{earlier}'),
           (4, '{earlier}'), (5, '{earlier}'), (6, '{earlier}'), (7, '{earlier}');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '2', '{earlier}');
         WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 30)
         INSERT INTO packages (scope, name, created_at, created_by)
           SELECT 'alpha', 'total-' || i, '{earlier}', 3 FROM seq
             WHERE i < {total}
           UNION ALL SELECT 'alpha', 'daily-' || i, '{now}', 4 FROM seq WHERE i < {daily}
           UNION ALL SELECT 'alpha', 'busy', '{earlier}', 5;
         WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 100)
         INSERT INTO versions (scope, name, version)
           SELECT 'alpha', 'busy', i || '.0.0' FROM seq WHERE i < {versions};
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by)
           SELECT scope, name, version, 'v' || rowid, 'v' || rowid, '{{}}', '{now}', 1, 5
           FROM versions;
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'busy', '1.0.0', 'rj', 'rj', '{{}}', '{earlier}', 1, 6,
                   'rejected');
         UPDATE users SET
           package_count = (SELECT COUNT(*) FROM packages WHERE created_by = users.id),
           stored_bytes = (SELECT COALESCE(SUM(archive_size), 0) FROM revisions
                           WHERE published_by = users.id AND verification != 'rejected');",
        total = quotas.max_packages_total,
        daily = quotas.max_new_packages_per_day,
        versions = quotas.max_versions_per_package_per_day,
    ))
    .expect("seed the near-limit accounts");
    // Users 2 and 7 sit 10 bytes under the storage quota; user 7 also
    // holds a rejected (refunded) 10-byte revision to revive.
    #[allow(clippy::cast_precision_loss)]
    let almost_full = quotas.max_total_bytes_per_user as f64 - 10.0;
    for user in [2.0, 7.0] {
        let name = format!("base-{user}");
        assert_eq!(
            publish(&conn, user, &name, "1.0.0", &name, almost_full, 0, earlier),
            1
        );
    }
    assert_eq!(
        publish(&conn, 7.0, "rev", "1.0.0", "r1", 10.0, 0, earlier),
        1
    );
    assert_eq!(reject(&conn, "rev", "r1", 10.0, earlier), 1);
    assert_eq!(tally_drift(&conn), 0);

    // Storage, 10 bytes of headroom: a new version wins over a new
    // package, another new version, and an opt-in respin.
    race(
        &conn,
        day,
        &|| publish(&conn, 2.0, "base-2", "2.0.0", "s1", 10.0, 0, now),
        &[
            (2.0, "fresh", 10, &|| {
                publish(&conn, 2.0, "fresh", "1.0.0", "s2", 10.0, 0, now)
            }),
            (2.0, "base-2", 10, &|| {
                publish(&conn, 2.0, "base-2", "3.0.0", "s3", 10.0, 0, now)
            }),
            (2.0, "base-2", 10, &|| {
                publish(&conn, 2.0, "base-2", "1.0.0", "s4", 10.0, 1, now)
            }),
        ],
        &quota::QUOTA_STORAGE,
    );
    // ... and a new version wins over a revival.
    race(
        &conn,
        day,
        &|| publish(&conn, 7.0, "base-7", "2.0.0", "s5", 10.0, 0, now),
        &[(7.0, "rev", 10, &|| {
            revive(&conn, 7.0, "rev", "r1", 10.0, now)
        })],
        &quota::QUOTA_STORAGE,
    );

    // The total and the daily package quotas: two new packages each.
    for (user, denial) in [
        (3.0, quota::QUOTA_PACKAGES_TOTAL),
        (4.0, quota::QUOTA_PACKAGES_DAILY),
    ] {
        let (a, b) = (format!("new-a-{user}"), format!("new-b-{user}"));
        race(
            &conn,
            day,
            &|| publish(&conn, user, &a, "1.0.0", &a, 1.0, 0, now),
            &[(user, &b, 1, &|| {
                publish(&conn, user, &b, "1.0.0", &b, 1.0, 0, now)
            })],
            &denial,
        );
    }

    // The daily version quota, across publishers: a new version of
    // `busy` wins over another publisher's new version, opt-in respin,
    // and revival.
    race(
        &conn,
        day,
        &|| publish(&conn, 5.0, "busy", "50.0.0", "b1", 1.0, 0, now),
        &[
            (6.0, "busy", 1, &|| {
                publish(&conn, 6.0, "busy", "51.0.0", "b2", 1.0, 0, now)
            }),
            (6.0, "busy", 1, &|| {
                publish(&conn, 6.0, "busy", "1.0.0", "b3", 1.0, 1, now)
            }),
            (6.0, "busy", 1, &|| {
                revive(&conn, 6.0, "busy", "rj", 1.0, now)
            }),
        ],
        &quota::QUOTA_VERSIONS_DAILY,
    );

    assert_eq!(tally_drift(&conn), 0);
}

/// The VM steps the per-publish preflight reads and the package
/// insert's guards take on an account and scope holding `earlier`
/// packages created the day before and `today` created today - two
/// versions each, every tenth one also holding a rejected revision. In
/// order: stored bytes, the package counts for a new and for an
/// existing package, the twin check, and the insert of a new and of an
/// existing package.
fn preflight_steps(earlier: u32, today: u32) -> [i32; 6] {
    let conn = migrated_connection();
    conn.execute_batch(&format!(
        "INSERT INTO users (id, created_at) VALUES (2, '2026-10-01T00:00:00.000Z');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '2', '2026-10-01T00:00:00.000Z');
         WITH RECURSIVE seq(i) AS
           (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < {earlier} + {today})
         INSERT INTO packages (scope, name, created_at, created_by)
           SELECT 'alpha', 'lib-' || i,
                  CASE WHEN i <= {earlier} THEN '2026-10-25T08:00:00.000Z'
                       ELSE '2026-10-26T08:00:00.000Z' END, 2 FROM seq;
         INSERT INTO versions (scope, name, version)
           SELECT scope, name, v FROM packages, (SELECT '1.0.0' AS v UNION ALL SELECT '2.0.0');
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           SELECT scope, name, version, 'r' || rowid, 'r' || rowid, '{{}}',
                  '2026-10-25T08:00:00.000Z', 100, 2,
                  CASE WHEN rowid % 10 = 0 THEN 'rejected' ELSE 'verified' END
           FROM versions;"
    ))
    .expect("seed the corpus");
    let day = "2026-10-26T";
    let mut steps = Vec::new();
    let mut read = |statement: &str, params: &[&dyn rusqlite::ToSql]| {
        let mut stmt = conn.prepare(statement).expect("prepare");
        stmt.query(params)
            .expect("query")
            .next()
            .expect("step")
            .expect("one row");
        steps.push(stmt.get_status(StatementStatus::VmStep));
    };
    read(sql::USER_STORED_BYTES, &[&2.0]);
    read(
        sql::USER_PACKAGE_COUNTS,
        &[&2.0, &day, &"alpha", &"lib_new"],
    );
    read(sql::USER_PACKAGE_COUNTS, &[&2.0, &day, &"alpha", &"lib-1"]);
    read(sql::TWIN_PACKAGE_EXISTS, &[&"alpha", &"lib_new"]);
    // The operator's limits, so even the largest corpus here still
    // admits the new package.
    let limits = Limits::of("operator");
    for (name, inserted) in [("lib_new", 1), ("lib-1", 0)] {
        let mut insert = conn.prepare(sql::INSERT_PACKAGE).expect("prepare");
        assert_eq!(
            insert
                .execute(params![
                    "alpha",
                    name,
                    "2026-10-26T09:00:00.000Z",
                    2.0,
                    100.0,
                    limits.bytes,
                    day,
                    limits.packages_daily,
                    limits.packages_total
                ])
                .expect("INSERT_PACKAGE"),
            inserted
        );
        steps.push(insert.get_status(StatementStatus::VmStep));
    }
    steps.try_into().expect("six measurements")
}

/// D1 bills rows read, and these statements run on every publish that
/// is not a byte-identical retry: their cost must not grow with the
/// account's or the scope's history, or a bulk run's cost grows
/// quadratically with its size.
#[test]
fn the_publish_preflight_cost_does_not_grow_with_the_corpus() {
    assert_eq!(preflight_steps(10, 3), preflight_steps(1_000, 3));
    // Today's new packages cost only a new package's own day counts,
    // which the daily quota bounds.
    let skip_new_count = |s: [i32; 6]| [s[0], s[2], s[3], s[5]];
    assert_eq!(
        skip_new_count(preflight_steps(10, 3)),
        skip_new_count(preflight_steps(10, 300))
    );
}

/// The VM steps of the preflight's version count and of every statement
/// of the publish batch (a new version) and the revival batch, on a
/// package holding `history` versions published the day before.
fn batch_steps(history: u32) -> Vec<i32> {
    let conn = migrated_connection();
    let earlier = "2026-10-25T08:00:00.000Z";
    conn.execute_batch(&format!(
        "INSERT INTO users (id, created_at) VALUES (2, '{earlier}');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '2', '{earlier}');
         INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'lib', '{earlier}', 2);
         WITH RECURSIVE seq(i) AS
           (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < {history})
         INSERT INTO versions (scope, name, version) SELECT 'alpha', 'lib', i || '.0.0' FROM seq;
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           SELECT scope, name, version, 'r' || rowid, 'r' || rowid, '{{}}', '{earlier}', 1, 2,
                  'verified' FROM versions;
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'lib', '1.0.0', 'rj', 'rj', '{{}}', '{earlier}', 1, 2, 'rejected');
         UPDATE users SET package_count = 1, stored_bytes = {history};"
    ))
    .expect("seed the package history");
    let now = "2026-10-26T08:00:00.000Z";
    let mut versions_today = conn
        .prepare(sql::COUNT_PACKAGE_VERSIONS_ON_DAY)
        .expect("prepare");
    let n: i64 = versions_today
        .query_row(params!["alpha", "lib", "2026-10-26T"], |row| row.get(0))
        .expect("versions today");
    assert_eq!(n, 0);
    let mut steps = vec![versions_today.get_status(StatementStatus::VmStep)];
    let published = publish_batch(&conn, 2.0, "lib", "next", "nx", 1.0, 0, now);
    let revived = revive_batch(&conn, 2.0, "lib", "rj", 1.0, now);
    assert_eq!((published[4].0, revived[1].0), (1, 1));
    steps.extend(published.iter().chain(&revived).map(|&(_, step)| step));
    steps
}

/// The batches' quota guards repeat the daily version count on every
/// write statement: it must cost one day's rows, never the package's
/// history, or a long-lived package's every publish grows with it.
#[test]
fn the_publish_batch_cost_does_not_grow_with_the_package_history() {
    assert_eq!(batch_steps(10), batch_steps(1_000));
}

/// `meta.total_stored_bytes` minus one size per live checksum.
fn counter_drift(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT CAST(value AS INTEGER) - (SELECT COALESCE(SUM(size), 0) FROM \
           (SELECT MAX(archive_size) AS size FROM revisions \
            WHERE verification != 'rejected' GROUP BY checksum)) \
         FROM meta WHERE key = 'total_stored_bytes'",
        [],
        |row| row.get(0),
    )
    .expect("counter drift")
}

/// A publish stamped a millisecond before UTC midnight counts only its
/// own day, though rows stamped after midnight - by requests whose
/// preflights started later - already exist: each daily window is one
/// calendar day, in the preflight reads, the guards, and their mirrors.
/// Every account and package here sits one below a daily quota on the
/// publish's day and holds one more row on the next.
#[test]
fn daily_windows_end_at_utc_midnight() {
    let conn = migrated_connection();
    let quotas = quota::quotas_for_class("default");
    let earlier = "2026-10-25T08:00:00.000Z";
    let tomorrow = "2026-10-27T00:00:00.000Z";
    let late = "2026-10-26T23:59:59.999Z";
    let day = "2026-10-26T";
    conn.execute_batch(&format!(
        "INSERT INTO users (id, created_at) VALUES (2, '{earlier}'), (3, '{earlier}');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '2', '{earlier}');
         WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 100)
         INSERT INTO packages (scope, name, created_at, created_by)
           SELECT 'alpha', 'day-' || i, '{day}12:00:00.000Z', 2 FROM seq
             WHERE i < {daily}
           UNION ALL SELECT 'alpha', 'next-day', '{tomorrow}', 2
           UNION ALL SELECT 'alpha', 'busy-' || n, '{earlier}', 3
             FROM (SELECT 'a' AS n UNION ALL SELECT 'b' UNION ALL SELECT 'c');
         WITH RECURSIVE seq(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM seq WHERE i < 100)
         INSERT INTO versions (scope, name, version)
           SELECT scope, name, i || '.0.0' FROM packages, seq
           WHERE name LIKE 'busy-%' AND i <= {versions};
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by)
           SELECT scope, name, version, 'v' || rowid, 'v' || rowid, '{{}}',
                  CASE WHEN version = '{versions}.0.0' THEN '{tomorrow}'
                       ELSE '{day}12:00:00.000Z' END, 1, 3
           FROM versions;
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'busy-c', '1.0.0', 'rj', 'rj', '{{}}', '{earlier}', 1, 3,
                   'rejected');
         UPDATE users SET
           package_count = (SELECT COUNT(*) FROM packages WHERE created_by = users.id),
           stored_bytes = (SELECT COALESCE(SUM(archive_size), 0) FROM revisions
                           WHERE published_by = users.id AND verification != 'rejected');
         UPDATE meta SET value = (SELECT COUNT(*) FROM revisions
                                  WHERE verification != 'rejected')
           WHERE key = 'total_stored_bytes';",
        daily = quotas.max_new_packages_per_day,
        versions = quotas.max_versions_per_package_per_day,
    ))
    .expect("seed rows on both sides of midnight");

    for (user, name) in [
        (2.0, "late"),
        (3.0, "busy-a"),
        (3.0, "busy-b"),
        (3.0, "busy-c"),
    ] {
        assert_eq!(preflight(&conn, user, name, 1, day), Ok(()), "{name}");
    }
    // A new package, a new version, a respin, and a revival.
    assert_eq!(publish(&conn, 2.0, "late", "1.0.0", "l1", 1.0, 0, late), 1);
    assert_eq!(publish(&conn, 3.0, "busy-a", "new", "a1", 1.0, 0, late), 1);
    assert_eq!(
        publish(&conn, 3.0, "busy-b", "1.0.0", "b1", 1.0, 1, late),
        1
    );
    assert_eq!(revive(&conn, 3.0, "busy-c", "rj", 1.0, late), 1);
    assert_eq!(tally_drift(&conn), 0);
    assert_eq!(counter_drift(&conn), 0);
}
