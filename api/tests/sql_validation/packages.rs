//! Executed-semantics tests for `src/sql/packages.rs`: yank,
//! verification, search, and how the admission gates treat a package's
//! names and a version's revisions.

use cabin_api_worker::sql;

use crate::common::{
    Limits, Op, admit_new, admit_revival, archive_verified, migrated_connection, reject, rows,
    seed_scope_collision,
};

/// One 10-byte default-class upload by the seeded user 2, under an
/// upload id named after its revision.
fn upload<'a>(
    scope: &'a str,
    name: &'a str,
    version: &'a str,
    revision: &'a str,
    opt_in: bool,
    stamp: &'a str,
) -> Op<'a> {
    Op {
        user: 2.0,
        class: "default",
        scope,
        name,
        version,
        revision,
        metadata: "{}",
        size: 10.0,
        opt_in,
        stamp,
        upload_id: revision,
    }
}

/// Whether a batch was admitted: every statement changed one row, or
/// none changed any.
fn admitted(results: &[(usize, i32)]) -> bool {
    let changed: Vec<usize> = results.iter().map(|&(changed, _)| changed).collect();
    assert!(
        changed.iter().all(|&n| n == changed[0]) && changed[0] <= 1,
        "{changed:?}"
    );
    changed[0] == 1
}

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
    let artifact: (String, Option<String>) = conn
        .query_row(
            sql::ARTIFACT_BY_REVISION,
            rusqlite::params!["alpha", "pkg", "1.0.0", "aa"],
            |row| Ok((row.get("verification")?, row.get("upload_id")?)),
        )
        .expect("alpha artifact row");
    assert_eq!(
        artifact,
        ("verified".to_owned(), Some("seed-aa".to_owned()))
    );
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
            sql::COUNT_PACKAGE_VERSIONS_ON_DAY,
            rusqlite::params!["alpha", "pkg", "2026-07-15T"],
            |row| row.get(0),
        )
        .expect("alpha versions on the day");
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

    // The rejection batch addresses one scope's row: bound to the wrong
    // scope it flips and refunds nothing, though the other scope holds
    // the same (name, version).
    conn.execute(
        "UPDATE revisions SET verification = 'pending' WHERE scope = 'beta'",
        [],
    )
    .expect("reset beta to pending");
    let stored = || rows(&conn, "SELECT stored_bytes FROM users WHERE id = 2");
    let rejection = |scope: &str| {
        let listed = "2026-07-15T00:00:00.000Z";
        let op = upload(scope, "pkg", "1.0.0", "bb", false, listed);
        reject(&conn, &op, listed, "2026-07-15T02:00:00.000Z")
    };
    assert_eq!(rejection("alpha"), 0);
    assert_eq!(
        stored(),
        ["Integer(30)"],
        "a wrong-scope refund must not fire"
    );
    assert_eq!(rejection("beta"), 1);
    assert_eq!(
        stored(),
        ["Integer(10)"],
        "the right-scope refund fires once"
    );
}

/// Seeds one user plus the packages and versions the search,
/// catalog, and reverse-dependency statements walk: a target package with two
/// verified versions, a pending-only lookalike, an underscore/plain
/// name pair for the literal-match check, and dependents in every
/// lifecycle state.
fn seed_search_corpus(conn: &rusqlite::Connection) {
    conn.execute_batch(
        r#"INSERT INTO users (id, created_at) VALUES (2, '2026-07-18T00:00:00.000Z');
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
           ('alpha', 'target', '1.0.0', 'c01', 'c01', '{"dependencies":{}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('alpha', 'target', '1.1.0', 'c02', 'c02', '{"dependencies":{}}',
            '2026-07-18T01:00:00.000Z', 10, 2, 'verified'),
           ('beta', 'target-pending', '1.0.0', 'c03', 'c03', '{"dependencies":{}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'pending'),
           ('alpha', 'my_pkg', '1.0.0', 'c04', 'c04', '{"dependencies":{}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('alpha', 'myxpkg', '1.0.0', 'c05', 'c05', '{"dependencies":{}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('gabime', 'spdlog', '1.13.0', 'c06', 'c06',
            '{"dependencies":{"alpha/target":"^1"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('gabime', 'spdlog', '1.14.0', 'c07', 'c07',
            '{"dependencies":{"alpha/target":{"version":"^1","optional":true}}}',
            '2026-07-18T01:00:00.000Z', 10, 2, 'verified'),
           ('acme', 'pending-dep', '1.0.0', 'c08', 'c08',
            '{"dependencies":{"alpha/target":"^1"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'pending'),
           ('acme', 'rejected-dep', '1.0.0', 'c09', 'c09',
            '{"dependencies":{"alpha/target":"^1"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'rejected'),
           ('acme', 'bare-dep', '1.0.0', 'c10', 'c10',
            '{"dependencies":{"target":"^1"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified'),
           ('acme', 'dev-dep', '1.0.0', 'c11', 'c11',
            '{"dependencies":{},"dev-dependencies":{"alpha/target":"^1"}}',
            '2026-07-18T00:00:00.000Z', 10, 2, 'verified');"#,
    )
    .expect("seed the search corpus");
    archive_verified(conn);
}

/// Runs [`sql::SEARCH_VERIFIED_VERSIONS`] with the term the session
/// plane would bind (validated and ASCII-lowercased by the real
/// parser), returning the matched canonical names (sorted for the
/// assertion; the statement itself returns rows in table order and
/// the host ranks them).
fn search_names(conn: &rusqlite::Connection, term: &str) -> Vec<String> {
    let query =
        cabin_api_worker::user_api::parse_search_query(Some(term)).expect("a valid search term");
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

/// The catalog lists exactly what the package documents serve as
/// resolvable, across every scope: each verified, non-yanked version
/// at its current revision. Pending-only and rejected-only packages and
/// yanked versions are absent, a superseded verified revision is not
/// listed, and a pending respin leaves the verified revision listed.
#[test]
fn catalog_rows_are_the_current_revisions_of_unyanked_verified_versions() {
    let conn = migrated_connection();
    seed_search_corpus(&conn);
    conn.execute(
        r#"INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
         VALUES ('alpha', 'target', '1.1.0', 'c12', 'c12', '{"dependencies":{}}',
                 '2026-07-18T02:00:00.000Z', 10, 2, 'pending'),
                ('alpha', 'target', '1.1.0', 'c00', 'c00', '{"dependencies":{}}',
                 '2026-07-17T00:00:00.000Z', 10, 2, 'verified')"#,
        [],
    )
    .expect("pending respin and superseded revision");
    archive_verified(&conn);

    let mut statement = conn
        .prepare(sql::CATALOG_VERSIONS)
        .expect("prepare catalog");
    let mut rows: Vec<String> = statement
        .query_map([], |row| {
            Ok(format!(
                "{}/{}@{} {}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?
            ))
        })
        .expect("run catalog")
        .collect::<Result<_, _>>()
        .expect("catalog rows");
    rows.sort();
    assert_eq!(
        rows,
        [
            "acme/bare-dep@1.0.0 c10",
            "acme/dev-dep@1.0.0 c11",
            "alpha/my_pkg@1.0.0 c04",
            "alpha/myxpkg@1.0.0 c05",
            "alpha/target@1.1.0 c02",
            "gabime/spdlog@1.14.0 c07",
        ]
    );
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
/// the matching condition of the admission gate are invisible to
/// `prepare`, so they are executed here.
#[test]
fn twin_guard_blocks_dash_underscore_twins_within_a_scope() {
    // Room for every upload below to stay pending.
    let limits = Limits {
        pending: 100.0,
        ..Limits::of("default")
    };
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

    // The gate refuses a twin, admits an unrelated name and the same
    // name in another scope, and keeps admitting the existing name
    // itself - the self exclusion.
    let admit = |scope: &str, name: &str, version: &str, revision: &str| -> bool {
        let op = upload(
            scope,
            name,
            version,
            revision,
            false,
            "2026-07-15T01:00:00.000Z",
        );
        admitted(&admit_new(&conn, &op, &limits))
    };
    assert!(!admit("alpha", "foo_bar", "1.0.0", "e1"));
    assert!(admit("alpha", "other", "1.0.0", "e2"));
    assert!(admit("beta", "foo_bar", "1.0.0", "e3"));
    assert!(admit("alpha", "foo-bar", "2.0.0", "e4"));

    // Legacy twins (possible only through operator surgery or restored
    // data) keep receiving new versions: the twin policy gates package
    // creation, not a publish into a package that exists.
    conn.execute(
        "INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'foo_bar', '2026-07-15T00:00:00.000Z', 2)",
        [],
    )
    .expect("seed the legacy twin directly");
    assert!(admit("alpha", "foo-bar", "3.0.0", "e5"));
    assert!(admit("alpha", "foo_bar", "3.0.0", "e6"));
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

/// The `new-revision` opt-in is a condition of the admission gates
/// themselves, so two concurrent publishes cannot both slip past a
/// stale preflight read: without the flag, an upload beside a live
/// different-bytes revision is refused; with it, the respin lands and
/// the superseded revision stays.
#[test]
fn revision_opt_in_is_enforced_inside_the_gates() {
    let limits = Limits::of("default");
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let new = |revision: &str, opt_in: bool| -> bool {
        let op = upload(
            "alpha",
            "pkg",
            "1.0.0",
            revision,
            opt_in,
            "2026-07-15T02:00:00.000Z",
        );
        admitted(&admit_new(&conn, &op, &limits))
    };

    // alpha/pkg@1.0.0 already has verified revision 'aa': different
    // bytes without the opt-in are refused.
    assert!(!new("a2", false));
    // The opt-in lands the respin; the original revision remains.
    assert!(new("a2", true));
    assert_eq!(
        rows(
            &conn,
            "SELECT revision FROM revisions WHERE scope = 'alpha' ORDER BY 1"
        ),
        ["Text(\"a2\")", "Text(\"aa\")"]
    );

    // A version whose only revisions are rejected accepts a fresh one
    // without the flag - the recovery path.
    conn.execute(
        "UPDATE revisions SET verification = 'rejected', archived_at = NULL \
         WHERE scope = 'alpha'",
        [],
    )
    .expect("reject both revisions");
    assert!(new("a3", false));

    // The revival gate holds the same condition: an unflagged revival
    // of a rejected revision beside the now-live different-bytes 'a3'
    // is refused; the flag lets it through.
    let revive = |opt_in: bool| -> bool {
        let op = upload(
            "alpha",
            "pkg",
            "1.0.0",
            "aa",
            opt_in,
            "2026-07-15T03:00:00.000Z",
        );
        admitted(&admit_revival(&conn, &op, &limits))
    };
    assert!(!revive(false));
    assert!(revive(true));
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

    // Nor does a verified one whose archive is not in ARCHIVES yet.
    conn.execute(
        "UPDATE revisions SET verification = 'verified' WHERE revision = 'a2'",
        [],
    )
    .expect("verify the respin");
    assert_eq!(current(), Some(("aa".to_owned(), "aa".to_owned())));

    // Once its archive is placed, the newer publish time takes over,
    // and both revisions stay listed for the composed document.
    archive_verified(&conn);
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

/// The anonymous package-document reads cost rows of the requested
/// package only. The migrated schema has no `sqlite_stat1` (the
/// registry never runs `ANALYZE`), so these are the plans production
/// gets: a `revisions_verification`
/// step would read every verified revision in the registry, and a
/// `SCAN` every row of a table.
#[test]
fn package_document_reads_seek_the_package() {
    let conn = migrated_connection();
    for statement in [
        sql::CURRENT_REVISIONS_BY_PACKAGE,
        sql::VERIFIED_REVISIONS_BY_PACKAGE,
        sql::VERIFIED_VERSION_DETAILS,
        sql::HAS_VERIFIED_VERSION,
    ] {
        let mut plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {statement}"))
            .expect("explain");
        let steps: Vec<String> = plan
            .query_map(rusqlite::params!["alpha", "pkg"], |row| row.get(3))
            .expect("query plan")
            .collect::<Result<_, _>>()
            .expect("plan rows");
        assert!(
            steps
                .iter()
                .all(|step| !step.contains("revisions_verification") && !step.starts_with("SCAN")),
            "{statement}\n{steps:#?}"
        );
    }
}

/// A respin may change what resolution reads: under the opt-in, a
/// revision whose `dependencies` differ from the live one's is admitted
/// and charged, and the read plane lists each revision's own stored
/// document for its `resolution`.
#[test]
fn a_respin_may_change_resolution_metadata() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let changed = r#"{"dependencies":{"acme/b":"^1"}}"#;
    let respin = Op {
        metadata: changed,
        size: 5.0,
        ..upload(
            "alpha",
            "pkg",
            "1.0.0",
            "cc",
            true,
            "2026-07-15T02:00:00.000Z",
        )
    };
    assert!(
        admitted(&admit_new(&conn, &respin, &Limits::of("default"))),
        "a dependency-changing respin must land"
    );
    assert_eq!(
        rows(&conn, "SELECT stored_bytes FROM users WHERE id = 2"),
        ["Integer(35)"]
    );

    conn.execute(
        "UPDATE revisions SET verification = 'verified' WHERE revision = 'cc'",
        [],
    )
    .expect("verify the respin");
    archive_verified(&conn);
    let mut listed = conn
        .prepare(sql::VERIFIED_REVISIONS_BY_PACKAGE)
        .expect("prepare listing");
    let documents: Vec<(String, String)> = listed
        .query_map(rusqlite::params!["alpha", "pkg"], |row| {
            Ok((row.get("revision")?, row.get("metadata_json")?))
        })
        .expect("query listing")
        .collect::<Result<_, _>>()
        .expect("listing rows");
    assert_eq!(
        documents,
        [
            ("aa".to_owned(), "{}".to_owned()),
            ("cc".to_owned(), changed.to_owned())
        ]
    );
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
    assert_eq!(
        statement.column_names().collect::<Vec<_>>(),
        ["yanked", "verified"]
    );
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

/// A verdict lands only on the pending generation the verifier listed:
/// a stale `published_at` (a byte-identical revival), other bytes, or a
/// row no longer pending changes nothing, for either verdict.
#[test]
fn verdicts_apply_only_to_the_listed_pending_generation() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let listed = "2026-07-15T00:00:00.000Z";
    let stale = "2026-07-14T00:00:00.000Z";
    let now = "2026-07-15T01:00:00.000Z";
    let apply = |statement: &str, checksum: &str, published_at: &str| {
        let scoped = ["beta", "pkg", "1.0.0", checksum, published_at, "bb"];
        let binds: Vec<&str> = if statement == sql::MARK_REVISION_VERIFIED {
            [now].into_iter().chain(scoped).collect()
        } else {
            ["unsafe archive"]
                .into_iter()
                .chain(scoped)
                .chain([now])
                .collect()
        };
        conn.execute(statement, rusqlite::params_from_iter(binds))
            .expect("apply the verdict")
    };
    for statement in [sql::MARK_REVISION_VERIFIED, sql::MARK_REVISION_REJECTED] {
        conn.execute(
            "UPDATE revisions SET verification = 'pending' WHERE scope = 'beta'",
            [],
        )
        .expect("reset beta to pending");
        assert_eq!(apply(statement, "bb", stale), 0, "a superseded generation");
        assert_eq!(apply(statement, "cc", listed), 0, "other bytes");
        assert_eq!(apply(statement, "bb", listed), 1, "the listed generation");
        assert_eq!(apply(statement, "bb", listed), 0, "no longer pending");
    }
}
