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
    // D1 parity: D1 caps LIKE/GLOB patterns at 50 bytes at evaluation
    // (bundled SQLite defaults to 50000), so a long pattern - say a
    // fixed-width GLOB in a CHECK - passes every host test while
    // failing every INSERT in production. Pinning the limit here makes
    // the whole suite evaluate patterns under D1's rules.
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

/// The `(user_id, login_snapshot, quota_class)` the session plane
/// resolves for an identity, if any.
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
    let (user_id, login, quota_class) =
        resolve(&conn, "github", "26405363").expect("identity resolves");
    assert_eq!(login, "ken-matsui");
    assert_eq!(quota_class, "default");
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

/// Creates a user and a token row through the real statements
/// ([`sql::INSERT_TOKEN`] writes none of the trustpub-era columns, so
/// every row it makes is a legacy-shaped one), returning the user id.
fn seed_token(conn: &rusqlite::Connection, token_id: &str, token_hash: &str) -> i64 {
    sign_in(
        conn,
        "github",
        "26405363",
        "ken-matsui",
        "2026-07-15T00:00:00.000Z",
    );
    let (user_id, ..) = resolve(conn, "github", "26405363").expect("identity resolves");
    conn.execute(
        sql::INSERT_TOKEN,
        rusqlite::params![
            token_id,
            user_id,
            "ci token",
            token_hash,
            "publish,yank",
            "2026-07-15T00:00:00.000Z"
        ],
    )
    .expect("insert token");
    user_id
}

/// Runs the bearer lookup as `glue::authenticate` does, returning the
/// matched `(id, scopes, quota_class, scope_limit)` if any.
fn auth_lookup(
    conn: &rusqlite::Connection,
    token_hash: &str,
    now: &str,
) -> Option<(String, String, String, Option<String>)> {
    conn.query_row(
        sql::AUTH_TOKEN_LOOKUP,
        rusqlite::params![token_hash, now],
        |row| Ok((row.get(0)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )
    .map(Some)
    .or_else(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
    .expect("auth lookup")
}

#[test]
fn an_expired_token_is_the_same_no_row_answer_as_an_unknown_one() {
    let conn = migrated_connection();
    seed_token(&conn, "tok-1", "hash-1");
    conn.execute(
        "UPDATE tokens SET expires_at = '2026-08-14T12:00:00.000Z' WHERE id = 'tok-1'",
        [],
    )
    .expect("set expiry");

    let now = "2026-08-14T12:00:00.001Z";
    // The uniform-401 invariant at the layer that decides it: the
    // expired row and a hash that never existed produce the exact same
    // lookup result, so the glue's single `unauthorized()` constructor
    // is the only possible answer for both.
    assert_eq!(auth_lookup(&conn, "hash-1", now), None);
    assert_eq!(
        auth_lookup(&conn, "hash-1", now),
        auth_lookup(&conn, "no-such-hash", now)
    );
    // Strictly-greater boundary: a token expiring exactly now is gone.
    assert_eq!(
        auth_lookup(&conn, "hash-1", "2026-08-14T12:00:00.000Z"),
        None
    );
    // One millisecond earlier it still authenticates.
    let live = auth_lookup(&conn, "hash-1", "2026-08-14T11:59:59.999Z").expect("not yet expired");
    assert_eq!(live.0, "tok-1");
}

#[test]
fn a_future_dated_token_does_not_authenticate_before_its_anchor() {
    // The trustpub lifetime ceiling anchors on created_at, so a
    // minting bug forging a far-future anchor must not buy a standing
    // credential: the row answers exactly like an unknown hash until
    // the anchor instant arrives.
    let conn = migrated_connection();
    seed_token(&conn, "tok-1", "hash-1");
    conn.execute(
        "UPDATE tokens SET created_at = '9998-01-01T00:00:00.000Z' WHERE id = 'tok-1'",
        [],
    )
    .expect("forge the anchor");
    let now = "2026-08-14T12:00:00.000Z";
    assert_eq!(auth_lookup(&conn, "hash-1", now), None);
    assert_eq!(
        auth_lookup(&conn, "hash-1", now),
        auth_lookup(&conn, "no-such-hash", now)
    );
    // Inclusive lower bound: at the anchor instant the row is live.
    assert!(auth_lookup(&conn, "hash-1", "9998-01-01T00:00:00.000Z").is_some());
    // A parseable NON-canonical anchor never gets that far: the
    // created_at shape CHECK refuses it at write time, so the ceiling
    // (which parses the anchor) and the not-before bound (which
    // compares its text) can never see different instants.
    for anchor in ["+5372750", "5372750", "2026-07-15 00:00:00.000Z"] {
        let err = conn
            .execute(
                "UPDATE tokens SET created_at = ?1 WHERE id = 'tok-1'",
                [anchor],
            )
            .expect_err(anchor);
        assert!(err.to_string().contains("CHECK"), "{anchor}: {err}");
    }
}

#[test]
fn legacy_token_rows_authenticate_exactly_as_before() {
    let conn = migrated_connection();
    seed_token(&conn, "tok-1", "hash-1");
    // A pre-trustpub row: NULL expires_at never expires, NULL
    // scope_limit stays unlimited, and the kind column defaulted.
    let (id, scopes, quota_class, scope_limit) =
        auth_lookup(&conn, "hash-1", "2099-01-01T00:00:00.000Z").expect("legacy row matches");
    assert_eq!(id, "tok-1");
    assert_eq!(scopes, "publish,yank");
    assert_eq!(quota_class, "default");
    assert_eq!(scope_limit, None);
    let kind: String = conn
        .query_row("SELECT kind FROM tokens WHERE id = 'tok-1'", [], |row| {
            row.get(0)
        })
        .expect("kind column");
    assert_eq!(kind, "user");
    // Revocation still refuses on the rewritten lookup, exactly as it
    // did before the expiry conjunct joined it.
    conn.execute(
        "UPDATE tokens SET revoked_at = '2026-07-16T00:00:00.000Z' WHERE id = 'tok-1'",
        [],
    )
    .expect("revoke token");
    assert_eq!(
        auth_lookup(&conn, "hash-1", "2099-01-01T00:00:00.000Z"),
        None
    );
}

#[test]
fn a_scope_limited_token_authenticates_and_carries_its_limit() {
    // The 403 confinement itself lives in the glue
    // (`AuthContext::scope_limit_refuses`, unit-tested in src/auth.rs);
    // this pins that the lookup delivers the column the glue decides on.
    let conn = migrated_connection();
    seed_token(&conn, "tok-1", "hash-1");
    conn.execute(
        "UPDATE tokens SET scope_limit = 'cabin-ports' WHERE id = 'tok-1'",
        [],
    )
    .expect("set scope limit");
    let (.., scope_limit) =
        auth_lookup(&conn, "hash-1", "2026-08-14T12:00:00.000Z").expect("limited row matches");
    assert_eq!(scope_limit.as_deref(), Some("cabin-ports"));
}

#[test]
fn token_kind_domain_is_closed_in_the_schema() {
    let conn = migrated_connection();
    let user_id = seed_token(&conn, "tok-1", "hash-1");
    let err = conn
        .execute(
            "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind) \
             VALUES ('tok-2', ?1, 'x', 'hash-2', 'publish', '2026-07-15T00:00:00.000Z', 'admin')",
            [user_id],
        )
        .expect_err("unknown kind must fail the CHECK");
    assert!(err.to_string().contains("CHECK"), "{err}");
    // The domain's positive side: a trustpub row with an expiry, a
    // scope limit, and a granted quota class is admitted.
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at, scope_limit, quota_class) \
         VALUES ('tok-3', ?1, 'x', 'hash-3', 'publish', '2026-07-15T00:00:00.000Z', 'trustpub', \
                 '2026-07-15T00:30:00.000Z', 'cabin-ports', 'operator')",
        [user_id],
    )
    .expect("a bounded trustpub token is in the domain");
    // Short-lived, confined, and explicitly classed is schema-enforced
    // for trustpub rows: a NULL expiry, a NULL or empty scope limit,
    // and a NULL quota class each fail the CHECK, whatever the minting
    // path does. Each case supplies every other required column so it
    // isolates exactly its own violation.
    for (label, columns, values) in [
        (
            "no expiry",
            "scope_limit, quota_class",
            "'cabin-ports', 'operator'",
        ),
        (
            "no scope limit",
            "expires_at, quota_class",
            "'2026-07-15T00:30:00.000Z', 'operator'",
        ),
        (
            "empty scope limit",
            "expires_at, scope_limit, quota_class",
            "'2026-07-15T00:30:00.000Z', '', 'operator'",
        ),
        (
            "no quota class",
            "expires_at, scope_limit",
            "'2026-07-15T00:30:00.000Z', 'cabin-ports'",
        ),
    ] {
        let err = conn
            .execute(
                &format!(
                    "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                                         kind, {columns}) \
                     VALUES ('tok-4', ?1, 'x', 'hash-4', 'publish', \
                             '2026-07-15T00:00:00.000Z', 'trustpub', {values})"
                ),
                [user_id],
            )
            .expect_err(label);
        assert!(err.to_string().contains("CHECK"), "{label}: {err}");
    }
    // Publish-only: any other scopes string - the verify scope that
    // would reach the governor plane included - fails the CHECK.
    for scopes in ["verify", "publish,yank", ""] {
        let err = conn
            .execute(
                &format!(
                    "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                                         kind, expires_at, scope_limit, quota_class) \
                     VALUES ('tok-4', ?1, 'x', 'hash-4', '{scopes}', \
                             '2026-07-15T00:00:00.000Z', 'trustpub', \
                             '2026-07-15T00:30:00.000Z', 'cabin-ports', 'operator')"
                ),
                [user_id],
            )
            .expect_err(scopes);
        assert!(err.to_string().contains("CHECK"), "{scopes}: {err}");
    }
    // The lifetime ceiling is one day from issuance, inclusive: the
    // boundary instant is admitted, one millisecond past it is not.
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at, scope_limit, quota_class) \
         VALUES ('tok-5', ?1, 'x', 'hash-5', 'publish', '2026-07-15T00:00:00.000Z', 'trustpub', \
                 '2026-07-16T00:00:00.000Z', 'cabin-ports', 'operator')",
        [user_id],
    )
    .expect("the ceiling boundary is admitted");
    let err = conn
        .execute(
            "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                                 expires_at, scope_limit, quota_class) \
             VALUES ('tok-6', ?1, 'x', 'hash-6', 'publish', '2026-07-15T00:00:00.000Z', \
                     'trustpub', '2026-07-16T00:00:00.001Z', 'cabin-ports', 'operator')",
            [user_id],
        )
        .expect_err("past the ceiling must fail the CHECK");
    assert!(err.to_string().contains("CHECK"), "{err}");
}

/// The lazy expiry cleanup the exchange rides on each mint: statement
/// semantics `prepare` cannot see - the `kind` filter (an expired
/// *user* token stays listed and revocable on its owner's dashboard),
/// the inclusive `<=` boundary matching the auth lookup's strict `>`
/// liveness, and the jti prune against the acceptance window's end.
#[test]
fn expiry_pruning_is_trustpub_only_and_boundary_inclusive() {
    let conn = migrated_connection();
    let user_id = seed_token(&conn, "tok-user", "hash-user");
    conn.execute(
        "UPDATE tokens SET expires_at = '2026-08-14T00:00:00.000Z' WHERE id = 'tok-user'",
        [],
    )
    .expect("expire the user token");
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at, scope_limit, quota_class) \
         VALUES ('tok-tp-dead', ?1, 'x', 'hash-tp-dead', 'publish', '2026-08-14T23:30:00.000Z', \
                 'trustpub', '2026-08-15T00:00:00.000Z', 'cabin-ports', 'operator'), \
                ('tok-tp-live', ?1, 'x', 'hash-tp-live', 'publish', '2026-08-15T00:00:00.000Z', \
                 'trustpub', '2026-08-15T00:30:00.000Z', 'cabin-ports', 'operator')",
        [user_id],
    )
    .expect("seed trustpub rows");

    // At the boundary instant the dead row no longer authenticates
    // (the lookup's strict >), so the prune's inclusive <= removes
    // exactly the rows the auth plane already refuses.
    let now = "2026-08-15T00:00:00.000Z";
    let pruned = conn
        .execute(sql::PRUNE_EXPIRED_TRUSTPUB_TOKENS, [now])
        .expect("prune tokens");
    assert_eq!(pruned, 1, "exactly the dead trustpub row");
    let survivors: Vec<String> = conn
        .prepare("SELECT id FROM tokens ORDER BY id")
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("list tokens")
        .collect::<Result<_, _>>()
        .expect("token ids");
    assert_eq!(survivors, ["tok-tp-live", "tok-user"]);

    // The jti prune: expires_at is the END of the verifier's acceptance
    // window, so a row at exactly now protects nothing and goes; a
    // later one stays.
    conn.execute_batch(
        "INSERT INTO oidc_used_jtis (jti, expires_at) VALUES ('jti-dead', 100), \
                                                                 ('jti-live', 101);",
    )
    .expect("seed jtis");
    let pruned = conn
        .execute(sql::PRUNE_EXPIRED_OIDC_JTIS, [100])
        .expect("prune jtis");
    assert_eq!(pruned, 1);
    let survivor: String = conn
        .query_row("SELECT jti FROM oidc_used_jtis", [], |row| row.get(0))
        .expect("surviving jti");
    assert_eq!(survivor, "jti-live");
}

/// [`sql::DELETE_TRUSTPUB_TOKEN`]'s `kind` guard: the id of a live
/// *user* token deletes nothing - the zero the glue answers with the
/// uniform 401, keeping the endpoint no token-kind oracle. (The
/// trustpub-side delete is exercised by the end-to-end flow test in
/// `src/trustpub.rs`.)
#[test]
fn trustpub_revocation_never_deletes_user_tokens() {
    let conn = migrated_connection();
    seed_token(&conn, "tok-user", "hash-user");
    let deleted = conn
        .execute(sql::DELETE_TRUSTPUB_TOKEN, ["tok-user"])
        .expect("guarded delete executes");
    assert_eq!(deleted, 0);
    assert!(
        auth_lookup(&conn, "hash-user", "2026-08-15T00:00:00.000Z").is_some(),
        "the user token stays live"
    );
}

#[test]
fn expiry_timestamps_must_be_the_fixed_width_iso_shape() {
    // The lookup compares expires_at lexicographically, which fails
    // OPEN for a malformed value sorting above the ISO range - so the
    // schema refuses every shape but toISOString's, whatever kind of
    // token a minting path writes.
    let conn = migrated_connection();
    seed_token(&conn, "tok-1", "hash-1");
    for malformed in [
        "z",
        "2026-08-14T12:00:00Z",
        "2026-08-14 12:00:00.000Z",
        "",
        // Shape-valid digits outside calendar ranges: only the
        // strftime round-trip catches these.
        "2026-99-99T99:99:99.999Z",
        // Calendar-invalid but normalizable: datetime() silently reads
        // this as March 3rd, so only the byte-identical re-render
        // refuses it.
        "2026-02-31T12:00:00.000Z",
    ] {
        let err = conn
            .execute(
                "UPDATE tokens SET expires_at = ?1 WHERE id = 'tok-1'",
                [malformed],
            )
            .expect_err(malformed);
        assert!(err.to_string().contains("CHECK"), "{malformed}: {err}");
    }
    conn.execute(
        "UPDATE tokens SET expires_at = '2026-08-14T12:00:00.000Z' WHERE id = 'tok-1'",
        [],
    )
    .expect("the canonical shape is admitted");
}

#[test]
fn used_jtis_refuse_null_and_replayed_ids() {
    let conn = migrated_connection();
    conn.execute(
        "INSERT INTO oidc_used_jtis (jti, expires_at) VALUES ('jti-1', 1)",
        [],
    )
    .expect("first consumption");
    let replay = conn
        .execute(
            "INSERT INTO oidc_used_jtis (jti, expires_at) VALUES ('jti-1', 2)",
            [],
        )
        .expect_err("a replayed jti must fail the primary key");
    assert!(replay.to_string().contains("UNIQUE"), "{replay}");
    // NOT NULL is load-bearing: SQLite would otherwise admit unlimited
    // duplicate NULLs through a TEXT primary key.
    let null = conn
        .execute(
            "INSERT INTO oidc_used_jtis (jti, expires_at) VALUES (NULL, 1)",
            [],
        )
        .expect_err("a null jti must fail NOT NULL");
    assert!(null.to_string().contains("NOT NULL"), "{null}");
}

#[test]
fn the_trustpub_seed_names_the_ports_publishing_workflow() {
    let conn = migrated_connection();
    // Exactly the one seeded row (query_row alone would silently
    // ignore extras).
    assert_eq!(count(&conn, "trustpub_configs"), 1);
    let row = conn
        .query_row(
            "SELECT scope, repository_owner_id, repository_id, workflow_filename, \
                    git_ref, environment, quota_class, created_at \
             FROM trustpub_configs",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .expect("exactly one seeded config");
    assert_eq!(
        row,
        (
            "cabin-ports".to_owned(),
            // The cabinpkg organization and cabinpkg/cabin, by their
            // immutable numeric GitHub ids.
            35_998_702,
            119_684_778,
            "ports-publish.yml".to_owned(),
            Some("refs/heads/main".to_owned()),
            None,
            "operator".to_owned(),
            "2026-08-14T00:00:00.000Z".to_owned(),
        )
    );
}

/// One claim's write, modeled on how the claim callback runs it: the
/// three statements inside one transaction (a D1 batch), aborting at
/// the first failure the way D1 aborts and rolls back a batch. The
/// error comes back to the caller because the scope insert's failure
/// is load-bearing: it is what makes the loser of a claim race roll
/// back seedless. `Ok(applied)` mirrors the glue's zero-changed-rows
/// read on the scope insert - an over-limit claim suppresses every
/// statement and refuses in-band.
fn claim(
    conn: &rusqlite::Connection,
    scope: &str,
    account_id: &str,
    user_id: i64,
    now: &str,
    limit: i64,
) -> rusqlite::Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let applied = tx.execute(
        sql::CLAIM_SCOPE,
        rusqlite::params![scope, "github", account_id, now, user_id, limit],
    )?;
    tx.execute(
        sql::SEED_CLAIM_OWNER,
        rusqlite::params![scope, user_id, limit],
    )?;
    tx.execute(
        sql::RECORD_SCOPE_CLAIM,
        rusqlite::params![scope, user_id, now, limit],
    )?;
    tx.commit()?;
    Ok(applied > 0)
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

    let applied =
        claim(&conn, "fmtlib", "7280970", 1, "2026-07-15T00:00:00.000Z", 3).expect("winning claim");
    assert!(applied);
    assert_eq!(member_role(&conn, "fmtlib", 1), Some("owner".to_owned()));

    // The claim callback pre-checks SCOPE_EXISTS, but the write must
    // stay correct without it: a claim that lost the race between the
    // pre-check and the batch fails the primary-key insert - even with
    // byte-identical proof and timestamp, the collision two same-instant
    // admins of one org produce - which aborts and rolls back its
    // batch, so the loser never becomes an owner and the winner's row
    // is untouched.
    let lost = claim(&conn, "fmtlib", "7280970", 2, "2026-07-15T00:00:00.000Z", 3);
    assert!(lost.is_err(), "a second claim must fail the insert");
    assert_eq!(member_role(&conn, "fmtlib", 2), None);
    // The rollback covers the claim-history insert too: a failed claim
    // never consumes claim capacity.
    assert_eq!(
        count(&conn, "scope_claims"),
        1,
        "only the winner's claim is on record"
    );
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
fn the_lifetime_claim_limit_counts_history_not_ownership() {
    let conn = migrated_connection();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (1, '2026-07-15T00:00:00.000Z'),
                                                   (2, '2026-07-15T00:00:00.000Z');",
    )
    .expect("seed users");
    let now = "2026-07-15T00:00:00.000Z";

    // The default class's lifetime capacity: three grants land...
    for (scope, account_id) in [("one", "10"), ("two", "20"), ("three", "30")] {
        assert!(
            claim(&conn, scope, account_id, 1, now, 3).expect("claim under the limit"),
            "scope: {scope}"
        );
    }
    // ...and the fourth refuses in-band: every statement of the batch
    // repeats the guard, so nothing is inserted anywhere.
    assert!(!claim(&conn, "four", "40", 1, now, 3).expect("over-limit claim still executes"));
    for (table, expected) in [("scopes", 3), ("scope_members", 3), ("scope_claims", 3)] {
        assert_eq!(count(&conn, table), expected, "table: {table}");
    }
    assert_eq!(member_role(&conn, "four", 1), None);

    // Releasing scopes - today the operator's manual surgery, tomorrow
    // a transfer/release endpoint - never restores capacity: the
    // append-only history outlives the `scopes` rows.
    conn.execute_batch(
        "DELETE FROM scope_members WHERE scope_name IN ('one', 'two');
         DELETE FROM scopes WHERE name IN ('one', 'two');",
    )
    .expect("release two scopes");
    assert!(!claim(&conn, "five", "50", 1, now, 3).expect("claim after release"));

    // The limit is per user: another account's capacity is untouched,
    // and a re-claim of a released name spends the new claimant's.
    assert!(claim(&conn, "one", "10", 2, now, 3).expect("another user's claim"));

    // The usage read reports the history count the guard enforces.
    for (user, expected) in [(1, 3), (2, 1)] {
        let n: i64 = conn
            .query_row(sql::USER_SCOPE_CLAIM_COUNT, [user], |row| row.get(0))
            .expect("claim count");
        assert_eq!(n, expected, "user: {user}");
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
    claim(&conn, "fmtlib", "7280970", 1, "2026-07-15T00:00:00.000Z", 3).expect("claim");

    // The role domain is closed in the schema itself:
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
         INSERT INTO versions (scope, name, version) VALUES ('alpha', 'pkg', '1.0.0'),
                  ('beta', 'pkg', '1.0.0');
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json, \
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'pkg', '1.0.0', 'aa', 'aa', '{}', '2026-07-15T00:00:00.000Z', 10, 1, 'verified'),
                  ('beta', 'pkg', '1.0.0', 'bb', 'bb', '{}', '2026-07-15T00:00:00.000Z', 20, 1, 'pending');
         UPDATE meta SET value = '30' WHERE key = 'total_stored_bytes';",
    )
    .expect("seed the cross-scope collision");
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

/// Seeds one user plus the packages and versions the search and
/// reverse-dependency statements walk: a target package with two
/// verified versions, a pending-only lookalike, an underscore/plain
/// name pair for the literal-match check, and dependents in every
/// lifecycle state.
fn seed_search_corpus(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (1, '2026-07-18T00:00:00.000Z');
         INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at)
           VALUES ('alpha', 'github', '1', '2026-07-18T00:00:00.000Z'),
                  ('beta', 'github', '2', '2026-07-18T00:00:00.000Z'),
                  ('gabime', 'github', '3', '2026-07-18T00:00:00.000Z'),
                  ('acme', 'github', '4', '2026-07-18T00:00:00.000Z');
         INSERT INTO packages (scope, name, created_at, created_by)
           VALUES ('alpha', 'target', '2026-07-18T00:00:00.000Z', 1),
                  ('beta', 'target-pending', '2026-07-18T00:00:00.000Z', 1),
                  ('alpha', 'my_pkg', '2026-07-18T00:00:00.000Z', 1),
                  ('alpha', 'myxpkg', '2026-07-18T00:00:00.000Z', 1),
                  ('gabime', 'spdlog', '2026-07-18T00:00:00.000Z', 1),
                  ('acme', 'pending-dep', '2026-07-18T00:00:00.000Z', 1),
                  ('acme', 'rejected-dep', '2026-07-18T00:00:00.000Z', 1),
                  ('acme', 'bare-dep', '2026-07-18T00:00:00.000Z', 1),
                  ('acme', 'dev-dep', '2026-07-18T00:00:00.000Z', 1);
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
            '2026-07-18T00:00:00.000Z', 10, 1, 'verified'),
           ('alpha', 'target', '1.1.0', 'c02', 'c02', '{\"dependencies\":{}}',
            '2026-07-18T01:00:00.000Z', 10, 1, 'verified'),
           ('beta', 'target-pending', '1.0.0', 'c03', 'c03', '{\"dependencies\":{}}',
            '2026-07-18T00:00:00.000Z', 10, 1, 'pending'),
           ('alpha', 'my_pkg', '1.0.0', 'c04', 'c04', '{\"dependencies\":{}}',
            '2026-07-18T00:00:00.000Z', 10, 1, 'verified'),
           ('alpha', 'myxpkg', '1.0.0', 'c05', 'c05', '{\"dependencies\":{}}',
            '2026-07-18T00:00:00.000Z', 10, 1, 'verified'),
           ('gabime', 'spdlog', '1.13.0', 'c06', 'c06',
            '{\"dependencies\":{\"alpha/target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 1, 'verified'),
           ('gabime', 'spdlog', '1.14.0', 'c07', 'c07',
            '{\"dependencies\":{\"alpha/target\":{\"version\":\"^1\",\"optional\":true}}}',
            '2026-07-18T01:00:00.000Z', 10, 1, 'verified'),
           ('acme', 'pending-dep', '1.0.0', 'c08', 'c08',
            '{\"dependencies\":{\"alpha/target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 1, 'pending'),
           ('acme', 'rejected-dep', '1.0.0', 'c09', 'c09',
            '{\"dependencies\":{\"alpha/target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 1, 'rejected'),
           ('acme', 'bare-dep', '1.0.0', 'c10', 'c10',
            '{\"dependencies\":{\"target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 1, 'verified'),
           ('acme', 'dev-dep', '1.0.0', 'c11', 'c11',
            '{\"dependencies\":{},\"dev-dependencies\":{\"alpha/target\":\"^1\"}}',
            '2026-07-18T00:00:00.000Z', 10, 1, 'verified');",
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
           VALUES ('alpha', 'pkg', '1.1.0', 'cc', 'cc', '{}', '2026-07-15T01:00:00.000Z', 10, 1, 'verified'),
                  ('beta', 'pkg', '2.0.0', 'dd', 'dd', '{}', '2026-07-15T02:00:00.000Z', 10, 1, 'verified');
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
           VALUES ('alpha', 'foo-bar', '2026-07-15T00:00:00.000Z', 1)",
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
            rusqlite::params![scope, name, "2026-07-15T01:00:00.000Z", 1],
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
                1,
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
           VALUES ('alpha', 'foo_bar', '2026-07-15T00:00:00.000Z', 1)",
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
            rusqlite::params![scope, name, version, "ee", "ee", "{}", stamp, 40, 1, 0],
        )
        .expect("guarded revision insert")
    };

    // The winner's batch: package + version land, no live reference
    // to checksum 'ee' exists yet, so the accounting counts the bytes
    // and the insert applies.
    conn.execute(
        sql::INSERT_PACKAGE,
        rusqlite::params!["alpha", "foo-bar", "2026-07-15T01:00:00.000Z", 1],
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
            rusqlite::params!["alpha", "foo_bar", "2026-07-15T01:00:01.000Z", 1],
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
           VALUES ('alpha', 'zed', '2026-07-15T00:00:00.000Z', 1),
                  ('beta', 'arc', '2026-07-15T00:00:00.000Z', 1);
         INSERT INTO versions (scope, name, version) VALUES ('alpha', 'zed', '1.0.0');
         INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json,
                                published_at, archive_size, published_by, verification)
           VALUES ('alpha', 'zed', '1.0.0', 'ff', 'ff', '{}', '2026-07-15T00:00:00.000Z', 10, 1, 'rejected');",
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

/// The claim confusability read is a plain ordered name listing.
#[test]
fn scope_names_list_in_order() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let mut statement = conn.prepare(sql::LIST_SCOPE_NAMES).expect("prepare scopes");
    let names: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("run scopes")
        .collect::<Result<_, _>>()
        .expect("scope names");
    assert_eq!(names, ["alpha", "beta"]);
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
                1,
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
                1,
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
                1,
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
                1,
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
                1,
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
                1,
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
                 '2026-07-15T02:00:00.000Z', 10, 1, 'pending')",
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
                   '2026-07-15T02:00:00.000Z', 10, 1, 'pending');",
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
                1,
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
                1,
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
                   '2026-07-15T02:00:00.000Z', 10, 1, 'verified');",
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
                1,
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
                1,
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

/// The verified-backup queue derives its R2 key from the canonical
/// `sha256:<64 lowercase hex>` column value by stripping the
/// algorithm prefix, keeping the OCI-style `blobs/sha256/<hex>` key
/// layout stable across the prefixed-column migration.
#[test]
fn verified_backup_enqueue_strips_the_checksum_prefix() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let hex = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";
    let checksum = format!("sha256:{hex}");
    conn.execute(
        "UPDATE revisions SET checksum = ?1 WHERE scope = 'alpha'",
        rusqlite::params![checksum],
    )
    .expect("store the canonical spelling");
    let queued = conn
        .execute(
            sql::ENQUEUE_VERIFIED_BACKUP,
            rusqlite::params![
                "alpha",
                "pkg",
                "1.0.0",
                checksum,
                "2026-07-15T00:00:00.000Z",
                "2026-07-15T01:00:00.000Z",
                "aa"
            ],
        )
        .expect("enqueue the verified blob");
    assert_eq!(queued, 1);
    let key: String = conn
        .query_row("SELECT key FROM backup_pending", [], |row| row.get(0))
        .expect("queued key");
    assert_eq!(key, format!("blobs/sha256/{hex}"));
}
