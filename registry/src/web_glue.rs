//! Cloudflare glue for the browser plane on the website origin: GitHub
//! OAuth sign-in (`/login`, `/callback`) and the session-cookie JSON user
//! API (`/api/v1/user/*`). The bearer-token planes live in
//! [`crate::glue`]; no plane accepts another's credential. Sessions,
//! GitHub access tokens, and issued registry tokens are never logged.

use serde::Deserialize;
use worker::{D1Database, Env, Fetch, Headers, Method, Request, RequestInit, Response};

use crate::glue::{js_int, non_negative, now_iso8601};
use crate::routes::{LOGIN_DENIED_REDIRECT, POST_LOGIN_REDIRECT, SessionRoute, WebRoute};
use crate::{allowlist, auth, error, quota, session, user_api};

/// Routes one OAuth request; the method check happens here, the path
/// already matched in [`crate::routes::match_web_route`].
pub async fn respond_web(req: &Request, env: &Env, route: WebRoute) -> worker::Result<Response> {
    let db = env.d1("DB")?;
    match (route, req.method()) {
        (WebRoute::Login, Method::Get) => login(env),
        (WebRoute::Callback, Method::Get) => callback(req, env, &db).await,
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
    let db = env.d1("DB")?;
    // The allowlist admitted the id, but its user row may be gone (the
    // dev-wipe scenario): every endpoint - the token routes included -
    // answers the same 401 as no session, never a phantom empty listing
    // or a foreign-key 500.
    let Some(user) = user_record(&db, &session).await? else {
        return json_error(401, error::AUTH_REQUIRED);
    };
    match (route, req.method()) {
        (SessionRoute::User, Method::Get) => json_response(&user_api::user_json(
            session.github_id,
            &user.login,
            &user.plan,
        )),
        (SessionRoute::Usage, Method::Get) => usage(&db, &session, user).await,
        (SessionRoute::Packages, Method::Get) => list_packages(&db, &session).await,
        (SessionRoute::Tokens, Method::Get) => list_tokens(&db, &session).await,
        (SessionRoute::Tokens, Method::Post) => create_token(req, &db, &session).await,
        (SessionRoute::RevokeToken { id }, Method::Post) => {
            let id = id.to_owned();
            revoke_token(req, &db, &session, &id).await
        }
        _ => json_error(405, error::METHOD_NOT_ALLOWED),
    }
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
        "https://github.com/login/oauth/authorize?client_id={client_id}&state={state}\
         &redirect_uri={redirect_uri}",
        client_id = url_encode(&client_id),
        redirect_uri = url_encode(&callback_url(env)?),
    );
    redirect_response(302, &location, &[cookie])
}

/// `<WEB_ORIGIN>/callback`, the one OAuth callback URL.
fn callback_url(env: &Env) -> worker::Result<String> {
    let origin = env.var("WEB_ORIGIN")?.to_string();
    Ok(format!("{}/callback", origin.trim_end_matches('/')))
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

    let Some(user) = github_user_for_code(env, &code).await? else {
        return denied(&[clear_state]);
    };
    // The numeric id is the identity; the login name is display-only.
    let allowed = allowlist::parse_allowed_ids(&env.var("ALLOWED_GITHUB_IDS")?.to_string());
    if !allowed.contains(&user.id) {
        return denied(&[clear_state]);
    }

    db.prepare(
        "INSERT INTO users (github_id, login, created_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT (github_id) DO UPDATE SET login = excluded.login",
    )
    .bind(&[js_int(user.id), user.login.into(), now_iso8601().into()])?
    .run()
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

#[derive(Deserialize)]
struct UserRecord {
    login: String,
    plan: String,
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
async fn usage(
    db: &D1Database,
    session: &session::Session,
    user: UserRecord,
) -> worker::Result<Response> {
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
        .prepare(
            "SELECT COALESCE(SUM(CASE WHEN verification != 'rejected' \
             THEN archive_size ELSE 0 END), 0) AS stored_bytes, \
             COALESCE(SUM(CASE WHEN published_at >= ?2 THEN 1 ELSE 0 END), 0) AS published_today, \
             COALESCE(SUM(verification = 'verified'), 0) AS verified_count, \
             COALESCE(SUM(verification = 'pending'), 0) AS pending_count, \
             COALESCE(SUM(verification = 'rejected'), 0) AS rejected_count \
             FROM versions WHERE published_by = ?1",
        )
        .bind(&[js_int(session.github_id), day_prefix.into()])?
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
        .prepare("SELECT COUNT(*) AS n FROM packages WHERE created_by = ?1")
        .bind(&[js_int(session.github_id)])?
        .first(None)
        .await?;
    let usage = user_api::UsageInfo {
        quotas: quota::quotas_for_plan(&user.plan),
        plan: user.plan,
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
    name: String,
    version: String,
    verification: String,
    yanked: i64,
    published_at: String,
}

/// `GET /api/v1/user/packages`: the packages the user created, every
/// version's verification and yanked state included. The ORDER BY keeps
/// the payload deterministic and the rows grouped by name for
/// [`user_api::packages_json`]; versions run newest first.
async fn list_packages(db: &D1Database, session: &session::Session) -> worker::Result<Response> {
    let records: Vec<PackageVersionRecord> = db
        .prepare(
            "SELECT v.name, v.version, v.verification, v.yanked, v.published_at \
             FROM packages p JOIN versions v ON v.name = p.name \
             WHERE p.created_by = ?1 \
             ORDER BY v.name, v.published_at DESC, v.version",
        )
        .bind(&[js_int(session.github_id)])?
        .all()
        .await?
        .results()?;
    let rows: Vec<user_api::PackageVersionRow> = records
        .into_iter()
        .map(|record| user_api::PackageVersionRow {
            name: record.name,
            version: record.version,
            verification: record.verification,
            yanked: record.yanked != 0,
            published_at: record.published_at,
        })
        .collect();
    json_response(&user_api::packages_json(&rows))
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
async fn list_tokens(db: &D1Database, session: &session::Session) -> worker::Result<Response> {
    let records: Vec<TokenListRecord> = db
        .prepare(
            "SELECT id, name, scopes, created_at, last_used_at, revoked_at \
             FROM tokens WHERE user_id = ?1 ORDER BY created_at DESC, id",
        )
        .bind(&[js_int(session.github_id)])?
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

/// A create-token body cannot legitimately get anywhere near this; the
/// cap keeps a hostile session from making the Worker buffer megabytes.
const MAX_SESSION_BODY_BYTES: usize = 4 * 1024;

/// `POST /api/v1/user/tokens`: issue a token. The plaintext is rendered
/// exactly once, on this response; D1 stores only the SHA-256 hex.
async fn create_token(
    req: &mut Request,
    db: &D1Database,
    session: &session::Session,
) -> worker::Result<Response> {
    if !csrf_ok(req)? {
        return json_error(403, error::CSRF_REQUIRED);
    }
    // Reject an oversized upload before buffering when the client
    // declared a length, mirroring the publish handler; the buffered
    // size is re-checked regardless (a chunked body has no length).
    if let Some(length) = req.headers().get("content-length")?
        && length
            .parse::<u64>()
            .is_ok_and(|n| n > MAX_SESSION_BODY_BYTES as u64)
    {
        return json_error(400, user_api::INVALID_CREATE_TOKEN_BODY);
    }
    let body = req.bytes().await?;
    if body.len() > MAX_SESSION_BODY_BYTES {
        return json_error(400, user_api::INVALID_CREATE_TOKEN_BODY);
    }
    let parsed = match user_api::parse_create_token(&body) {
        Ok(parsed) => parsed,
        Err(detail) => return json_error(400, detail),
    };

    let id = auth::hex(&random_bytes::<16>()?);
    let token = auth::format_token(&random_bytes()?);
    db.prepare(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&[
        id.as_str().into(),
        js_int(session.github_id),
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
    session: &session::Session,
    id: &str,
) -> worker::Result<Response> {
    if !csrf_ok(req)? {
        return json_error(403, error::CSRF_REQUIRED);
    }
    db.prepare(
        "UPDATE tokens SET revoked_at = ?1 \
         WHERE id = ?2 AND user_id = ?3 AND revoked_at IS NULL",
    )
    .bind(&[now_iso8601().into(), id.into(), js_int(session.github_id)])?
    .run()
    .await?;
    json_response(r#"{"ok":true}"#)
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

/// Trades the OAuth `code` for the user's numeric id and login. `None` is
/// the uniform "GitHub said no" answer; only infrastructure errors
/// surface as `Err`. The access token never leaves this function.
async fn github_user_for_code(env: &Env, code: &str) -> worker::Result<Option<GithubUser>> {
    let client_id = env.secret("GITHUB_CLIENT_ID")?.to_string();
    let client_secret = env.secret("GITHUB_CLIENT_SECRET")?.to_string();
    // GitHub requires the exchange's redirect_uri to match the one the
    // authorize request carried.
    let body = format!(
        "client_id={}&client_secret={}&code={}&redirect_uri={}",
        url_encode(&client_id),
        url_encode(&client_secret),
        url_encode(code),
        url_encode(&callback_url(env)?),
    );
    let headers = Headers::new();
    headers.set("accept", "application/json")?;
    headers.set("content-type", "application/x-www-form-urlencoded")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(body.into()));
    let request = Request::new_with_init("https://github.com/login/oauth/access_token", &init)?;
    let mut response = Fetch::Request(request).send().await?;
    if response.status_code() != 200 {
        return Ok(None);
    }
    let AccessTokenResponse { access_token } = response.json().await?;
    let Some(access_token) = access_token else {
        return Ok(None);
    };

    let headers = Headers::new();
    headers.set("authorization", &format!("Bearer {access_token}"))?;
    headers.set("accept", "application/vnd.github+json")?;
    // GitHub's API rejects requests without a User-Agent.
    headers.set("user-agent", "cabin-registry")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers);
    let request = Request::new_with_init("https://api.github.com/user", &init)?;
    let mut response = Fetch::Request(request).send().await?;
    if response.status_code() != 200 {
        return Ok(None);
    }
    Ok(Some(response.json().await?))
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

/// The session's D1 user row; `None` (the transient, post-wipe case of a
/// session whose user row is gone) answers the same 401 as no session.
async fn user_record(
    db: &D1Database,
    session: &session::Session,
) -> worker::Result<Option<UserRecord>> {
    db.prepare("SELECT login, plan FROM users WHERE github_id = ?1")
        .bind(&[js_int(session.github_id)])?
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
