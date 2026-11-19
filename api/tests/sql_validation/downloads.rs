//! Executed-semantics tests for `src/sql/downloads.rs`: download
//! counting and the public stats totals.

use cabin_api_worker::sql;

use crate::common::{archive_verified, migrated_connection, seed_scope_collision};

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
        rusqlite::params![
            1,
            "alpha",
            "pkg",
            "1.0.0",
            "2026-07-16T00:00:00.000Z",
            30,
            2,
            90
        ],
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
    archive_verified(&conn);
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

/// A package's total sums its served versions only, and its days run
/// from the later of where counting started and the first of the 90
/// kept days to today, a day without downloads 0 and another package's
/// downloads never added.
#[test]
fn a_packages_downloads_are_its_served_versions_and_its_counted_days() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    conn.execute_batch(
        "INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'other', '2026-07-15T00:00:00.000Z', 2);
         INSERT INTO versions (scope, name, version, downloads) VALUES ('alpha', 'pkg', '1.1.0', 5),
                  ('alpha', 'other', '1.0.0', 40);
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json, \
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'pkg', '1.1.0', 'cc', 'cc', '{}', '2026-07-15T01:00:00.000Z', 10, 2, 'verified'),
                  ('alpha', 'other', '1.0.0', 'dd', 'dd', '{}', '2026-07-15T01:00:00.000Z', 10, 2, 'verified');
         UPDATE versions SET downloads = 2 WHERE scope = 'alpha' AND name = 'pkg' AND version = '1.0.0';
         UPDATE versions SET downloads = 100 WHERE scope = 'beta';
         INSERT INTO version_daily_downloads (scope, name, version, day, downloads)
           VALUES ('alpha', 'pkg', '1.0.0', '2026-08-21', 11),
                  ('alpha', 'pkg', '1.0.0', '2026-08-22', 13),
                  ('alpha', 'pkg', '1.0.0', '2026-09-30', 9),
                  ('alpha', 'pkg', '1.0.0', '2026-10-01', 1),
                  ('alpha', 'pkg', '1.0.0', '2026-11-19', 2),
                  ('alpha', 'pkg', '1.1.0', '2026-11-19', 3),
                  ('alpha', 'other', '1.0.0', '2026-11-19', 40),
                  ('beta', 'pkg', '1.0.0', '2026-11-19', 4);",
    )
    .expect("seed the versions and their days");
    archive_verified(&conn);

    let total = |scope: &str, name: &str| -> (i64, i64) {
        conn.query_row(sql::PACKAGE_DOWNLOADS, [scope, name], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("the package total")
    };
    assert_eq!(total("alpha", "pkg"), (2, 7));
    assert_eq!(total("alpha", "other"), (1, 40));
    assert_eq!(total("alpha", "missing"), (0, 0));
    assert_eq!(
        total("beta", "pkg"),
        (0, 0),
        "a pending version is not served"
    );
    assert_eq!(total("ghost", "pkg"), (0, 0));

    let days = |start: &str| -> Vec<(String, i64)> {
        conn.execute(sql::UPSERT_META, ["downloads_cutoff", start])
            .expect("the start");
        let mut statement = conn.prepare(sql::PACKAGE_DAILY_DOWNLOADS).expect("prepare");
        statement
            .query_map(["alpha", "pkg", "2026-11-19T12:00:00.000Z"], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("the days")
    };
    let since_start = days("2026-10-01T06:00:00.000Z");
    assert_eq!(since_start.len(), 50, "2026-10-01 to 2026-11-19");
    assert_eq!(since_start[0], ("2026-10-01".to_owned(), 1));
    assert_eq!(since_start[1], ("2026-10-02".to_owned(), 0));
    assert_eq!(since_start[49], ("2026-11-19".to_owned(), 5));
    assert_eq!(since_start.iter().map(|(_, n)| n).sum::<i64>(), 6);

    let kept = days("2026-01-01T00:00:00.000Z");
    assert_eq!(kept.len(), 90);
    assert_eq!(kept[0], ("2026-08-22".to_owned(), 13));
    assert_eq!(kept.iter().map(|(_, n)| n).sum::<i64>(), 13 + 9 + 1 + 5);

    assert!(
        days("2026-11-20T00:00:00.000Z").is_empty(),
        "a start after today"
    );
}
