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
//!
//! The executed-semantics tests live in one module per `src/sql/`
//! statement module; the every-statement guarantee stays here, global.
#![cfg(not(target_arch = "wasm32"))]

mod auth;
mod backup;
mod common;
mod downloads;
mod packages;
mod scopes;
mod trustpub;

use cabin_registry_worker::sql;

use crate::common::migrated_connection;

/// Statements `rusqlite` cannot prepare because they need a D1-only
/// construct. Deliberately empty - D1 speaks `SQLite`'s dialect for
/// everything the service executes today - and every future entry must
/// carry a rationale comment plus its own dedicated test.
const EXCLUDED_D1_ONLY: &[&str] = &[];

#[test]
fn every_executed_statement_prepares_against_the_migrated_schema() {
    let conn = migrated_connection();
    for statement in sql::ALL.iter().flat_map(|group| group.iter()) {
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
            sql::ALL.iter().any(|group| group.contains(excluded)),
            "EXCLUDED_D1_ONLY entry is not in sql::ALL: {excluded}"
        );
    }
}
