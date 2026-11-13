//! Executed-semantics tests for `src/sql/publication.rs`: what the read
//! plane serves before and after an archive is placed, the guards of
//! the placement pass's two writes, and the generations and jobs of the
//! publish pass.

use cabin_registry_worker::{publication, sql, verify};
use rusqlite::Connection;
use rusqlite::types::Value;

use crate::common::{batch, migrated_connection, rows, seed_scope_collision, sweep};

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

/// One batch that changes what `scope/pkg`'s document says: the change
/// itself, then the statements `glue::publication::document_changed`
/// puts after it; each statement's changed rows.
fn change(conn: &Connection, statement: &str, values: Vec<Value>, scope: &str) -> Vec<usize> {
    batch(
        conn,
        vec![
            (statement, values),
            (sql::ADVANCE_CATALOG_GENERATION, vec![]),
            (
                sql::STAMP_PACKAGE_GENERATION,
                vec![text(scope), text("pkg")],
            ),
            (
                sql::RECORD_PUBLICATION_JOB,
                vec![text(scope), text("pkg"), text(NOW)],
            ),
        ],
    )
    .into_iter()
    .map(|(changed, _)| changed)
    .collect()
}

/// A yank or un-yank of `alpha/pkg@1.0.0`, the flag bound as the
/// number D1 binds.
fn yank(conn: &Connection, yanked: f64) -> Vec<usize> {
    let values = vec![yanked.into(), text("alpha"), text("pkg"), text("1.0.0")];
    change(conn, sql::SET_VERSION_YANKED, values, "alpha")
}

/// Completes `scope/pkg`'s jobs up to `generation`, bound as the number
/// D1 binds, that were recorded by `settled`; the rows changed.
fn complete(conn: &Connection, scope: &str, generation: f64, settled: &str) -> usize {
    conn.execute(
        sql::COMPLETE_PUBLICATION_JOBS,
        rusqlite::params![NOW, scope, "pkg", generation, settled],
    )
    .expect("COMPLETE_PUBLICATION_JOBS")
}

/// A revision becoming served, a yank, and an un-yank each take the
/// registry's next generation, give it to their package, and record
/// one job; a statement that changes nothing does none of it.
#[test]
fn only_a_change_to_a_document_takes_a_generation_and_records_a_job() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    verify_beta(&conn);
    let mark = || {
        let values = vec![
            text(NOW),
            text("beta"),
            text("pkg"),
            text("1.0.0"),
            text("bb"),
        ];
        change(&conn, sql::MARK_REVISION_ARCHIVED, values, "beta")
    };
    let state = |conn: &Connection| {
        rows(
            conn,
            "SELECT value, typeof(value), \
             (SELECT group_concat(scope || '=' || generation) FROM packages), \
             (SELECT group_concat(job) FROM (SELECT generation || ':' || scope AS job \
              FROM publication_jobs \
              WHERE completed_at IS NULL AND created_at = '2026-07-16T00:00:00.000Z' \
              ORDER BY generation)) \
             FROM meta WHERE key = 'catalog_generation'",
        )
    };
    assert_eq!(
        state(&conn),
        [r#"Text("0") | Text("text") | Text("alpha=0,beta=0") | Null"#]
    );

    assert_eq!(mark(), [1; 4]);
    let served = [r#"Text("1") | Text("text") | Text("alpha=0,beta=1") | Text("1:beta")"#];
    assert_eq!(state(&conn), served);
    assert_eq!(mark(), [0; 4], "already served");
    assert_eq!(yank(&conn, 0.0), [0; 4], "not yanked");
    assert_eq!(state(&conn), served);

    assert_eq!(yank(&conn, 1.0), [1; 4]);
    assert_eq!(yank(&conn, 1.0), [0; 4], "already yanked");
    assert_eq!(yank(&conn, 0.0), [1; 4]);
    assert_eq!(
        state(&conn),
        [r#"Text("3") | Text("text") | Text("alpha=3,beta=1") | Text("1:beta,2:alpha,3:alpha")"#]
    );
    let generation = |statement: &str, params: &[&str]| -> i64 {
        conn.query_row(statement, rusqlite::params_from_iter(params), |row| {
            row.get("generation")
        })
        .expect(statement)
    };
    assert_eq!(generation(sql::PACKAGE_GENERATION, &["alpha", "pkg"]), 3);
    assert_eq!(generation(sql::PACKAGE_GENERATION, &["beta", "pkg"]), 1);
    assert_eq!(generation(sql::CATALOG_GENERATION, &[]), 3);
}

/// The publish pass lists the packages with open jobs by their oldest
/// one, and completes a package's jobs only up to the generation it
/// published, and only those recorded by the time it names.
#[test]
fn a_job_is_listed_until_its_generation_is_published() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let unpublished = |conn: &Connection| rows(conn, sql::UNPUBLISHED_PACKAGES);
    assert!(unpublished(&conn).is_empty());
    assert_eq!(yank(&conn, 1.0), [1; 4]);
    verify_beta(&conn);
    let mark = vec![
        text(NOW),
        text("beta"),
        text("pkg"),
        text("1.0.0"),
        text("bb"),
    ];
    assert_eq!(
        change(&conn, sql::MARK_REVISION_ARCHIVED, mark, "beta"),
        [1; 4]
    );
    assert_eq!(yank(&conn, 0.0), [1; 4]);
    let alpha = r#"Text("alpha") | Text("pkg")"#;
    let beta = r#"Text("beta") | Text("pkg")"#;
    assert_eq!(unpublished(&conn), [alpha, beta], "generations 1 and 3, 2");

    // The jobs are recorded at `NOW`: a pass leaves the ones recorded
    // after the time it names.
    assert_eq!(complete(&conn, "alpha", 3.0, PUBLISHED), 0, "not settled");
    assert_eq!(unpublished(&conn), [alpha, beta]);

    // A pass that rendered alpha at generation 1 leaves its later job.
    assert_eq!(complete(&conn, "alpha", 1.0, NOW), 1);
    assert_eq!(unpublished(&conn), [beta, alpha], "generations 2, 3");
    assert_eq!(complete(&conn, "alpha", 2.0, NOW), 0, "beta's generation");
    assert_eq!(complete(&conn, "beta", 3.0, NOW), 1);
    assert_eq!(unpublished(&conn), [alpha]);
    assert_eq!(complete(&conn, "alpha", 3.0, NOW), 1);
    assert_eq!(complete(&conn, "alpha", 3.0, NOW), 0, "once");
    assert!(unpublished(&conn).is_empty());
    assert_eq!(
        rows(
            &conn,
            "SELECT COUNT(*) FROM publication_jobs WHERE completed_at = '2026-07-16T00:00:00.000Z'"
        ),
        ["Integer(3)"]
    );
}

/// The cron deletes a change thirty days after it completes, and never
/// an open one.
#[test]
fn a_completed_job_is_deleted_after_thirty_days_and_an_open_one_never() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    assert_eq!(yank(&conn, 1.0), [1; 4]);
    assert_eq!(yank(&conn, 0.0), [1; 4]);
    assert_eq!(complete(&conn, "alpha", 1.0, NOW), 1, "completed at NOW");
    let jobs = |conn: &Connection| {
        rows(
            conn,
            "SELECT generation FROM publication_jobs ORDER BY generation",
        )
    };

    sweep(&conn, "2026-08-15T00:00:00.000Z");
    assert_eq!(jobs(&conn), ["Integer(1)", "Integer(2)"], "thirty days");
    sweep(&conn, "2026-08-15T00:00:00.001Z");
    assert_eq!(jobs(&conn), ["Integer(2)"]);
    sweep(&conn, "2036-07-16T00:00:00.000Z");
    assert_eq!(jobs(&conn), ["Integer(2)"], "open, whatever its age");
}

/// `sql::PIPELINE_HEALTH` at `now`: its one row.
fn health(conn: &Connection, now: &str) -> String {
    conn.query_row(
        sql::PIPELINE_HEALTH,
        rusqlite::params![now, verify::STALE_PENDING, publication::STALL],
        |row| {
            (0..6)
                .map(|column| row.get::<_, Value>(column).map(|cell| format!("{cell:?}")))
                .collect::<Result<Vec<_>, _>>()
        },
    )
    .expect("PIPELINE_HEALTH")
    .join(" | ")
}

/// The health read counts an upload as stale once it has awaited its
/// verdict for over an hour, and names a verified archive or an open
/// change once it is over forty-five minutes old; a completed change
/// only moves the newest completion.
#[test]
fn the_health_read_names_what_has_waited_too_long() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let stored = r#"Text("2026-07-15T00:00:00.000Z")"#;
    let at_now = r#"Text("2026-07-16T00:00:00.000Z")"#;
    assert_eq!(
        health(&conn, "2026-07-15T01:00:00.000Z"),
        format!("Integer(0) | {stored} | Null | Null | Null | Null")
    );
    assert_eq!(
        health(&conn, "2026-07-15T01:00:00.001Z"),
        format!("Integer(1) | {stored} | Null | Null | Null | Null")
    );

    // Verified at `NOW`, not placed.
    verify_beta(&conn);
    let healthy = "Integer(0) | Null | Null | Null | Null | Null";
    assert_eq!(health(&conn, "2026-07-16T00:45:00.000Z"), healthy);
    assert_eq!(
        health(&conn, "2026-07-16T00:45:00.001Z"),
        format!("Integer(0) | Null | {at_now} | Null | Null | Null")
    );

    // Placed, which records a change at `NOW`.
    let mark = vec![
        text(NOW),
        text("beta"),
        text("pkg"),
        text("1.0.0"),
        text("bb"),
    ];
    assert_eq!(
        change(&conn, sql::MARK_REVISION_ARCHIVED, mark, "beta"),
        [1; 4]
    );
    assert_eq!(health(&conn, "2026-07-16T00:45:00.000Z"), healthy);
    assert_eq!(
        health(&conn, "2026-07-16T00:45:00.001Z"),
        format!("Integer(0) | Null | Null | {at_now} | Null | Null")
    );

    assert_eq!(complete(&conn, "beta", 1.0, NOW), 1);
    assert_eq!(
        health(&conn, "2036-07-16T00:00:00.000Z"),
        format!("Integer(0) | Null | Null | Null | {at_now} | Null")
    );

    // Of several, the open change named is the oldest and the
    // completion the newest.
    conn.execute_batch(
        "INSERT INTO publication_jobs (generation, scope, name, created_at, completed_at) VALUES
         (11, 'beta', 'pkg', '2026-07-16T00:20:00.000Z', NULL),
         (12, 'beta', 'pkg', '2026-07-16T00:10:00.000Z', NULL),
         (13, 'beta', 'pkg', '2026-07-16T00:05:00.000Z', '2026-07-16T00:40:00.000Z'),
         (14, 'beta', 'pkg', '2026-07-16T00:05:00.000Z', '2026-07-16T00:30:00.000Z');",
    )
    .expect("more jobs");
    assert_eq!(
        health(&conn, "2036-07-16T00:00:00.000Z"),
        r#"Integer(0) | Null | Null | Text("2026-07-16T00:10:00.000Z") | Text("2026-07-16T00:40:00.000Z") | Null"#
    );
}
