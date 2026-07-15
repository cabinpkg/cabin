//! Host-target schema validation for every SQL statement the Worker
//! executes: apply every file in `migrations/` in filename order (a
//! from-zero migration test in itself), then `prepare` each statement
//! in [`cabin_registry_worker::sql::ALL`] against the migrated schema.
//! `prepare` validates syntax and table/column existence without
//! executing and accepts D1's `?N` placeholders, so a typo, a wrong
//! column name, or schema drift fails here instead of in production -
//! the assurance an ORM's typed columns would give at compile time,
//! without one (`docs/architecture.md`, "Why no ORM"). The identity
//! upsert additionally gets **executed** here: its two statements are
//! coupled through `last_insert_rowid()`, which `prepare` cannot check.
#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::Path;

use cabin_registry_worker::sql;

/// Statements `rusqlite` cannot prepare because they need a D1-only
/// construct. Deliberately empty - D1 speaks `SQLite`'s dialect for
/// everything the service executes today - and every future entry must
/// carry a rationale comment plus its own dedicated test.
const EXCLUDED_D1_ONLY: &[&str] = &[];

/// An in-memory database with every migration applied, oldest first.
/// Foreign keys are enforced, as they are on D1.
fn migrated_connection() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    conn.pragma_update(None, "foreign_keys", true)
        .expect("enable foreign_keys");
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

#[test]
fn every_executed_statement_prepares_against_the_migrated_schema() {
    let conn = migrated_connection();
    for statement in sql::ALL {
        if EXCLUDED_D1_ONLY.contains(statement) {
            continue;
        }
        if let Err(err) = conn.prepare(statement) {
            panic!("statement does not prepare against the migrated schema: {err}\n  {statement}");
        }
    }
}

#[test]
fn exclusions_are_executed_statements() {
    // A stale or misspelled exclusion would silently weaken coverage.
    for excluded in EXCLUDED_D1_ONLY {
        assert!(
            sql::ALL.contains(excluded),
            "EXCLUDED_D1_ONLY entry is not in sql::ALL: {excluded}"
        );
    }
}

/// One sign-in's identity upsert, exactly as the OAuth callback runs it:
/// both statements back-to-back on one connection, user creation first
/// (a D1 batch is one transaction on one connection, so the
/// `last_insert_rowid()` coupling behaves identically there).
fn sign_in(conn: &rusqlite::Connection, provider: &str, account_id: &str, login: &str, now: &str) {
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

/// The `(user_id, login_snapshot, plan)` the session plane resolves for
/// an identity, if any.
fn resolve(
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

fn count(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .expect("count rows")
}

#[test]
fn first_sign_in_creates_the_user_and_binds_the_identity() {
    let conn = migrated_connection();
    sign_in(
        &conn,
        "github",
        "26405363",
        "ken-matsui",
        "2026-07-15T00:00:00.000Z",
    );
    assert_eq!(count(&conn, "users"), 1);
    assert_eq!(count(&conn, "identities"), 1);
    let (user_id, login, plan) = resolve(&conn, "github", "26405363").expect("identity resolves");
    assert_eq!(login, "ken-matsui");
    assert_eq!(plan, "free");
    let created_at: String = conn
        .query_row(
            "SELECT created_at FROM users WHERE id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .expect("user row");
    assert_eq!(created_at, "2026-07-15T00:00:00.000Z");
}

#[test]
fn repeat_sign_in_refreshes_the_login_and_keeps_the_user_binding() {
    let conn = migrated_connection();
    sign_in(
        &conn,
        "github",
        "26405363",
        "ken-matsui",
        "2026-07-15T00:00:00.000Z",
    );
    let (user_id, _, _) = resolve(&conn, "github", "26405363").expect("identity resolves");
    // A second account's sign-in leaves a different, newer
    // `last_insert_rowid()` behind on the connection; the repeat
    // sign-in's conflict arm must discard it, not rebind the identity.
    sign_in(
        &conn,
        "github",
        "583231",
        "octocat",
        "2026-07-15T01:00:00.000Z",
    );
    sign_in(
        &conn,
        "github",
        "26405363",
        "renamed",
        "2026-07-15T02:00:00.000Z",
    );
    assert_eq!(count(&conn, "users"), 2);
    assert_eq!(count(&conn, "identities"), 2);
    let (resolved_id, login, _) = resolve(&conn, "github", "26405363").expect("identity resolves");
    assert_eq!(resolved_id, user_id);
    assert_eq!(login, "renamed");
}

#[test]
fn distinct_accounts_get_distinct_users() {
    let conn = migrated_connection();
    sign_in(
        &conn,
        "github",
        "26405363",
        "ken-matsui",
        "2026-07-15T00:00:00.000Z",
    );
    sign_in(
        &conn,
        "github",
        "583231",
        "octocat",
        "2026-07-15T01:00:00.000Z",
    );
    let (first, ..) = resolve(&conn, "github", "26405363").expect("first identity");
    let (second, ..) = resolve(&conn, "github", "583231").expect("second identity");
    assert_ne!(first, second);
    assert_eq!(count(&conn, "users"), 2);
}

#[test]
fn identities_are_keyed_by_provider_and_account_never_login() {
    let conn = migrated_connection();
    sign_in(
        &conn,
        "github",
        "26405363",
        "ken-matsui",
        "2026-07-15T00:00:00.000Z",
    );
    // The same numeric account id under another provider is a distinct
    // identity and a distinct user (the schema is provider-neutral even
    // though policy admits only GitHub today)...
    sign_in(
        &conn,
        "other",
        "26405363",
        "ken-matsui",
        "2026-07-15T01:00:00.000Z",
    );
    // ...and a login reused by a different account never merges
    // identities: logins are display-only snapshots.
    sign_in(
        &conn,
        "github",
        "583231",
        "ken-matsui",
        "2026-07-15T02:00:00.000Z",
    );
    assert_eq!(count(&conn, "users"), 3);
    assert_eq!(count(&conn, "identities"), 3);
    let (github_user, ..) = resolve(&conn, "github", "26405363").expect("github identity");
    let (other_user, ..) = resolve(&conn, "other", "26405363").expect("other-provider identity");
    let (reused_login_user, ..) =
        resolve(&conn, "github", "583231").expect("reused-login identity");
    assert_ne!(github_user, other_user);
    assert_ne!(github_user, reused_login_user);
}

#[test]
fn an_unknown_identity_resolves_to_nothing() {
    // The post-wipe ghost: a sealed session whose identity row is gone
    // answers as no user at all.
    let conn = migrated_connection();
    sign_in(
        &conn,
        "github",
        "26405363",
        "ken-matsui",
        "2026-07-15T00:00:00.000Z",
    );
    assert_eq!(resolve(&conn, "github", "583231"), None);
}
