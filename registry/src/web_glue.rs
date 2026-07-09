//! Cloudflare glue for the browser plane: GitHub OAuth sign-in and the
//! `/me` token-management page (session-cookie auth, server-rendered
//! HTML). The bearer-token data plane lives in [`crate::glue`]; neither
//! plane accepts the other's credential. Sessions, GitHub access tokens,
//! and issued registry tokens are never logged.

use serde::Deserialize;
use worker::{
    D1Database, Env, Fetch, FormData, FormEntry, Headers, Method, Request, RequestInit, Response,
};

use crate::glue::now_iso8601;
use crate::routes::WebRoute;
use crate::{allowlist, auth, pages, session};

/// Routes one browser-plane request; the method check happens here, the
/// path already matched in [`crate::routes::match_web_route`].
pub async fn respond(
    req: &mut Request,
    env: &Env,
    route: WebRoute<'_>,
) -> worker::Result<Response> {
    let db = env.d1("DB")?;
    match (route, req.method()) {
        (WebRoute::Login, Method::Get) => login(env),
        (WebRoute::Callback, Method::Get) => callback(req, env, &db).await,
        (WebRoute::Me, Method::Get) => me(req, env, &db).await,
        (WebRoute::CreateToken, Method::Post) => create_token(req, env, &db).await,
        (WebRoute::RevokeToken { id }, Method::Post) => {
            let id = id.to_owned();
            revoke_token(req, env, &db, &id).await
        }
        _ => html_response(
            405,
            &pages::simple_page(
                "Method not allowed",
                "This page does not answer that method.",
            ),
        ),
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
    let cookie = set_cookie(session::STATE_COOKIE, &sealed, session::STATE_MAX_AGE_SECS);
    let location = format!(
        "https://github.com/login/oauth/authorize?client_id={client_id}&state={state}",
        client_id = url_encode(&client_id),
    );
    redirect_response(302, &location, &[cookie])
}

/// `GET /callback`: verify the `state` against the sealed cookie, trade
/// the `code` for an access token, read the numeric GitHub id, and admit
/// only allowlisted ids. The access token is used for that one `/user`
/// call and dropped. Every refusal is the same plain 403 with no account
/// details.
async fn callback(req: &Request, env: &Env, db: &D1Database) -> worker::Result<Response> {
    let secret = session_secret(env)?;
    // The state cookie is one-shot: cleared on every outcome.
    let clear_state = set_cookie(session::STATE_COOKIE, "", 0);

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
        return forbidden(&[clear_state]);
    };
    if state_param != expected_state {
        return forbidden(&[clear_state]);
    }

    let Some(user) = github_user_for_code(env, &code).await? else {
        return forbidden(&[clear_state]);
    };
    // The numeric id is the identity; the login name is display-only.
    let allowed = allowlist::parse_allowed_ids(&env.var("ALLOWED_GITHUB_IDS")?.to_string());
    if !allowed.contains(&user.id) {
        return forbidden(&[clear_state]);
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
    let cookie = set_cookie(
        session::SESSION_COOKIE,
        &sealed,
        session::SESSION_MAX_AGE_SECS,
    );
    redirect_response(302, "/me", &[cookie, clear_state])
}

/// `GET /me`: the token list plus the create-token form, session required.
async fn me(req: &Request, env: &Env, db: &D1Database) -> worker::Result<Response> {
    let Some(session) = session_from_request(req, env)? else {
        return redirect_response(302, "/login", &[]);
    };
    let user: Option<UserRecord> = db
        .prepare("SELECT login FROM users WHERE github_id = ?1")
        .bind(&[js_int(session.github_id)])?
        .first(None)
        .await?;
    let Some(user) = user else {
        return redirect_response(302, "/login", &[]);
    };
    let records: Vec<TokenListRecord> = db
        .prepare(
            "SELECT id, name, scopes, created_at, last_used_at, revoked_at \
             FROM tokens WHERE user_id = ?1 ORDER BY created_at DESC, id",
        )
        .bind(&[js_int(session.github_id)])?
        .all()
        .await?
        .results()?;
    let rows: Vec<pages::TokenRow> = records
        .into_iter()
        .map(|record| pages::TokenRow {
            id: record.id,
            name: record.name,
            scopes: record.scopes,
            created_at: record.created_at,
            last_used_at: record.last_used_at,
            revoked: record.revoked_at.is_some(),
        })
        .collect();
    let csrf = session::csrf_token(&session_secret(env)?, &session);
    html_response(200, &pages::me_page(&user.login, &rows, &csrf))
}

/// `POST /me/tokens`: issue a token. The plaintext is rendered exactly
/// once, on this response; D1 stores only the SHA-256 hex.
async fn create_token(req: &mut Request, env: &Env, db: &D1Database) -> worker::Result<Response> {
    let Some(session) = session_from_request(req, env)? else {
        return redirect_response(302, "/login", &[]);
    };
    let form = req.form_data().await?;
    if !csrf_ok(&session_secret(env)?, &session, &form) {
        return csrf_mismatch();
    }
    let name = field(&form, "name").map(|name| name.trim().to_owned());
    let Some(name) = name.filter(|name| !name.is_empty() && name.chars().count() <= 64) else {
        return html_response(
            400,
            &pages::simple_page("Invalid token name", "A token name is 1 to 64 characters."),
        );
    };
    let mut scopes = Vec::new();
    if field(&form, "scope_publish").is_some() {
        scopes.push("publish");
    }
    if field(&form, "scope_yank").is_some() {
        scopes.push("yank");
    }

    let token = auth::format_token(&random_bytes()?);
    db.prepare(
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(&[
        auth::hex(&random_bytes::<16>()?).into(),
        js_int(session.github_id),
        name.clone().into(),
        auth::token_hash(&token).into(),
        scopes.join(",").into(),
        now_iso8601().into(),
    ])?
    .run()
    .await?;
    html_response(200, &pages::token_created_page(&name, &token))
}

/// `POST /me/tokens/<id>/revoke`: idempotent, scoped to the session's own
/// tokens (a foreign or unknown id is a no-op), first `revoked_at` wins.
async fn revoke_token(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    id: &str,
) -> worker::Result<Response> {
    let Some(session) = session_from_request(req, env)? else {
        return redirect_response(302, "/login", &[]);
    };
    let form = req.form_data().await?;
    if !csrf_ok(&session_secret(env)?, &session, &form) {
        return csrf_mismatch();
    }
    db.prepare(
        "UPDATE tokens SET revoked_at = ?1 \
         WHERE id = ?2 AND user_id = ?3 AND revoked_at IS NULL",
    )
    .bind(&[now_iso8601().into(), id.into(), js_int(session.github_id)])?
    .run()
    .await?;
    redirect_response(303, "/me", &[])
}

#[derive(Deserialize)]
struct UserRecord {
    login: String,
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
    let body = format!(
        "client_id={}&client_secret={}&code={}",
        url_encode(&client_id),
        url_encode(&client_secret),
        url_encode(code),
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

fn session_secret(env: &Env) -> worker::Result<Vec<u8>> {
    Ok(env.secret("SESSION_SECRET")?.to_string().into_bytes())
}

fn csrf_ok(secret: &[u8], current: &session::Session, form: &FormData) -> bool {
    field(form, "csrf").is_some_and(|token| session::csrf_matches(secret, current, &token))
}

fn csrf_mismatch() -> worker::Result<Response> {
    html_response(
        403,
        &pages::simple_page(
            "Request rejected",
            "The form's session check failed; go back to /me and retry.",
        ),
    )
}

fn field(form: &FormData, name: &str) -> Option<String> {
    match form.get(name) {
        Some(FormEntry::Field(value)) => Some(value),
        _ => None,
    }
}

/// `Set-Cookie` value for a browser-plane cookie. `SameSite=Lax` keeps
/// cross-site POSTs cookie-less (the CSRF field is the second factor);
/// `Max-Age=0` clears.
fn set_cookie(name: &str, value: &str, max_age_secs: u64) -> String {
    format!("{name}={value}; Max-Age={max_age_secs}; Path=/; HttpOnly; Secure; SameSite=Lax")
}

/// Security headers on every browser-plane response: scripts and external
/// resources are locked out wholesale, and nothing (in particular `/me`,
/// `/callback`, and the one page carrying a plaintext token) is cached.
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

fn html_response(status: u16, body: &str) -> worker::Result<Response> {
    let mut response = Response::ok(body)?.with_status(status);
    let headers = response.headers_mut();
    headers.set("content-type", "text/html; charset=utf-8")?;
    web_headers(headers)?;
    Ok(response)
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

/// The uniform sign-in refusal: a plain 403 with no account details.
fn forbidden(cookies: &[String]) -> worker::Result<Response> {
    let mut response = html_response(
        403,
        &pages::simple_page(
            "Sign-in refused",
            "This registry restricts sign-in to an allowlist of GitHub accounts.",
        ),
    )?;
    for cookie in cookies {
        response.headers_mut().append("set-cookie", cookie)?;
    }
    Ok(response)
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

/// A numeric D1 binding. D1 has no `BigInt` support, so the id rides as a
/// float; GitHub ids sit far below 2^53, where f64 is exact.
#[allow(clippy::cast_precision_loss)]
fn js_int(value: i64) -> worker::wasm_bindgen::JsValue {
    worker::wasm_bindgen::JsValue::from_f64(value as f64)
}

fn url_encode(value: &str) -> String {
    String::from(worker::js_sys::encode_uri_component(value))
}

fn now_secs() -> u64 {
    worker::Date::now().as_millis() / 1000
}
