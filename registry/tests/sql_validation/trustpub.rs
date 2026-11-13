//! Executed-semantics tests for `src/sql/trustpub.rs`: OIDC jti replay,
//! the verifier's heartbeat, and the trusted-publishing seed.

use cabin_registry_worker::{publication, sql, verify};
use rusqlite::types::Value;

use crate::common::{batch, count, migrated_connection};

#[test]
fn used_jtis_refuse_null_and_replayed_ids() {
    let conn = migrated_connection();
    conn.execute(
        "INSERT INTO oidc_used_jtis (jti, expires_at) VALUES ('jti-1', 1)",
        [],
    )
    .expect("first consumption");
    let replay = conn
        .execute(
            "INSERT INTO oidc_used_jtis (jti, expires_at) VALUES ('jti-1', 2)",
            [],
        )
        .expect_err("a replayed jti must fail the primary key");
    assert!(replay.to_string().contains("UNIQUE"), "{replay}");
    // NOT NULL is load-bearing: SQLite would otherwise admit unlimited
    // duplicate NULLs through a TEXT primary key.
    let null = conn
        .execute(
            "INSERT INTO oidc_used_jtis (jti, expires_at) VALUES (NULL, 1)",
            [],
        )
        .expect_err("a null jti must fail NOT NULL");
    assert!(null.to_string().contains("NOT NULL"), "{null}");
}

/// The verifier arm's exchange, as `glue::bearer::tokens::exchange_mint`
/// batches it: the heartbeat moves with a stored mint and never with a
/// replayed jti, and the health read returns it.
#[test]
fn the_verifier_heartbeat_moves_with_a_stored_mint_only() {
    let conn = migrated_connection();
    let text = |value: &str| Value::Text(value.to_owned());
    let exchange = |jti: &str, minute: u8| {
        let token = format!("token-{minute}");
        batch(
            &conn,
            vec![
                (sql::CONSUME_OIDC_JTI, vec![text(jti), Value::Integer(1)]),
                (
                    sql::INSERT_TRUSTPUB_VERIFY_TOKEN,
                    vec![
                        text(&token),
                        Value::Integer(1),
                        text("trusted-publishing: verify.yml"),
                        text(&token),
                        text(&format!("2026-07-16T00:{minute:02}:00.000Z")),
                        text(&format!("2026-07-16T01:{minute:02}:00.000Z")),
                    ],
                ),
                (
                    sql::RECORD_VERIFIER_SEEN,
                    vec![text(&format!("2026-07-16T00:{minute:02}:00.000Z"))],
                ),
            ],
        )
        .into_iter()
        .map(|(changed, _)| changed)
        .collect::<Vec<_>>()
    };
    let seen = || {
        conn.query_row(
            sql::PIPELINE_HEALTH,
            rusqlite::params![
                "2026-07-16T02:00:00.000Z",
                verify::STALE_PENDING,
                publication::STALL
            ],
            |row| row.get::<_, Option<String>>("verifier_seen_at"),
        )
        .expect("PIPELINE_HEALTH")
    };
    assert_eq!(seen(), None);
    assert_eq!(exchange("jti-1", 10), [1, 1, 1]);
    assert_eq!(seen().as_deref(), Some("2026-07-16T00:10:00.000Z"));
    assert_eq!(exchange("jti-1", 20), [0, 0, 0], "replayed");
    assert_eq!(seen().as_deref(), Some("2026-07-16T00:10:00.000Z"));
    assert_eq!(exchange("jti-2", 30), [1, 1, 1]);
    assert_eq!(seen().as_deref(), Some("2026-07-16T00:30:00.000Z"));
}

#[test]
fn the_trustpub_seed_names_the_ports_publishing_workflow() {
    let conn = migrated_connection();
    // Exactly the one seeded row (query_row alone would silently
    // ignore extras).
    assert_eq!(count(&conn, "trustpub_configs"), 1);
    let row = conn
        .query_row(
            "SELECT scope, repository_owner_id, repository_id, workflow_filename, \
                    git_ref, environment, quota_class, created_at \
             FROM trustpub_configs",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .expect("exactly one seeded config");
    assert_eq!(
        row,
        (
            "cabin-ports".to_owned(),
            // The cabin-ports organization and cabin-ports/ports, by
            // their immutable numeric GitHub ids (migration 0002).
            308_231_483,
            1_446_491_029,
            "publish.yml".to_owned(),
            Some("refs/heads/main".to_owned()),
            None,
            "operator".to_owned(),
            "2026-08-14T00:00:00.000Z".to_owned(),
        )
    );
}
