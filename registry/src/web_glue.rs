//! Cloudflare glue for the browser plane on the website origin: GitHub
//! OAuth sign-in (`/login`, `/callback`), the scope-claim flow's
//! dedicated OAuth roundtrip (`/claim/<scope>`, `/callback/claim`), and
//! the session-cookie user API (`/api/v1/user/*`) - JSON except the
//! source viewer's ranged byte reads ([`package_source`]). The
//! bearer-token planes live in [`crate::glue`]; no plane accepts
//! another's credential. Sessions, GitHub access tokens, and issued
//! registry tokens are never logged.

use serde::Deserialize;
use worker::{
    D1Database, Env, Fetch, Headers, Method, Request, RequestInit, Response, console_error,
};

use crate::glue::{js_int, non_negative, now_iso8601};
use crate::routes::{
    CLAIM_DENIED_REDIRECT, CLAIM_GRANTED_REDIRECT, LOGIN_DENIED_REDIRECT, POST_LOGIN_REDIRECT,
    STATS_PATH, SessionRoute, WebRoute,
};
use crate::{allowlist, auth, claim, error, quota, session, source, sql, stats, user_api};

/// The one identity provider policy admits today; the `identities`
/// schema stays provider-neutral (docs/architecture.md, "Two credential
/// planes").
const GITHUB_PROVIDER: &str = "github";

/// Routes one OAuth request; the method check happens here, the path
/// already matched in [`crate::routes::match_web_route`].
pub async fn respond_web(
    req: &Request,
    env: &Env,
    route: WebRoute<'_>,
) -> worker::Result<Response> {
    let db = env.d1("DB")?;
    match (route, req.method()) {
        (WebRoute::Login, Method::Get) => login(env),
        (WebRoute::Callback, Method::Get) => callback(req, env, &db).await,
        (WebRoute::Claim { scope }, Method::Get) => claim_start(env, scope),
        (WebRoute::ClaimCallback, Method::Get) => claim_callback(req, env, &db).await,
        _ => json_error(405, error::METHOD_NOT_ALLOWED),
    }
}

/// Routes one session-plane request: JSON in, JSON out. The session is
/// verified before anything else - method checks included - so an
/// unauthenticated request always gets the plain 401 envelope, never a
/// redirect (sending the browser to a sign-in page is the website
/// frontend's job) and never the Bearer plane's `WWW-Authenticate`
/// challenge (the planes stay distinguishable on purpose).
pub async fn respond_session(
    req: &mut Request,
    env: &Env,
    route: SessionRoute<'_>,
) -> worker::Result<Response> {
    let Some(session) = session_from_request(req, env)? else {
        return json_error(401, error::AUTH_REQUIRED);
    };
    // Logout needs no D1 state - and must work even when the session's
    // identity row is gone (the post-wipe ghost), where the endpoints
    // below answer 401: a valid cookie presented for logout is always
    // cleared.
    if route == SessionRoute::Logout {
        return match req.method() {
            Method::Post => logout(req),
            _ => json_error(405, error::METHOD_NOT_ALLOWED),
        };
    }
    let db = env.d1("DB")?;
    // The allowlist admitted the id, but its identity row may be gone
    // (the post-wipe scenario): every endpoint - the token routes
    // included - answers the same 401 as no session, never a phantom
    // empty listing or a foreign-key 500.
    let Some(user) = user_record(&db, session.github_id).await? else {
        return json_error(401, error::AUTH_REQUIRED);
    };
    match (route, req.method()) {
        (SessionRoute::User, Method::Get) => json_response(&user_api::user_json(
            session.github_id,
            &user.login_snapshot,
            &user.quota_class,
        )),
        (SessionRoute::Usage, Method::Get) => usage(&db, user).await,
        (SessionRoute::Packages, Method::Get) => list_packages(&db, user.user_id).await,
        (
            SessionRoute::PackageSource {
                scope,
                name,
                version,
            },
            Method::Get,
        ) => {
            let (scope, name, version) = (scope.to_owned(), name.to_owned(), version.to_owned());
            package_source(req, env, &db, &scope, &name, &version).await
        }
        (SessionRoute::Tokens, Method::Get) => list_tokens(&db, user.user_id).await,
        (SessionRoute::Tokens, Method::Post) => create_token(req, &db, user.user_id).await,
        (SessionRoute::RevokeToken { id }, Method::Post) => {
            let id = id.to_owned();
            revoke_token(req, &db, user.user_id, &id).await
        }
        (SessionRoute::ScopeMembers { scope }, Method::Get) => {
            let scope = scope.to_owned();
            list_scope_members(&db, user.user_id, &scope).await
        }
        (SessionRoute::ScopeMembers { scope }, Method::Post) => {
            let scope = scope.to_owned();
            add_scope_member(req, &db, user.user_id, &scope).await
        }
        (SessionRoute::RemoveScopeMember { scope, github_id }, Method::Post) => {
            let scope = scope.to_owned();
            remove_scope_member(req, &db, user.user_id, &scope, github_id).await
        }
        _ => json_error(405, error::METHOD_NOT_ALLOWED),
    }
}

/// The public stats plane (`docs/architecture.md`, "Download counts"):
/// the one unauthenticated JSON subtree on the website origin. Exactly
/// `GET /api/v1/stats` exists; unknown paths under the subtree are
/// public 404s (never the bearer plane's 401), and non-GET is 405.
pub async fn respond_stats(req: &Request, env: &Env, path: &str) -> worker::Result<Response> {
    if path != STATS_PATH {
        return json_error(404, error::NOT_FOUND);
    }
    if req.method() != Method::Get {
        return json_error(405, error::METHOD_NOT_ALLOWED);
    }
    stats_summary(env).await
}

#[derive(Deserialize)]
struct StatsRecord {
    packages: i64,
    versions: i64,
    downloads: i64,
}

/// The summary's edge-cache TTL. The `STATS_CACHE_TTL_SECS` env var
/// overrides it; 0 disables caching entirely (the smoke test pins 0 so
/// a fresh download's count is immediately observable).
const STATS_CACHE_TTL_SECS: u64 = 300;

/// `GET /api/v1/stats`: the registry-wide totals, served through the
/// Cache API under one fixed, query-less key - the canonical stats URL
/// on the website origin - so a request's own query string can never
/// bust the edge cache.
async fn stats_summary(env: &Env) -> worker::Result<Response> {
    let ttl_secs = env
        .var("STATS_CACHE_TTL_SECS")
        .ok()
        .and_then(|var| var.to_string().parse::<u64>().ok())
        .unwrap_or(STATS_CACHE_TTL_SECS);
    let origin = env.var("WEB_ORIGIN")?.to_string();
    let cache_url = format!("{}{}", origin.trim_end_matches('/'), STATS_PATH);
    let cache = worker::Cache::default();
    if ttl_secs > 0
        && let Some(cached) = cache.get(cache_url.as_str(), false).await?
    {
        return Ok(cached);
    }

    let db = env.d1("DB")?;
    let record: Option<StatsRecord> = db.prepare(sql::REGISTRY_STATS).first(None).await?;
    let totals = record.map_or(
        stats::RegistryTotals {
            packages: 0,
            versions: 0,
            downloads: 0,
        },
        |record| stats::RegistryTotals {
            packages: non_negative(record.packages),
            versions: non_negative(record.versions),
            downloads: non_negative(record.downloads),
        },
    );
    let mut response = json_response(&stats::summary_json(&totals))?;
    if ttl_secs > 0 {
        // Public and cacheable, overriding the browser plane's blanket
        // no-store; the Cache API only stores responses that carry a
        // max-age.
        response
            .headers_mut()
            .set("cache-control", &format!("public, max-age={ttl_secs}"))?;
        cache.put(cache_url.as_str(), response.cloned()?).await?;
    }
    Ok(response)
}

/// `POST /api/v1/user/logout`: clear the session cookie. Only a
/// `Set-Cookie` can remove it (it is `HttpOnly`), so signing out is a
/// session-plane mutation like any other, CSRF discipline included.
/// Sessions are stateless HMAC values, so the sealed value itself stays
/// verifiable until its expiry - clearing the browser's cookie is the
/// sign-out; the allowlist is the hard revocation lever.
fn logout(req: &Request) -> worker::Result<Response> {
    if !csrf_ok(req)? {
        return json_error(403, error::CSRF_REQUIRED);
    }
    let clear = session::set_cookie(session::SESSION_COOKIE, "", 0, session::SESSION_COOKIE_PATH);
    let mut response = json_response(r#"{"ok":true}"#)?;
    response.headers_mut().append("set-cookie", &clear)?;
    Ok(response)
}

/// `GET /login`: mint a random `state`, seal it into a short-lived cookie,
/// and send the browser to GitHub's authorize page. No extra OAuth scopes:
/// the public profile is all the callback reads.
fn login(env: &Env) -> worker::Result<Response> {
    let client_id = env.secret("GITHUB_CLIENT_ID")?.to_string();
    let state = auth::hex(&random_bytes::<16>()?);
    let sealed = session::seal_state(
        &session_secret(env)?,
        &state,
        now_secs() + session::STATE_MAX_AGE_SECS,
    );
    let cookie = session::set_cookie(
        session::STATE_COOKIE,
        &sealed,
        session::STATE_MAX_AGE_SECS,
        session::STATE_COOKIE_PATH,
    );
    // The explicit redirect_uri (recommended by GitHub, echoed in the
    // token exchange) pins the callback to the website origin.
    let location = format!(
        "{oauth_base}/login/oauth/authorize?client_id={client_id}&state={state}\
         &redirect_uri={redirect_uri}",
        oauth_base = github_oauth_base(env),
        client_id = url_encode(&client_id),
        redirect_uri = url_encode(&callback_url(env)?),
    );
    redirect_response(302, &location, &[cookie])
}

/// `GET /claim/<scope>`: start a scope claim's dedicated OAuth roundtrip.
/// Mirrors `/login`, with two deliberate differences: the sealed state
/// also carries the scope being claimed (so the callback grants exactly
/// what this route validated), and the authorize request asks for
/// `read:org` - the org-claim check reads the user's own membership -
/// while ordinary sign-in keeps its scopeless request.
fn claim_start(env: &Env, scope: &str) -> worker::Result<Response> {
    let client_id = env.secret("GITHUB_CLIENT_ID")?.to_string();
    let state = auth::hex(&random_bytes::<16>()?);
    let sealed = session::seal_claim_state(
        &session_secret(env)?,
        scope,
        &state,
        now_secs() + session::STATE_MAX_AGE_SECS,
    );
    let cookie = session::set_cookie(
        session::CLAIM_STATE_COOKIE,
        &sealed,
        session::STATE_MAX_AGE_SECS,
        session::CLAIM_STATE_COOKIE_PATH,
    );
    let location = format!(
        "{oauth_base}/login/oauth/authorize?client_id={client_id}&state={state}\
         &redirect_uri={redirect_uri}&scope={oauth_scope}",
        oauth_base = github_oauth_base(env),
        client_id = url_encode(&client_id),
        redirect_uri = url_encode(&claim_callback_url(env)?),
        oauth_scope = url_encode("read:org"),
    );
    redirect_response(302, &location, &[cookie])
}

/// `<WEB_ORIGIN>/callback`, the OAuth app's registered callback URL.
fn callback_url(env: &Env) -> worker::Result<String> {
    let origin = env.var("WEB_ORIGIN")?.to_string();
    Ok(format!("{}/callback", origin.trim_end_matches('/')))
}

/// `<WEB_ORIGIN>/callback/claim`, the claim flow's own callback: a
/// subdirectory of the registered callback URL, which is as far as a
/// GitHub OAuth app's `redirect_uri` may deviate from it.
fn claim_callback_url(env: &Env) -> worker::Result<String> {
    Ok(format!("{}/claim", callback_url(env)?))
}

/// GitHub's OAuth and API endpoints, overridable for the local smoke
/// test only (the `CF_API_BASE` pattern); deployed environments use the
/// real hosts.
fn github_oauth_base(env: &Env) -> String {
    env.var("GITHUB_OAUTH_BASE")
        .map_or_else(|_| "https://github.com".to_owned(), |var| var.to_string())
}

fn github_api_base(env: &Env) -> String {
    env.var("GITHUB_API_BASE").map_or_else(
        |_| "https://api.github.com".to_owned(),
        |var| var.to_string(),
    )
}

/// `GET /callback`: verify the `state` against the sealed cookie, trade
/// the `code` for an access token, read the numeric GitHub id, and admit
/// only allowlisted ids. The access token is used for that one `/user`
/// call and dropped. Every refusal is the same redirect to the website's
/// `/login/denied` page with no account details; success redirects to
/// `/dashboard`. Both targets are fixed relative paths
/// ([`crate::routes::POST_LOGIN_REDIRECT`]), never derived from request
/// input, so the callback cannot be turned into an open redirect.
async fn callback(req: &Request, env: &Env, db: &D1Database) -> worker::Result<Response> {
    let secret = session_secret(env)?;
    // The state cookie is one-shot: cleared on every outcome.
    let clear_state = session::set_cookie(session::STATE_COOKIE, "", 0, session::STATE_COOKIE_PATH);

    let url = req.url()?;
    let mut code = None;
    let mut state_param = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state_param = Some(value.into_owned()),
            _ => {}
        }
    }
    let expected_state = req
        .headers()
        .get("cookie")?
        .as_deref()
        .and_then(|header| session::cookie_value(header, session::STATE_COOKIE))
        .and_then(|sealed| session::open_state(&secret, sealed, now_secs()));
    let (Some(code), Some(state_param), Some(expected_state)) = (code, state_param, expected_state)
    else {
        return denied(&[clear_state]);
    };
    if state_param != expected_state {
        return denied(&[clear_state]);
    }

    // The access token is transient by design: one `/user` read, then
    // dropped at the end of this scope.
    let Some(access_token) = github_access_token(env, &code, &callback_url(env)?).await? else {
        return denied(&[clear_state]);
    };
    let Some(user) = github_user(env, &access_token).await? else {
        return denied(&[clear_state]);
    };
    // The numeric id is the identity; the login name is display-only.
    let allowed = allowlist::parse_allowed_ids(&env.var("ALLOWED_GITHUB_IDS")?.to_string());
    if !allowed.contains(&user.id) {
        return denied(&[clear_state]);
    }

    // The identity upsert: one transaction, user-creation first - the
    // identity insert reads its `last_insert_rowid()`. The account id is
    // bound as text on purpose (the column is TEXT, and a numeric D1
    // bind rides as a float, which would store "26405363.0").
    let account_id = user.id.to_string();
    db.batch(vec![
        db.prepare(sql::INSERT_USER_FOR_NEW_IDENTITY).bind(&[
            now_iso8601().into(),
            GITHUB_PROVIDER.into(),
            account_id.as_str().into(),
        ])?,
        db.prepare(sql::UPSERT_IDENTITY).bind(&[
            GITHUB_PROVIDER.into(),
            account_id.as_str().into(),
            user.login.into(),
        ])?,
    ])
    .await?;

    let expires_at = now_secs() + session::SESSION_MAX_AGE_SECS;
    let sealed = session::seal_session(&secret, user.id, expires_at);
    let cookie = session::set_cookie(
        session::SESSION_COOKIE,
        &sealed,
        session::SESSION_MAX_AGE_SECS,
        session::SESSION_COOKIE_PATH,
    );
    redirect_response(302, POST_LOGIN_REDIRECT, &[cookie, clear_state])
}

/// `GET /callback/claim`: the claim flow's OAuth callback. Verifies the
/// sealed claim state (which names the scope), then - with the transient
/// access token - proves control of the same-named GitHub account:
/// self-claim when the scope equals the authenticated user's lowercased
/// login, org claim when the user is an active admin of the same-named
/// organization ([`crate::claim`]). A granted claim freezes the scope to
/// the account's numeric id and seeds the claiming user as the first
/// `owner`, in one D1 batch; claims are permanent, so an already-claimed
/// scope refuses whoever asks - the original claimant included. Every
/// refusal is the same redirect to the denied target with no detail, and
/// the token is dropped at the end of this scope, never stored -
/// membership was proved *now*; the registry never holds GitHub
/// credentials.
async fn claim_callback(req: &Request, env: &Env, db: &D1Database) -> worker::Result<Response> {
    let secret = session_secret(env)?;
    // The claim-state cookie is one-shot: cleared on every outcome.
    let clear_state = session::set_cookie(
        session::CLAIM_STATE_COOKIE,
        "",
        0,
        session::CLAIM_STATE_COOKIE_PATH,
    );

    let url = req.url()?;
    let mut code = None;
    let mut state_param = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state_param = Some(value.into_owned()),
            _ => {}
        }
    }
    let opened = req
        .headers()
        .get("cookie")?
        .as_deref()
        .and_then(|header| session::cookie_value(header, session::CLAIM_STATE_COOKIE))
        .and_then(|sealed| session::open_claim_state(&secret, sealed, now_secs()));
    let (Some(code), Some(state_param), Some((scope, expected_state))) =
        (code, state_param, opened)
    else {
        return claim_denied(&[clear_state]);
    };
    if state_param != expected_state {
        return claim_denied(&[clear_state]);
    }

    let Some(access_token) = github_access_token(env, &code, &claim_callback_url(env)?).await?
    else {
        return claim_denied(&[clear_state]);
    };
    let Some(user) = github_user(env, &access_token).await? else {
        return claim_denied(&[clear_state]);
    };
    // Claiming is for signed-up users: the allowlist gate mirrors
    // sign-in, and the claimant must already have a registry account -
    // the seeded owner row needs its registry-native user id.
    let allowed = allowlist::parse_allowed_ids(&env.var("ALLOWED_GITHUB_IDS")?.to_string());
    if !allowed.contains(&user.id) {
        return claim_denied(&[clear_state]);
    }
    let Some(claimant) = user_record(db, user.id).await? else {
        return claim_denied(&[clear_state]);
    };

    // The numeric account id the scope string freezes to, from
    // `GET /users/<scope>` - bound before it is trusted: a self-claim
    // must resolve to the authenticated user themself, and an org claim
    // to the same organization the membership response proved the
    // claimant administers. Logins can be renamed and reassigned
    // between any two of these calls; the id equalities close that gap.
    let resolved = github_get(env, &access_token, &format!("/users/{scope}"))
        .await?
        .and_then(|body| claim::account_id(&body));
    let Some(proof_account_id) = resolved else {
        return claim_denied(&[clear_state]);
    };
    let bound = if claim::is_self_claim(&scope, &user.login) {
        proof_account_id == user.id
    } else {
        let membership_path = format!(
            "/orgs/{scope}/memberships/{login}",
            login = url_encode(&user.login)
        );
        github_get(env, &access_token, &membership_path)
            .await?
            .and_then(|body| claim::org_membership_grant(&body, user.id))
            == Some(proof_account_id)
    };
    if !bound {
        return claim_denied(&[clear_state]);
    }

    // Permanence pre-check: an already-claimed scope refuses before
    // touching anything, whoever asks.
    if scope_exists(db, &scope).await? {
        return claim_denied(&[clear_state]);
    }

    let account_id = proof_account_id.to_string();
    let claimed_at = now_iso8601();
    let batch = db
        .batch(vec![
            db.prepare(sql::CLAIM_SCOPE).bind(&[
                scope.as_str().into(),
                GITHUB_PROVIDER.into(),
                account_id.as_str().into(),
                claimed_at.as_str().into(),
            ])?,
            db.prepare(sql::SEED_CLAIM_OWNER)
                .bind(&[scope.as_str().into(), js_int(claimant.user_id)])?,
        ])
        .await;
    if let Err(err) = batch {
        // The batch is one transaction: the loser of a claim race fails
        // the primary-key insert and seeds nothing. When that is what
        // happened, refuse like any other claim of a taken scope;
        // anything else is a real error.
        if scope_exists(db, &scope).await? {
            return claim_denied(&[clear_state]);
        }
        return Err(err);
    }
    redirect_response(302, CLAIM_GRANTED_REDIRECT, &[clear_state])
}

/// The uniform claim refusal: whatever failed, the same redirect with no
/// detail.
fn claim_denied(cookies: &[String]) -> worker::Result<Response> {
    redirect_response(302, CLAIM_DENIED_REDIRECT, cookies)
}

#[derive(Deserialize)]
struct UserRecord {
    user_id: i64,
    login_snapshot: String,
    quota_class: String,
}

#[derive(Deserialize)]
struct CountRecord {
    n: i64,
}

/// Whether the scope is already claimed: the claim callback's
/// permanence check.
async fn scope_exists(db: &D1Database, scope: &str) -> worker::Result<bool> {
    let record: Option<CountRecord> = db
        .prepare(sql::SCOPE_EXISTS)
        .bind(&[scope.into()])?
        .first(None)
        .await?;
    Ok(record.is_some_and(|record| record.n > 0))
}

/// Whether the user holds the `owner` role in the scope: the gate on
/// every membership-management endpoint.
async fn scope_owner(db: &D1Database, scope: &str, user_id: i64) -> worker::Result<bool> {
    let record: Option<CountRecord> = db
        .prepare(sql::SCOPE_OWNER_MEMBERSHIP)
        .bind(&[scope.into(), js_int(user_id)])?
        .first(None)
        .await?;
    Ok(record.is_some_and(|record| record.n > 0))
}

#[derive(Deserialize)]
struct UsageRecord {
    stored_bytes: i64,
    published_today: i64,
    verified_count: i64,
    pending_count: i64,
    rejected_count: i64,
}

#[derive(Deserialize)]
struct PackageCountRecord {
    n: i64,
}

/// `GET /api/v1/user/usage`: the usage-and-quotas payload.
async fn usage(db: &D1Database, user: UserRecord) -> worker::Result<Response> {
    // "Today" is the UTC calendar day, the same lexicographic window the
    // publish quotas use; a non-ISO clock (impossible in practice) only
    // zeroes the today counter. The package count matches the quota
    // semantics: packages the user created, not merely published into.
    let now = now_iso8601();
    let day_prefix = quota::utc_day_prefix(&now).unwrap_or(&now);
    // Rejected versions keep their row (the verdict trail and the
    // republish carve-out live there) but their bytes were refunded, so
    // they are excluded from the stored sum; the per-status counts show
    // where everything the user published stands.
    let usage_record: Option<UsageRecord> = db
        .prepare(sql::USER_USAGE)
        .bind(&[js_int(user.user_id), day_prefix.into()])?
        .first(None)
        .await?;
    let usage_record = usage_record.unwrap_or(UsageRecord {
        stored_bytes: 0,
        published_today: 0,
        verified_count: 0,
        pending_count: 0,
        rejected_count: 0,
    });
    let package_record: Option<PackageCountRecord> = db
        .prepare(sql::USER_CREATED_PACKAGE_COUNT)
        .bind(&[js_int(user.user_id)])?
        .first(None)
        .await?;
    let usage = user_api::UsageInfo {
        quotas: quota::quotas_for_class(&user.quota_class),
        quota_class: user.quota_class,
        package_count: non_negative(package_record.map_or(0, |record| record.n)),
        stored_bytes: non_negative(usage_record.stored_bytes),
        published_today: non_negative(usage_record.published_today),
        verified_count: non_negative(usage_record.verified_count),
        pending_count: non_negative(usage_record.pending_count),
        rejected_count: non_negative(usage_record.rejected_count),
    };
    json_response(&user_api::usage_json(&usage))
}

#[derive(Deserialize)]
struct PackageVersionRecord {
    scope: String,
    name: String,
    version: String,
    verification: String,
    yanked: i64,
    published_at: String,
    downloads: i64,
}

/// `GET /api/v1/user/packages`: the packages the user created, every
/// version's verification and yanked state included. Each package is
/// listed under its canonical `<scope>/<name>` name. The ORDER BY keeps
/// the payload deterministic and the rows grouped by name for
/// [`user_api::packages_json`]; versions run newest first.
async fn list_packages(db: &D1Database, user_id: i64) -> worker::Result<Response> {
    let records: Vec<PackageVersionRecord> = db
        .prepare(sql::LIST_USER_PACKAGES)
        .bind(&[js_int(user_id)])?
        .all()
        .await?
        .results()?;
    let rows: Vec<user_api::PackageVersionRow> = records
        .into_iter()
        .map(|record| user_api::PackageVersionRow {
            name: format!("{}/{}", record.scope, record.name),
            version: record.version,
            verification: record.verification,
            yanked: record.yanked != 0,
            published_at: record.published_at,
            downloads: non_negative(record.downloads),
        })
        .collect();
    json_response(&user_api::packages_json(&rows))
}

#[derive(Deserialize)]
struct SourceVersionRecord {
    checksum: String,
    archive_size: i64,
}

/// `GET /api/v1/user/source/<scope>/<name>/<version>`: a ranged read of
/// a verified version's archive for the website's source viewer
/// (`docs/architecture.md`, "Origins and roles"). Any verified version
/// is readable - not only the session user's own packages, and yanked
/// stays viewable, both matching the artifact route - while pending,
/// rejected, corrupt-status, and missing rows are all the same 404 by
/// construction (the verified filter sits in the query). The `Range`
/// header is required (`400` without one), capped at
/// [`source::MAX_RANGE_BYTES`] (`416` otherwise), and resolved against
/// the row's stored archive size before R2 is consulted, so R2 never
/// sees an unsatisfiable range. A source read is never counted as a
/// download and never consults the service mode: it is a read, and
/// reads fail open (`docs/architecture.md`, "Download counts").
async fn package_source(
    req: &Request,
    env: &Env,
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    let range = match source::parse_range(req.headers().get("range")?.as_deref()) {
        Ok(range) => range,
        Err(refusal) => return json_error(refusal.status, refusal.detail),
    };
    let record: Option<SourceVersionRecord> = db
        .prepare(sql::SOURCE_VERSION_LOOKUP)
        .bind(&[scope.into(), name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(record) = record else {
        return json_error(404, error::NOT_FOUND);
    };
    let size = non_negative(record.archive_size);
    let Some(resolved) = source::resolve_range(range, size) else {
        let mut response = json_error(416, source::RANGE_UNSATISFIABLE)?;
        response
            .headers_mut()
            .set("content-range", &source::unsatisfiable_content_range(size))?;
        return Ok(response);
    };

    let key = format!("blobs/sha256/{}", record.checksum);
    let object = env
        .bucket("BLOBS")?
        .get(&key)
        .range(worker::Range::OffsetWithLength {
            offset: resolved.offset,
            length: resolved.length,
        })
        .execute()
        .await?;
    let Some(object) = object else {
        console_error!("blob {key} for {scope}/{name}@{version} is missing from R2");
        return json_error(500, error::INTERNAL);
    };
    // The row's archive_size resolved the range; if the blob disagrees
    // (a drift the content-addressed store should make impossible), the
    // headers below would lie about the bytes, so refuse instead.
    if object.size() != size {
        console_error!(
            "blob {key} for {scope}/{name}@{version} is {} bytes but the row says {size}",
            object.size()
        );
        return json_error(500, error::INTERNAL);
    }
    let Some(body) = object.body() else {
        console_error!("blob {key} for {scope}/{name}@{version} has no body");
        return json_error(500, error::INTERNAL);
    };
    // Buffered, not streamed: the cap keeps a slice comfortably in
    // memory, and only a fixed-length body makes the runtime emit the
    // exact Content-Length (a generic stream is re-framed as chunked).
    // The byte count doubles as the integrity check on the R2 read.
    let bytes = body.bytes().await?;
    if usize::try_from(resolved.length) != Ok(bytes.len()) {
        console_error!(
            "blob {key} for {scope}/{name}@{version} returned {} bytes for a {}-byte range",
            bytes.len(),
            resolved.length
        );
        return json_error(500, error::INTERNAL);
    }
    let mut response = Response::from_bytes(bytes)?.with_status(206);
    let headers = response.headers_mut();
    headers.set("content-type", "application/octet-stream")?;
    headers.set("accept-ranges", "bytes")?;
    headers.set("content-range", &source::content_range(resolved, size))?;
    web_headers(headers)?;
    Ok(response)
}

#[derive(Deserialize)]
struct TokenListRecord {
    id: String,
    name: String,
    scopes: String,
    created_at: String,
    last_used_at: Option<String>,
    revoked_at: Option<String>,
}

/// `GET /api/v1/user/tokens`: metadata only, never hashes.
async fn list_tokens(db: &D1Database, user_id: i64) -> worker::Result<Response> {
    let records: Vec<TokenListRecord> = db
        .prepare(sql::LIST_USER_TOKENS)
        .bind(&[js_int(user_id)])?
        .all()
        .await?
        .results()?;
    let rows: Vec<user_api::TokenRow> = records
        .into_iter()
        .map(|record| user_api::TokenRow {
            id: record.id,
            name: record.name,
            scopes: record.scopes,
            created_at: record.created_at,
            last_used_at: record.last_used_at,
            revoked: record.revoked_at.is_some(),
        })
        .collect();
    json_response(&user_api::tokens_json(&rows))
}

/// A session-plane mutation body cannot legitimately get anywhere near
/// this; the cap keeps a hostile session from making the Worker buffer
/// megabytes.
const MAX_SESSION_BODY_BYTES: usize = 4 * 1024;

/// Buffers a session-plane mutation body. `None` refuses an oversized
/// upload - before buffering when the client declared a length,
/// mirroring the publish handler, and re-checked after regardless (a
/// chunked body has no length).
async fn session_body(req: &mut Request) -> worker::Result<Option<Vec<u8>>> {
    if let Some(length) = req.headers().get("content-length")?
        && length
            .parse::<u64>()
            .is_ok_and(|n| n > MAX_SESSION_BODY_BYTES as u64)
    {
        return Ok(None);
    }
    let body = req.bytes().await?;
    Ok((body.len() <= MAX_SESSION_BODY_BYTES).then_some(body))
}

/// `POST /api/v1/user/tokens`: issue a token. The plaintext is rendered
/// exactly once, on this response; D1 stores only the SHA-256 hex.
async fn create_token(
    req: &mut Request,
    db: &D1Database,
    user_id: i64,
) -> worker::Result<Response> {
    if !csrf_ok(req)? {
        return json_error(403, error::CSRF_REQUIRED);
    }
    let Some(body) = session_body(req).await? else {
        return json_error(400, user_api::INVALID_CREATE_TOKEN_BODY);
    };
    let parsed = match user_api::parse_create_token(&body) {
        Ok(parsed) => parsed,
        Err(detail) => return json_error(400, detail),
    };

    let id = auth::hex(&random_bytes::<16>()?);
    let token = auth::format_token(&random_bytes()?);
    db.prepare(sql::INSERT_TOKEN)
        .bind(&[
            id.as_str().into(),
            js_int(user_id),
            parsed.name.as_str().into(),
            auth::token_hash(&token).into(),
            parsed.scopes.as_str().into(),
            now_iso8601().into(),
        ])?
        .run()
        .await?;
    let body = user_api::token_created_json(&id, &parsed.name, &parsed.scopes, &token);
    Ok(json_response(&body)?.with_status(201))
}

/// `POST /api/v1/user/tokens/<id>/revoke`: idempotent, scoped to the
/// session's own tokens (a foreign or unknown id is a no-op), first
/// `revoked_at` wins.
async fn revoke_token(
    req: &Request,
    db: &D1Database,
    user_id: i64,
    id: &str,
) -> worker::Result<Response> {
    if !csrf_ok(req)? {
        return json_error(403, error::CSRF_REQUIRED);
    }
    db.prepare(sql::REVOKE_TOKEN)
        .bind(&[now_iso8601().into(), id.into(), js_int(user_id)])?
        .run()
        .await?;
    json_response(r#"{"ok":true}"#)
}

#[derive(Deserialize)]
struct MemberRecord {
    provider_account_id: String,
    login_snapshot: String,
    role: String,
}

/// `GET /api/v1/user/scopes/<scope>/members`: the scope's members with
/// their GitHub ids and display logins. Owner-gated like every
/// membership-management endpoint, behind the uniform 403.
async fn list_scope_members(
    db: &D1Database,
    user_id: i64,
    scope: &str,
) -> worker::Result<Response> {
    if !scope_owner(db, scope, user_id).await? {
        return json_error(403, error::SCOPE_OWNER_REQUIRED);
    }
    let records: Vec<MemberRecord> = db
        .prepare(sql::LIST_SCOPE_MEMBERS)
        .bind(&[scope.into(), GITHUB_PROVIDER.into()])?
        .all()
        .await?
        .results()?;
    let rows: Vec<user_api::MemberRow> = records
        .into_iter()
        .map(|record| user_api::MemberRow {
            // Written from an i64 on sign-in; 0 would only make a
            // corrupted row visible, never hide it.
            github_id: record.provider_account_id.parse().unwrap_or(0),
            login: record.login_snapshot,
            role: record.role,
        })
        .collect();
    json_response(&user_api::members_json(&rows))
}

/// The target member's registry user and current role in the scope, by
/// GitHub id: members are managed by the external identity the claim
/// proofs speak, resolved through `identities` like the session itself.
async fn member_state(
    db: &D1Database,
    scope: &str,
    github_id: i64,
) -> worker::Result<Option<(i64, Option<String>)>> {
    let Some(target) = user_record(db, github_id).await? else {
        return Ok(None);
    };
    let role: Option<RoleRecord> = db
        .prepare(sql::SCOPE_MEMBER_ROLE)
        .bind(&[scope.into(), js_int(target.user_id)])?
        .first(None)
        .await?;
    Ok(Some((target.user_id, role.map(|record| record.role))))
}

#[derive(Deserialize)]
struct RoleRecord {
    role: String,
}

/// `POST /api/v1/user/scopes/<scope>/members`: add a member by GitHub
/// numeric id. The account must already have signed in - membership rows
/// key on the registry-native user - and an existing member keeps their
/// role (there is no role-change endpoint).
async fn add_scope_member(
    req: &mut Request,
    db: &D1Database,
    user_id: i64,
    scope: &str,
) -> worker::Result<Response> {
    if !csrf_ok(req)? {
        return json_error(403, error::CSRF_REQUIRED);
    }
    if !scope_owner(db, scope, user_id).await? {
        return json_error(403, error::SCOPE_OWNER_REQUIRED);
    }
    let Some(body) = session_body(req).await? else {
        return json_error(400, user_api::INVALID_ADD_MEMBER_BODY);
    };
    let parsed = match user_api::parse_add_member(&body) {
        Ok(parsed) => parsed,
        Err(detail) => return json_error(400, detail),
    };
    let Some((target_user_id, existing_role)) = member_state(db, scope, parsed.github_id).await?
    else {
        return json_error(400, error::MEMBER_HAS_NO_ACCOUNT);
    };
    db.prepare(sql::ADD_SCOPE_MEMBER)
        .bind(&[
            scope.into(),
            js_int(target_user_id),
            parsed.role.as_str().into(),
        ])?
        .run()
        .await?;
    let changed = existing_role.is_none();
    let role = existing_role.unwrap_or(parsed.role);
    json_response(&user_api::member_added_json(
        parsed.github_id,
        &role,
        changed,
    ))
}

/// `POST /api/v1/user/scopes/<scope>/members/<github_id>/remove`:
/// idempotent (an unknown account or a non-member is a `changed: false`
/// no-op), except that removing the last owner is refused - the SQL
/// guard enforces it under concurrency, and a removal it blocked answers
/// 409.
async fn remove_scope_member(
    req: &Request,
    db: &D1Database,
    user_id: i64,
    scope: &str,
    github_id: i64,
) -> worker::Result<Response> {
    if !csrf_ok(req)? {
        return json_error(403, error::CSRF_REQUIRED);
    }
    if !scope_owner(db, scope, user_id).await? {
        return json_error(403, error::SCOPE_OWNER_REQUIRED);
    }
    let Some((target_user_id, existing_role)) = member_state(db, scope, github_id).await? else {
        return json_response(&user_api::member_removed_json(github_id, false));
    };
    if existing_role.is_none() {
        return json_response(&user_api::member_removed_json(github_id, false));
    }
    db.prepare(sql::REMOVE_SCOPE_MEMBER)
        .bind(&[scope.into(), js_int(target_user_id)])?
        .run()
        .await?;
    // The membership after the guarded DELETE is the outcome: still
    // present means the last-owner rule blocked it.
    let remaining: Option<RoleRecord> = db
        .prepare(sql::SCOPE_MEMBER_ROLE)
        .bind(&[scope.into(), js_int(target_user_id)])?
        .first(None)
        .await?;
    if remaining.is_some() {
        return json_error(409, error::LAST_OWNER);
    }
    json_response(&user_api::member_removed_json(github_id, true))
}

/// GitHub's access-token endpoint answers errors as 200s with an error
/// body; a missing `access_token` field means the code was refused.
#[derive(Deserialize)]
struct AccessTokenResponse {
    access_token: Option<String>,
}

/// The two `/user` fields the callback reads. The numeric `id` - stable
/// across renames, unlike `login` - is the identity the allowlist keys on.
#[derive(Deserialize)]
struct GithubUser {
    id: i64,
    login: String,
}

/// Trades the OAuth `code` for an access token. `None` is the uniform
/// "GitHub said no" answer; only infrastructure errors surface as `Err`.
/// Callers hold the token for their few verification reads and drop it -
/// it is never stored and never logged.
async fn github_access_token(
    env: &Env,
    code: &str,
    redirect_uri: &str,
) -> worker::Result<Option<String>> {
    let client_id = env.secret("GITHUB_CLIENT_ID")?.to_string();
    let client_secret = env.secret("GITHUB_CLIENT_SECRET")?.to_string();
    // GitHub requires the exchange's redirect_uri to match the one the
    // authorize request carried.
    let body = format!(
        "client_id={}&client_secret={}&code={}&redirect_uri={}",
        url_encode(&client_id),
        url_encode(&client_secret),
        url_encode(code),
        url_encode(redirect_uri),
    );
    let headers = Headers::new();
    headers.set("accept", "application/json")?;
    headers.set("content-type", "application/x-www-form-urlencoded")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(body.into()));
    let url = format!("{}/login/oauth/access_token", github_oauth_base(env));
    let request = Request::new_with_init(&url, &init)?;
    let mut response = Fetch::Request(request).send().await?;
    if response.status_code() != 200 {
        return Ok(None);
    }
    let AccessTokenResponse { access_token } = response.json().await?;
    Ok(access_token)
}

/// One authenticated read against the GitHub API: the body on a 200,
/// `None` on any refusal.
async fn github_get(env: &Env, access_token: &str, path: &str) -> worker::Result<Option<Vec<u8>>> {
    let headers = Headers::new();
    headers.set("authorization", &format!("Bearer {access_token}"))?;
    headers.set("accept", "application/vnd.github+json")?;
    // GitHub's API rejects requests without a User-Agent.
    headers.set("user-agent", "cabin-registry")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers);
    let url = format!("{}{path}", github_api_base(env));
    let request = Request::new_with_init(&url, &init)?;
    let mut response = Fetch::Request(request).send().await?;
    if response.status_code() != 200 {
        return Ok(None);
    }
    Ok(Some(response.bytes().await?))
}

/// The authenticated user's numeric id and login; `None` covers both a
/// refused read and a body that does not parse.
async fn github_user(env: &Env, access_token: &str) -> worker::Result<Option<GithubUser>> {
    Ok(github_get(env, access_token, "/user")
        .await?
        .and_then(|body| serde_json::from_slice(&body).ok()))
}

/// The verified session presented by the request's cookie, if any. The
/// secret is only consulted once there is a cookie to verify, so a
/// session-less request never needs it. The allowlist is re-checked on
/// every request, so removing an id from `ALLOWED_GITHUB_IDS` locks its
/// sessions out immediately rather than at expiry.
fn session_from_request(req: &Request, env: &Env) -> worker::Result<Option<session::Session>> {
    let Some(header) = req.headers().get("cookie")? else {
        return Ok(None);
    };
    let Some(sealed) = session::cookie_value(&header, session::SESSION_COOKIE) else {
        return Ok(None);
    };
    let Some(current) = session::open_session(&session_secret(env)?, sealed, now_secs()) else {
        return Ok(None);
    };
    let allowed = allowlist::parse_allowed_ids(&env.var("ALLOWED_GITHUB_IDS")?.to_string());
    Ok(allowed.contains(&current.github_id).then_some(current))
}

/// The registry-native user a GitHub id resolves to through
/// `identities`; `None` for an account that never signed in - for a
/// session (the transient, post-wipe ghost case) that answers the same
/// 401 as no session, and the claim and membership planes refuse the
/// account.
async fn user_record(db: &D1Database, github_id: i64) -> worker::Result<Option<UserRecord>> {
    db.prepare(sql::USER_BY_IDENTITY)
        .bind(&[GITHUB_PROVIDER.into(), github_id.to_string().into()])?
        .first(None)
        .await
}

fn session_secret(env: &Env) -> worker::Result<Vec<u8>> {
    Ok(env.secret("SESSION_SECRET")?.to_string().into_bytes())
}

/// The JSON API's CSRF discipline ([`session::csrf_headers_ok`]).
fn csrf_ok(req: &Request) -> worker::Result<bool> {
    let content_type = req.headers().get("content-type")?;
    let csrf = req.headers().get(session::CSRF_HEADER)?;
    Ok(session::csrf_headers_ok(
        content_type.as_deref(),
        csrf.as_deref(),
    ))
}

/// Security headers on every browser-plane response: scripts and external
/// resources are locked out wholesale, and nothing (in particular the one
/// response carrying a plaintext token) is cached.
fn web_headers(headers: &mut Headers) -> worker::Result<()> {
    headers.set(
        "content-security-policy",
        "default-src 'none'; style-src 'unsafe-inline'",
    )?;
    headers.set("x-content-type-options", "nosniff")?;
    headers.set("referrer-policy", "no-referrer")?;
    headers.set("cache-control", "no-store")?;
    Ok(())
}

fn json_response(body: &str) -> worker::Result<Response> {
    let mut response = Response::ok(body)?;
    let headers = response.headers_mut();
    headers.set("content-type", "application/json")?;
    web_headers(headers)?;
    Ok(response)
}

fn json_error(status: u16, detail: &str) -> worker::Result<Response> {
    Ok(json_response(&error::envelope(detail))?.with_status(status))
}

fn redirect_response(status: u16, location: &str, cookies: &[String]) -> worker::Result<Response> {
    let mut response = Response::empty()?.with_status(status);
    let headers = response.headers_mut();
    headers.set("location", location)?;
    web_headers(headers)?;
    for cookie in cookies {
        headers.append("set-cookie", cookie)?;
    }
    Ok(response)
}

/// The uniform sign-in refusal: a redirect to the website's
/// `/login/denied` page with no account details.
fn denied(cookies: &[String]) -> worker::Result<Response> {
    redirect_response(302, LOGIN_DENIED_REDIRECT, cookies)
}

/// `N` bytes from the runtime CSPRNG (`crypto.getRandomValues`).
fn random_bytes<const N: usize>() -> worker::Result<[u8; N]> {
    use worker::js_sys::{Function, Reflect, Uint8Array};
    use worker::wasm_bindgen::{JsCast, JsValue};

    let crypto = Reflect::get(&worker::js_sys::global(), &JsValue::from_str("crypto"))?;
    let get_random_values: Function =
        Reflect::get(&crypto, &JsValue::from_str("getRandomValues"))?.dyn_into()?;
    let array = Uint8Array::new_with_length(u32::try_from(N).expect("length fits u32"));
    get_random_values.call1(&crypto, &array)?;
    let mut bytes = [0u8; N];
    array.copy_to(&mut bytes);
    Ok(bytes)
}

fn url_encode(value: &str) -> String {
    String::from(worker::js_sys::encode_uri_component(value))
}

fn now_secs() -> u64 {
    worker::Date::now().as_millis() / 1000
}
