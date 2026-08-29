//! Shared fixtures: the migrated in-memory database and the helpers
//! more than one statement module exercises.

use std::fs;
use std::path::Path;

use cabin_registry_worker::sql;

/// An in-memory database with every migration applied, oldest first.
/// Foreign keys are enforced, as they are on D1.
pub fn migrated_connection() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    conn.pragma_update(None, "foreign_keys", true)
        .expect("enable foreign_keys");
    // D1 parity: D1 caps LIKE/GLOB patterns at 50 bytes at evaluation
    // (bundled SQLite defaults to 50000), so a long pattern - say a
    // fixed-width GLOB in a CHECK - passes every host test while
    // failing every INSERT in production. Pinning the limit here makes
    // the whole suite evaluate patterns under D1's rules.
    conn.set_limit(
        rusqlite::limits::Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH,
        50,
    )
    .expect("pin the D1 LIKE/GLOB pattern limit");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut migrations: Vec<_> = fs::read_dir(&dir)
        .expect("read migrations/")
        .map(|entry| entry.expect("read migrations/ entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    migrations.sort();
    assert!(
        !migrations.is_empty(),
        "no migrations found in {}",
        dir.display()
    );
    for path in migrations {
        let statements = fs::read_to_string(&path).expect("read migration");
        if let Err(err) = conn.execute_batch(&statements) {
            panic!("{} failed to apply: {err}", path.display());
        }
    }
    conn
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
