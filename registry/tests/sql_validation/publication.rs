//! Executed-semantics tests for `src/sql/publication.rs`: what the read
//! plane serves before and after an archive is placed, and the guards
//! of the placement pass's two writes.

use cabin_registry_worker::{publication, sql};
use rusqlite::Connection;
use rusqlite::types::Value;

use crate::common::{batch, migrated_connection, rows, seed_scope_collision};

const NOW: &str = "2026-07-16T00:00:00.000Z";
/// The seed rows' `published_at`.
const PUBLISHED: &str = "2026-07-15T00:00:00.000Z";

fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

/// Turns the seed's pending `beta/pkg@1.0.0#bb` (20 bytes, upload
/// `seed-bb`) verified, as the verdict does.
fn verify_beta(conn: &Connection) {
    let changed = conn
        .execute(
            sql::MARK_REVISION_VERIFIED,
            rusqlite::params![NOW, "beta", "pkg", "1.0.0", "bb", PUBLISHED, "bb"],
        )
        .expect("MARK_REVISION_VERIFIED");
    assert_eq!(changed, 1);
}

fn unarchived(conn: &Connection) -> Vec<String> {
    rows(
        conn,
        &format!(
            "SELECT scope, revision, upload_id FROM ({})",
            sql::UNARCHIVED_REVISIONS
        ),
    )
}

fn mark_archived(conn: &Connection, scope: &str, revision: &str) -> usize {
    conn.execute(
        sql::MARK_REVISION_ARCHIVED,
        rusqlite::params![NOW, scope, "pkg", "1.0.0", revision],
    )
    .expect("MARK_REVISION_ARCHIVED")
}

/// The placement pass's rejection batch for `beta/pkg@1.0.0#bb`,
/// guarded on `published_at`; each statement's changed rows.
fn reject_unarchived(conn: &Connection, published_at: &str) -> Vec<usize> {
    let revision = || vec![text("beta"), text("pkg"), text("1.0.0"), text("bb")];
    let mut flip = vec![text(publication::SOURCE_LOST), text(NOW)];
    flip.extend(revision());
    flip.push(text(published_at));
    batch(
        conn,
        vec![
            (sql::REJECT_UNARCHIVED_REVISION, flip),
            (sql::REFUND_USER_STORED_BYTES, revision()),
        ],
    )
    .into_iter()
    .map(|(changed, _)| changed)
    .collect()
}

/// A verified revision reaches no read projection until its archive is
/// recorded in ARCHIVES, and only that makes it leave the work list.
#[test]
fn a_verified_revision_is_served_only_once_its_archive_is_placed() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    assert!(unarchived(&conn).is_empty(), "served and pending rows");
    assert_eq!(mark_archived(&conn, "beta", "bb"), 0, "a pending row");
    verify_beta(&conn);
    assert_eq!(
        unarchived(&conn),
        [r#"Text("beta") | Text("bb") | Text("seed-bb")"#]
    );

    let served = |conn: &Connection| -> [i64; 5] {
        let count = |statement: &str, params: &[&str]| -> i64 {
            conn.query_row(
                &format!("SELECT COUNT(*) FROM ({statement})"),
                rusqlite::params_from_iter(params),
                |row| row.get(0),
            )
            .expect(statement)
        };
        let flag = |statement: &str, column: &str, params: &[&str]| -> i64 {
            conn.query_row(statement, rusqlite::params_from_iter(params), |row| {
                row.get(column)
            })
            .expect(statement)
        };
        [
            count(sql::CURRENT_REVISIONS_BY_PACKAGE, &["beta", "pkg"]),
            count(sql::VERIFIED_REVISIONS_BY_PACKAGE, &["beta", "pkg"]),
            flag(sql::HAS_VERIFIED_VERSION, "n", &["beta", "pkg"]),
            flag(
                sql::VERSION_YANK_STATE,
                "verified",
                &["beta", "pkg", "1.0.0"],
            ),
            flag(
                sql::ARTIFACT_BY_REVISION,
                "archived",
                &["beta", "pkg", "1.0.0", "bb"],
            ),
        ]
    };
    assert_eq!(served(&conn), [0; 5], "verified, not placed");

    assert_eq!(mark_archived(&conn, "beta", "bb"), 1);
    assert_eq!(served(&conn), [1; 5], "placed");
    assert!(unarchived(&conn).is_empty());
    assert_eq!(mark_archived(&conn, "beta", "bb"), 0, "already recorded");
    assert_eq!(
        rows(
            &conn,
            "SELECT archived_at FROM revisions WHERE revision = 'bb'"
        ),
        [format!("Text({NOW:?})")]
    );
}

/// A verified revision whose upload is lost is rejected and refunded,
/// only while it is unplaced and still the generation the pass listed.
#[test]
fn only_the_unplaced_generation_the_pass_listed_is_rejected() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let state = |conn: &Connection| {
        rows(
            conn,
            "SELECT r.verification, r.verification_reason, r.verified_at, r.rejected_at, \
             u.stored_bytes FROM revisions r JOIN users u ON u.id = r.published_by \
             WHERE r.revision = 'bb'",
        )
    };
    assert_eq!(reject_unarchived(&conn, PUBLISHED), [0, 0], "pending");
    verify_beta(&conn);
    let verified = state(&conn);

    // The pass listed an earlier publish event of the row: nothing
    // changes.
    assert_eq!(reject_unarchived(&conn, "2026-07-14T00:00:00.000Z"), [0, 0]);
    assert_eq!(state(&conn), verified);

    assert_eq!(reject_unarchived(&conn, PUBLISHED), [1, 1]);
    assert_eq!(
        state(&conn),
        [format!(
            r#"Text("rejected") | Text("publication_source_lost") | Null | Text({NOW:?}) | Integer(10)"#
        )],
        "the 20 bytes are refunded from the seed's 30"
    );
    assert_eq!(reject_unarchived(&conn, PUBLISHED), [0, 0], "once");

    // A served revision is never rejected.
    assert_eq!(
        conn.execute(
            sql::REJECT_UNARCHIVED_REVISION,
            rusqlite::params![
                publication::SOURCE_LOST,
                NOW,
                "alpha",
                "pkg",
                "1.0.0",
                "aa",
                PUBLISHED
            ],
        )
        .expect("REJECT_UNARCHIVED_REVISION"),
        0
    );
}
