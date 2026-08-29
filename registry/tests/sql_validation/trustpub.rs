//! Executed-semantics tests for `src/sql/trustpub.rs`: OIDC jti replay
//! and the trusted-publishing seed.

use crate::common::{count, migrated_connection};

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
            // The cabinpkg organization and cabinpkg/cabin, by their
            // immutable numeric GitHub ids.
            35_998_702,
            119_684_778,
            "ports-publish.yml".to_owned(),
            Some("refs/heads/main".to_owned()),
            None,
            "operator".to_owned(),
            "2026-08-14T00:00:00.000Z".to_owned(),
        )
    );
}
