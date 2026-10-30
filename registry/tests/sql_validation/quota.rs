//! Executed-semantics tests for `src/sql/quota.rs`: the publish token
//! bucket's compare-and-swap and the per-user quota tallies.

use cabin_registry_worker::sql;
use rusqlite::{Connection, StatementStatus, params};

use crate::common::{migrated_connection, resolve, sign_in};

/// One `glue::cas_bucket` write: D1 binds every JS number as a REAL
/// (the user id too) and the timestamp as the `f64` `Display` text.
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

/// `persist_new_revision`'s batch in the glue's statement order, as one
/// transaction, with D1's REAL binds; the revision insert's changed rows.
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
    let tx = conn.unchecked_transaction().expect("begin");
    tx.execute(sql::INSERT_PACKAGE, params!["alpha", name, stamp, user])
        .expect("INSERT_PACKAGE");
    tx.execute(sql::COUNT_USER_PACKAGE, [user])
        .expect("COUNT_USER_PACKAGE");
    tx.execute(sql::INSERT_VERSION_ROW, params!["alpha", name, version])
        .expect("INSERT_VERSION_ROW");
    tx.execute(
        sql::COUNT_STORED_BYTES_ON_PUBLISH,
        params![revision, size, "alpha", name, version, revision, opt_in],
    )
    .expect("COUNT_STORED_BYTES_ON_PUBLISH");
    let inserted = tx
        .execute(
            sql::INSERT_REVISION,
            params![
                "alpha", name, version, revision, revision, "{}", stamp, size, user, opt_in
            ],
        )
        .expect("INSERT_REVISION");
    tx.execute(sql::COUNT_USER_STORED_BYTES, params![user, size])
        .expect("COUNT_USER_STORED_BYTES");
    tx.commit().expect("commit");
    inserted
}

/// `revive_rejected_revision`'s batch; the flip's changed rows.
fn revive(
    conn: &Connection,
    user: f64,
    name: &str,
    revision: &str,
    size: f64,
    stamp: &str,
) -> usize {
    let tx = conn.unchecked_transaction().expect("begin");
    tx.execute(
        sql::COUNT_STORED_BYTES_ON_REVIVAL,
        params![
            "alpha", name, "1.0.0", revision, revision, size, revision, 1, stamp
        ],
    )
    .expect("COUNT_STORED_BYTES_ON_REVIVAL");
    let flipped = tx
        .execute(
            sql::REVIVE_REJECTED_REVISION,
            params![
                "{}", stamp, user, "alpha", name, 1, "1.0.0", revision, revision
            ],
        )
        .expect("REVIVE_REJECTED_REVISION");
    tx.execute(sql::COUNT_USER_STORED_BYTES, params![user, size])
        .expect("COUNT_USER_STORED_BYTES");
    tx.commit().expect("commit");
    flipped
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

/// The VM steps the per-publish preflight reads and the package
/// insert's twin guard take on an account and scope holding `earlier`
/// packages created the day before and `today` created today - two
/// versions each, every tenth one also holding a rejected revision. In
/// order: stored bytes, the package counts for a new and for an
/// existing package, the twin check, and the insert.
fn preflight_steps(earlier: u32, today: u32) -> [i32; 5] {
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
    let mut insert = conn.prepare(sql::INSERT_PACKAGE).expect("prepare");
    assert_eq!(
        insert
            .execute(params!["alpha", "lib_new", "2026-10-26T09:00:00.000Z", 2.0])
            .expect("INSERT_PACKAGE"),
        1
    );
    steps.push(insert.get_status(StatementStatus::VmStep));
    steps.try_into().expect("five measurements")
}

/// D1 bills rows read, and these statements run on every publish that
/// is not a byte-identical retry: their cost must not grow with the
/// account's or the scope's history, or a bulk run's cost grows
/// quadratically with its size.
#[test]
fn the_publish_preflight_cost_does_not_grow_with_the_corpus() {
    assert_eq!(preflight_steps(10, 3), preflight_steps(1_000, 3));
    // Today's new packages cost only a new package's own day count,
    // which the daily quota bounds.
    let skip_new_count = |s: [i32; 5]| [s[0], s[2], s[3], s[4]];
    assert_eq!(
        skip_new_count(preflight_steps(10, 3)),
        skip_new_count(preflight_steps(10, 300))
    );
}
