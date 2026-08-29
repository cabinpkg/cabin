//! Executed-semantics tests for `src/sql/packages.rs` (and the meta
//! accounting its write path drives): publish, yank, verification,
//! search, and the revision guards.

use cabin_registry_worker::sql;

use crate::common::{migrated_connection, seed_scope_collision};

/// The scoped statements executed against colliding `(name, version)`
/// rows: `prepare` alone cannot catch a missing scope predicate or a
/// wrong bind order, so this pins per-statement isolation between
/// scopes. (The wasm glue's end-to-end flow is `cargo registry-smoke`'s
/// job; this covers the SQL itself.)
#[test]
#[allow(clippy::too_many_lines)] // one seeded scenario walked through every scoped statement
fn scoped_statements_never_cross_scopes() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);

    // Reads address exactly one scope's row.
    let checksum: String = conn
        .query_row(
            sql::ARTIFACT_BY_REVISION,
            rusqlite::params!["alpha", "pkg", "1.0.0", "aa"],
            |row| row.get(0),
        )
        .expect("alpha artifact row");
    assert_eq!(checksum, "aa");
    let verified: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM ({})",
                sql::CURRENT_REVISIONS_BY_PACKAGE
            ),
            rusqlite::params!["beta", "pkg"],
            |row| row.get(0),
        )
        .expect("beta verified count");
    assert_eq!(verified, 0, "beta's row is pending, alpha's must not leak");

    // Membership is per scope; a scope with no members answers like a
    // missing scope.
    for (scope, expected) in [("alpha", 1), ("beta", 0), ("ghost", 0)] {
        let members: i64 = conn
            .query_row(sql::SCOPE_MEMBERSHIP, rusqlite::params![scope, 2], |row| {
                row.get(0)
            })
            .expect("membership count");
        assert_eq!(members, expected, "scope: {scope}");
    }

    // Quota counts key on (scope, name).
    let versions_today: i64 = conn
        .query_row(
            sql::COUNT_PACKAGE_VERSIONS_SINCE,
            rusqlite::params!["alpha", "pkg", "2026-07-15"],
            |row| row.get(0),
        )
        .expect("alpha versions since");
    assert_eq!(versions_today, 1);

    // Mutations only touch the addressed scope.
    let changed = conn
        .execute(
            sql::SET_VERSION_YANKED,
            rusqlite::params![1, "alpha", "pkg", "1.0.0"],
        )
        .expect("yank alpha");
    assert_eq!(changed, 1);
    let beta_yanked: i64 = conn
        .query_row(
            sql::VERSION_YANK_STATE,
            rusqlite::params!["beta", "pkg", "1.0.0"],
            |row| row.get(0),
        )
        .expect("beta yank state");
    assert_eq!(beta_yanked, 0, "yanking alpha/pkg must not touch beta/pkg");
    let changed = conn
        .execute(
            sql::MARK_REVISION_VERIFIED,
            rusqlite::params![
                "2026-07-15T01:00:00.000Z",
                "beta",
                "pkg",
                "1.0.0",
                "bb",
                "2026-07-15T00:00:00.000Z",
                "bb"
            ],
        )
        .expect("verify beta");
    assert_eq!(changed, 1);

    // The rejection refund's guards address one scope's row: refunding
    // with the wrong scope bound must be a no-op even though the other
    // scope holds the same (name, version).
    conn.execute(
        "UPDATE revisions SET verification = 'pending' WHERE scope = 'beta'",
        [],
    )
    .expect("reset beta to pending");
    conn.execute(
        sql::REFUND_STORED_BYTES_ON_REJECTION,
        rusqlite::params![
            "bb",
            "alpha",
            "pkg",
            "1.0.0",
            20,
            "2026-07-15T00:00:00.000Z",
            "bb"
        ],
    )
    .expect("refund bound to the wrong scope");
    let stored: String = conn
        .query_row(sql::META_VALUE, ["total_stored_bytes"], |row| row.get(0))
        .expect("stored bytes");
    assert_eq!(stored, "30", "a wrong-scope refund must not fire");
    conn.execute(
        sql::REFUND_STORED_BYTES_ON_REJECTION,
        rusqlite::params![
            "bb",
            "beta",
            "pkg",
            "1.0.0",
            20,
            "2026-07-15T00:00:00.000Z",
            "bb"
        ],
    )
    .expect("refund bound to the right scope");
    let stored: String = conn
        .query_row(sql::META_VALUE, ["total_stored_bytes"], |row| row.get(0))
        .expect("stored bytes");
    assert_eq!(stored, "10", "the right-scope refund fires exactly once");
}

/// Seeds one user plus the packages and versions the search and
/// reverse-dependency statements walk: a target package with two
/// verified versions, a pending-only lookalike, an underscore/plain
/// name pair for the literal-match check, and dependents in every
/// lifecycle state.
fn seed_search_corpus(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (2, '2026-07-18T00:00:00.000Z');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '1', '2026-07-18T00:00:00.000Z'),
                  ('beta', 'github', '2', '2026-07-18T00:00:00.000Z'),
                  ('gabime', 'github', '3', '2026-07-18T00:00:00.000Z'),
                  ('acme', 'github', '4', '2026-07-18T00:00:00.000Z');
         INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'target', '2026-07-18T00:00:00.000Z', 2),
                  ('beta', 'target-pending', '2026-07-18T00:00:00.000Z', 2),
                  ('alpha', 'my_pkg', '2026-07-18T00:00:00.000Z', 2),
                  ('alpha', 'myxpkg', '2026-07-18T00:00:00.000Z', 2),
                  ('gabime', 'spdlog', '2026-07-18T00:00:00.000Z', 2),
                  ('acme', 'pending-dep', '2026-07-18T00:00:00.000Z', 2),
                  ('acme', 'rejected-dep', '2026-07-18T00:00:00.000Z', 2),
                  ('acme', 'bare-dep', '2026-07-18T00:00:00.000Z', 2),
                  ('acme', 'dev-dep', '2026-07-18T00:00:00.000Z', 2);
         INSERT INTO versions (scope, name, version, yanked, downloads)
           VALUES ('alpha', 'target', '1.0.0', 1, 3),
                  ('alpha', 'target', '1.1.0', 0, 4),
                  ('beta', 'target-pending', '1.0.0', 0, 100),
                  ('alpha', 'my_pkg', '1.0.0', 0, 0),
                  ('alpha', 'myxpkg', '1.0.0', 0, 0),
                  ('gabime', 'spdlog', '1.13.0', 1, 0),
                  ('gabime', 'spdlog', '1.14.0', 0, 0),
                  ('acme', 'pending-dep', '1.0.0', 0, 0),
                  ('acme', 'rejected-dep', '1.0.0', 0, 0),
                  ('acme', 'bare-dep', '1.0.0', 0, 0),
                  ('acme', 'dev-dep', '1.0.0', 0, 0);
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           VALUES
           ('alpha', 'target', '1.0.0', 'c01', 'c01', '{\"dependencies\":{}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('alpha', 'target', '1.1.0', 'c02', 'c02', '{\"dependencies\":{}}',
            '2026-07-18T01:00:00.000Z', 10, 2, 'verified'),
           ('beta', 'target-pending', '1.0.0', 'c03', 'c03', '{\"dependencies\":{}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'pending'),
           ('alpha', 'my_pkg', '1.0.0', 'c04', 'c04', '{\"dependencies\":{}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('alpha', 'myxpkg', '1.0.0', 'c05', 'c05', '{\"dependencies\":{}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('gabime', 'spdlog', '1.13.0', 'c06', 'c06',
            '{\"dependencies\":{\"alpha/target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('gabime', 'spdlog', '1.14.0', 'c07', 'c07',
            '{\"dependencies\":{\"alpha/target\":{\"version\":\"^1\",\"optional\":true}}}',
            '2026-07-18T01:00:00.000Z', 10, 2, 'verified'),
           ('acme', 'pending-dep', '1.0.0', 'c08', 'c08',
            '{\"dependencies\":{\"alpha/target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'pending'),
           ('acme', 'rejected-dep', '1.0.0', 'c09', 'c09',
            '{\"dependencies\":{\"alpha/target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'rejected'),
           ('acme', 'bare-dep', '1.0.0', 'c10', 'c10',
            '{\"dependencies\":{\"target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('acme', 'dev-dep', '1.0.0', 'c11', 'c11',
            '{\"dependencies\":{},\"dev-dependencies\":{\"alpha/target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified');",
    )
    .expect("seed the search corpus");
}

/// Runs [`sql::SEARCH_VERIFIED_VERSIONS`] with the term the session
/// plane would bind (validated and ASCII-lowercased by the real
/// parser), returning the matched canonical names (sorted for the
/// assertion; the statement itself returns rows in table order and
/// the host ranks them).
fn search_names(conn: &rusqlite::Connection, term: &str) -> Vec<String> {
    let query = cabin_registry_worker::user_api::parse_search_query(Some(term))
        .expect("a valid search term");
    let mut statement = conn
        .prepare(sql::SEARCH_VERIFIED_VERSIONS)
        .expect("prepare search");
    let mut names: Vec<String> = statement
        .query_map([query], |row| {
            Ok(format!(
                "{}/{}@{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?
            ))
        })
        .expect("run search")
        .collect::<Result<_, _>>()
        .expect("search rows");
    names.sort();
    names
}

/// The search statement's semantics - the verified-only filter and
/// `instr`'s literal, byte-exact matching (the reason it is not a
/// `LIKE`; see the statement's doc) - are invisible to `prepare`, so
/// they are executed here with the real term parser.
#[test]
fn search_rows_are_verified_only_and_terms_stay_literal() {
    let conn = migrated_connection();
    seed_search_corpus(&conn);

    // Substring match over verified rows only: the pending lookalike
    // carries 100 downloads and must not exist for search, however
    // the host would rank it.
    assert_eq!(
        search_names(&conn, "target"),
        ["alpha/target@1.0.0", "alpha/target@1.1.0"]
    );
    // Case-insensitive by normalization: `instr` compares bytes, so
    // the parser's ASCII fold is what makes uppercase input match.
    assert_eq!(search_names(&conn, "TARGET").len(), 2);

    // Wildcard metacharacters are literals to `instr`: `_` matches
    // only itself (a `LIKE` would also match `myxpkg`), and `%` / `\`
    // - impossible in names - match nothing instead of everything.
    assert_eq!(search_names(&conn, "my_pkg"), ["alpha/my_pkg@1.0.0"]);
    assert_eq!(search_names(&conn, "my%"), Vec::<String>::new());
    assert_eq!(search_names(&conn, "my\\pkg"), Vec::<String>::new());
    // A maximum-length term executes and simply matches nothing -
    // there is no pattern-length ceiling to trip (D1 caps LIKE
    // patterns at 50 bytes; instr takes the term as a plain value).
    assert_eq!(search_names(&conn, &"x".repeat(64)), Vec::<String>::new());
}

/// The reverse-dependency walk's semantics - the verified-only
/// filter, the runtime-map-only `json_each` path, and the exact
/// canonical-key match - are invisible to `prepare`, so they are
/// executed here.
#[test]
fn reverse_dependencies_match_exact_scoped_keys_on_verified_rows_only() {
    let conn = migrated_connection();
    seed_search_corpus(&conn);

    let dependents = |key: &str| -> Vec<String> {
        let mut statement = conn
            .prepare(sql::REVERSE_DEPENDENCIES)
            .expect("prepare reverse dependencies");
        let mut rows: Vec<String> = statement
            .query_map([key], |row| {
                Ok(format!(
                    "{}/{}@{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?
                ))
            })
            .expect("run reverse dependencies")
            .collect::<Result<_, _>>()
            .expect("dependent rows");
        rows.sort();
        rows
    };

    // Both verified spdlog versions carry the key - the bare-string
    // and the rich-table entry shapes alike, and the yanked 1.13.0
    // still counts (yanked versions stay resolvable). The pending and
    // rejected dependents never count, the dev-dependencies map is
    // not consulted, and the bare `target` key contributes no edge:
    // reverse dependencies are defined over registry-resolvable
    // references, and a bare key cannot denote a hosted package.
    // Publish rejects bare dependency keys outright
    // (src/publish.rs), so this row models pre-enforcement data and
    // pins the query-side contract regardless.
    assert_eq!(
        dependents("alpha/target"),
        ["gabime/spdlog@1.13.0", "gabime/spdlog@1.14.0"]
    );
    // No probe can reach a bare key either: the route's key is always
    // `<scope>/<name>` (both segments grammar-validated), and a bare
    // stored key never contains the `/`.
    assert_eq!(dependents("acme/target"), Vec::<String>::new());

    // The visibility gate the session routes share: the pending-only
    // package is invisible, the verified one is not.
    let visible = |scope: &str, name: &str| -> i64 {
        conn.query_row(
            sql::HAS_VERIFIED_VERSION,
            rusqlite::params![scope, name],
            |row| row.get(0),
        )
        .expect("visibility count")
    };
    assert_eq!(visible("alpha", "target"), 2);
    assert_eq!(visible("beta", "target-pending"), 0);
    assert_eq!(visible("ghost", "none"), 0);

    // The detail rows are the verified subset with their stored
    // metadata; the pending-only package composes nothing.
    let details = |scope: &str, name: &str| -> i64 {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM ({})", sql::VERIFIED_VERSION_DETAILS),
            rusqlite::params![scope, name],
            |row| row.get(0),
        )
        .expect("detail count")
    };
    assert_eq!(details("alpha", "target"), 2);
    assert_eq!(details("beta", "target-pending"), 0);
}

/// The `-`/`_` twin fold, its per-scope and self-exclusion rules, and
/// the matching in-batch guards on the publish inserts are invisible
/// to `prepare`, so they are executed here.
#[test]
fn twin_guard_blocks_dash_underscore_twins_within_a_scope() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);

    let twins = |scope: &str, name: &str| -> i64 {
        conn.query_row(
            sql::TWIN_PACKAGE_EXISTS,
            rusqlite::params![scope, name],
            |row| row.get(0),
        )
        .expect("twin count")
    };
    conn.execute(
        "INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'foo-bar', '2026-07-15T00:00:00.000Z', 2)",
        [],
    )
    .expect("seed the twinnable package");
    assert_eq!(twins("alpha", "foo_bar"), 1, "the fold sees the twin");
    assert_eq!(twins("alpha", "foo-bar"), 0, "a name is not its own twin");
    assert_eq!(
        twins("alpha", "foobar"),
        0,
        "folding interchanges, never removes"
    );
    assert_eq!(twins("beta", "foo_bar"), 0, "the collision is per scope");

    // The guarded inserts suppress a twin (both statements, zero
    // changes), pass an unrelated name, and keep accepting the
    // existing name itself - the self exclusion.
    let insert_package = |scope: &str, name: &str| -> usize {
        conn.execute(
            sql::INSERT_PACKAGE,
            rusqlite::params![scope, name, "2026-07-15T01:00:00.000Z", 2],
        )
        .expect("guarded package insert")
    };
    // The version-row + revision pair, exactly as one publish batch
    // runs them; the returned count is the revision insert's (the
    // twin signal).
    let insert_version = |scope: &str, name: &str, version: &str| -> usize {
        conn.execute(
            sql::INSERT_VERSION_ROW,
            rusqlite::params![scope, name, version],
        )
        .expect("guarded version-row insert");
        conn.execute(
            sql::INSERT_REVISION,
            rusqlite::params![
                scope,
                name,
                version,
                "ee",
                "ee",
                "{}",
                "2026-07-15T01:00:00.000Z",
                10,
                2,
                0
            ],
        )
        .expect("guarded revision insert")
    };
    assert_eq!(insert_package("alpha", "foo_bar"), 0);
    assert_eq!(insert_version("alpha", "foo_bar", "1.0.0"), 0);
    assert_eq!(insert_package("alpha", "other"), 1);
    assert_eq!(insert_version("alpha", "other", "1.0.0"), 1);
    assert_eq!(
        insert_package("alpha", "foo-bar"),
        0,
        "OR IGNORE on the existing row"
    );
    assert_eq!(insert_version("alpha", "foo-bar", "2.0.0"), 1);

    // Legacy twins (possible only through operator surgery or restored
    // data) keep receiving new versions: the twin policy gates package
    // creation, and the version guard only requires its own package
    // row.
    conn.execute(
        "INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'foo_bar', '2026-07-15T00:00:00.000Z', 2)",
        [],
    )
    .expect("seed the legacy twin directly");
    assert_eq!(insert_version("alpha", "foo-bar", "3.0.0"), 1);
    assert_eq!(insert_version("alpha", "foo_bar", "3.0.0"), 1);
}

/// The publish accounting decides before the insert, against the same
/// pre-insert state and under the same guards - invisible to
/// `prepare`, so the batches' statements are executed here in glue
/// order.  Three losing shapes must add nothing: a suppressed twin, a
/// racing byte-identical publish of the same revision key (which must
/// lose by zero changed rows, not by aborting the batch on the
/// primary key), and a shared blob already counted by another live
/// row.
#[test]
fn publish_accounting_mirrors_the_insert_guards() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let stored = |conn: &rusqlite::Connection| -> String {
        conn.query_row(
            "SELECT value FROM meta WHERE key = 'total_stored_bytes'",
            [],
            |row| row.get(0),
        )
        .expect("stored bytes")
    };
    let account = |scope: &str, name: &str, version: &str, checksum: &str, size: i64| {
        conn.execute(
            sql::COUNT_STORED_BYTES_ON_PUBLISH,
            rusqlite::params![checksum, size, scope, name, version, checksum, "{}", 0],
        )
        .expect("accounting upsert");
    };
    let insert = |scope: &str, name: &str, version: &str, stamp: &str| -> usize {
        conn.execute(
            sql::INSERT_REVISION,
            rusqlite::params![scope, name, version, "ee", "ee", "{}", stamp, 40, 2, 0],
        )
        .expect("guarded revision insert")
    };

    // The winner's batch: package + version land, no live reference
    // to checksum 'ee' exists yet, so the accounting counts the bytes
    // and the insert applies.
    conn.execute(
        sql::INSERT_PACKAGE,
        rusqlite::params!["alpha", "foo-bar", "2026-07-15T01:00:00.000Z", 2],
    )
    .expect("winner package");
    conn.execute(
        sql::INSERT_VERSION_ROW,
        rusqlite::params!["alpha", "foo-bar", "1.0.0"],
    )
    .expect("winner version row");
    account("alpha", "foo-bar", "1.0.0", "ee", 40);
    assert_eq!(
        insert("alpha", "foo-bar", "1.0.0", "2026-07-15T01:00:00.000Z"),
        1
    );
    assert_eq!(stored(&conn), "70", "30 seeded + the winner's 40");

    // A racing byte-identical publish of the same version: its
    // preflight raced the winner's commit, so its batch still runs -
    // the same-key guard suppresses the insert cleanly (zero rows, no
    // primary-key abort) and the mirrored accounting adds nothing.
    account("alpha", "foo-bar", "1.0.0", "ee", 40);
    assert_eq!(
        insert("alpha", "foo-bar", "1.0.0", "2026-07-15T01:00:01.000Z"),
        0,
        "the same-key race must lose by the guard, not the primary key"
    );
    assert_eq!(stored(&conn), "70", "the racing loser must add nothing");

    // The twin's batch: package and version row suppressed, so the
    // version-exists conjunct refuses both the count and the insert.
    assert_eq!(
        conn.execute(
            sql::INSERT_PACKAGE,
            rusqlite::params!["alpha", "foo_bar", "2026-07-15T01:00:01.000Z", 2],
        )
        .expect("twin package"),
        0
    );
    assert_eq!(
        conn.execute(
            sql::INSERT_VERSION_ROW,
            rusqlite::params!["alpha", "foo_bar", "1.0.0"],
        )
        .expect("twin version row"),
        0
    );
    account("alpha", "foo_bar", "1.0.0", "ee", 40);
    assert_eq!(
        insert("alpha", "foo_bar", "1.0.0", "2026-07-15T01:00:01.000Z"),
        0
    );
    assert_eq!(stored(&conn), "70", "the suppressed twin must add nothing");

    // A different version publishing the same bytes: the insert lands
    // (its own revision key), but the blob is already a live row's -
    // no double count.
    conn.execute(
        sql::INSERT_VERSION_ROW,
        rusqlite::params!["alpha", "foo-bar", "2.0.0"],
    )
    .expect("second version row");
    account("alpha", "foo-bar", "2.0.0", "ee", 40);
    assert_eq!(
        insert("alpha", "foo-bar", "2.0.0", "2026-07-15T01:00:02.000Z"),
        1
    );
    assert_eq!(stored(&conn), "70", "a shared blob is counted once");
}

/// The corpus listing's vetted flag and deterministic order are
/// invisible to `prepare`, so they are executed here.
#[test]
fn admin_packages_reports_names_and_vetted_flags_deterministically() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    // Only a verified version vets a name: alpha/pkg is vetted, the
    // pending-only beta/pkg, the versionless beta/arc, and - the load-
    // bearing case - the rejected-only alpha/zed are not (a rejection
    // never vets a name, or rejecting an abstained squat would exempt
    // that same name's next version from the advisories).
    conn.execute_batch(
        "INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'zed', '2026-07-15T00:00:00.000Z', 2),
                  ('beta', 'arc', '2026-07-15T00:00:00.000Z', 2);
         INSERT INTO versions (scope, name, version) VALUES ('alpha', 'zed', '1.0.0');
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'zed', '1.0.0', 'ff', 'ff', '{}', '2026-07-15T00:00:00.000Z', 10, 2, 'rejected');",
    )
    .expect("seed the rejected-only and versionless packages");

    let mut statement = conn.prepare(sql::ADMIN_PACKAGES).expect("prepare corpus");
    let rows: Vec<(String, String, i64)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("run corpus")
        .collect::<Result<_, _>>()
        .expect("corpus rows");
    assert_eq!(
        rows,
        [
            ("alpha".to_owned(), "pkg".to_owned(), 1),
            ("alpha".to_owned(), "zed".to_owned(), 0),
            ("beta".to_owned(), "arc".to_owned(), 0),
            ("beta".to_owned(), "pkg".to_owned(), 0),
        ]
    );
}

/// The `new-revision` opt-in guard lives inside [`sql::INSERT_REVISION`]
/// itself, so two concurrent publishes cannot both slip past a stale
/// preflight read: without the flag, an insert beside a live
/// different-bytes revision changes zero rows; with it, the respin
/// lands and the superseded revision stays.
#[test]
fn revision_opt_in_guard_is_enforced_inside_the_insert() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);

    let insert = |revision: &str, checksum: &str, opt_in: i64| -> usize {
        conn.execute(
            sql::INSERT_REVISION,
            rusqlite::params![
                "alpha",
                "pkg",
                "1.0.0",
                revision,
                checksum,
                "{}",
                "2026-07-15T02:00:00.000Z",
                10,
                2,
                opt_in
            ],
        )
        .expect("guarded revision insert")
    };

    // alpha/pkg@1.0.0 already has verified revision 'aa': different
    // bytes without the opt-in are suppressed by the guard.
    assert_eq!(insert("a2", "a2", 0), 0);
    // The opt-in lands the respin; the original revision remains.
    assert_eq!(insert("a2", "a2", 1), 1);
    let revisions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM revisions WHERE scope = 'alpha' AND name = 'pkg'",
            [],
            |row| row.get(0),
        )
        .expect("revision count");
    assert_eq!(revisions, 2);

    // Once every live sibling shares the incoming bytes... it cannot:
    // distinct revisions imply distinct bytes. But a version whose only
    // revisions are rejected accepts a fresh one without the flag - the
    // recovery path.
    conn.execute(
        "UPDATE revisions SET verification = 'rejected' WHERE scope = 'alpha'",
        [],
    )
    .expect("reject both revisions");
    assert_eq!(insert("a3", "a3", 0), 1);

    // The revival guard mirrors the insert's: an unflagged revival of a
    // rejected revision beside the now-live different-bytes 'a3' row is
    // suppressed; the flag lets it through.
    let revive = |revision: &str, checksum: &str, opt_in: i64| -> usize {
        conn.execute(
            sql::REVIVE_REJECTED_REVISION,
            rusqlite::params![
                "{}",
                "2026-07-15T03:00:00.000Z",
                2,
                "alpha",
                "pkg",
                opt_in,
                "1.0.0",
                revision,
                checksum
            ],
        )
        .expect("guarded revival")
    };
    assert_eq!(revive("aa", "aa", 0), 0);
    assert_eq!(revive("aa", "aa", 1), 1);
}

/// The `links` conjunct inside the transactional guards is one-way:
/// a respin may add a claim table beside link-less live siblings,
/// but once a live sibling carries one, a respin that changes or
/// omits it changes zero rows - opt-in or not.  The revival guard
/// mirrors the insert's, and the preflight's sibling read prefers a
/// links-bearing row so its diagnostic matches the transactional
/// truth.
#[test]
fn links_revision_guards_are_one_way_and_prefer_links_bearing_siblings() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);

    let insert = |revision: &str, metadata: &str| -> usize {
        conn.execute(
            sql::INSERT_REVISION,
            rusqlite::params![
                "alpha",
                "pkg",
                "1.0.0",
                revision,
                revision,
                metadata,
                "2026-07-15T02:00:00.000Z",
                10,
                2,
                1 // always opt in: the links rule must hold regardless
            ],
        )
        .expect("guarded revision insert")
    };

    // The seeded live revision 'aa' has metadata '{}' - no links.
    // Adding a claim table is a legal respin.
    let stamped = r#"{"links":{"z":"z"}}"#;
    assert_eq!(insert("b1", stamped), 1);

    // With a links-bearing live sibling, a respin that changes the
    // table, or omits it, is suppressed by the in-SQL conjunct.
    assert_eq!(insert("b2", r#"{"links":{"z":"zlib"}}"#), 0);
    assert_eq!(insert("b3", "{}"), 0);
    // A respin carrying the identical table still lands.
    assert_eq!(insert("b4", stamped), 1);

    // The preflight's sibling read prefers the links-bearing row,
    // even though the link-less 'aa' sorts first by revision id -
    // that row is the constraining one once it exists.
    let preferred: String = conn
        .query_row(
            sql::LIVE_REVISION_METADATA,
            rusqlite::params!["alpha", "pkg", "1.0.0"],
            |row| row.get(0),
        )
        .expect("live sibling metadata");
    assert_eq!(preferred, stamped);

    // The revival guard mirrors the insert's: reject 'b1', then try
    // to revive it with a mutated links table - suppressed; the
    // original table revives.
    conn.execute(
        "UPDATE revisions SET verification = 'rejected' WHERE revision = 'b1'",
        [],
    )
    .expect("reject b1");
    let revive = |metadata: &str| -> usize {
        conn.execute(
            sql::REVIVE_REJECTED_REVISION,
            rusqlite::params![
                metadata,
                "2026-07-15T03:00:00.000Z",
                2,
                "alpha",
                "pkg",
                1,
                "1.0.0",
                "b1",
                "b1"
            ],
        )
        .expect("guarded revival")
    };
    assert_eq!(revive(r#"{"links":{"z":"zlib"}}"#), 0);
    assert_eq!(revive(stamped), 1);
}

/// The storage-accounting statements repeat the revision guards'
/// conjuncts - links included - one-for-one: a respin the links rule
/// refuses must add zero bytes on both the publish and the revival
/// path, or the counter drifts from what was actually persisted.
#[test]
fn links_accounting_guards_mirror_the_revision_guards() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let stamped = r#"{"links":{"z":"z"}}"#;
    let stored = |conn: &rusqlite::Connection| -> String {
        conn.query_row(
            "SELECT value FROM meta WHERE key = 'total_stored_bytes'",
            [],
            |row| row.get(0),
        )
        .expect("stored bytes")
    };
    let account = |checksum: &str, metadata: &str| {
        conn.execute(
            sql::COUNT_STORED_BYTES_ON_PUBLISH,
            rusqlite::params![checksum, 40, "alpha", "pkg", "1.0.0", checksum, metadata, 1],
        )
        .expect("accounting upsert");
    };
    let insert = |checksum: &str, metadata: &str| -> usize {
        conn.execute(
            sql::INSERT_REVISION,
            rusqlite::params![
                "alpha",
                "pkg",
                "1.0.0",
                checksum,
                checksum,
                metadata,
                "2026-07-15T02:00:00.000Z",
                40,
                2,
                1
            ],
        )
        .expect("guarded revision insert")
    };

    // Absent -> present beside the link-less live 'aa': counts + lands.
    account("f1", stamped);
    assert_eq!(insert("f1", stamped), 1);
    assert_eq!(stored(&conn), "70", "30 seeded + the stamped respin's 40");

    // With a links-bearing live sibling, a changed or omitted table
    // is refused - and the mirrored accounting must add nothing.
    account("f2", r#"{"links":{"z":"zlib"}}"#);
    assert_eq!(insert("f2", r#"{"links":{"z":"zlib"}}"#), 0);
    account("f3", "{}");
    assert_eq!(insert("f3", "{}"), 0);
    assert_eq!(stored(&conn), "70", "refused respins must add nothing");

    // Revival path: reject the stamped row, then a revival whose
    // document mutates the table (against the still-live stamped
    // sibling 'f4') counts nothing and flips nothing; the matching
    // revival counts its bytes back and applies.
    account("f4", stamped);
    assert_eq!(insert("f4", stamped), 1);
    assert_eq!(stored(&conn), "110");
    conn.execute(
        "UPDATE revisions SET verification = 'rejected' WHERE revision = 'f1'",
        [],
    )
    .expect("reject f1");
    let revive_count = |metadata: &str| {
        conn.execute(
            sql::COUNT_STORED_BYTES_ON_REVIVAL,
            rusqlite::params!["alpha", "pkg", "1.0.0", "f1", "f1", 40, "f1", 1, metadata],
        )
        .expect("revival accounting");
    };
    let revive = |metadata: &str| -> usize {
        conn.execute(
            sql::REVIVE_REJECTED_REVISION,
            rusqlite::params![
                metadata,
                "2026-07-15T03:00:00.000Z",
                2,
                "alpha",
                "pkg",
                1,
                "1.0.0",
                "f1",
                "f1"
            ],
        )
        .expect("guarded revival")
    };
    revive_count(r#"{"links":{"z":"zlib"}}"#);
    assert_eq!(revive(r#"{"links":{"z":"zlib"}}"#), 0);
    assert_eq!(stored(&conn), "110", "a refused revival must add nothing");
    revive_count(stamped);
    assert_eq!(revive(stamped), 1);
    assert_eq!(
        stored(&conn),
        "150",
        "the applied revival re-counts its bytes"
    );
}

/// `current_revisions` is the single served-revision definition: per
/// verified version, the newest `published_at` wins (revision id as
/// the deterministic tie-break), pending respins never surface, and a
/// version with no verified revision has no row at all.
#[test]
fn current_revisions_view_serves_the_newest_verified_revision() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);

    let current = || -> Option<(String, String)> {
        conn.query_row(
            "SELECT revision, checksum FROM current_revisions
             WHERE scope = 'alpha' AND name = 'pkg' AND version = '1.0.0'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .expect("current revision")
    };
    assert_eq!(current(), Some(("aa".to_owned(), "aa".to_owned())));

    // A pending respin must not disturb what is served.
    conn.execute(
        "INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
         VALUES ('alpha', 'pkg', '1.0.0', 'a2', 'a2', '{}',
                 '2026-07-15T02:00:00.000Z', 10, 2, 'pending')",
        [],
    )
    .expect("pending respin");
    assert_eq!(current(), Some(("aa".to_owned(), "aa".to_owned())));

    // Once verified, the newer publish time takes over, and both
    // revisions stay listed for the composed document.
    conn.execute(
        "UPDATE revisions SET verification = 'verified' WHERE revision = 'a2'",
        [],
    )
    .expect("verify the respin");
    assert_eq!(current(), Some(("a2".to_owned(), "a2".to_owned())));
    let listed: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM ({})",
                sql::VERIFIED_REVISIONS_BY_PACKAGE
            ),
            rusqlite::params!["alpha", "pkg"],
            |row| row.get(0),
        )
        .expect("verified revision listing");
    assert_eq!(listed, 2, "the superseded revision stays fetchable");

    // Equal publish times fall back to the revision id: the greater id
    // wins, deterministically (`aa` > `a2` byte-wise).
    conn.execute(
        "UPDATE revisions SET published_at = '2026-07-15T02:00:00.000Z'",
        [],
    )
    .expect("collapse publish times");
    assert_eq!(current(), Some(("aa".to_owned(), "aa".to_owned())));
}

/// The yank-state read's result columns are a deserialization
/// contract: the wasm glue's `YankedRecord` names them field-for-
/// field, and `prepare` alone cannot catch a renamed result column
/// (the serde mismatch only surfaces at runtime, as a 500 on every
/// yank).  Executing the read pins both names and the verified bit's
/// semantics.
#[test]
fn version_yank_state_serves_the_yanked_and_verified_columns() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let mut statement = conn
        .prepare(sql::VERSION_YANK_STATE)
        .expect("prepare yank state");
    assert_eq!(statement.column_names(), ["yanked", "verified"]);
    let (yanked, verified): (i64, i64) = statement
        .query_row(rusqlite::params!["alpha", "pkg", "1.0.0"], |row| {
            Ok((row.get("yanked")?, row.get("verified")?))
        })
        .expect("alpha yank state");
    assert_eq!((yanked, verified), (0, 1), "alpha's revision is verified");
    let (_, verified): (i64, i64) = conn
        .query_row(
            sql::VERSION_YANK_STATE,
            rusqlite::params!["beta", "pkg", "1.0.0"],
            |row| Ok((row.get("yanked")?, row.get("verified")?)),
        )
        .expect("beta yank state");
    assert_eq!(verified, 0, "a pending-only version is not yankable");
}

/// The revival accounting's guards mirror the revival flip's - the
/// opt-in conjunct included - so a batch whose flip the opt-in guard
/// refuses adds nothing to `total_stored_bytes` even though both
/// statements commit in the same transaction.
#[test]
fn a_refused_revival_never_counts_its_bytes() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    // Reject alpha's revision, refund its 10 bytes, then land a live
    // different-bytes sibling: the revival of 'aa' now needs the
    // opt-in.
    conn.execute_batch(
        "UPDATE revisions SET verification = 'rejected' WHERE scope = 'alpha';
         UPDATE meta SET value = '20' WHERE key = 'total_stored_bytes';
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'pkg', '1.0.0', 'a2', 'a2', '{}',
                   '2026-07-15T02:00:00.000Z', 10, 2, 'pending');",
    )
    .expect("reject alpha and land a live sibling");
    let stored = || -> String {
        conn.query_row(sql::META_VALUE, ["total_stored_bytes"], |row| row.get(0))
            .expect("stored bytes")
    };

    // The losing batch, statement order as in the glue: accounting
    // first, then the guarded flip - both with opt-in 0.
    conn.execute(
        sql::COUNT_STORED_BYTES_ON_REVIVAL,
        rusqlite::params!["alpha", "pkg", "1.0.0", "aa", "aa", 10, "aa", 0, "{}"],
    )
    .expect("revival accounting");
    let flipped = conn
        .execute(
            sql::REVIVE_REJECTED_REVISION,
            rusqlite::params![
                "{}",
                "2026-07-15T03:00:00.000Z",
                2,
                "alpha",
                "pkg",
                0,
                "1.0.0",
                "aa",
                "aa"
            ],
        )
        .expect("guarded revival");
    assert_eq!(flipped, 0, "the opt-in guard must refuse the flip");
    assert_eq!(stored(), "20", "a refused revival must add nothing");

    // With the opt-in both fire together and the bytes are re-counted.
    conn.execute(
        sql::COUNT_STORED_BYTES_ON_REVIVAL,
        rusqlite::params!["alpha", "pkg", "1.0.0", "aa", "aa", 10, "aa", 1, "{}"],
    )
    .expect("revival accounting");
    let flipped = conn
        .execute(
            sql::REVIVE_REJECTED_REVISION,
            rusqlite::params![
                "{}",
                "2026-07-15T03:00:00.000Z",
                2,
                "alpha",
                "pkg",
                1,
                "1.0.0",
                "aa",
                "aa"
            ],
        )
        .expect("guarded revival");
    assert_eq!(flipped, 1);
    assert_eq!(stored(), "30");
}

/// A revival re-enters the live set, so the resolver-metadata
/// invariance inside [`sql::REVIVE_REJECTED_REVISION`] must hold
/// against the live siblings of today - the rejected document never
/// constrained anyone, and the opt-in must not bypass the rule.  The
/// accounting mirrors the refusal.
#[test]
fn revival_invariance_guard_is_enforced_inside_the_update() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    // Reject 'aa' (it declared a dependency), then land a live
    // different-bytes sibling that dropped it.
    conn.execute_batch(
        "UPDATE revisions SET verification = 'rejected',
                              metadata_json = '{\"dependencies\":{\"acme/dep\":\"^1\"}}'
         WHERE scope = 'alpha';
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'pkg', '1.0.0', 'a2', 'a2', '{\"dependencies\":{}}',
                   '2026-07-15T02:00:00.000Z', 10, 2, 'verified');",
    )
    .expect("reject alpha and land a conflicting live sibling");
    let stored = || -> String {
        conn.query_row(sql::META_VALUE, ["total_stored_bytes"], |row| row.get(0))
            .expect("stored bytes")
    };
    let baseline = stored();
    let revive = |metadata: &str| -> usize {
        conn.execute(
            sql::COUNT_STORED_BYTES_ON_REVIVAL,
            rusqlite::params!["alpha", "pkg", "1.0.0", "aa", "aa", 10, "aa", 1, metadata],
        )
        .expect("revival accounting");
        conn.execute(
            sql::REVIVE_REJECTED_REVISION,
            rusqlite::params![
                metadata,
                "2026-07-15T03:00:00.000Z",
                2,
                "alpha",
                "pkg",
                1,
                "1.0.0",
                "aa",
                "aa"
            ],
        )
        .expect("guarded revival")
    };
    // The rejected document contradicts the live sibling: refused
    // even with the opt-in, and no bytes re-counted.
    assert_eq!(revive("{\"dependencies\":{\"acme/dep\":\"^1\"}}"), 0);
    assert_eq!(stored(), baseline);
    // A document agreeing with the live sibling revives.
    assert_eq!(revive("{\"dependencies\":{}}"), 1);
    assert_eq!(
        stored(),
        baseline
            .parse::<i64>()
            .unwrap()
            .checked_add(10)
            .unwrap()
            .to_string()
    );
}

/// The resolver-metadata invariance lives inside [`sql::INSERT_REVISION`]
/// itself and is deliberately NOT bypassed by the opt-in: a respin
/// that changes `dependencies`, `features`, or `standards` is refused
/// even when the publisher passed `new-revision=true`, while a
/// packaging-only change (any other field) lands.
#[test]
fn revision_inserts_enforce_resolver_metadata_invariance() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    // Give alpha's live revision a concrete document.
    conn.execute(
        "UPDATE revisions SET metadata_json =
           '{\"dependencies\":{\"acme/dep\":\"^1\"},\"features\":{\"default\":[]}}'
         WHERE scope = 'alpha'",
        [],
    )
    .expect("seed the live document");
    let insert = |revision: &str, metadata: &str| -> usize {
        conn.execute(
            sql::INSERT_REVISION,
            rusqlite::params![
                "alpha",
                "pkg",
                "1.0.0",
                revision,
                revision,
                metadata,
                "2026-07-15T02:00:00.000Z",
                10,
                2,
                1 // opted in - the invariance must hold regardless
            ],
        )
        .expect("guarded revision insert")
    };
    // Changed dependencies: refused.
    assert_eq!(
        insert("a2", r#"{"dependencies":{},"features":{"default":[]}}"#),
        0
    );
    // Changed features: refused.
    assert_eq!(
        insert(
            "a3",
            r#"{"dependencies":{"acme/dep":"^1"},"features":{"default":["simd"]}}"#
        ),
        0
    );
    // A packaging-only difference (an added upstream block) lands.
    assert_eq!(
        insert(
            "a4",
            r#"{"dependencies":{"acme/dep":"^1"},"features":{"default":[]},"upstream":{"url":"https://example.com/a.zip"}}"#
        ),
        1
    );
}
