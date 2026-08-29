//! Executed-semantics tests for `src/sql/downloads.rs`: download
//! counting and the public stats totals.

use cabin_registry_worker::sql;

use crate::common::{migrated_connection, seed_scope_collision};

/// The download counter's guard lives inside the statement: `prepare`
/// cannot check that only verified rows count or that the increment
/// stays within its scope, so both are executed here.
#[test]
fn download_counting_is_verified_only_and_scope_isolated() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);

    let downloads = |scope: &str| -> i64 {
        conn.query_row(
            "SELECT downloads FROM versions WHERE scope = ?1 AND name = 'pkg'",
            [scope],
            |row| row.get(0),
        )
        .expect("downloads column")
    };

    // A batched flush of two downloads counts; the identical
    // (name, version) under the other scope - pending there - stays
    // untouched.
    let changed = conn
        .execute(
            sql::ADD_VERSION_DOWNLOADS,
            rusqlite::params!["alpha", "pkg", "1.0.0", 2],
        )
        .expect("add verified downloads");
    assert_eq!(changed, 1);
    assert_eq!(downloads("alpha"), 2);
    assert_eq!(downloads("beta"), 0);

    // A pending row never counts (the verifier's fetch), and neither
    // does an unknown triple.
    for (scope, name, version) in [("beta", "pkg", "1.0.0"), ("ghost", "pkg", "1.0.0")] {
        let changed = conn
            .execute(
                sql::ADD_VERSION_DOWNLOADS,
                rusqlite::params![scope, name, version, 1],
            )
            .expect("guarded increment");
        assert_eq!(changed, 0, "scope: {scope}");
    }
    assert_eq!(downloads("beta"), 0);

    // Yanked versions stay downloadable and keep counting.
    conn.execute(
        sql::SET_VERSION_YANKED,
        rusqlite::params![1, "alpha", "pkg", "1.0.0"],
    )
    .expect("yank alpha");
    conn.execute(
        sql::ADD_VERSION_DOWNLOADS,
        rusqlite::params!["alpha", "pkg", "1.0.0", 1],
    )
    .expect("increment yanked download");
    assert_eq!(downloads("alpha"), 3);
}

/// The stats totals' semantics - the verified-only filter and the
/// distinct-canonical-name package count - are invisible to `prepare`,
/// so they are executed here.
#[test]
fn registry_stats_totals_are_verified_only_and_name_distinct() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    // A second verified version under alpha/pkg and a verified version
    // under beta/pkg: the same `pkg` name part under two scopes is two
    // distinct canonical packages (a `COUNT(DISTINCT name)` regression
    // would collapse them). beta/pkg@1.0.0 stays pending and gets a
    // nonzero counter written directly, so a dropped verified filter
    // would surface in every one of the three totals.
    conn.execute_batch(
        "INSERT INTO versions (scope, name, version, downloads)
           VALUES ('alpha', 'pkg', '1.1.0', 5), ('beta', 'pkg', '2.0.0', 7);
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json, \
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'pkg', '1.1.0', 'cc', 'cc', '{}', '2026-07-15T01:00:00.000Z', 10, 2, 'verified'),
                  ('beta', 'pkg', '2.0.0', 'dd', 'dd', '{}', '2026-07-15T02:00:00.000Z', 10, 2, 'verified');
         UPDATE versions SET downloads = 100 WHERE scope = 'beta' AND version = '1.0.0';",
    )
    .expect("seed verified versions and a pending counter");
    conn.execute(
        sql::ADD_VERSION_DOWNLOADS,
        rusqlite::params!["alpha", "pkg", "1.0.0", 2],
    )
    .expect("add verified downloads");

    let (packages, versions, downloads): (i64, i64, i64) = conn
        .query_row(sql::REGISTRY_STATS, [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("registry stats totals");
    assert_eq!(packages, 2, "alpha/pkg and beta/pkg are distinct packages");
    assert_eq!(versions, 3, "the pending beta/pkg@1.0.0 must not count");
    assert_eq!(downloads, 14, "2 + 5 + 7; the pending row's 100 must not");
}
