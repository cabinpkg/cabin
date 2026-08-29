//! Executed-semantics tests for `src/sql/auth.rs`: identity sign-in,
//! bearer-token lookup, and the token shapes across kinds.

use cabin_registry_worker::sql;

use crate::common::{count, migrated_connection, resolve, sign_in};

#[test]
fn first_sign_in_creates_the_user_and_binds_the_identity() {
    let conn = migrated_connection();
    sign_in(
        &conn,
        "github",
        "424242",
        "mona",
        "2026-07-15T00:00:00.000Z",
    );
    // The migrated baseline already holds one user and one identity
    // (the seeded operator account), so every count here starts at 1.
    assert_eq!(count(&conn, "users"), 2);
    assert_eq!(count(&conn, "identities"), 2);
    let (user_id, login, quota_class) =
        resolve(&conn, "github", "424242").expect("identity resolves");
    assert_eq!(login, "mona");
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
        "424242",
        "mona",
        "2026-07-15T00:00:00.000Z",
    );
    let (user_id, _, _) = resolve(&conn, "github", "424242").expect("identity resolves");
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
        "424242",
        "renamed",
        "2026-07-15T02:00:00.000Z",
    );
    assert_eq!(count(&conn, "users"), 3);
    assert_eq!(count(&conn, "identities"), 3);
    let (resolved_id, login, _) = resolve(&conn, "github", "424242").expect("identity resolves");
    assert_eq!(resolved_id, user_id);
    assert_eq!(login, "renamed");
}

#[test]
fn distinct_accounts_get_distinct_users() {
    let conn = migrated_connection();
    sign_in(
        &conn,
        "github",
        "424242",
        "mona",
        "2026-07-15T00:00:00.000Z",
    );
    sign_in(
        &conn,
        "github",
        "583231",
        "octocat",
        "2026-07-15T01:00:00.000Z",
    );
    let (first, ..) = resolve(&conn, "github", "424242").expect("first identity");
    let (second, ..) = resolve(&conn, "github", "583231").expect("second identity");
    assert_ne!(first, second);
    assert_eq!(count(&conn, "users"), 3);
}

#[test]
fn identities_are_keyed_by_provider_and_account_never_login() {
    let conn = migrated_connection();
    sign_in(
        &conn,
        "github",
        "424242",
        "mona",
        "2026-07-15T00:00:00.000Z",
    );
    // The same numeric account id under another provider is a distinct
    // identity and a distinct user (the schema is provider-neutral even
    // though policy admits only GitHub today)...
    sign_in(&conn, "other", "424242", "mona", "2026-07-15T01:00:00.000Z");
    // ...and a login reused by a different account never merges
    // identities: logins are display-only snapshots.
    sign_in(
        &conn,
        "github",
        "583231",
        "mona",
        "2026-07-15T02:00:00.000Z",
    );
    assert_eq!(count(&conn, "users"), 4);
    assert_eq!(count(&conn, "identities"), 4);
    let (github_user, ..) = resolve(&conn, "github", "424242").expect("github identity");
    let (other_user, ..) = resolve(&conn, "other", "424242").expect("other-provider identity");
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
        "424242",
        "mona",
        "2026-07-15T00:00:00.000Z",
    );
    assert_eq!(resolve(&conn, "github", "583231"), None);
}

#[test]
fn the_migration_seeds_the_operator_pre_promoted() {
    // The baseline migration itself seeds the operator's account
    // (users.id 1, class 'operator') and its github identity, so the
    // class survives a wipe and the verifier arm's backing row exists
    // from apply time. Literal values on purpose: editing the seed must
    // fail this test.
    let conn = migrated_connection();
    assert_eq!(count(&conn, "users"), 1);
    assert_eq!(count(&conn, "identities"), 1);
    let (user_id, login, quota_class) =
        resolve(&conn, "github", "26405363").expect("seeded identity resolves");
    assert_eq!(user_id, 1);
    assert_eq!(login, "ken-matsui");
    assert_eq!(quota_class, "operator");
    let seeded_created_at = || -> String {
        conn.query_row("SELECT created_at FROM users WHERE id = 1", [], |row| {
            row.get(0)
        })
        .expect("seeded user row")
    };
    assert_eq!(seeded_created_at(), "2026-08-27T00:00:00.000Z");
    // A sign-in for the seeded identity binds to the seeded row instead
    // of creating a user: only `login_snapshot` changes.
    sign_in(
        &conn,
        "github",
        "26405363",
        "renamed",
        "2026-09-01T00:00:00.000Z",
    );
    assert_eq!(count(&conn, "users"), 1);
    let (resolved_id, login, quota_class) =
        resolve(&conn, "github", "26405363").expect("identity still resolves");
    assert_eq!(resolved_id, 1);
    assert_eq!(login, "renamed");
    assert_eq!(quota_class, "operator");
    assert_eq!(seeded_created_at(), "2026-08-27T00:00:00.000Z");
}

/// Creates a user and a session token row through the real statements
/// (the mint's own 12-hour TTL: issued 2026-07-15T00:00, expiring
/// 2026-07-15T12:00), returning the user id.
fn seed_token(conn: &rusqlite::Connection, token_id: &str, token_hash: &str) -> i64 {
    sign_in(conn, "github", "424242", "mona", "2026-07-15T00:00:00.000Z");
    let (user_id, ..) = resolve(conn, "github", "424242").expect("identity resolves");
    conn.execute(
        sql::INSERT_SESSION_TOKEN,
        rusqlite::params![
            token_id,
            user_id,
            token_hash,
            "2026-07-15T00:00:00.000Z",
            "2026-07-15T12:00:00.000Z"
        ],
    )
    .expect("insert session token");
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
    // The seeded session row expires at 2026-07-15T12:00:00.000Z.
    seed_token(&conn, "tok-1", "hash-1");

    let now = "2026-07-15T12:00:00.001Z";
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
        auth_lookup(&conn, "hash-1", "2026-07-15T12:00:00.000Z"),
        None
    );
    // One millisecond earlier it still authenticates.
    let live = auth_lookup(&conn, "hash-1", "2026-07-15T11:59:59.999Z").expect("not yet expired");
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
        "UPDATE tokens SET created_at = '9998-01-01T00:00:00.000Z', \
                           expires_at = '9998-01-01T12:00:00.000Z' WHERE id = 'tok-1'",
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
fn a_revoked_token_is_the_same_no_row_answer_as_an_unknown_one() {
    let conn = migrated_connection();
    seed_token(&conn, "tok-1", "hash-1");
    // The live session row delivers its stored columns: the full human
    // scope set, the owner's inherited class, no confinement.
    let (id, scopes, quota_class, scope_limit) =
        auth_lookup(&conn, "hash-1", "2026-07-15T06:00:00.000Z").expect("live row matches");
    assert_eq!(id, "tok-1");
    assert_eq!(scopes, "publish,yank,verify");
    assert_eq!(quota_class, "default");
    assert_eq!(scope_limit, None);
    // A revoked row answers exactly like an unknown hash, within its
    // expiry window included.
    conn.execute(
        "UPDATE tokens SET revoked_at = '2026-07-15T06:00:00.000Z' WHERE id = 'tok-1'",
        [],
    )
    .expect("revoke token");
    let now = "2026-07-15T06:00:00.001Z";
    assert_eq!(auth_lookup(&conn, "hash-1", now), None);
    assert_eq!(
        auth_lookup(&conn, "hash-1", now),
        auth_lookup(&conn, "no-such-hash", now)
    );
}

#[test]
fn a_scope_limited_token_authenticates_and_carries_its_limit() {
    // The 403 confinement itself lives in the glue
    // (`AuthContext::scope_limit_refuses`, unit-tested in src/auth.rs);
    // this pins that the lookup delivers the column the glue decides on.
    let conn = migrated_connection();
    let user_id = seed_token(&conn, "tok-1", "hash-1");
    // A confined row is the trustpub exchange's publish shape; the
    // session shape schema-forbids a scope_limit.
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at, scope_limit, quota_class) \
         VALUES ('tok-2', ?1, 'x', 'hash-2', 'publish', '2026-07-15T00:00:00.000Z', 'trustpub', \
                 '2026-07-15T00:30:00.000Z', 'cabin-ports', 'operator')",
        [user_id],
    )
    .expect("seed a confined trustpub row");
    let (.., scope_limit) =
        auth_lookup(&conn, "hash-2", "2026-07-15T00:10:00.000Z").expect("limited row matches");
    assert_eq!(scope_limit.as_deref(), Some("cabin-ports"));
}

#[test]
fn token_kind_domain_is_closed_in_the_schema() {
    let conn = migrated_connection();
    let user_id = seed_token(&conn, "tok-1", "hash-1");
    // 'user' is out of the domain with the legacy token plane: only the
    // machine-minted kinds remain.
    for kind in ["admin", "user"] {
        let err = conn
            .execute(
                &format!(
                    "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                                         kind) \
                     VALUES ('tok-2', ?1, 'x', 'hash-2', 'publish', \
                             '2026-07-15T00:00:00.000Z', '{kind}')"
                ),
                [user_id],
            )
            .expect_err(kind);
        assert!(err.to_string().contains("CHECK"), "{kind}: {err}");
    }
    // No DEFAULT survives either: a mint that forgets the column fails
    // outright instead of minting a kind outside the domain.
    let err = conn
        .execute(
            "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at) \
             VALUES ('tok-2', ?1, 'x', 'hash-2', 'publish', '2026-07-15T00:00:00.000Z')",
            [user_id],
        )
        .expect_err("kind omitted must fail NOT NULL");
    assert!(err.to_string().contains("NOT NULL"), "{err}");
    // The publish shape's positive side: a trustpub row with an expiry,
    // a scope limit, and a granted quota class is admitted.
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at, scope_limit, quota_class) \
         VALUES ('tok-3', ?1, 'x', 'hash-3', 'publish', '2026-07-15T00:00:00.000Z', 'trustpub', \
                 '2026-07-15T00:30:00.000Z', 'cabin-ports', 'operator')",
        [user_id],
    )
    .expect("a bounded publish-shape trustpub token is in the domain");
    // Short-lived, confined, and explicitly classed is schema-enforced
    // for publish-shape trustpub rows: a NULL expiry, a NULL or empty
    // scope limit, and a NULL quota class each fail the CHECK, whatever
    // the minting path does. Each case supplies every other required
    // column so it isolates exactly its own violation.
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
    // Two shapes exactly: any other scopes string - the verify scope on
    // the publish arm's columns included - fails the CHECK.
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

#[test]
fn the_verify_shape_is_bounded_unconfined_and_unclassed() {
    let conn = migrated_connection();
    let user_id = seed_token(&conn, "tok-1", "hash-1");
    // The verify shape's positive side: the verifier arm's mint is
    // bounded like the publish shape but unconfined and unclassed -
    // NULL quota_class inherits the backing user's class, exactly what
    // the static verify token's row carried.
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at) \
         VALUES ('tok-v', ?1, 'x', 'hash-v', 'verify', '2026-07-15T00:00:00.000Z', 'trustpub', \
                 '2026-07-15T00:30:00.000Z')",
        [user_id],
    )
    .expect("a bounded verify-shape trustpub token is in the domain");
    // The verify shape admits nothing beyond the bare scope: a scope
    // limit, a quota class, or a missing expiry each fail the CHECK, so
    // the verifier arm's mint can never carry a grant the static verify
    // token did not.
    for (label, columns, values) in [
        (
            "verify shape with a scope limit",
            "expires_at, scope_limit",
            "'2026-07-15T00:30:00.000Z', 'cabin-ports'",
        ),
        (
            "verify shape with a quota class",
            "expires_at, quota_class",
            "'2026-07-15T00:30:00.000Z', 'operator'",
        ),
        ("verify shape without an expiry", "scope_limit", "NULL"),
    ] {
        let err = conn
            .execute(
                &format!(
                    "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                                         kind, {columns}) \
                     VALUES ('tok-4', ?1, 'x', 'hash-4', 'verify', \
                             '2026-07-15T00:00:00.000Z', 'trustpub', {values})"
                ),
                [user_id],
            )
            .expect_err(label);
        assert!(err.to_string().contains("CHECK"), "{label}: {err}");
    }
    // The verify shape's scopes conjunct is itself load-bearing: an
    // unconfined, unclassed row with any OTHER scopes string satisfies
    // neither shape.
    for scopes in ["yank", "publish", ""] {
        let err = conn
            .execute(
                &format!(
                    "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                                         kind, expires_at) \
                     VALUES ('tok-4', ?1, 'x', 'hash-4', '{scopes}', \
                             '2026-07-15T00:00:00.000Z', 'trustpub', \
                             '2026-07-15T00:30:00.000Z')"
                ),
                [user_id],
            )
            .expect_err(scopes);
        assert!(err.to_string().contains("CHECK"), "{scopes}: {err}");
    }
}

/// The session shape CHECK: `kind = 'session'` means bounded (within a
/// day of issuance), carrying exactly the full human scope set,
/// unconfined, and unclassed - so a bug in the minting path can never
/// widen a login into a standing or confined credential, and a
/// long-lived session can never be inserted again once the legacy
/// token plane is gone.
#[test]
fn the_session_shape_is_bounded_unconfined_and_unclassed() {
    let conn = migrated_connection();
    let user_id = seed_token(&conn, "tok-1", "hash-1");
    // The positive side: the mint's own shape - a 12-hour window, the
    // full human scope set, nothing else - is admitted.
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at) \
         VALUES ('tok-s', ?1, 'x', 'hash-s', 'publish,yank,verify', \
                 '2026-07-15T00:00:00.000Z', 'session', '2026-07-15T12:00:00.000Z')",
        [user_id],
    )
    .expect("a bounded session token is in the domain");
    // The ceiling is one day from issuance, inclusive, like trustpub's.
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at) \
         VALUES ('tok-s2', ?1, 'x', 'hash-s2', 'publish,yank,verify', \
                 '2026-07-15T00:00:00.000Z', 'session', '2026-07-16T00:00:00.000Z')",
        [user_id],
    )
    .expect("the ceiling boundary is admitted");
    for (label, columns, values) in [
        ("no expiry", "scope_limit", "NULL"),
        (
            "past the ceiling",
            "expires_at",
            "'2026-07-16T00:00:00.001Z'",
        ),
        (
            "a scope limit",
            "expires_at, scope_limit",
            "'2026-07-15T12:00:00.000Z', 'cabin-ports'",
        ),
        (
            "a quota class",
            "expires_at, quota_class",
            "'2026-07-15T12:00:00.000Z', 'operator'",
        ),
    ] {
        let err = conn
            .execute(
                &format!(
                    "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                                         kind, {columns}) \
                     VALUES ('tok-s3', ?1, 'x', 'hash-s3', 'publish,yank,verify', \
                             '2026-07-15T00:00:00.000Z', 'session', {values})"
                ),
                [user_id],
            )
            .expect_err(label);
        assert!(err.to_string().contains("CHECK"), "{label}: {err}");
    }
    // The scopes conjunct is load-bearing: any other scopes string -
    // a narrowed set included - fails the CHECK.
    for scopes in ["publish", "publish,yank", "verify", ""] {
        let err = conn
            .execute(
                &format!(
                    "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                                         kind, expires_at) \
                     VALUES ('tok-s3', ?1, 'x', 'hash-s3', '{scopes}', \
                             '2026-07-15T00:00:00.000Z', 'session', \
                             '2026-07-15T12:00:00.000Z')"
                ),
                [user_id],
            )
            .expect_err(scopes);
        assert!(err.to_string().contains("CHECK"), "{scopes}: {err}");
    }
}

/// The lazy expiry cleanup the exchange and the session mint ride on
/// each mint: statement semantics `prepare` cannot see - the inclusive
/// `<=` boundary matching the auth lookup's strict `>` liveness across
/// both kinds, and the jti prune against the acceptance window's end.
#[test]
fn expiry_pruning_is_boundary_inclusive() {
    let conn = migrated_connection();
    // The seeded session row expired 2026-07-15T12:00 - long dead at
    // the prune instant below, so it goes with the boundary rows.
    let user_id = seed_token(&conn, "tok-seed", "hash-seed");
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
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at) \
         VALUES ('tok-ses-dead', ?1, 'x', 'hash-ses-dead', 'publish,yank,verify', \
                 '2026-08-14T12:00:00.000Z', 'session', '2026-08-15T00:00:00.000Z'), \
                ('tok-ses-live', ?1, 'x', 'hash-ses-live', 'publish,yank,verify', \
                 '2026-08-15T00:00:00.000Z', 'session', '2026-08-15T12:00:00.000Z')",
        [user_id],
    )
    .expect("seed session rows");

    // At the boundary instant the dead rows no longer authenticate
    // (the lookup's strict >), so the prune's inclusive <= removes
    // exactly the rows the auth plane already refuses - across both
    // kinds in the one statement.
    let now = "2026-08-15T00:00:00.000Z";
    let pruned = conn
        .execute(sql::PRUNE_EXPIRED_SHORT_LIVED_TOKENS, [now])
        .expect("prune tokens");
    assert_eq!(pruned, 3, "the seed row and the dead boundary rows");
    let survivors: Vec<String> = conn
        .prepare("SELECT id FROM tokens ORDER BY id")
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("list tokens")
        .collect::<Result<_, _>>()
        .expect("token ids");
    assert_eq!(survivors, ["tok-ses-live", "tok-tp-live"]);

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

/// The `kind` guards on [`sql::DELETE_TRUSTPUB_TOKEN`] and
/// [`sql::DELETE_SESSION_TOKEN`]: any other kind's id deletes nothing -
/// the zero the glue answers with the uniform 401, keeping each
/// endpoint no token-kind oracle. (The own-kind deletes are exercised
/// by the end-to-end flow tests in `src/trustpub.rs` and
/// `src/session_tokens.rs`.)
#[test]
fn self_revocation_never_crosses_token_kinds() {
    let conn = migrated_connection();
    let user_id = seed_token(&conn, "tok-ses", "hash-ses");
    conn.execute(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, kind, \
                             expires_at) \
         VALUES ('tok-tp', ?1, 'x', 'hash-tp', 'verify', \
                 '2026-07-15T00:00:00.000Z', 'trustpub', '2026-07-15T00:30:00.000Z')",
        [user_id],
    )
    .expect("seed a trustpub row");
    for (statement, foreign) in [
        (sql::DELETE_TRUSTPUB_TOKEN, "tok-ses"),
        (sql::DELETE_SESSION_TOKEN, "tok-tp"),
    ] {
        let deleted = conn
            .execute(statement, [foreign])
            .expect("guarded delete executes");
        assert_eq!(deleted, 0, "{statement} must not delete {foreign}");
    }
    assert!(
        auth_lookup(&conn, "hash-ses", "2026-07-15T06:00:00.000Z").is_some(),
        "the session token stays live"
    );
    assert!(
        auth_lookup(&conn, "hash-tp", "2026-07-15T00:10:00.000Z").is_some(),
        "the trustpub token stays live"
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
        "UPDATE tokens SET expires_at = '2026-07-15T18:00:00.000Z' WHERE id = 'tok-1'",
        [],
    )
    .expect("the canonical shape is admitted");
}
