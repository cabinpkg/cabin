//! The token surfaces: the trusted-publishing exchange, the `cabin
//! login` session mint, and self-revocation. JWT verification, config
//! matching, and the token formats live in `crate::trustpub`,
//! `crate::session_tokens`, and `crate::auth`; this is their D1
//! plumbing.

use serde::Deserialize;
use worker::{D1Database, Env, Request, Response, console_error, console_log};

use crate::auth::{self, AuthContext};
use crate::error;
use crate::glue::{
    MAX_MUTATION_BODY_BYTES, bounded_body, error_response, js_int, json_response, now_epoch_ms,
    unauthorized, write_gate,
};
use crate::{allowlist, session_tokens, sql, trustpub, web_glue};

use super::{oidc_admission, verifier_pins};

#[derive(Deserialize)]
struct OwnerRecord {
    user_id: i64,
}

/// The trusted-publishing exchange body, exactly `{"jwt": "<token>"}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeBody {
    jwt: String,
}

/// GitHub Actions OIDC JWTs run ~1.5-2.5 KB today. A dedicated cap
/// rather than [`MAX_MUTATION_BODY_BYTES`]: 4 KB leaves thin headroom
/// against GitHub growing its claim set, and a cap breach here is a
/// ports-publishing outage, not a client bug.
const MAX_EXCHANGE_BODY_BYTES: usize = 16 * 1024;

/// `PUT /api/v1/trusted_publishing/tokens`
/// (`docs/remote-registry.md`, "Trusted publishing"): exchanges a
/// verified GitHub Actions OIDC JWT for a short-lived multi-use
/// `trustpub` token, through one of two arms. [`oidc_admission`] runs
/// first of all - before the body is read - then the fully stateless
/// JWT verification (JWKS via Cache/network, never D1). Claims
/// matching the deployment-pinned verifier identity take the verifier
/// arm: a verify-scoped mint backed by the operator identity
/// `VERIFIER_BACKING_ACCOUNT_ID` names, deliberately in front of the
/// breaker's write gate - like the verdict route and the read plane's
/// verify exemption, the verification pipeline must be able to drain
/// the pending queue whatever the service mode. Everything else is the
/// config arm: the write gate (`503`, like every write - moved behind
/// the verification, so the gate now speaks only to callers presenting
/// a verifiable JWT, as it elsewhere speaks only to authenticated
/// ones), then config match and backing owner. Both arms end in
/// [`exchange_mint`]'s one transaction. Every refusal other than the
/// gate's answers the byte-identical uniform 401 - no
/// config/signature/replay oracle - with the real reason logged for
/// the operator. Returns the minted token row id so the request log
/// ties to the row.
pub(super) async fn trustpub_exchange_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
) -> worker::Result<(Response, Option<String>)> {
    if let Some(refusal) = oidc_admission(req, env).await? {
        return Ok((refusal, None));
    }
    let refused = |reason: String| -> worker::Result<(Response, Option<String>)> {
        console_log!("trustpub exchange refused: {reason}");
        Ok((unauthorized(env)?, None))
    };
    // A missing, oversized, or malformed body is an absent credential,
    // not a 400: the JWT in the body is the credential, and the refusal
    // stays the one uniform envelope.
    let Some(body) = bounded_body(req, MAX_EXCHANGE_BODY_BYTES).await? else {
        return refused("body over the cap".to_owned());
    };
    let Ok(ExchangeBody { jwt }) = serde_json::from_slice(&body) else {
        return refused("malformed body".to_owned());
    };

    // Worker clocks are epoch MILLISECONDS; the verifier takes seconds.
    #[allow(clippy::cast_possible_truncation)]
    let now_secs = (now_epoch_ms() / 1000.0) as i64;
    let claims = match trustpub::verify(
        &jwt,
        &trustpub::GithubJwks::from_env(env),
        trustpub::DEFAULT_AUDIENCE,
        now_secs,
    )
    .await
    {
        Ok(claims) => claims,
        Err(err) => return refused(format!("jwt verification failed: {err:?}")),
    };

    // The verifier arm. Missing or unparsable pins fail the arm closed
    // (never a default identity); the claims then fall through to the
    // config arm like anybody else's. The backing lookup runs before
    // the jti consume, like the config arm's owner lookup: the
    // identity is migration-seeded, so a missing row is a
    // var/seed mismatch, and the refusal must not burn the JWT so
    // the same run can retry once the mismatch is fixed.
    if let Some(pins) = verifier_pins(env)
        && pins.refuses(&claims).is_none()
    {
        let account = env
            .var("VERIFIER_BACKING_ACCOUNT_ID")
            .ok()
            .map(|value| value.to_string())
            .unwrap_or_default();
        if account.is_empty() {
            return refused("the VERIFIER_BACKING_ACCOUNT_ID var is unset".to_owned());
        }
        let backing: Option<OwnerRecord> = db
            .prepare(sql::VERIFIER_BACKING_USER)
            .bind(&[account.as_str().into()])?
            .first(None)
            .await?;
        let Some(backing) = backing else {
            return refused(format!(
                "the verifier backing account {account} has no identity row"
            ));
        };
        return exchange_mint(
            env,
            db,
            &claims,
            now_secs,
            backing.user_id,
            &pins.workflow_filename,
            ExchangeMint::Verify,
        )
        .await;
    }

    if let Some(blocked) = write_gate(env, db).await? {
        return Ok((blocked, None));
    }
    let configs: Vec<trustpub::TrustpubConfig> = db
        .prepare(sql::TRUSTPUB_CONFIGS_BY_REPOSITORY)
        .bind(&[
            js_int(claims.repository_owner_id),
            js_int(claims.repository_id),
        ])?
        .all()
        .await?
        .results()?;
    let config = match trustpub::select_config(&claims, &configs) {
        Ok(config) => config,
        Err(err) => return refused(err.to_string()),
    };

    // Before the jti consume on purpose: an unclaimed scope must not
    // burn the JWT, so claiming it and retrying the same run stays
    // possible while the token is still fresh.
    let owner: Option<OwnerRecord> = db
        .prepare(sql::TRUSTPUB_BACKING_OWNER)
        .bind(&[config.scope.as_str().into()])?
        .first(None)
        .await?;
    let Some(owner) = owner else {
        return refused(format!(
            "scope {} has no owner to back a token",
            config.scope
        ));
    };

    exchange_mint(
        env,
        db,
        &claims,
        now_secs,
        owner.user_id,
        &config.workflow_filename,
        ExchangeMint::Publish {
            scope: &config.scope,
            quota_class: &config.quota_class,
        },
    )
    .await
}

/// Which arm's token [`exchange_mint`] writes: the config arm's
/// scope-confined, quota-classed publish shape, or the verifier arm's
/// bare verify shape.
enum ExchangeMint<'a> {
    Publish {
        scope: &'a str,
        quota_class: &'a str,
    },
    Verify,
}

/// The exchange's terminal transaction, shared by both arms - the
/// spec's steps 6-8: the once-only jti consume, the mint (guarded
/// inside the SQL on the consume's `changes()`, so it must stay the
/// immediately following statement), then the lazy expiry prunes
/// (deliberately no cron). A replayed jti answers as zero consumed
/// rows without minting - the uniform 401; any batch failure rolls the
/// consume back with everything else, so a transient 500 never burns a
/// still-valid JWT.
async fn exchange_mint(
    env: &Env,
    db: &D1Database,
    claims: &trustpub::GithubClaims,
    now_secs: i64,
    user_id: i64,
    workflow_filename: &str,
    mint: ExchangeMint<'_>,
) -> worker::Result<(Response, Option<String>)> {
    let id = auth::hex(&web_glue::random_bytes::<16>()?);
    let token = auth::format_trustpub_token(&web_glue::random_bytes()?);
    let (created_at, expires_at) = token_window(trustpub::TOKEN_TTL_SECS);
    let name = format!("trusted-publishing: {workflow_filename}");
    let minted = match mint {
        ExchangeMint::Publish { scope, quota_class } => {
            db.prepare(sql::INSERT_TRUSTPUB_TOKEN).bind(&[
                id.as_str().into(),
                js_int(user_id),
                name.as_str().into(),
                auth::token_hash(&token).into(),
                created_at.as_str().into(),
                expires_at.as_str().into(),
                scope.into(),
                quota_class.into(),
            ])?
        }
        ExchangeMint::Verify => db.prepare(sql::INSERT_TRUSTPUB_VERIFY_TOKEN).bind(&[
            id.as_str().into(),
            js_int(user_id),
            name.as_str().into(),
            auth::token_hash(&token).into(),
            created_at.as_str().into(),
            expires_at.as_str().into(),
        ])?,
    };
    let results = db
        .batch(vec![
            db.prepare(sql::CONSUME_OIDC_JTI)
                .bind(&[claims.jti.as_str().into(), js_int(claims.verifiable_until)])?,
            minted,
            db.prepare(sql::PRUNE_EXPIRED_OIDC_JTIS)
                .bind(&[js_int(now_secs)])?,
            db.prepare(sql::PRUNE_EXPIRED_SHORT_LIVED_TOKENS)
                .bind(&[created_at.as_str().into()])?,
        ])
        .await?;
    let changed = |index: usize| -> usize {
        results
            .get(index)
            .and_then(|result| result.meta().ok().flatten())
            .and_then(|meta| meta.changes)
            .unwrap_or(0)
    };
    if changed(0) == 0 {
        console_log!("trustpub exchange refused: replayed jti");
        return Ok((unauthorized(env)?, None));
    }
    if changed(1) == 0 {
        // The guard misfired: the jti was consumed but no row was
        // stored, and the plaintext below would be a dead credential.
        console_error!("trustpub exchange consumed a jti without minting");
        return Ok((error_response(500, error::INTERNAL)?, None));
    }

    // The plaintext is rendered exactly once, here; D1 holds only the
    // SHA-256 hex.
    let body = serde_json::json!({ "token": token, "expires_at": expires_at }).to_string();
    Ok((json_response(&body)?, Some(id)))
}

/// A minted token's `(created_at, expires_at)` pair, exactly
/// `ttl_secs` apart from a single clock read.
fn token_window(ttl_secs: i64) -> (String, String) {
    let now_ms = now_epoch_ms();
    let iso = |ms: f64| {
        worker::js_sys::Date::new(&worker::wasm_bindgen::JsValue::from_f64(ms))
            .to_iso_string()
            .as_string()
            .unwrap_or_default()
    };
    #[allow(clippy::cast_precision_loss)] // both TTLs are exact in f64
    let ttl_ms = ttl_secs as f64 * 1000.0;
    (iso(now_ms), iso(now_ms + ttl_ms))
}

/// `DELETE /api/v1/trusted_publishing/tokens` and
/// `DELETE /api/v1/sessions/tokens`: the presented (already
/// authenticated) token revokes itself, iff it is the route's kind -
/// any other kind's id changes no rows and answers the same uniform
/// 401 as any invalid credential, so neither endpoint is a token-kind
/// oracle. Deletion also makes a repeat DELETE indistinguishable from
/// an unknown token (the 401 again), which is the documented
/// idempotent answer. Deliberately not behind the write gate, like the
/// verdict route: revocation removes a live credential, and blocking
/// it while over budget would keep that credential alive - the wrong
/// fail direction for a security operation.
pub(super) async fn self_revoke_response(
    env: &Env,
    statement: worker::D1PreparedStatement,
    auth: &AuthContext,
) -> worker::Result<Response> {
    let deleted = statement
        .bind(&[auth.token_id.as_str().into()])?
        .run()
        .await?;
    if deleted.meta()?.and_then(|meta| meta.changes).unwrap_or(0) == 0 {
        return unauthorized(env);
    }
    Ok(Response::empty()?.with_status(204))
}

/// The production [`session_tokens::GithubUserProvider`]: one
/// check-token call through [`web_glue::github_check_token`] (which
/// carries the User-Agent GitHub requires and honors the
/// `GITHUB_API_BASE` override), so a 200 also proves the token was
/// issued by the registry's own OAuth app - another app's grant for
/// the same account is a refusal, not a login. Every failure shape is
/// a reason string for the log - never the access token itself - and
/// the caller's one uniform 401.
struct GithubUserApi<'a> {
    env: &'a Env,
}

impl session_tokens::GithubUserProvider for GithubUserApi<'_> {
    async fn user_id(&self, github_token: &str) -> Result<i64, String> {
        let body = match web_glue::github_check_token(self.env, github_token).await {
            Ok(Some(body)) => body,
            Ok(None) => return Err("github check-token refused the token".to_owned()),
            Err(err) => return Err(format!("github check-token fetch failed: {err}")),
        };
        session_tokens::parse_check_token_user_id(&body)
            .ok_or_else(|| "github check-token body did not parse".to_owned())
    }
}

/// `PUT /api/v1/sessions/tokens` (`docs/remote-registry.md`, "Login
/// sessions"): trades a GitHub access token
/// for a 12-hour `session` bearer token. [`oidc_admission`] answers
/// first, then the write gate, both before the credential is even
/// read: the mint is a write that spends an outbound GitHub call, and
/// a doomed request must not buy one - the pre-auth 503 deliberately
/// tells an anonymous caller the breaker state, like the read plane's
/// public over-budget answer. Then the
/// GitHub check-token proof, the allowlist, and the identity lookup - the
/// exact resolution the web OAuth login uses, minus its account
/// creation: `cabin login` is for accounts that exist, so an unknown
/// or unallowlisted id is a refusal, never a signup. Every refusal
/// past the gate answers the byte-identical uniform 401 with the real
/// reason logged. Returns the minted row id so the request log ties to
/// the row.
pub(super) async fn session_mint_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
) -> worker::Result<(Response, Option<String>)> {
    if let Some(refusal) = oidc_admission(req, env).await? {
        return Ok((refusal, None));
    }
    if let Some(blocked) = write_gate(env, db).await? {
        return Ok((blocked, None));
    }
    let refused = |reason: String| -> worker::Result<(Response, Option<String>)> {
        console_log!("session mint refused: {reason}");
        Ok((unauthorized(env)?, None))
    };
    // A missing, oversized, or malformed body is an absent credential,
    // not a 400, like the trustpub exchange's.
    let Some(body) = bounded_body(req, MAX_MUTATION_BODY_BYTES).await? else {
        return refused("body over the cap".to_owned());
    };
    let github_id = match session_tokens::resolve_github_id(&body, &GithubUserApi { env }).await {
        Ok(github_id) => github_id,
        Err(reason) => return refused(reason),
    };
    let allowed = allowlist::parse_allowed_ids(&env.var("ALLOWED_GITHUB_IDS")?.to_string());
    if !allowed.contains(&github_id) {
        return refused(format!("github id {github_id} is not allowlisted"));
    }
    let Some(user) = web_glue::user_record(db, github_id).await? else {
        return refused(format!("github id {github_id} has never signed in"));
    };

    let id = auth::hex(&web_glue::random_bytes::<16>()?);
    let token = auth::format_session_token(&web_glue::random_bytes()?);
    let (created_at, expires_at) = token_window(session_tokens::TOKEN_TTL_SECS);
    db.batch(vec![
        db.prepare(sql::INSERT_SESSION_TOKEN).bind(&[
            id.as_str().into(),
            js_int(user.user_id),
            auth::token_hash(&token).into(),
            created_at.as_str().into(),
            expires_at.as_str().into(),
        ])?,
        db.prepare(sql::PRUNE_EXPIRED_SHORT_LIVED_TOKENS)
            .bind(&[created_at.as_str().into()])?,
    ])
    .await?;

    // The plaintext is rendered exactly once, here; D1 holds only the
    // SHA-256 hex.
    let body = serde_json::json!({ "token": token, "expires_at": expires_at }).to_string();
    Ok((json_response(&body)?, Some(id)))
}
