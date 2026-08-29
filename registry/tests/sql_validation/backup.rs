//! Executed-semantics tests for `src/sql/backup.rs`: the verified
//! backup queue.

use cabin_registry_worker::sql;

use crate::common::{migrated_connection, seed_scope_collision};

/// The verified-backup queue derives its R2 key from the canonical
/// `sha256:<64 lowercase hex>` column value by stripping the
/// algorithm prefix, keeping the OCI-style `blobs/sha256/<hex>` key
/// layout stable across the prefixed-column migration.
#[test]
fn verified_backup_enqueue_strips_the_checksum_prefix() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let hex = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";
    let checksum = format!("sha256:{hex}");
    conn.execute(
        "UPDATE revisions SET checksum = ?1 WHERE scope = 'alpha'",
        rusqlite::params![checksum],
    )
    .expect("store the canonical spelling");
    let queued = conn
        .execute(
            sql::ENQUEUE_VERIFIED_BACKUP,
            rusqlite::params![
                "alpha",
                "pkg",
                "1.0.0",
                checksum,
                "2026-07-15T00:00:00.000Z",
                "2026-07-15T01:00:00.000Z",
                "aa"
            ],
        )
        .expect("enqueue the verified blob");
    assert_eq!(queued, 1);
    let key: String = conn
        .query_row("SELECT key FROM backup_pending", [], |row| row.get(0))
        .expect("queued key");
    assert_eq!(key, format!("blobs/sha256/{hex}"));
}
