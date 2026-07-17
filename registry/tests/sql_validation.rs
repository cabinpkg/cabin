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

/// One claim's write, modeled on how the claim callback runs it: both
/// statements inside one transaction (a D1 batch), aborting at the
/// first failure the way D1 aborts and rolls back a batch. The result
/// comes back to the caller because the scope insert's failure is
/// load-bearing: it is what makes the loser of a claim race roll back
/// seedless.
fn claim(
    conn: &rusqlite::Connection,
    scope: &str,
    account_id: &str,
    user_id: i64,
    now: &str,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        sql::CLAIM_SCOPE,
        rusqlite::params![scope, "github", account_id, now],
    )?;
    tx.execute(sql::SEED_CLAIM_OWNER, rusqlite::params![scope, user_id])?;
    tx.commit()
}

fn member_role(conn: &rusqlite::Connection, scope: &str, user_id: i64) -> Option<String> {
    conn.query_row(
        sql::SCOPE_MEMBER_ROLE,
        rusqlite::params![scope, user_id],
        |row| row.get(0),
    )
    .map(Some)
    .or_else(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
    .expect("member role")
}

#[test]
fn a_claim_seeds_its_owner_and_a_lost_race_fails_seedless() {
    let conn = migrated_connection();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (1, '2026-07-15T00:00:00.000Z'),
                                                   (2, '2026-07-15T00:00:00.000Z');",
    )
    .expect("seed users");

    claim(&conn, "fmtlib", "7280970", 1, "2026-07-15T00:00:00.000Z").expect("winning claim");
    assert_eq!(member_role(&conn, "fmtlib", 1), Some("owner".to_owned()));

    // The claim callback pre-checks SCOPE_EXISTS, but the write must
    // stay correct without it: a claim that lost the race between the
    // pre-check and the batch fails the primary-key insert - even with
    // byte-identical proof and timestamp, the collision two same-instant
    // admins of one org produce - which aborts and rolls back its
    // batch, so the loser never becomes an owner and the winner's row
    // is untouched.
    let lost = claim(&conn, "fmtlib", "7280970", 2, "2026-07-15T00:00:00.000Z");
    assert!(lost.is_err(), "a second claim must fail the insert");
    assert_eq!(member_role(&conn, "fmtlib", 2), None);
    let (proof, claimed_at): (String, String) = conn
        .query_row(
            "SELECT proof_account_id, claimed_at FROM scopes WHERE name = 'fmtlib'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("scope row");
    assert_eq!(proof, "7280970");
    assert_eq!(claimed_at, "2026-07-15T00:00:00.000Z");
    assert_eq!(
        count(&conn, "scope_members"),
        1,
        "the winner stays the sole owner"
    );

    // SCOPE_EXISTS is the pre-check the callback's refusal rests on.
    for (scope, expected) in [("fmtlib", 1), ("ghost", 0)] {
        let n: i64 = conn
            .query_row(sql::SCOPE_EXISTS, [scope], |row| row.get(0))
            .expect("scope exists");
        assert_eq!(n, expected, "scope: {scope}");
    }
}

#[test]
fn membership_management_enforces_the_last_owner_rule() {
    let conn = migrated_connection();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (1, '2026-07-15T00:00:00.000Z'),
                                                   (2, '2026-07-15T00:00:00.000Z');
         INSERT INTO identities (provider, provider_account_id, login_snapshot, user_id)
           VALUES ('github', '26405363', 'ken-matsui', 1),
                  ('github', '583231', 'octocat', 2);",
    )
    .expect("seed users");
    claim(&conn, "fmtlib", "7280970", 1, "2026-07-15T00:00:00.000Z").expect("claim");

    // The role domain is closed in the schema itself (migration 0007):
    // membership disputes are manual SQL, and a typo there must not
    // silently widen access or orphan a scope. (Through the API's
    // INSERT OR IGNORE a bad role is swallowed instead - either way it
    // never lands.)
    let bad_role = conn.execute(
        "INSERT INTO scope_members (scope_name, user_id, role) VALUES ('fmtlib', 2, 'admin')",
        [],
    );
    assert!(bad_role.is_err(), "the role CHECK must refuse 'admin'");
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 2, "admin"],
    )
    .expect("an ignored bad-role insert");
    assert_eq!(member_role(&conn, "fmtlib", 2), None);

    // Only the owner role passes the management gate.
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 2, "member"],
    )
    .expect("add member");
    let owner_gate = |user_id: i64| -> i64 {
        conn.query_row(
            sql::SCOPE_OWNER_MEMBERSHIP,
            rusqlite::params!["fmtlib", user_id],
            |row| row.get(0),
        )
        .expect("owner gate")
    };
    assert_eq!(owner_gate(1), 1);
    assert_eq!(owner_gate(2), 0);

    // Adding an existing member never rewrites their role: an upsert
    // here could demote the last owner.
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 1, "member"],
    )
    .expect("re-add owner");
    assert_eq!(member_role(&conn, "fmtlib", 1), Some("owner".to_owned()));

    // The listing resolves members back to their GitHub identity,
    // deterministically ordered.
    let mut statement = conn.prepare(sql::LIST_SCOPE_MEMBERS).expect("prepare");
    let members: Vec<(String, String, String)> = statement
        .query_map(rusqlite::params!["fmtlib", "github"], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("list members")
        .collect::<Result<_, _>>()
        .expect("member rows");
    assert_eq!(
        members,
        vec![
            (
                "26405363".to_owned(),
                "ken-matsui".to_owned(),
                "owner".to_owned()
            ),
            (
                "583231".to_owned(),
                "octocat".to_owned(),
                "member".to_owned()
            ),
        ]
    );

    // Removing the last owner is refused inside the statement itself;
    // an ordinary member and a co-owned owner both remove fine.
    let removed = conn
        .execute(sql::REMOVE_SCOPE_MEMBER, rusqlite::params!["fmtlib", 1])
        .expect("remove last owner");
    assert_eq!(removed, 0, "the last owner must survive removal");
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 2, "owner"],
    )
    .expect("promote nobody");
    // User 2 is already a member: the add was ignored, so user 1 is
    // still the only owner and still protected.
    let removed = conn
        .execute(sql::REMOVE_SCOPE_MEMBER, rusqlite::params!["fmtlib", 1])
        .expect("remove still-last owner");
    assert_eq!(removed, 0);
    let removed = conn
        .execute(sql::REMOVE_SCOPE_MEMBER, rusqlite::params!["fmtlib", 2])
        .expect("remove member");
    assert_eq!(removed, 1);

    // With a genuine second owner the first one may leave.
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 2, "owner"],
    )
    .expect("add second owner");
    let removed = conn
        .execute(sql::REMOVE_SCOPE_MEMBER, rusqlite::params!["fmtlib", 1])
        .expect("remove co-owner");
    assert_eq!(removed, 1);
    assert_eq!(owner_gate(2), 1);
}

/// Seeds one user, two scopes the user is a member of, and the same
/// `(name, version)` under both - the collision the scoped statements
/// must keep apart.
fn seed_scope_collision(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (1, '2026-07-15T00:00:00.000Z');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '1', '2026-07-15T00:00:00.000Z'),
                  ('beta', 'github', '2', '2026-07-15T00:00:00.000Z');
         INSERT INTO scope_members (scope_name, user_id, role) VALUES ('alpha', 1, 'owner');
         INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'pkg', '2026-07-15T00:00:00.000Z', 1),
                  ('beta', 'pkg', '2026-07-15T00:00:00.000Z', 1);
         INSERT INTO versions (scope, name, version, checksum, metadata_json, \
                               published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'pkg', '1.0.0', 'aa', '{}', '2026-07-15T00:00:00.000Z', 10, 1, 'verified'),
                  ('beta', 'pkg', '1.0.0', 'bb', '{}', '2026-07-15T00:00:00.000Z', 20, 1, 'pending');
         UPDATE meta SET value = '30' WHERE key = 'total_stored_bytes';",
    )
    .expect("seed the cross-scope collision");
}

/// The scoped statements executed against colliding `(name, version)`
/// rows: `prepare` alone cannot catch a missing scope predicate or a
/// wrong bind order, so this pins per-statement isolation between
/// scopes. (The wasm glue's end-to-end flow is `scripts/smoke.sh`'s
/// job; this covers the SQL itself.)
#[test]
fn scoped_statements_never_cross_scopes() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);

    // Reads address exactly one scope's row.
    let checksum: String = conn
        .query_row(
            sql::ARTIFACT_BY_PACKAGE_VERSION,
            rusqlite::params!["alpha", "pkg", "1.0.0"],
            |row| row.get(0),
        )
        .expect("alpha artifact row");
    assert_eq!(checksum, "aa");
    let verified: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM ({})",
                sql::VERIFIED_VERSIONS_BY_PACKAGE
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
            .query_row(sql::SCOPE_MEMBERSHIP, rusqlite::params![scope, 1], |row| {
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
            sql::MARK_VERSION_VERIFIED,
            rusqlite::params![
                "2026-07-15T01:00:00.000Z",
                "beta",
                "pkg",
                "1.0.0",
                "bb",
                "2026-07-15T00:00:00.000Z"
            ],
        )
        .expect("verify beta");
    assert_eq!(changed, 1);

    // The rejection refund's guards address one scope's row: refunding
    // with the wrong scope bound must be a no-op even though the other
    // scope holds the same (name, version).
    conn.execute(
        "UPDATE versions SET verification = 'pending' WHERE scope = 'beta'",
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
            "2026-07-15T00:00:00.000Z"
        ],
    )
    .expect("refund bound to the wrong scope");
    let stored: String = conn
        .query_row(sql::META_VALUE, ["total_stored_bytes"], |row| row.get(0))
        .expect("stored bytes");
    assert_eq!(stored, "30", "a wrong-scope refund must not fire");
    conn.execute(
        sql::REFUND_STORED_BYTES_ON_REJECTION,
        rusqlite::params!["bb", "beta", "pkg", "1.0.0", 20, "2026-07-15T00:00:00.000Z"],
    )
    .expect("refund bound to the right scope");
    let stored: String = conn
        .query_row(sql::META_VALUE, ["total_stored_bytes"], |row| row.get(0))
        .expect("stored bytes");
    assert_eq!(stored, "10", "the right-scope refund fires exactly once");
}

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

    // Two verified downloads count; the identical (name, version) under
    // the other scope - pending there - stays untouched.
    for _ in 0..2 {
        let changed = conn
            .execute(
                sql::INCREMENT_VERSION_DOWNLOADS,
                rusqlite::params!["alpha", "pkg", "1.0.0"],
            )
            .expect("increment verified download");
        assert_eq!(changed, 1);
    }
    assert_eq!(downloads("alpha"), 2);
    assert_eq!(downloads("beta"), 0);

    // A pending row never counts (the verifier's fetch), and neither
    // does an unknown triple.
    for (scope, name, version) in [("beta", "pkg", "1.0.0"), ("ghost", "pkg", "1.0.0")] {
        let changed = conn
            .execute(
                sql::INCREMENT_VERSION_DOWNLOADS,
                rusqlite::params![scope, name, version],
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
        sql::INCREMENT_VERSION_DOWNLOADS,
        rusqlite::params!["alpha", "pkg", "1.0.0"],
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
        "INSERT INTO versions (scope, name, version, checksum, metadata_json, \
                               published_at, archive_size, published_by, verification, downloads)
           VALUES ('alpha', 'pkg', '1.1.0', 'cc', '{}', '2026-07-15T01:00:00.000Z', 10, 1, 'verified', 5),
                  ('beta', 'pkg', '2.0.0', 'dd', '{}', '2026-07-15T02:00:00.000Z', 10, 1, 'verified', 7);
         UPDATE versions SET downloads = 100 WHERE scope = 'beta' AND version = '1.0.0';",
    )
    .expect("seed verified versions and a pending counter");
    for _ in 0..2 {
        conn.execute(
            sql::INCREMENT_VERSION_DOWNLOADS,
            rusqlite::params!["alpha", "pkg", "1.0.0"],
        )
        .expect("increment verified download");
    }

    let (packages, versions, downloads): (i64, i64, i64) = conn
        .query_row(sql::REGISTRY_STATS, [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("registry stats totals");
    assert_eq!(packages, 2, "alpha/pkg and beta/pkg are distinct packages");
    assert_eq!(versions, 3, "the pending beta/pkg@1.0.0 must not count");
    assert_eq!(downloads, 14, "2 + 5 + 7; the pending row's 100 must not");
}
