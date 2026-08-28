//! Login sessions: the CLI's short-lived human credential
//! (`docs/remote-registry.md`, "Login sessions").
//!
//! `PUT /api/v1/sessions/tokens` trades a GitHub access token for a
//! 12-hour `session` bearer token. The
//! host-testable parts live here: the GitHub check-token proof behind
//! [`GithubUserProvider`] (the [`crate::trustpub::JwksProvider`]
//! pattern - tests inject a fake, no network) and the mint body's
//! parsing. The D1 statements and the uniform-401 wiring are the wasm
//! glue's job, mirrored by this module's tests running the same SQL in
//! the glue's order against the really-migrated schema.

use serde::Deserialize;

/// A session's lifetime: half a day, so a leaked credential dies the
/// same day while one login still covers a working session. The
/// schema's session CHECK caps the window at one day
/// (`migrations/0001_init.sql`), a ceiling like trustpub's - this
/// constant is the policy.
pub const TOKEN_TTL_SECS: i64 = 12 * 60 * 60;

/// The mint body, exactly `{"github_token": "<access token>"}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MintBody {
    github_token: String,
}

/// The check-token fields the mint reads: the token's user, and of the
/// user the numeric id alone - stable across renames, the identity the
/// allowlist and `identities` key on.
#[derive(Deserialize)]
struct CheckedToken {
    user: GithubUser,
}

#[derive(Deserialize)]
struct GithubUser {
    id: i64,
}

/// The source of the GitHub identity a presented access token proves.
/// Production is the wasm glue's check-token call against
/// `GITHUB_API_BASE`, which proves the token was issued by the
/// registry's own OAuth app - not merely that it reads as somebody -
/// so another app's grant for an allowlisted account can never become
/// a registry login. Tests substitute a fake.
// async fn without a Send bound is fine here: the Worker runtime is
// single-threaded, and the host-side consumer is the test executor.
#[allow(async_fn_in_trait)]
pub trait GithubUserProvider {
    /// The numeric GitHub id `github_token` authenticates as. `Err`
    /// carries the refusal reason for the operator log; every failure -
    /// a refused read, a network error or timeout, an unparsable body -
    /// must end in the caller's one uniform 401. Implementations never
    /// persist or log the access token itself.
    async fn user_id(&self, github_token: &str) -> Result<i64, String>;
}

/// Parses a check-token response body to the token user's numeric id;
/// unknown fields are GitHub's business.
pub fn parse_check_token_user_id(body: &[u8]) -> Option<i64> {
    serde_json::from_slice::<CheckedToken>(body)
        .ok()
        .map(|checked| checked.user.id)
}

/// The mint's credential resolution: request body to proven GitHub id.
/// Every refusal is a reason for the log, and the caller's one uniform
/// 401 - a malformed body is an absent credential, not a 400. The
/// GitHub access token lives only on this call's stack: never
/// persisted, never logged.
///
/// # Errors
///
/// The refusal reason, for the operator log only.
pub async fn resolve_github_id(
    body: &[u8],
    provider: &impl GithubUserProvider,
) -> Result<i64, String> {
    let Ok(MintBody { github_token }) = serde_json::from_slice(body) else {
        return Err("malformed body".to_owned());
    };
    // Graphic-ASCII only: no GitHub credential contains anything else,
    // so junk refuses here - before it can reach any outbound encoding
    // or spend the check-token call.
    if github_token.is_empty() || !github_token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("malformed body".to_owned());
    }
    provider.user_id(&github_token).await
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::*;
    use crate::{allowlist, auth, sql};

    /// The test provider's futures are always immediately ready, so one
    /// poll is the whole executor.
    fn block_on<F: Future>(future: F) -> F::Output {
        match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(output) => output,
            Poll::Pending => unreachable!("test futures never pend"),
        }
    }

    /// A fake GitHub: one known access token proving one numeric id,
    /// everything else refused - or a provider-level failure, the
    /// network-error case.
    enum FakeGithub {
        Knows { github_token: &'static str, id: i64 },
        Broken,
    }

    impl GithubUserProvider for FakeGithub {
        async fn user_id(&self, github_token: &str) -> Result<i64, String> {
            // The ready-future await keeps the trait's async signature
            // under clippy 1.98's `unused_async_trait_impl`, like the
            // trustpub `TestJwks` fake.
            std::future::ready(match self {
                Self::Knows {
                    github_token: known,
                    id,
                } if *known == github_token => Ok(*id),
                Self::Knows { .. } => Err("github check-token refused the token".to_owned()),
                Self::Broken => Err("github check-token fetch failed".to_owned()),
            })
            .await
        }
    }

    #[test]
    fn the_body_grammar_is_exact() {
        let provider = FakeGithub::Knows {
            github_token: "gho_abc",
            id: 42,
        };
        let resolve = |body: &str| block_on(resolve_github_id(body.as_bytes(), &provider));
        assert_eq!(resolve(r#"{"github_token": "gho_abc"}"#), Ok(42));
        // Unknown fields, a missing field, non-JSON, and a token that
        // could not be a GitHub token (empty, or carrying bytes no
        // GitHub credential contains) all read as an absent
        // credential.
        for body in [
            r#"{"github_token": "gho_abc", "scopes": ["publish"]}"#,
            r#"{"token": "gho_abc"}"#,
            r"{}",
            "not json",
            r#"{"github_token": ""}"#,
            "{\"github_token\": \"gho_abc\\r\\nx-injected: 1\"}",
            r#"{"github_token": "gho abc"}"#,
        ] {
            assert_eq!(
                resolve(body),
                Err("malformed body".to_owned()),
                "body: {body:?}"
            );
        }
    }

    #[test]
    fn the_provider_answer_passes_through_verbatim() {
        let provider = FakeGithub::Knows {
            github_token: "gho_abc",
            id: 42,
        };
        assert_eq!(
            block_on(resolve_github_id(
                br#"{"github_token": "gho_other"}"#,
                &provider
            )),
            Err("github check-token refused the token".to_owned())
        );
        assert_eq!(
            block_on(resolve_github_id(
                br#"{"github_token": "gho_abc"}"#,
                &FakeGithub::Broken
            )),
            Err("github check-token fetch failed".to_owned())
        );
    }

    #[test]
    fn the_check_token_body_parses_on_the_token_users_id_alone() {
        // GitHub's real check-token answer carries dozens of fields
        // around the nested token user.
        assert_eq!(
            parse_check_token_user_id(
                br#"{"scopes": [], "app": {"client_id": "Ov23x"}, "user": {"login": "octocat", "id": 583231, "type": "User"}}"#
            ),
            Some(583_231)
        );
        // A user-less 200 (a client-credentials shape) is no identity.
        assert_eq!(parse_check_token_user_id(br#"{"scopes": []}"#), None);
        assert_eq!(
            parse_check_token_user_id(br#"{"user": {"login": "octocat"}}"#),
            None
        );
        assert_eq!(parse_check_token_user_id(b"not json"), None);
    }

    /// The migrations applied to an in-memory database, as
    /// `tests/sql_validation.rs` applies them. Duplicated here (an
    /// integration test cannot reach unit-test fixtures, and the fake
    /// provider lives in this module) so the mint flow below runs end
    /// to end: the fake GitHub through [`resolve_github_id`], then the
    /// mint's real statements against the really-migrated schema.
    fn migrated_connection() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
        conn.pragma_update(None, "foreign_keys", true)
            .expect("enable foreign_keys");
        // D1 parity, like tests/sql_validation.rs's copy: patterns
        // evaluate under D1's 50-byte LIKE/GLOB cap here too, so a
        // statement this module exercises cannot pass on host defaults
        // while failing in production.
        conn.set_limit(
            rusqlite::limits::Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH,
            50,
        )
        .expect("pin the D1 LIKE/GLOB pattern limit");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut migrations: Vec<_> = std::fs::read_dir(&dir)
            .expect("read migrations/")
            .map(|entry| entry.expect("read migrations/ entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        migrations.sort();
        assert!(!migrations.is_empty(), "no migrations in {}", dir.display());
        for path in migrations {
            let statements = std::fs::read_to_string(&path).expect("read migration");
            conn.execute_batch(&statements).expect("apply migration");
        }
        conn
    }

    /// The publish path's bearer lookup, as `glue::authenticate` binds
    /// it: `(scopes, quota_class, scope_limit)` for the hash, if any.
    fn auth_lookup(
        conn: &rusqlite::Connection,
        token_hash: &str,
        now: &str,
    ) -> Option<(String, String, Option<String>)> {
        conn.query_row(
            sql::AUTH_TOKEN_LOOKUP,
            rusqlite::params![token_hash, now],
            |row| Ok((row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .expect("auth lookup")
    }

    /// The glue's mint transaction: the insert and the lazy prune,
    /// committed as one unit like a D1 batch.
    fn mint_batch(
        conn: &rusqlite::Connection,
        token_id: &str,
        user_id: i64,
        token_hash: &str,
        created_at: &str,
        expires_at: &str,
    ) -> rusqlite::Result<()> {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            sql::INSERT_SESSION_TOKEN,
            rusqlite::params![token_id, user_id, token_hash, created_at, expires_at],
        )?;
        tx.execute(sql::PRUNE_EXPIRED_SHORT_LIVED_TOKENS, [created_at])?;
        tx.commit()
    }

    /// One allowlisted GitHub identity (id 42) with a signed-in user
    /// row behind it, the mint's resolution fixture.
    fn seed_identity(conn: &rusqlite::Connection) {
        conn.execute_batch(
            "INSERT INTO users (id, created_at) VALUES (2, '2026-08-15T00:00:00.000Z');
             INSERT INTO identities (provider, provider_account_id, login_snapshot, user_id)
               VALUES ('github', '42', 'octocat', 2);",
        )
        .expect("seed the identity");
    }

    /// The registry-native user a GitHub id resolves to, as
    /// `web_glue::user_record` binds it.
    fn user_by_identity(conn: &rusqlite::Connection, github_id: i64) -> Option<i64> {
        conn.query_row(
            sql::USER_BY_IDENTITY,
            rusqlite::params!["github", github_id.to_string()],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .expect("identity lookup")
    }

    fn count_tokens(conn: &rusqlite::Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM tokens", [], |row| row.get(0))
            .expect("count tokens")
    }

    /// The mint end to end at the layer host tests can drive: the fake
    /// GitHub through [`resolve_github_id`], the allowlist gate, the
    /// identity resolution, and every D1 statement the glue executes -
    /// in the glue's order - against the migrated schema, through the
    /// publish path's auth check, expiry, and revocation. (The wasm
    /// handler's wiring - breaker gate, uniform 401, auth exemption -
    /// is `cargo registry-smoke`'s job.)
    #[test]
    fn the_session_flow_mints_authenticates_expires_and_revokes() {
        let conn = migrated_connection();
        seed_identity(&conn);

        let provider = FakeGithub::Knows {
            github_token: "gho_abc",
            id: 42,
        };
        let github_id = block_on(resolve_github_id(
            br#"{"github_token": "gho_abc"}"#,
            &provider,
        ))
        .expect("the fake proves the id");
        assert!(allowlist::parse_allowed_ids("0,42").contains(&github_id));
        let user_id = user_by_identity(&conn, github_id).expect("the identity resolves");
        assert_eq!(user_id, 2);

        let token = auth::format_session_token(&[7; 32]);
        let hash = auth::token_hash(&token);
        let created_at = "2026-08-15T00:00:00.000Z";
        let expires_at = "2026-08-15T12:00:00.000Z";
        mint_batch(&conn, "tok-ses-1", user_id, &hash, created_at, expires_at)
            .expect("the mint batch commits");

        // The publish path's auth check: within the TTL the token
        // authenticates with the full human scope set, unconfined, at
        // the owning user's class (the row's own quota_class is NULL;
        // COALESCE must answer the user's).
        let (scopes, quota_class, scope_limit) =
            auth_lookup(&conn, &hash, "2026-08-15T06:00:00.000Z").expect("the token is live");
        assert_eq!(scopes, "publish,yank,verify");
        assert_eq!(quota_class, "default");
        assert_eq!(scope_limit, None);

        // Expiry is boundary-inclusive refusal, and an expired session
        // is byte-for-byte the same no-row answer an unknown hash gets -
        // one lookup, one uniform 401, no oracle.
        assert_eq!(auth_lookup(&conn, &hash, "2026-08-15T12:00:00.000Z"), None);
        assert_eq!(
            auth_lookup(
                &conn,
                &auth::token_hash("cabin_ses_unknown"),
                "2026-08-15T06:00:00.000Z"
            ),
            None
        );

        // Revocation by the token itself: the first DELETE removes the
        // row, a repeat changes nothing - the glue reads that zero as
        // the same uniform 401, the documented idempotent answer.
        let deleted = conn
            .execute(sql::DELETE_SESSION_TOKEN, ["tok-ses-1"])
            .expect("delete the session");
        assert_eq!(deleted, 1);
        let repeat = conn
            .execute(sql::DELETE_SESSION_TOKEN, ["tok-ses-1"])
            .expect("repeat the delete");
        assert_eq!(repeat, 0);
        assert_eq!(auth_lookup(&conn, &hash, "2026-08-15T06:00:00.000Z"), None);
    }

    #[test]
    fn an_unknown_or_unallowlisted_identity_resolves_to_nothing() {
        let conn = migrated_connection();
        seed_identity(&conn);

        // A GitHub id that never signed in has no identity row: the
        // glue answers the uniform 401 before any mint statement runs.
        assert_eq!(user_by_identity(&conn, 977), None);
        // The allowlist gate runs first and an empty allowlist admits
        // nobody, the seeded identity included.
        assert!(!allowlist::parse_allowed_ids("").contains(&42));
        assert!(!allowlist::parse_allowed_ids("0,1,3").contains(&42));
        assert_eq!(count_tokens(&conn), 0);
    }

    #[test]
    fn a_github_failure_leaves_no_partial_state() {
        let conn = migrated_connection();
        seed_identity(&conn);

        // The provider failure - network error, timeout, refused read -
        // surfaces before any statement runs: the resolution is the
        // first fallible step and the tokens table stays untouched.
        let refused = block_on(resolve_github_id(
            br#"{"github_token": "gho_abc"}"#,
            &FakeGithub::Broken,
        ));
        assert_eq!(refused, Err("github check-token fetch failed".to_owned()));
        assert_eq!(count_tokens(&conn), 0);
    }

    /// The lazy prune sweeps expired trustpub AND session rows in one
    /// statement, boundary-inclusive, and never touches a live row.
    #[test]
    fn the_prune_sweeps_both_short_lived_kinds() {
        let conn = migrated_connection();
        seed_identity(&conn);
        conn.execute_batch(
            "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, expires_at, kind)
               VALUES ('ses-dead', 2, 'login session', 'h1', 'publish,yank,verify',
                       '2026-08-15T00:00:00.000Z', '2026-08-15T12:00:00.000Z', 'session');
             INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, expires_at, kind)
               VALUES ('ses-live', 2, 'login session', 'h2', 'publish,yank,verify',
                       '2026-08-15T06:00:00.000Z', '2026-08-15T18:00:00.000Z', 'session');
             INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at,
                                 expires_at, scope_limit, kind, quota_class)
               VALUES ('tp-dead', 2, 'tp', 'h3', 'publish',
                       '2026-08-15T00:00:00.000Z', '2026-08-15T00:30:00.000Z', 'smoke',
                       'trustpub', 'default');",
        )
        .expect("seed the three rows");

        let swept = conn
            .execute(
                sql::PRUNE_EXPIRED_SHORT_LIVED_TOKENS,
                ["2026-08-15T12:00:00.000Z"],
            )
            .expect("prune");
        assert_eq!(swept, 2);
        let survivor: String = conn
            .query_row("SELECT id FROM tokens", [], |row| row.get(0))
            .expect("list survivors");
        assert_eq!(survivor, "ses-live");
    }
}
