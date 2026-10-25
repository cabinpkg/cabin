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
//! The executed-semantics tests are grouped by `src/sql/` statement
//! domain; the every-statement guarantee stays here, global.
#![cfg(not(target_arch = "wasm32"))]

mod auth;
mod backup;
mod common;
mod downloads;
mod packages;
mod quota;
mod scopes;
mod trustpub;

use cabin_registry_worker::sql;

use crate::common::migrated_connection;

#[test]
fn every_executed_statement_prepares_against_the_migrated_schema() {
    let conn = migrated_connection();
    for statement in sql::ALL.iter().flat_map(|group| group.iter()) {
        if let Err(err) = conn.prepare(statement) {
            panic!("statement does not prepare against the migrated schema: {err}\n  {statement}");
        }
    }
}
