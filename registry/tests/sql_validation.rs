//! Host-target schema validation for every SQL statement the Worker
//! executes: apply every file in `migrations/` in filename order (a
//! from-zero migration test in itself), then `prepare` each statement
//! in [`cabin_registry_worker::sql::ALL`] against the migrated schema.
//! `prepare` validates syntax and table/column existence without
//! executing and accepts D1's `?N` placeholders, so a typo, a wrong
//! column name, or schema drift fails here instead of in production -
//! the assurance an ORM's typed columns would give at compile time,
//! without one (`docs/architecture.md`, "Why no ORM").
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
fn migrated_connection() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
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
