//! Executed-semantics tests for `src/sql/quota.rs`: the publish token
//! bucket's compare-and-swap.

use cabin_registry_worker::sql;

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
