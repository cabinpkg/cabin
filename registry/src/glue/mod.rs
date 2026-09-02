//! Cloudflare-specific glue: binding access, D1/R2 I/O, and response
//! plumbing. Everything with behavior worth testing lives in the host-target
//! modules; keep this module thin. This file holds the entrypoints, the
//! hostname-role dispatch, authentication, and the helpers every plane
//! shares; the planes live in [`read`] (registry domain), [`bearer`]
//! (website-origin mutation/admin), and [`cron`] (breaker budgets and
//! governor reconciliation).

mod bearer;
mod cron;
mod read;

use std::cell::Cell;

use futures_util::StreamExt;
use serde::Deserialize;
use worker::{
    Context, D1Database, Env, Fetch, Headers, Method, Request, RequestInit, Response,
    ScheduleContext, ScheduledEvent, console_error, console_log, event,
};

use crate::auth::{self, AuthContext, Scope};
use crate::error;
use crate::governor::{self, Consume, Decision, OpPool, Refusal, StoragePool};
use crate::{breaker, quota, sql};

use bearer::handle_website;
use cron::{evaluate_budgets, reconcile_governor};
use read::handle_registry;

const GENERATION_HEADER: &str = "x-cabin-registry-generation";

/// Mutation JSON is tiny. This shared cap covers the bearer and session
/// planes; publish passes its larger protocol limit to [`bounded_body`].
pub(crate) const MAX_MUTATION_BODY_BYTES: usize = 4 * 1024;

#[derive(Deserialize)]
struct TokenRecord {
    id: String,
    user_id: i64,
    scopes: String,
    quota_class: String,
    scope_limit: Option<String>,
    user_quota_class: String,
    rl_tokens: Option<f64>,
    rl_updated_at: Option<String>,
}

#[derive(Deserialize)]
struct MetaRecord {
    value: String,
}

#[event(fetch)]
pub async fn fetch(mut req: Request, env: Env, ctx: Context) -> worker::Result<Response> {
    let request_id = request_id(&req);
    let method = req.method();
    let path = req.path();

    let (response, token_id) = match handle(&mut req, &env, &ctx).await {
        Ok(handled) => handled,
        Err(err) => {
            console_error!("req={request_id} internal error: {err}");
            (error_response(500, error::INTERNAL)?, None)
        }
    };
    // The token row id is safe to log; the token and its hash never are.
    console_log!(
        "req={request_id} method={method} path={path} status={status} token={token}",
        method = method.as_ref(),
        status = response.status_code(),
        token = token_id.as_deref().unwrap_or("-"),
    );
    Ok(response)
}

/// Routes one request by hostname role (`docs/architecture.md`, "Origins
/// and roles"): the registry custom domain serves only the machine read
/// plane; the website origin serves the OAuth, session, and Bearer
/// mutation planes. Returns the response plus the authenticated token row
/// id for logging.
async fn handle(
    req: &mut Request,
    env: &Env,
    ctx: &Context,
) -> worker::Result<(Response, Option<String>)> {
    let path = req.path();
    // The Host header, not `req.url()`: the edge routes on it, and the
    // local `wrangler dev` proxy rewrites the URL's authority while
    // preserving the header (which is how `cargo registry-smoke`
    // exercises both roles on one server).
    let host = req.headers().get("host")?.unwrap_or_default();
    let host = crate::routes::host_without_port(&host);
    match crate::routes::role_for_host(host, &web_host(env)) {
        crate::routes::Role::Registry => handle_registry(req, env, ctx, &path).await,
        crate::routes::Role::Website => handle_website(req, env, ctx, &path).await,
    }
}

/// Reads at most `limit` request-body bytes without asking the runtime to
/// materialize the complete body. The declared length is an early refusal;
/// the streaming count is authoritative for chunked or dishonest requests,
/// and a body it refuses is still drained rather than abandoned.
pub(crate) async fn bounded_body(
    req: &mut Request,
    limit: usize,
) -> worker::Result<Option<Vec<u8>>> {
    if let Some(length) = req.headers().get("content-length")?
        && length
            .parse::<u64>()
            .is_ok_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
    {
        return Ok(None);
    }
    if req.inner().body().is_none() {
        return Ok(Some(Vec::new()));
    }

    let mut body = Vec::new();
    let mut stream = req.stream()?;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if chunk.len() > limit.saturating_sub(body.len()) {
            // Refused, but read to its end so the connection carrying it
            // stays usable: a chunked upload abandoned mid-stream cannot
            // be resynchronised by the server in front of the worker.
            // The local `wrangler dev` proxy shows the cost of abandoning
            // it: its next request hangs or fails on 4.112.0, and 4.128.0
            // exits wrangler outright. The accepted prefix is released
            // first so the discard holds no memory, and the drain is not
            // bounded: the edge caps the request body, and an under-cap
            // trickle already holds an invocation open just as long. A
            // stream error just ends the drain; the refusal is decided.
            drop(body);
            drop(chunk);
            while let Some(Ok(_)) = stream.next().await {}
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

/// The website origin mutations and the browser plane live on
/// (`config.json`'s `api` field, the challenge's `login_url`, and the
/// quota details' usage URL all derive from it).
fn web_origin(env: &Env) -> worker::Result<String> {
    Ok(env.var("WEB_ORIGIN")?.to_string())
}

/// The website origin's host for the role dispatch. An unset or
/// unparsable `WEB_ORIGIN` yields an empty host, which grants nobody
/// the website role - deny by default.
fn web_host(env: &Env) -> String {
    web_origin(env)
        .ok()
        .and_then(|origin| worker::Url::parse(&origin).ok())
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The uniform Bearer-plane 401: the fixed envelope plus the
/// byte-identical `WWW-Authenticate` challenge on every path and failure
/// reason (missing token, invalid token, unknown path), so
/// unauthenticated responses stay indistinguishable.
fn unauthorized(env: &Env) -> worker::Result<Response> {
    let mut response = error_response(401, error::AUTH_REQUIRED)?;
    response.headers_mut().set(
        "www-authenticate",
        &error::www_authenticate(&web_origin(env)?),
    )?;
    Ok(response)
}

/// Looks up the presented bearer token. `None` is the uniform "no valid
/// token" answer regardless of what failed; only infrastructure errors
/// surface as `Err`.
async fn authenticate(
    req: &Request,
    db: &D1Database,
    ctx: &Context,
) -> worker::Result<Option<AuthContext>> {
    let Some(header) = req.headers().get("authorization")? else {
        return Ok(None);
    };
    let Some(token) = auth::bearer_token(&header) else {
        return Ok(None);
    };
    let hash = auth::token_hash(token);
    let record: Option<TokenRecord> = db
        .prepare(sql::AUTH_TOKEN_LOOKUP)
        .bind(&[hash.into(), now_iso8601().into()])?
        .first(None)
        .await?;
    let Some(record) = record else {
        return Ok(None);
    };

    // Best-effort bookkeeping: never fail or delay the request over it.
    if let Ok(update) = db
        .prepare(sql::TOUCH_TOKEN_LAST_USED)
        .bind(&[now_iso8601().into(), record.id.clone().into()])
    {
        ctx.wait_until(async move {
            let _ = update.run().await;
        });
    }

    let bucket = bucket_from_columns(record.rl_tokens, record.rl_updated_at.as_deref());
    Ok(Some(AuthContext {
        token_id: record.id,
        user_id: record.user_id,
        scopes: auth::parse_scopes(&record.scopes),
        quota_class: record.quota_class,
        scope_limit: record.scope_limit,
        user_quota_class: record.user_quota_class,
        bucket,
    }))
}

fn has_verify_scope(auth: &AuthContext) -> bool {
    auth.scopes.contains(&Scope::Verify)
}

/// Both bucket columns must be present and coherent; anything else is a
/// fresh (full) bucket.
fn bucket_from_columns(tokens: Option<f64>, updated_at: Option<&str>) -> Option<quota::Bucket> {
    let tokens = tokens?;
    let updated_at_ms = updated_at?.parse::<f64>().ok()?;
    Some(quota::Bucket {
        tokens,
        updated_at_ms,
    })
}

/// Rows changed by a statement, from its result metadata.
pub(crate) fn changed_rows(meta: Option<worker::D1ResultMeta>) -> usize {
    meta.and_then(|meta| meta.changes).unwrap_or(0)
}

/// Reads `meta.registry_generation`; best-effort (the header is a debug
/// aid, not part of the client contract).
async fn registry_generation(db: &D1Database) -> Option<String> {
    let record: Option<MetaRecord> = db
        .prepare(sql::REGISTRY_GENERATION)
        .first(None)
        .await
        .ok()?;
    record.map(|record| record.value)
}

#[derive(Deserialize)]
pub(crate) struct CountRecord {
    pub(crate) n: i64,
}

/// Clamps a D1 aggregate to zero; the counters can never really go
/// negative.
pub(crate) fn non_negative(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

thread_local! {
    /// Isolate-memory service-mode cache: `(mode, expiry epoch ms)`.
    static MODE_CACHE: Cell<Option<(breaker::Mode, f64)>> = const { Cell::new(None) };
}

const SERVICE_MODE_TTL_SECS: f64 = 60.0;

/// The service mode, cached in isolate memory for ~60 s (one cheap D1
/// point read on expiry; the `SERVICE_MODE_TTL_SECS` env var overrides
/// the TTL, and the smoke test pins it to 0 via `.dev.vars` so it can
/// flip modes without waiting it out). The fail direction is the
/// caller's: writes fail closed - a missing or unknown
/// `meta.service_mode` parses to `WritesBlocked` here, and a D1 failure
/// propagates into [`write_gate`]'s 500 - while the read gate drops the
/// error with `.ok()` and refuses only on an affirmatively read
/// `ReadsBlocked` (`breaker::read_gate_refuses`), which the fail-closed
/// parse can never produce. [`count_download`]'s deferred task follows
/// the write direction, where any failure only skips a telemetry
/// increment, never a response.
async fn service_mode(env: &Env, db: &D1Database) -> worker::Result<breaker::Mode> {
    let now_ms = now_epoch_ms();
    if let Some((mode, expires_at_ms)) = MODE_CACHE.with(Cell::get)
        && now_ms < expires_at_ms
    {
        return Ok(mode);
    }
    let mode = read_meta(db, "service_mode")
        .await?
        .and_then(|value| breaker::Mode::parse(&value))
        .unwrap_or(breaker::Mode::WritesBlocked);
    let ttl_secs = env
        .var("SERVICE_MODE_TTL_SECS")
        .ok()
        .and_then(|var| var.to_string().parse::<f64>().ok())
        .unwrap_or(SERVICE_MODE_TTL_SECS);
    MODE_CACHE.with(|cell| cell.set(Some((mode, now_ms + ttl_secs * 1000.0))));
    Ok(mode)
}

/// `Some(503)` when the budget breaker has writes blocked
/// (`docs/architecture.md`, "Billing model: the governor and the breaker").
/// `>=`, not `==`: `reads_blocked` sits above `writes_blocked` on the
/// ladder and blocks writes too.
async fn write_gate(env: &Env, db: &D1Database) -> worker::Result<Option<Response>> {
    if service_mode(env, db).await? >= breaker::Mode::WritesBlocked {
        return Ok(Some(error_response_with_code(
            breaker::OVER_BUDGET_STATUS,
            breaker::OVER_BUDGET_DETAIL,
            breaker::OVER_BUDGET_CODE,
            Some(breaker::OVER_BUDGET_RETRY_AFTER_SECS),
        )?));
    }
    Ok(None)
}

/// Renders a governor gate refusal for the Bearer plane
/// (`docs/architecture.md`, "The cost governor"): the per-user fairness
/// refusal is a `429` with its own code and a `Retry-After` reaching
/// the next UTC day; every other refusal - pool exhausted, key
/// conflict, or an unreachable governor - is the breaker's `503` +
/// `registry_over_budget` envelope, with the detail picking the plane.
fn governor_refusal_response(
    refusal: Option<&Refusal>,
    write_plane: bool,
) -> worker::Result<Response> {
    match refusal {
        Some(Refusal::PrincipalExhausted {
            retry_after_secs, ..
        }) => error_response_with_code(
            quota::READ_RATE_LIMITED.status,
            quota::READ_RATE_LIMITED.detail,
            quota::READ_RATE_LIMITED.code,
            Some(*retry_after_secs),
        ),
        Some(_) => error_response_with_code(
            breaker::OVER_BUDGET_STATUS,
            if write_plane {
                breaker::OVER_BUDGET_DETAIL
            } else {
                breaker::OVER_BUDGET_READS_DETAIL
            },
            breaker::OVER_BUDGET_CODE,
            Some(breaker::OVER_BUDGET_RETRY_AFTER_SECS),
        ),
        None => error_response_with_code(
            breaker::OVER_BUDGET_STATUS,
            breaker::GOVERNOR_UNAVAILABLE_DETAIL,
            breaker::OVER_BUDGET_CODE,
            Some(breaker::GOVERNOR_UNAVAILABLE_RETRY_AFTER_SECS),
        ),
    }
}

pub(crate) fn consume_one(pool: OpPool) -> Decision {
    Decision {
        consume: vec![Consume {
            pool,
            n: 1,
            principal: None,
            principal_cap: None,
        }],
        ..Decision::default()
    }
}

pub(crate) fn commit_object(pool: StoragePool, key: &str, bytes: u64) -> Decision {
    Decision {
        commit: vec![governor::Commit {
            pool,
            key: key.to_owned(),
            bytes,
        }],
        ..Decision::default()
    }
}

pub(crate) async fn read_meta(db: &D1Database, key: &str) -> worker::Result<Option<String>> {
    let record: Option<MetaRecord> = db
        .prepare(sql::META_VALUE)
        .bind(&[key.into()])?
        .first(None)
        .await?;
    Ok(record.map(|record| record.value))
}

pub(crate) fn upsert_meta(
    db: &D1Database,
    key: &str,
    value: &str,
) -> worker::Result<worker::D1PreparedStatement> {
    db.prepare(sql::UPSERT_META)
        .bind(&[key.into(), value.into()])
}

/// The budget-breaker schedule (`wrangler.jsonc` `triggers`); the cron
/// entry point routes on this exact expression.
const BREAKER_CRON: &str = "*/15 * * * *";

/// The cron entry point. The breaker's [`BREAKER_CRON`] runs the budget
/// evaluation (every 15 minutes: gather usage, evaluate it against the
/// budgets, persist the resulting service mode - failed analytics
/// queries leave their metric unset, which can escalate but never
/// unblock writes, [`breaker::next_mode`]), then the governor
/// reconciliation pass and a backup-queue drain. Any other trigger -
/// the nightly `0 3 * * *`, or a temporary schedule added for an ops
/// rehearsal - runs the D1 dump job, so exercising the backup path
/// never needs a recompile.
#[event(scheduled)]
pub async fn scheduled(event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    if event.cron() == BREAKER_CRON {
        if let Err(err) = evaluate_budgets(&env).await {
            console_error!("budget evaluation failed; keeping the last service mode: {err}");
        }
        match env.d1("DB") {
            Ok(db) => reconcile_governor(&env, &db).await,
            Err(err) => console_error!("governor reconciliation: no DB binding: {err}"),
        }
        crate::backup_glue::drain_backup_queue(&env).await;
    } else if let Err(err) = crate::backup_glue::run_nightly_dump(&env).await {
        console_error!("nightly backup failed: {err}");
    }
}

/// A JSON POST, optionally with a bearer token; used for the analytics
/// queries, the state-change webhook, and the D1 export calls.
pub(crate) async fn post_json(
    url: &str,
    body: &str,
    bearer: Option<&str>,
) -> worker::Result<Response> {
    let headers = Headers::new();
    headers.set("content-type", "application/json")?;
    if let Some(bearer) = bearer {
        headers.set("authorization", &format!("Bearer {bearer}"))?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(body.to_owned().into()));
    Fetch::Request(Request::new_with_init(url, &init)?)
        .send()
        .await
}

fn json_response(body: &str) -> worker::Result<Response> {
    let mut response = Response::ok(body)?;
    response
        .headers_mut()
        .set("content-type", "application/json")?;
    Ok(response)
}

fn json_response_with_status(status: u16, body: &str) -> worker::Result<Response> {
    Ok(json_response(body)?.with_status(status))
}

fn error_response(status: u16, detail: &str) -> worker::Result<Response> {
    let mut response = Response::ok(error::envelope(detail))?.with_status(status);
    response
        .headers_mut()
        .set("content-type", "application/json")?;
    Ok(response)
}

fn error_response_with_code(
    status: u16,
    detail: &str,
    code: &str,
    retry_after_secs: Option<u64>,
) -> worker::Result<Response> {
    let mut response = Response::ok(error::envelope_with_code(detail, code))?.with_status(status);
    response
        .headers_mut()
        .set("content-type", "application/json")?;
    if let Some(secs) = retry_after_secs {
        response
            .headers_mut()
            .set("retry-after", &secs.to_string())?;
    }
    Ok(response)
}

/// A numeric D1 binding. D1 has no `BigInt` support, so the value rides
/// as a float; everything bound this way (registry user ids, byte
/// counts, row counts) sits far below 2^53, where f64 is exact. Never
/// use it for `identities.provider_account_id`: that column is TEXT,
/// and a float bind would store "26405363.0".
#[allow(clippy::cast_precision_loss)]
pub(crate) fn js_int(value: i64) -> worker::wasm_bindgen::JsValue {
    worker::wasm_bindgen::JsValue::from_f64(value as f64)
}

/// Unix epoch milliseconds, exact in f64 until the year 287396.
#[allow(clippy::cast_precision_loss)]
fn now_epoch_ms() -> f64 {
    worker::Date::now().as_millis() as f64
}

pub(crate) fn now_iso8601() -> String {
    worker::js_sys::Date::new_0()
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
}

/// Correlation id for log lines: the edge's ray id, or a coarse local
/// fallback under `wrangler dev`.
fn request_id(req: &Request) -> String {
    req.headers()
        .get("cf-ray")
        .ok()
        .flatten()
        .unwrap_or_else(|| format!("local-{}", worker::Date::now().as_millis()))
}
