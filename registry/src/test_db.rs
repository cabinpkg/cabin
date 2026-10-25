//! The migrated in-memory database the D1-schema tests execute SQL
//! against.
//!
//! One source file, two compilations: `lib.rs` declares it for the unit
//! tests only (`cfg(test)`), and `tests/sql_validation/common.rs`
//! includes it by `#[path]`, because an integration test cannot reach
//! the library's test-only items. The library's real build never sees
//! it, so `rusqlite` stays a host dev-dependency.

use std::fs;
use std::path::Path;

/// A fresh in-memory database with every migration applied, oldest
/// first. Foreign keys are enforced, as they are on D1.
pub fn migrated_connection() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    conn.pragma_update(None, "foreign_keys", true)
        .expect("enable foreign_keys");
    // D1 parity: D1 caps LIKE/GLOB patterns at 50 bytes at evaluation
    // (bundled SQLite defaults to 50000), so a long pattern - say a
    // fixed-width GLOB in a CHECK - passes every host test while
    // failing every INSERT in production. Pinning the limit here makes
    // every test on this fixture evaluate patterns under D1's rules.
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
