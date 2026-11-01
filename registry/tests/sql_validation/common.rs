//! Shared fixtures: the migrated in-memory database and the helpers
//! more than one statement module exercises.

use cabin_registry_worker::{quota, sql};

#[path = "../../src/test_db.rs"]
mod test_db;

pub use test_db::migrated_connection;

/// One class's publish limits as the publish glue binds them into the
/// write batches' quota guards: D1 binds every number as a REAL.
pub struct Limits {
    pub bytes: f64,
    pub versions_daily: f64,
    pub packages_daily: f64,
    pub packages_total: f64,
}

impl Limits {
    #[allow(clippy::cast_precision_loss)]
    pub fn of(class: &str) -> Self {
        let quotas = quota::quotas_for_class(class);
        Self {
            bytes: quotas.max_total_bytes_per_user as f64,
            versions_daily: quotas.max_versions_per_package_per_day as f64,
            packages_daily: quotas.max_new_packages_per_day as f64,
            packages_total: quotas.max_packages_total as f64,
        }
    }
}

/// One sign-in's identity upsert, exactly as the OAuth callback runs it:
/// both statements back-to-back on one connection, user creation first
/// (a D1 batch is one transaction on one connection, so the
/// `last_insert_rowid()` coupling behaves identically there).
pub fn sign_in(
    conn: &rusqlite::Connection,
    provider: &str,
    account_id: &str,
    login: &str,
    now: &str,
) {
    conn.execute(
        sql::INSERT_USER_FOR_NEW_IDENTITY,
        rusqlite::params![now, provider, account_id],
    )
    .expect("insert user for new identity");
    conn.execute(
        sql::UPSERT_IDENTITY,
        rusqlite::params![provider, account_id, login],
    )
    .expect("upsert identity");
}

/// The `(user_id, login_snapshot, quota_class)` the session plane
/// resolves for an identity, if any.
pub fn resolve(
    conn: &rusqlite::Connection,
    provider: &str,
    account_id: &str,
) -> Option<(i64, String, String)> {
    conn.query_row(
        sql::USER_BY_IDENTITY,
        rusqlite::params![provider, account_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .map(Some)
    .or_else(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
    .expect("resolve identity")
}

pub fn count(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .expect("count rows")
}

/// Seeds one user, two scopes the user is a member of, and the same
/// `(name, version)` under both - the collision the scoped statements
/// must keep apart.
pub fn seed_scope_collision(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (2, '2026-07-15T00:00:00.000Z');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '1', '2026-07-15T00:00:00.000Z'),
                  ('beta', 'github', '2', '2026-07-15T00:00:00.000Z');
         INSERT INTO scope_members (scope_name, user_id, role) VALUES ('alpha', 2, 'owner');
         INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'pkg', '2026-07-15T00:00:00.000Z', 2),
                  ('beta', 'pkg', '2026-07-15T00:00:00.000Z', 2);
         INSERT INTO versions (scope, name, version) VALUES ('alpha', 'pkg', '1.0.0'),
                  ('beta', 'pkg', '1.0.0');
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json, \
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'pkg', '1.0.0', 'aa', 'aa', '{}', '2026-07-15T00:00:00.000Z', 10, 2, 'verified'),
                  ('beta', 'pkg', '1.0.0', 'bb', 'bb', '{}', '2026-07-15T00:00:00.000Z', 20, 2, 'pending');
         UPDATE meta SET value = '30' WHERE key = 'total_stored_bytes';",
    )
    .expect("seed the cross-scope collision");
}
