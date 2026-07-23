//! Cloudflare-specific glue: binding access, D1/R2 I/O, and response
//! plumbing. Everything with behavior worth testing lives in the host-target
//! modules; keep this file thin.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use worker::{
    Context, D1Database, Delay, Env, Fetch, Headers, Method, Request, RequestInit, Response,
    ScheduleContext, ScheduledEvent, console_error, console_log, event,
};

use crate::auth::{self, AuthContext, Scope};
use crate::documents::{self, VersionRow};
use crate::error;
use crate::governor::{self, Consume, Decision, OpPool, Refusal, Reserve, StoragePool};
use crate::governor_client::{self, Gate};
use crate::publish;
use crate::routes::{ApiRoute, Route, match_api_route, match_route, match_web_route};
use crate::web_glue;
use crate::{analytics, backup, breaker, quota, sql, telemetry, verify};

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
    rl_tokens: Option<f64>,
    rl_updated_at: Option<String>,
}

#[derive(Deserialize)]
struct VersionRecord {
    version: String,
    metadata_json: String,
    yanked: i64,
}

#[derive(Deserialize)]
struct ArtifactRecord {
    checksum: String,
    verification: String,
}

#[derive(Deserialize)]
struct MetaRecord {
    value: String,
}

#[derive(Deserialize)]
struct StoredVersionRecord {
    metadata_json: String,
    checksum: String,
    verification: String,
}

#[derive(Deserialize)]
struct YankedRecord {
    yanked: i64,
    verification: String,
}

/// The yank request body, exactly `{"yanked": <bool>}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct YankBody {
    yanked: bool,
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
    // preserving the header (which is how scripts/smoke.sh exercises
    // both roles on one server).
    let host = req.headers().get("host")?.unwrap_or_default();
    let host = crate::routes::host_without_port(&host);
    match crate::routes::role_for_host(host, &web_host(env)) {
        crate::routes::Role::Registry => handle_registry(req, env, ctx, &path).await,
        crate::routes::Role::Website => handle_website(req, env, ctx, &path).await,
    }
}

/// Reads at most `limit` request-body bytes without asking the runtime to
/// materialize the complete body. The declared length is an early refusal;
/// the streaming count is authoritative for chunked or dishonest requests.
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

/// The registry custom domain: only the machine read plane exists here.
/// Every other path - including all of `/api/*` - answers the uniform
/// 401 without consulting the `Authorization` header at all, so a
/// misdirected credential or a probe of the mutation routes is
/// indistinguishable from any unknown path.
async fn handle_registry(
    req: &Request,
    env: &Env,
    ctx: &Context,
    path: &str,
) -> worker::Result<(Response, Option<String>)> {
    // The only unauthenticated route; 200 with no body.
    if path == "/healthz" {
        return Ok((Response::empty()?, None));
    }
    let Some(route) = match_route(path) else {
        return Ok((unauthorized(env)?, None));
    };

    // Deny by default: the uniform 401 is emitted before any D1/R2 data
    // lookup, so non-callers cannot probe package existence.
    let db = env.d1("DB")?;
    let Some(auth) = authenticate(req, &db, ctx).await? else {
        return Ok((unauthorized(env)?, None));
    };

    let mut response = if req.method() == Method::Get {
        // The read-side budget gate (`docs/architecture.md`, "Billing
        // model and the budget breaker"). After the Bearer gate, so
        // unauthenticated callers cannot observe service state; inside
        // the GET arm, so method discipline keeps its 405; and fail-open
        // on the mode lookup (`.ok()`): only an affirmatively read
        // `reads_blocked` refuses, so downloads keep working through an
        // outage of the breaker itself. The verifier's fetches - the
        // config it discovers the api origin from and the artifacts it
        // inspects, never the package documents - are exempt: it must
        // be able to drain the pending queue while reads are blocked,
        // and its spend is negligible.
        let verify_exempt =
            has_verify_scope(&auth) && matches!(route, Route::Config | Route::Artifact { .. });
        let mode = service_mode(env, &db).await.ok();
        if breaker::read_gate_refuses(mode, verify_exempt) {
            error_response_with_code(
                breaker::OVER_BUDGET_STATUS,
                breaker::OVER_BUDGET_READS_DETAIL,
                breaker::OVER_BUDGET_CODE,
                Some(breaker::OVER_BUDGET_RETRY_AFTER_SECS),
            )?
        } else {
            match route {
                Route::Config => json_response(&documents::config_json(&web_origin(env)?))?,
                Route::Package { scope, name } => package_response(&db, scope, name).await?,
                Route::Artifact {
                    scope,
                    name,
                    version,
                } => artifact_response(env, &db, ctx, &auth, scope, name, version).await?,
                // Answered above before the auth gate.
                Route::Healthz => Response::empty()?,
            }
        }
    } else {
        error_response(405, error::METHOD_NOT_ALLOWED)?
    };

    // Debug aid for the pre-launch registry (see docs/runbook.md):
    // stamp every authenticated response with the registry generation.
    if let Some(generation) = registry_generation(&db).await {
        response.headers_mut().set(GENERATION_HEADER, &generation)?;
    }
    Ok((response, Some(auth.token_id)))
}

/// The website origin: the OAuth plane (`/login`, `/callback`, and the
/// claim flow's `/claim/<scope>` and `/callback/claim`), the
/// session-only `/api/v1/user` subtree, and the Bearer mutation plane.
/// The read plane does not exist here - nothing outside those planes
/// matches a data route, so this origin never serves `/config.json`,
/// `/packages/*`, or `/artifacts/*`.
async fn handle_website(
    req: &mut Request,
    env: &Env,
    ctx: &Context,
    path: &str,
) -> worker::Result<(Response, Option<String>)> {
    if let Some(web_route) = match_web_route(path) {
        return Ok((web_glue::respond_web(req, env, web_route).await?, None));
    }
    // The whole subtree is session-only: a bearer token never reaches
    // it, and unknown paths under it answer as the session plane rather
    // than falling through to the bearer plane.
    if crate::routes::is_session_path(path) {
        let Some(session_route) = crate::routes::match_session_route(path) else {
            return Ok((error_response(404, error::NOT_FOUND)?, None));
        };
        let response = web_glue::respond_session(req, env, session_route).await?;
        return Ok((response, None));
    }
    // The public stats subtree: the one unauthenticated JSON plane on
    // this origin (`docs/architecture.md`, "Download counts").
    if crate::routes::is_stats_path(path) {
        return Ok((web_glue::respond_stats(req, env, path).await?, None));
    }

    // Everything else is the Bearer plane: deny by default, the uniform
    // 401 before any route matching or data lookup.
    let db = env.d1("DB")?;
    let Some(auth) = authenticate(req, &db, ctx).await? else {
        return Ok((unauthorized(env)?, None));
    };

    let mut response = match req.method() {
        // The admin listings and the governor snapshot are the only API
        // routes read with GET; anything else is an authenticated 404.
        Method::Get => match match_api_route(path) {
            Some(ApiRoute::AdminVersions) => admin_versions_response(req, &db, &auth).await?,
            Some(ApiRoute::AdminPackages) => admin_packages_response(&db, &auth).await?,
            Some(ApiRoute::AdminGovernor) => admin_governor_usage_response(env, &auth).await?,
            _ => error_response(404, error::NOT_FOUND)?,
        },
        Method::Put => match match_api_route(path) {
            Some(ApiRoute::Publish {
                scope,
                name,
                version,
            }) => {
                let (scope, name, version) =
                    (scope.to_owned(), name.to_owned(), version.to_owned());
                publish_response(req, env, &db, &auth, &scope, &name, &version).await?
            }
            Some(_) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        Method::Post => match match_api_route(path) {
            Some(ApiRoute::AdminGovernor) => {
                admin_governor_mutation_response(req, env, &db, &auth).await?
            }
            Some(_) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        Method::Patch => match match_api_route(path) {
            Some(ApiRoute::Yank {
                scope,
                name,
                version,
            }) => {
                let (scope, name, version) =
                    (scope.to_owned(), name.to_owned(), version.to_owned());
                yank_response(req, env, &db, &auth, &scope, &name, &version).await?
            }
            Some(ApiRoute::AdminVerdict {
                scope,
                name,
                version,
            }) => {
                let (scope, name, version) =
                    (scope.to_owned(), name.to_owned(), version.to_owned());
                verdict_response(req, env, ctx, &db, &auth, &scope, &name, &version).await?
            }
            Some(_) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        _ => error_response(405, error::METHOD_NOT_ALLOWED)?,
    };

    // The same generation stamp as the read plane (docs/runbook.md).
    if let Some(generation) = registry_generation(&db).await {
        response.headers_mut().set(GENERATION_HEADER, &generation)?;
    }
    Ok((response, Some(auth.token_id)))
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
        .bind(&[hash.into()])?
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
        bucket,
    }))
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

/// `GET /packages/<scope>/<name>.json`: composed from **verified**
/// versions only - the filter is in the query, so pending and rejected
/// rows never reach composition, and a package with no verified versions
/// is indistinguishable from an unknown one (fail safe: if the verifier
/// never runs, nothing new ever becomes resolvable).
async fn package_response(db: &D1Database, scope: &str, name: &str) -> worker::Result<Response> {
    let records: Vec<VersionRecord> = db
        .prepare(sql::VERIFIED_VERSIONS_BY_PACKAGE)
        .bind(&[scope.into(), name.into()])?
        .all()
        .await?
        .results()?;
    if records.is_empty() {
        return error_response(404, error::NOT_FOUND);
    }
    let rows: Vec<VersionRow> = records
        .into_iter()
        .map(|record| VersionRow {
            version: record.version,
            metadata_json: record.metadata_json,
            yanked: record.yanked != 0,
        })
        .collect();
    let full_name = format!("{scope}/{name}");
    match documents::package_json(&full_name, &rows) {
        Ok(body) => json_response(&body),
        Err(detail) => {
            console_error!("package document for {full_name}: {detail}");
            error_response(500, error::INTERNAL)
        }
    }
}

/// `PUT /api/v1/packages/<scope>/<name>/<version>`: the publish route
/// (`docs/remote-registry.md`, "Publish"). Validation order and status
/// mapping follow `crate::publish`, preceded by the budget gate (`503`),
/// the publish rate limit (`429`), and the scope-membership gate (the
/// uniform `403` - publishing under a scope creates the package row, so
/// membership alone decides, and a scope that does not exist answers
/// exactly like one the user is not a member of), and followed - for
/// genuinely new versions and replacements of rejected ones only - by
/// the archive-size cap (`413`), the `-`/`_` twin-name reject for new
/// packages (`400`, before the quotas - name validity does not depend
/// on quota state), and the per-user quota checks (`403`);
/// on success the archive lands in R2 first (an orphaned blob from a
/// crash between the two writes stays conservatively represented by
/// its governor reservation - see `docs/runbook.md`), then one atomic
/// D1 batch inserts (or, for a
/// rejected row, replaces) the package and version rows and bumps the
/// storage self-accounting. New rows start `pending` and the `201`
/// reports it: nothing becomes resolvable before the verifier says so.
// The route triple plus the request plumbing exceeds the argument lint,
// and the publish pipeline is one deliberately linear sequence of gate
// checks in documented order, so it also runs long; splitting either
// would scatter that structure across helpers rather than clarify it.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn publish_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    if let Some(blocked) = write_gate(env, db).await? {
        return Ok(blocked);
    }
    if !auth.scopes.contains(&Scope::Publish) {
        return error_response(403, error::PUBLISH_SCOPE_REQUIRED);
    }

    let quotas = quota::quotas_for_class(&auth.quota_class);
    if let Some(limited) = publish_rate_limit(env, db, auth, &quotas).await? {
        return Ok(limited);
    }

    // After the rate limit, so probing scopes is throttled like any
    // other publish attempt; before the body is buffered, like every
    // other authorization check.
    if !is_scope_member(db, scope, auth.user_id).await? {
        return error_response(403, error::SCOPE_MEMBERSHIP_REQUIRED);
    }

    let Some(body) = bounded_body(req, publish::MAX_BODY_BYTES).await? else {
        return error_response(400, publish::BODY_TOO_LARGE);
    };
    let frame = match publish::decode_frame(&body) {
        Ok(frame) => frame,
        Err(detail) => return error_response(400, detail),
    };
    let archive_bytes = frame.archive.len() as u64;
    let metadata = match publish::validate_metadata(scope, name, version, frame.metadata) {
        Ok(metadata) => metadata,
        Err(detail) => return error_response(400, detail),
    };
    // Reject a body that cannot be a profile zip before hashing it; the
    // full profile is checked later by the async verifier.
    if let Err(detail) = publish::sanity_check_zip(frame.archive) {
        return error_response(400, detail);
    }
    let computed_hex = sha256_hex(frame.archive).await?;
    if let Err(detail) = publish::verify_checksum(&metadata, &computed_hex) {
        return error_response(400, detail);
    }
    // The frame parsed as JSON, so it is valid UTF-8; the stored column
    // is the uploaded document verbatim.
    let Ok(metadata_text) = std::str::from_utf8(frame.metadata) else {
        return error_response(400, publish::METADATA_NOT_JSON);
    };

    let replaced = match existing_version(db, scope, name, version, metadata_text).await? {
        Some(ExistingVersion::Answered(response)) => {
            // The idempotent no-op (200) is a retry of a committed
            // publish that still holds the row's exact bytes, so it is
            // the one chance to self-heal a primary blob a reclaim
            // race deleted. The 409 arm gets no heal: its uploaded
            // bytes were rejected.
            if response.status_code() == 200 {
                heal_blobs_on_retry(env, &computed_hex, frame.archive).await?;
            }
            return Ok(response);
        }
        Some(ExistingVersion::Rejected { old_checksum }) => Some(old_checksum),
        None => None,
    };

    // The archive-size cap and the per-user quotas gate genuinely new
    // versions - including a replacement of a rejected one, whose new
    // archive consumes quota like any other (the rejected row's own
    // bytes were refunded at rejection): a byte-identical re-publish of
    // an already-stored archive (even one grandfathered above the
    // current cap) stays the idempotent no-op above and never consumes
    // quota.
    if let Err(denial) = quota::check_archive_size(archive_bytes, &quotas) {
        return denial_response(env, &denial, None);
    }
    // ponytail: the quota counts below are a preflight, not a serialized
    // transaction - concurrent publishes can each pass the same
    // near-limit check and overshoot a quota by up to the in-flight
    // request count. The CAS'd rate limit bounds that per token at the
    // bucket burst (an allowlisted user holding several tokens scales it
    // by their token count); the budget headroom and the breaker absorb
    // the transient. Move the checks into conditional inserts if that
    // ever stops holding.
    let now = now_iso8601();
    let Some(day_prefix) = quota::utc_day_prefix(&now).map(str::to_owned) else {
        console_error!("clock produced a non-ISO timestamp: {now}");
        return error_response(500, error::INTERNAL);
    };
    let (counts, twin_exists) = publish_counts(db, auth.user_id, scope, name, &day_prefix).await?;
    // The deterministic `-`/`_` twin reject (`docs/architecture.md`,
    // "Name fidelity") gates new packages only, and answers before the
    // quota 403s: whether a name can exist does not depend on the
    // publisher's quota state.
    if !counts.package_exists && twin_exists {
        return error_response(400, publish::NAME_TWIN_CONFLICT);
    }
    if let Err(denial) = quota::check_publish(archive_bytes, &counts, &quotas) {
        return denial_response(env, &denial, None);
    }

    let new = NewVersion {
        scope,
        name,
        version,
        checksum_hex: &computed_hex,
        metadata_text,
        published_at: &now,
        archive: frame.archive,
        user_id: auth.user_id,
    };
    match &replaced {
        Some(old_checksum) => {
            match replace_rejected_version(env, db, &new, old_checksum).await? {
                Persist::Done => {}
                Persist::Refused(response) => return Ok(response),
                Persist::Lost => {
                    // A concurrent replacement or verdict moved the
                    // rejected row first; answer for the row's new
                    // state exactly as if this request had arrived
                    // after the winner.
                    return match existing_version(db, scope, name, version, metadata_text).await? {
                        Some(ExistingVersion::Answered(response)) => {
                            if response.status_code() == 200 {
                                heal_blobs_on_retry(env, &computed_hex, frame.archive).await?;
                            }
                            Ok(response)
                        }
                        // Rejected again (a third racer) or gone: the
                        // conservative refusal; a retry resolves it.
                        _ => error_response(409, error::VERSION_IMMUTABLE),
                    };
                }
            }
        }
        None => match persist_new_version(env, db, &new).await? {
            Persist::Done => {}
            Persist::Refused(response) => return Ok(response),
            Persist::Lost => {
                // A twin publish won the race between this request's
                // preflight and its batch; answer exactly like the
                // preflight would have.
                return error_response(400, publish::NAME_TWIN_CONFLICT);
            }
        },
    }

    json_response_with_status(
        201,
        &serde_json::json!({
            "ok": true,
            "name": format!("{scope}/{name}"),
            "version": version,
            "checksum": metadata.checksum,
            "verification": "pending",
        })
        .to_string(),
    )
}

/// Whether the token's user is a member (any role) of `scope`. A scope
/// that does not exist has no members, so the caller's uniform refusal
/// needs no separate existence check.
async fn is_scope_member(db: &D1Database, scope: &str, user_id: i64) -> worker::Result<bool> {
    let membership: CountRecord = db
        .prepare(sql::SCOPE_MEMBERSHIP)
        .bind(&[scope.into(), js_int(user_id)])?
        .first(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("empty COUNT(*) result".to_owned()))?;
    Ok(membership.n > 0)
}

/// `PATCH /api/v1/packages/<scope>/<name>/<version>/yank`
/// (`docs/remote-registry.md`, "Yank"): idempotent, and the row's
/// `yanked` column is the single home of yank state - the read path
/// overrides the stored metadata's field from it. Gated by the budget
/// breaker (`503`) like publish; yank has no rate limit or quota.
/// The scope-membership gate (the uniform `403`) answers before the
/// version lookup, so a non-member can never probe which versions exist
/// under a foreign scope. Yank applies to **verified** versions only: a
/// pending or rejected version was never part of the registry's
/// resolvable surface, so there is nothing to retract and the triple
/// answers an authenticated 404.
async fn yank_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    if let Some(blocked) = write_gate(env, db).await? {
        return Ok(blocked);
    }
    if !auth.scopes.contains(&Scope::Yank) {
        return error_response(403, error::YANK_SCOPE_REQUIRED);
    }
    if !is_scope_member(db, scope, auth.user_id).await? {
        return error_response(403, error::SCOPE_MEMBERSHIP_REQUIRED);
    }
    let Some(body) = bounded_body(req, MAX_MUTATION_BODY_BYTES).await? else {
        return error_response(400, error::INVALID_YANK_BODY);
    };
    let Ok(YankBody { yanked }) = serde_json::from_slice(&body) else {
        return error_response(400, error::INVALID_YANK_BODY);
    };

    let existing: Option<YankedRecord> = db
        .prepare(sql::VERSION_YANK_STATE)
        .bind(&[scope.into(), name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(existing) = existing else {
        return error_response(404, error::NOT_FOUND);
    };
    if verify::Status::parse(&existing.verification) != Some(verify::Status::Verified) {
        return error_response(404, error::NOT_FOUND);
    }
    let changed = (existing.yanked != 0) != yanked;
    if changed {
        db.prepare(sql::SET_VERSION_YANKED)
            .bind(&[
                i32::from(yanked).into(),
                scope.into(),
                name.into(),
                version.into(),
            ])?
            .run()
            .await?;
    }
    // The resulting state, plus whether this request changed it (the
    // idempotent no-op reports `changed: false`).
    json_response_with_status(
        200,
        &serde_json::json!({ "ok": true, "yanked": yanked, "changed": changed }).to_string(),
    )
}

fn has_verify_scope(auth: &AuthContext) -> bool {
    auth.scopes.contains(&Scope::Verify)
}

#[derive(Deserialize)]
struct AdminVersionRecord {
    scope: String,
    name: String,
    version: String,
    checksum: String,
    published_by: i64,
    published_at: String,
    metadata_json: String,
}

/// `GET /api/v1/admin/versions?status=<status>` (`verify` scope): the
/// verifier's work list. Each entry's `name` is the canonical
/// `<scope>/<name>` and carries the stored canonical metadata document
/// (parsed, so the response is one JSON value); the listing is
/// deterministic: ordered by scope, then name, then version.
async fn admin_versions_response(
    req: &Request,
    db: &D1Database,
    auth: &AuthContext,
) -> worker::Result<Response> {
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    let url = req.url()?;
    let status = url
        .query_pairs()
        .find(|(key, _)| key == "status")
        .map(|(_, value)| value.into_owned());
    let Some(status) = status.as_deref().and_then(verify::Status::parse) else {
        return error_response(400, error::INVALID_STATUS_QUERY);
    };
    let records: Vec<AdminVersionRecord> = db
        .prepare(sql::VERSIONS_BY_VERIFICATION_STATUS)
        .bind(&[status.as_str().into()])?
        .all()
        .await?
        .results()?;
    let mut versions = Vec::with_capacity(records.len());
    for record in records {
        let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&record.metadata_json) else {
            console_error!(
                "stored metadata for {}/{}@{} is not valid JSON",
                record.scope,
                record.name,
                record.version
            );
            return error_response(500, error::INTERNAL);
        };
        versions.push(serde_json::json!({
            "name": format!("{}/{}", record.scope, record.name),
            "version": record.version,
            "checksum": record.checksum,
            "published_by": record.published_by,
            "published_at": record.published_at,
            "metadata": metadata,
        }));
    }
    json_response(&serde_json::json!({ "versions": versions }).to_string())
}

#[derive(Deserialize)]
struct AdminPackageRecord {
    scope: String,
    name: String,
    vetted: i64,
}

/// `GET /api/v1/admin/packages` (`verify` scope): the corpus for the
/// verifier's name advisories (`docs/architecture.md`, "Name
/// fidelity"). Admin infrastructure like the versions listing: no
/// scope membership, and deliberately not budget-gated - the
/// verification pipeline must be able to drain the pending queue
/// whatever the service mode.
async fn admin_packages_response(db: &D1Database, auth: &AuthContext) -> worker::Result<Response> {
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    let records: Vec<AdminPackageRecord> =
        db.prepare(sql::ADMIN_PACKAGES).all().await?.results()?;
    let packages: Vec<verify::CorpusPackage> = records
        .into_iter()
        .map(|record| verify::CorpusPackage {
            scope: record.scope,
            name: record.name,
            vetted: record.vetted != 0,
        })
        .collect();
    json_response(&verify::packages_json(&packages))
}

#[derive(Deserialize)]
struct VerdictTargetRecord {
    verification: String,
    checksum: String,
    published_at: String,
    archive_size: i64,
}

/// `PATCH /api/v1/admin/versions/<scope>/<name>/<version>` (`verify`
/// scope): the verifier's verdict. Pending versions accept either verdict; a
/// repeat of the verdict a verified version already carries is the
/// idempotent 200; anything else is the 409 matrix in
/// [`verify::transition`]. The body's optional `checksum` binds the
/// verdict to the bytes the verifier actually inspected, and the
/// applying updates are themselves guarded on the row still being
/// pending with the bytes this request read - a verdict racing a
/// conflicting verdict or a replacement answers 409 instead of landing
/// on content it never saw. A rejection records the reason, refunds
/// the archive's bytes from the storage self-accounting when the row
/// was the blob's sole live reference (decided inside the same
/// transaction that flips the row, so a duplicate concurrent verdict
/// cannot refund twice), and then reclaims the blob itself.
/// Deliberately **not** gated by the budget breaker, unlike publish
/// and yank: a verdict stores no new bytes (a rejection frees them),
/// so blocking it would only stall the pending queue - verification
/// must be able to drain it whatever the service mode
/// (`docs/architecture.md`, "Billing model: the governor and the breaker").
/// The response reports the resulting state plus whether this request
/// changed it.
#[allow(clippy::too_many_arguments)] // the route triple plus the verdict plumbing
async fn verdict_response(
    req: &mut Request,
    env: &Env,
    ctx: &Context,
    db: &D1Database,
    auth: &AuthContext,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    let Some(body) = bounded_body(req, MAX_MUTATION_BODY_BYTES).await? else {
        return error_response(400, error::INVALID_VERDICT_BODY);
    };
    let parsed = match verify::parse_verdict(&body) {
        Ok(parsed) => parsed,
        Err(detail) => return error_response(400, detail),
    };

    let target: Option<VerdictTargetRecord> = db
        .prepare(sql::VERDICT_TARGET)
        .bind(&[scope.into(), name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(target) = target else {
        return error_response(404, error::NOT_FOUND);
    };
    let Some(current) = verify::Status::parse(&target.verification) else {
        console_error!(
            "stored verification for {scope}/{name}@{version} is invalid: {}",
            target.verification
        );
        return error_response(500, error::INTERNAL);
    };
    // The listing binding: the row must still be the generation the
    // verifier listed, both its bytes and its publish event (a
    // same-bytes replacement changes published_at but not checksum).
    let target_changed = parsed
        .checksum
        .as_deref()
        .is_some_and(|expected| expected != target.checksum)
        || parsed
            .published_at
            .as_deref()
            .is_some_and(|expected| expected != target.published_at);
    if target_changed {
        return error_response(409, error::VERDICT_TARGET_CHANGED);
    }

    let changed = match verify::transition(current, parsed.verdict) {
        verify::Transition::Conflict(detail) => return error_response(409, detail),
        verify::Transition::NoOp => false,
        verify::Transition::Apply => {
            if !apply_verdict(env, db, scope, name, version, &parsed, &target).await? {
                // The row moved between this request's read and its
                // guarded update: a concurrent conflicting verdict or a
                // replacement won the race.
                return error_response(409, error::VERDICT_TARGET_CHANGED);
            }
            // The fast replication path: drain the just-enqueued
            // backup work off the response path. The queue row is
            // durable, so a lost kick only defers to the next breaker
            // cron pass.
            if parsed.verdict == verify::Verdict::Verified {
                let env = env.clone();
                ctx.wait_until(async move { drain_backup_queue(&env).await });
            }
            true
        }
    };
    let resulting = match parsed.verdict {
        verify::Verdict::Verified => verify::Status::Verified,
        verify::Verdict::Rejected => verify::Status::Rejected,
    };
    json_response_with_status(
        200,
        &serde_json::json!({
            "ok": true,
            "name": format!("{scope}/{name}"),
            "version": version,
            "verification": resulting.as_str(),
            "changed": changed,
        })
        .to_string(),
    )
}

/// `GET /api/v1/admin/governor` (`verify` scope): the governor
/// ledger's usage snapshot, for the operator (`docs/runbook.md`, "The
/// cost governor"). Admin infrastructure like the verifier listings:
/// no scope membership, not budget-gated - inspecting the ledger must
/// work in every service mode.
async fn admin_governor_usage_response(env: &Env, auth: &AuthContext) -> worker::Result<Response> {
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    match governor_client::usage(env).await {
        Some(snapshot) => json_response(
            &serde_json::to_string(&snapshot)
                .map_err(|err| worker::Error::RustError(err.to_string()))?,
        ),
        None => error_response(503, error::GOVERNOR_UNAVAILABLE),
    }
}

/// The admin governor mutation body: exactly one of an evidence-backed
/// release or the pre-launch ledger wipe.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminGovernorBody {
    #[serde(default)]
    release: Option<governor::Release>,
    #[serde(default)]
    wipe: Option<bool>,
}

/// `POST /api/v1/admin/governor` (`verify` scope): the two explicit
/// operator actions on the ledger. `release` frees one object's entry
/// and must only follow evidence the object is gone - the endpoint
/// cannot check R2 for the operator, and a release for a live object
/// would make the ledger understate reality (`docs/runbook.md`, "The
/// cost governor"). `wipe` clears the primary storage rows and the
/// daily fairness windows (backup and dump rows survive - their
/// objects are never wiped - and the monthly op windows survive too:
/// they mirror already-metered R2 operations and cannot be rebuilt)
/// and is the registry wipe's companion, guarded on `meta.launched`
/// exactly like
/// `scripts/wipe.sh`: only an affirmatively read `'false'` proceeds.
async fn admin_governor_mutation_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
) -> worker::Result<Response> {
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    let Some(body) = bounded_body(req, MAX_MUTATION_BODY_BYTES).await? else {
        return error_response(400, error::INVALID_GOVERNOR_BODY);
    };
    let parsed: AdminGovernorBody = match serde_json::from_slice(&body) {
        Ok(parsed) => parsed,
        Err(_) => return error_response(400, error::INVALID_GOVERNOR_BODY),
    };
    match (parsed.release, parsed.wipe) {
        (Some(release), None) => {
            let decision = Decision {
                release: vec![release],
                ..Decision::default()
            };
            match governor_client::decide(env, &decision).await {
                Gate::Allowed => json_response(r#"{"ok":true}"#),
                Gate::Refused(_) => error_response(503, error::GOVERNOR_UNAVAILABLE),
            }
        }
        (None, Some(true)) => {
            if read_meta(db, "launched").await?.as_deref() != Some("false") {
                return error_response(403, error::GOVERNOR_LEDGER_LAUNCHED);
            }
            if governor_client::wipe(env).await {
                json_response(r#"{"ok":true}"#)
            } else {
                error_response(503, error::GOVERNOR_UNAVAILABLE)
            }
        }
        _ => error_response(400, error::INVALID_GOVERNOR_BODY),
    }
}

/// Rows changed by a statement, from its result metadata.
fn changed_rows(meta: Option<worker::D1ResultMeta>) -> usize {
    meta.and_then(|meta| meta.changes).unwrap_or(0)
}

/// Applies a verdict to a pending row under the transactional guards
/// (still pending, still the checksum and `published_at` this request
/// read); `false` means the row moved first and nothing was changed.
#[allow(clippy::too_many_arguments)] // the route triple plus the verdict plumbing
async fn apply_verdict(
    env: &Env,
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
    parsed: &verify::ParsedVerdict,
    target: &VerdictTargetRecord,
) -> worker::Result<bool> {
    match parsed.verdict {
        verify::Verdict::Verified => {
            let now = now_iso8601();
            // One batch: the verified transition and its backup-queue
            // row commit together, so a crash right after can never
            // lose the replication work - the enqueue's guards repeat
            // the mark's, so the row appears exactly when the
            // transition applied (`sql::ENQUEUE_VERIFIED_BACKUP`).
            let results = db
                .batch(vec![
                    db.prepare(sql::MARK_VERSION_VERIFIED).bind(&[
                        now.as_str().into(),
                        scope.into(),
                        name.into(),
                        version.into(),
                        target.checksum.as_str().into(),
                        target.published_at.as_str().into(),
                    ])?,
                    db.prepare(sql::ENQUEUE_VERIFIED_BACKUP).bind(&[
                        scope.into(),
                        name.into(),
                        version.into(),
                        target.checksum.as_str().into(),
                        target.published_at.as_str().into(),
                        now.as_str().into(),
                    ])?,
                ])
                .await?;
            let mark = results
                .first()
                .ok_or_else(|| worker::Error::RustError("missing batch result 0".to_owned()))?;
            Ok(changed_rows(mark.meta()?) > 0)
        }
        verify::Verdict::Rejected => {
            let applied = apply_rejection(
                db,
                scope,
                name,
                version,
                parsed.reason.as_deref().unwrap_or_default(),
                target,
            )
            .await?;
            if applied {
                delete_blob_if_unreferenced(env, db, &target.checksum).await?;
            }
            Ok(applied)
        }
    }
}

/// The rejection transaction: the storage refund is decided **before**
/// the row flips (statement order inside one atomic batch), so it fires
/// exactly when this row - still pending, still storing the bytes the
/// verdict was read against - is the checksum's sole live reference: a
/// concurrent duplicate rejection sees the row already rejected and
/// refunds nothing, a replacement that swapped the bytes disarms both
/// statements, and a shared blob (another live row with the same bytes)
/// is never refunded. The row-flip carries the same guards; `false`
/// means it lost such a race and nothing changed. `MAX(..., 0)` keeps
/// the counter integer-parseable even under drift; the breaker treats a
/// non-numeric value as unavailable and fails closed.
async fn apply_rejection(
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
    reason: &str,
    target: &VerdictTargetRecord,
) -> worker::Result<bool> {
    let archive_size = js_int(target.archive_size);
    let results = db
        .batch(vec![
            db.prepare(sql::REFUND_STORED_BYTES_ON_REJECTION).bind(&[
                target.checksum.as_str().into(),
                scope.into(),
                name.into(),
                version.into(),
                archive_size,
                target.published_at.as_str().into(),
            ])?,
            db.prepare(sql::MARK_VERSION_REJECTED).bind(&[
                reason.into(),
                scope.into(),
                name.into(),
                version.into(),
                target.checksum.as_str().into(),
                target.published_at.as_str().into(),
            ])?,
        ])
        .await?;
    let row_flip = results
        .get(1)
        .ok_or_else(|| worker::Error::RustError("missing batch result 1".to_owned()))?;
    Ok(changed_rows(row_flip.meta()?) > 0)
}

/// Lowercase SHA-256 hex of `bytes` via the runtime's native
/// `SubtleCrypto` digest - hashing multi-MiB archives with a wasm `sha2`
/// would burn CPU budget instead. If per-request CPU ever nears the free
/// plan's limit anyway, the next step is the runtime's `DigestStream`
/// (hash while the body streams in, no full buffer); measurement is
/// deferred to the load-testing step, do not rework speculatively.
async fn sha256_hex(bytes: &[u8]) -> worker::Result<String> {
    use worker::js_sys::{Function, Promise, Reflect, Uint8Array};
    use worker::wasm_bindgen::{JsCast, JsValue};
    use worker::wasm_bindgen_futures::JsFuture;

    let crypto = Reflect::get(&worker::js_sys::global(), &JsValue::from_str("crypto"))?;
    let subtle = Reflect::get(&crypto, &JsValue::from_str("subtle"))?;
    let digest: Function = Reflect::get(&subtle, &JsValue::from_str("digest"))?.dyn_into()?;
    let promise: Promise = digest
        .call2(
            &subtle,
            &JsValue::from_str("SHA-256"),
            &Uint8Array::from(bytes),
        )?
        .dyn_into()?;
    let buffer = JsFuture::from(promise).await?;
    Ok(crate::auth::hex(&Uint8Array::new(&buffer).to_vec()))
}

/// The synthetic edge-cache identity for immutable verified archives:
/// derived from the content checksum only, never from the outward URL
/// or query string, so no request input can alias or bust an entry.
/// The path exists on no route (the registry host answers its uniform
/// 401 there), and the Worker runs on every request to its hostnames,
/// so the entry is reachable only through this handler - after Bearer
/// auth and the D1 verified-version gate.
fn blob_cache_url(checksum: &str) -> String {
    format!("https://registry.cabinpkg.com/__cache/blobs/sha256/{checksum}")
}

/// The stored copy's freshness. Archives are content-addressed and
/// immutable, but the TTL is one day, not forever: an operator
/// takedown (direct R2/D1 surgery) cannot purge warm colos, so the
/// entry must age out on its own within an operationally useful
/// window. Re-fills are governor-bounded and cheap at one charged
/// read per blob per colo per day.
const BLOB_CACHE_CONTROL: &str = "public, max-age=86400, immutable";

/// Only archives up to this size are buffered and cached: `cache.put`
/// needs a fixed-length body to store reliably, but buffering is
/// isolate memory, and the publish protocol admits bodies past the
/// default 16 MiB archive quota (raised quota classes, the 64 MiB
/// frame cap). Twice the default quota covers everything the registry
/// actually serves today; anything larger streams straight from R2 -
/// charged and admission-controlled like any miss, just uncached.
const BLOB_CACHE_MAX_BYTES: u64 = 32 * 1024 * 1024;

thread_local! {
    /// Checksums with an R2 read in flight in this isolate, for the
    /// cache-stampede single-flight ([`artifact_response`]).
    static INFLIGHT_BLOB_READS: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

async fn artifact_response(
    env: &Env,
    db: &D1Database,
    ctx: &Context,
    auth: &AuthContext,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    let record: Option<ArtifactRecord> = db
        .prepare(sql::ARTIFACT_BY_PACKAGE_VERSION)
        .bind(&[scope.into(), name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(record) = record else {
        return error_response(404, error::NOT_FOUND);
    };
    // Verified versions download with any valid token; pending ones
    // only with the `verify` scope (the verifier fetches the bytes it
    // inspects); rejected ones - whose blob is reclaimed - and rows
    // with an unreadable status gate like missing rows.
    let status = verify::Status::parse(&record.verification);
    let readable =
        status.is_some_and(|status| verify::artifact_readable(status, has_verify_scope(auth)));
    if !readable {
        return error_response(404, error::NOT_FOUND);
    }

    // Archives are immutable and content-addressed; yanked versions stay
    // downloadable on purpose (docs/remote-registry.md, "Yank").
    let key = format!("blobs/sha256/{}", record.checksum);
    if status == Some(verify::Status::Verified) {
        let response =
            verified_artifact_response(env, auth, &key, &record.checksum, scope, name, version)
                .await?;
        if response.status_code() == 200 {
            count_download(env, ctx, scope, name, version);
        }
        return Ok(response);
    }

    // The verifier's pending fetch: never cached (the bytes are not yet
    // part of the registry), charged to the isolated verifier pool so
    // ordinary traffic can never starve verification - and vice versa.
    // The per-user cap rides along because the verify scope is mintable
    // by every allowlisted user today: one user must not be able to
    // drain the whole verifier pool either.
    let quotas = quota::quotas_for_class(&auth.quota_class);
    let decision = Decision {
        consume: vec![Consume {
            pool: OpPool::BVerifier,
            n: 1,
            principal: Some(auth.user_id.to_string()),
            principal_cap: Some(quotas.artifact_reads_per_day),
        }],
        ..Decision::default()
    };
    match governor_client::decide(env, &decision).await {
        Gate::Allowed => {}
        Gate::Refused(refusal) => return governor_refusal_response(refusal.as_ref(), false),
    }
    let Some(object) = env.bucket("BLOBS")?.get(&key).execute().await? else {
        console_error!("blob {key} for {scope}/{name}@{version} is missing from R2");
        return error_response(500, error::INTERNAL);
    };
    let size = object.size();
    let Some(body) = object.body() else {
        console_error!("blob {key} for {scope}/{name}@{version} has no body");
        return error_response(500, error::INTERNAL);
    };
    let mut response = Response::from_stream(body.stream()?)?;
    response
        .headers_mut()
        .set("content-type", "application/zip")?;
    response
        .headers_mut()
        .set("content-length", &size.to_string())?;
    Ok(response)
}

/// A verified archive download: edge cache first (a hit costs no R2
/// operation and no governor call), then a single-flighted, governor-
/// charged R2 read that fills the cache for everyone else. On a
/// governor refusal or outage the R2 read is never initiated - only
/// already-cached bodies keep serving (`docs/architecture.md`, "The
/// cost governor").
/// A cache-matched response carries immutable headers; rebuild a
/// mutable response around the cached body and headers so the shared
/// response plumbing (the generation stamp) can write to it.
fn thaw_cached(mut cached: Response) -> worker::Result<Response> {
    let status = cached.status_code();
    let headers = cached.headers().clone();
    let mut response = Response::from_stream(cached.stream()?)?
        .with_status(status)
        .with_headers(headers);
    // The stored copy carries the internal `public` freshness header;
    // the outward answer to an authenticated request must not.
    response.headers_mut().set("cache-control", "no-store")?;
    Ok(response)
}

async fn verified_artifact_response(
    env: &Env,
    auth: &AuthContext,
    key: &str,
    checksum: &str,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    let cache_url = blob_cache_url(checksum);
    let cache = worker::Cache::default();
    // Cache errors read as misses: the charged path is admission-
    // controlled, so failing open here risks one governed R2 read, not
    // unbounded spend - while failing the request would take downloads
    // down with the cache.
    if let Ok(Some(cached)) = cache.get(cache_url.as_str(), false).await {
        return thaw_cached(cached);
    }

    // In-isolate single-flight: one uncached checksum must not fan out
    // into simultaneous R2 reads. The first request becomes the loader;
    // the rest poll the cache briefly and fall through to their own
    // (charged, admission-controlled) read once the loader vanished or
    // the bounded wait ran out. Only the marker's OWNER removes it: a
    // timed-out follower proceeding alongside a still-active loader
    // must not clear the loader's marker, or later requests would stop
    // waiting entirely. Cross-isolate concurrency stays possible and
    // stays correct: every actual R2 read is charged.
    let mut owns_marker =
        INFLIGHT_BLOB_READS.with(|set| set.borrow_mut().insert(checksum.to_owned()));
    if !owns_marker {
        for _ in 0..20 {
            Delay::from(Duration::from_millis(100)).await;
            if let Ok(Some(cached)) = cache.get(cache_url.as_str(), false).await {
                return thaw_cached(cached);
            }
            let gone = INFLIGHT_BLOB_READS.with(|set| !set.borrow().contains(checksum));
            if gone {
                break;
            }
        }
        owns_marker = INFLIGHT_BLOB_READS.with(|set| set.borrow_mut().insert(checksum.to_owned()));
    }
    let result = charged_blob_read(env, auth, key, &cache_url, scope, name, version).await;
    if owns_marker {
        INFLIGHT_BLOB_READS.with(|set| set.borrow_mut().remove(checksum));
    }
    result
}

/// The cache-miss path: charge one ordinary Class B read (with the
/// caller's per-user fairness cap) immediately before the R2 `get`,
/// then serve and fill the edge cache.
async fn charged_blob_read(
    env: &Env,
    auth: &AuthContext,
    key: &str,
    cache_url: &str,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    let quotas = quota::quotas_for_class(&auth.quota_class);
    let decision = Decision {
        consume: vec![Consume {
            pool: OpPool::BOrdinary,
            n: 1,
            principal: Some(auth.user_id.to_string()),
            principal_cap: Some(quotas.artifact_reads_per_day),
        }],
        ..Decision::default()
    };
    match governor_client::decide(env, &decision).await {
        Gate::Allowed => {}
        Gate::Refused(refusal) => return governor_refusal_response(refusal.as_ref(), false),
    }

    let Some(object) = env.bucket("BLOBS")?.get(key).execute().await? else {
        console_error!("blob {key} for {scope}/{name}@{version} is missing from R2");
        return error_response(500, error::INTERNAL);
    };
    let size = object.size();
    let Some(body) = object.body() else {
        console_error!("blob {key} for {scope}/{name}@{version} has no body");
        return error_response(500, error::INTERNAL);
    };
    if size > BLOB_CACHE_MAX_BYTES {
        // Too large to buffer: stream it out uncached. The read was
        // charged like any miss, so oversized archives simply keep
        // paying per download instead of pressuring isolate memory.
        let mut response = Response::from_stream(body.stream()?)?;
        let headers = response.headers_mut();
        headers.set("content-type", "application/zip")?;
        headers.set("content-length", &size.to_string())?;
        headers.set("cache-control", "no-store")?;
        return Ok(response);
    }
    // Buffered, not streamed, for everything cacheable: a fixed body
    // is what lets the runtime tee one copy into the cache without a
    // second R2 read (a plain stream does not store reliably).
    let bytes = body.bytes().await?;
    let mut response = Response::from_bytes(bytes)?;
    response
        .headers_mut()
        .set("content-type", "application/zip")?;
    // The freshness directives go on the internal cache copy ONLY: the
    // outward response answered an authenticated request, and `public`
    // would explicitly license shared caches to store and re-serve it
    // past the Worker's auth gates (RFC 9111's Authorization
    // exception).
    let mut for_cache = response.cloned()?;
    for_cache
        .headers_mut()
        .set("cache-control", BLOB_CACHE_CONTROL)?;
    response.headers_mut().set("cache-control", "no-store")?;
    if let Err(err) = worker::Cache::default().put(cache_url, for_cache).await {
        console_error!("caching blob {key} failed: {err}");
    }
    Ok(response)
}

thread_local! {
    /// Buffered download counts per served verified version, flushed
    /// to D1 in one batch under `crate::telemetry`'s policy - the
    /// replacement for the old one-D1-write-per-download pattern.
    static PENDING_DOWNLOADS: RefCell<HashMap<(String, String, String), u32>> =
        RefCell::new(HashMap::new());
    static LAST_DOWNLOAD_FLUSH_MS: Cell<f64> = const { Cell::new(0.0) };
}

/// Buffers one served verified download and flushes the buffer when
/// the batching policy says so (`docs/architecture.md`, "Download
/// counts"). Called only once a 200 artifact response is constructed -
/// refusals and missing-blob 500s never count. The counter is
/// approximate telemetry, never the hard accounting ledger: counts
/// buffered in an isolate that dies are lost, a failed flush is logged
/// and dropped, and nothing here can fail or delay a download. The
/// flush - the breaker-mode read included - runs off the response path
/// and is suppressed while the breaker blocks writes, treating an
/// unreadable mode as blocked (the write plane's fail-closed
/// direction).
fn count_download(env: &Env, ctx: &Context, scope: &str, name: &str, version: &str) {
    let pending = PENDING_DOWNLOADS.with(|map| {
        let mut map = map.borrow_mut();
        *map.entry((scope.to_owned(), name.to_owned(), version.to_owned()))
            .or_insert(0) += 1;
        map.len()
    });
    let now = now_epoch_ms();
    let interval_ms = env
        .var("DOWNLOAD_FLUSH_INTERVAL_MS")
        .ok()
        .and_then(|var| var.to_string().parse().ok())
        .unwrap_or(telemetry::FLUSH_INTERVAL_MS);
    if !telemetry::should_flush(
        pending,
        now - LAST_DOWNLOAD_FLUSH_MS.with(Cell::get),
        interval_ms,
    ) {
        return;
    }
    LAST_DOWNLOAD_FLUSH_MS.with(|cell| cell.set(now));
    let batch: Vec<((String, String, String), u32)> =
        PENDING_DOWNLOADS.with(|map| map.borrow_mut().drain().collect());
    let env = env.clone();
    ctx.wait_until(async move {
        let Ok(db) = env.d1("DB") else {
            return;
        };
        let mode = service_mode(&env, &db)
            .await
            .unwrap_or(breaker::Mode::WritesBlocked);
        if mode >= breaker::Mode::WritesBlocked {
            return;
        }
        let statements: Vec<_> = batch
            .iter()
            .filter_map(|((scope, name, version), count)| {
                db.prepare(sql::ADD_VERSION_DOWNLOADS)
                    .bind(&[
                        scope.as_str().into(),
                        name.as_str().into(),
                        version.as_str().into(),
                        js_int(i64::from(*count)),
                    ])
                    .ok()
            })
            .collect();
        if statements.is_empty() {
            return;
        }
        if let Err(err) = db.batch(statements).await {
            console_error!(
                "download-count flush of {} versions failed: {err}",
                batch.len()
            );
        }
    });
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
struct UserUsageRecord {
    stored_bytes: i64,
}

#[derive(Deserialize)]
struct PackageCountsRecord {
    package_count: i64,
    new_today: i64,
}

#[derive(Deserialize)]
struct CountRecord {
    n: i64,
}

/// Everything the write phase of a validated, quota-cleared publish
/// needs.
struct NewVersion<'a> {
    scope: &'a str,
    name: &'a str,
    version: &'a str,
    checksum_hex: &'a str,
    metadata_text: &'a str,
    published_at: &'a str,
    archive: &'a [u8],
    user_id: i64,
}

/// The write phase's outcome: persisted, lost its guarded race, or
/// refused by the governor before any billable R2 call.
enum Persist {
    Done,
    Lost,
    Refused(Response),
}

/// The publish write phase: R2 before D1, skipping the upload when the
/// content-addressed blob is already there (e.g. the same archive
/// published under a name it was yanked from, or a retry after a crash
/// between the two writes), then one atomic D1 batch for the package and
/// version rows plus the storage self-accounting. The row starts
/// `pending`: it becomes resolvable only once the verifier says so.
///
/// Every billable R2 call is governed (`docs/architecture.md`, "The
/// cost governor"): the existence head consumes a publish-plane Class B
/// op, and a fresh upload consumes a Class A op plus a storage
/// reservation keyed by the content-addressed object key - so retries
/// and concurrent identical publishes share one reservation instead of
/// double-counting, and a crash after the put leaves the reservation
/// conservatively held (never auto-released; reconciliation settles it
/// once the D1 rows prove the blob live, and reports it otherwise).
/// After the batch commits the reservation settles into committed
/// usage.
///
/// The accounting decision lives inside the batch (one transaction): the
/// meta bump counts the archive only when the row just inserted is the
/// checksum's sole **live** (non-rejected) reference - a rejected row's
/// bytes were refunded when its blob was reclaimed, so it must not
/// suppress re-counting a re-uploaded blob. That way the crash-retry
/// path - blob already uploaded but never counted - still accounts for
/// it, a second name sharing the blob never double-counts it, and two
/// concurrent first publishes of the same archive serialize on the
/// transaction so exactly one of them counts it. Backup replication no
/// longer rides publish at all: only versions that become **verified**
/// enter the durable backup queue ([`sql::ENQUEUE_VERIFIED_BACKUP`]).
///
/// [`Persist::Lost`] means the batch's `-`/`_` twin guard suppressed
/// both inserts - a twin publish won the race after this request's
/// preflight - and nothing was persisted (the uploaded blob stays
/// behind exactly like a crash between the two writes: an orphan the
/// ledger keeps conservatively represented); the caller answers the
/// twin `400`.
async fn persist_new_version(
    env: &Env,
    db: &D1Database,
    new: &NewVersion<'_>,
) -> worker::Result<Persist> {
    let key = format!("blobs/sha256/{}", new.checksum_hex);
    let bucket = env.bucket("BLOBS")?;
    match governor_client::decide(env, &consume_one(OpPool::BPublish)).await {
        Gate::Allowed => {}
        Gate::Refused(refusal) => {
            return Ok(Persist::Refused(governor_refusal_response(
                refusal.as_ref(),
                true,
            )?));
        }
    }
    if bucket.head(&key).await?.is_none() {
        let admit = Decision {
            consume: vec![Consume {
                pool: OpPool::APublish,
                n: 1,
                principal: None,
                principal_cap: None,
            }],
            reserve: vec![Reserve {
                pool: StoragePool::Primary,
                key: key.clone(),
                bytes: new.archive.len() as u64,
            }],
            ..Decision::default()
        };
        match governor_client::decide(env, &admit).await {
            Gate::Allowed => {}
            Gate::Refused(refusal) => {
                return Ok(Persist::Refused(governor_refusal_response(
                    refusal.as_ref(),
                    true,
                )?));
            }
        }
        bucket.put(&key, new.archive.to_vec()).execute().await?;
    }

    let archive_size = js_int(i64::try_from(new.archive.len()).unwrap_or(i64::MAX));
    let results = db
        .batch(vec![
            db.prepare(sql::INSERT_PACKAGE).bind(&[
                new.scope.into(),
                new.name.into(),
                new.published_at.into(),
                js_int(new.user_id),
            ])?,
            db.prepare(sql::INSERT_VERSION).bind(&[
                new.scope.into(),
                new.name.into(),
                new.version.into(),
                new.checksum_hex.into(),
                new.metadata_text.into(),
                new.published_at.into(),
                archive_size.clone(),
                js_int(new.user_id),
            ])?,
            db.prepare(sql::COUNT_STORED_BYTES_ON_PUBLISH).bind(&[
                new.checksum_hex.into(),
                archive_size.clone(),
                new.checksum_hex.into(),
                archive_size,
                new.scope.into(),
                new.name.into(),
                new.version.into(),
            ])?,
        ])
        .await?;
    // The version insert changes zero rows only under the twin guard
    // (a duplicate `(scope, name, version)` fails the primary key and
    // rolls the batch back instead); the accounting statement is
    // gated on this exact row existing, so it added nothing then.
    let version_insert = results
        .get(1)
        .ok_or_else(|| worker::Error::RustError("missing batch result 1".to_owned()))?;
    if changed_rows(version_insert.meta()?) == 0 {
        return Ok(Persist::Lost);
    }

    // The row now references the blob: settle the reservation into
    // committed usage (best-effort - a lost settle leaves conservative
    // reserved state for reconciliation, never unaccounted spend).
    governor_client::settle(
        env,
        &commit_object(StoragePool::Primary, &key, new.archive.len() as u64),
    )
    .await;

    heal_blob_if_reclaimed(env, &bucket, &key, new.archive).await?;
    Ok(Persist::Done)
}

/// Self-heal for the head-skip/reclaim race: a reclaim delete whose
/// refcount was read before the publish batch committed can land after
/// the earlier head, leaving the just-inserted row's blob missing; the
/// request still holds the bytes, so one more head buys the repair.
/// Every call here is billable, so the whole repair is opportunistic:
/// a governor refusal skips it (the publish already succeeded, and the
/// missing-blob case stays loud on the artifact route) rather than
/// initiating unpaid R2 work.
async fn heal_blob_if_reclaimed(
    env: &Env,
    bucket: &worker::Bucket,
    key: &str,
    archive: &[u8],
) -> worker::Result<()> {
    match governor_client::decide(env, &consume_one(OpPool::BPublish)).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_log!("governor refused the post-publish heal head for {key}; skipping");
            return Ok(());
        }
    }
    if bucket.head(key).await?.is_some() {
        return Ok(());
    }
    let admit = Decision {
        consume: vec![Consume {
            pool: OpPool::APublish,
            n: 1,
            principal: None,
            principal_cap: None,
        }],
        // Idempotent against the committed row: same key, same bytes.
        reserve: vec![Reserve {
            pool: StoragePool::Primary,
            key: key.to_owned(),
            bytes: archive.len() as u64,
        }],
        ..Decision::default()
    };
    match governor_client::decide(env, &admit).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_log!("governor refused the post-publish heal put for {key}; skipping");
            return Ok(());
        }
    }
    bucket.put(key, archive.to_vec()).execute().await?;
    Ok(())
}

/// The write phase for a publish over a **rejected** row
/// (`docs/remote-registry.md`, "Verification lifecycle"): the rejected
/// version never became part of the registry, so any bytes replace the
/// row in place - new checksum, metadata, size, publisher, and
/// timestamp, verification back to `pending` with the old verdict
/// cleared. R2 first, like [`persist_new_version`]. Both statements are
/// guarded on the row still being the rejected row this request read -
/// `false` means a concurrent replacement or verdict moved it first and
/// nothing was changed (a stale replacement must never rewrite a live
/// row, least of all drag a verified one back to pending). The
/// accounting is decided **before** the row flips, mirroring
/// [`apply_rejection`]: the counter gains the new archive's bytes
/// exactly when the guards will let the flip apply and no other live
/// row already references the new checksum - so this row is about to
/// become its sole live reference, including the same-bytes republish,
/// where the reclaimed blob is re-counted exactly once. The rejected
/// row's own bytes were already refunded when the verdict landed
/// ([`verdict_response`]), so the only old-blob work here is retrying
/// the conditional delete, which heals a reclaim whose best-effort R2
/// delete failed.
async fn replace_rejected_version(
    env: &Env,
    db: &D1Database,
    new: &NewVersion<'_>,
    old_checksum: &str,
) -> worker::Result<Persist> {
    let key = format!("blobs/sha256/{}", new.checksum_hex);
    let bucket = env.bucket("BLOBS")?;
    // The unconditional put is one Class A op plus a storage
    // reservation; the reservation is idempotent when the ledger
    // already carries the content-addressed key (same key means the
    // same bytes).
    let admit = Decision {
        consume: vec![Consume {
            pool: OpPool::APublish,
            n: 1,
            principal: None,
            principal_cap: None,
        }],
        reserve: vec![Reserve {
            pool: StoragePool::Primary,
            key: key.clone(),
            bytes: new.archive.len() as u64,
        }],
        ..Decision::default()
    };
    match governor_client::decide(env, &admit).await {
        Gate::Allowed => {}
        Gate::Refused(refusal) => {
            return Ok(Persist::Refused(governor_refusal_response(
                refusal.as_ref(),
                true,
            )?));
        }
    }
    // Unconditional put, unlike persist_new_version's head-first skip:
    // when the replacement re-uses the rejected bytes, the rejecting
    // verdict's reclaim delete may still be in flight, and a head could
    // observe the object right before that delete lands - skipping the
    // upload would then leave a pending row whose blob is gone.
    // ponytail: a delete decided before this batch can still land after
    // this put (two stores, no shared transaction); that residual
    // window needs the same version's verdict and replacement in flight
    // simultaneously, fails loudly (the artifact route's missing-blob
    // 500), and the verified-only BACKUP replica holds the bytes for
    // recovery when the version had ever been verified.
    bucket.put(&key, new.archive.to_vec()).execute().await?;

    let archive_size = js_int(i64::try_from(new.archive.len()).unwrap_or(i64::MAX));
    let results = db
        .batch(vec![
            db.prepare(sql::COUNT_STORED_BYTES_ON_REPLACEMENT).bind(&[
                new.scope.into(),
                new.name.into(),
                new.version.into(),
                old_checksum.into(),
                new.checksum_hex.into(),
                archive_size.clone(),
            ])?,
            db.prepare(sql::REPLACE_REJECTED_VERSION).bind(&[
                new.checksum_hex.into(),
                new.metadata_text.into(),
                new.published_at.into(),
                archive_size,
                js_int(new.user_id),
                new.scope.into(),
                new.name.into(),
                new.version.into(),
                old_checksum.into(),
            ])?,
        ])
        .await?;
    let row_flip = results
        .get(1)
        .ok_or_else(|| worker::Error::RustError("missing batch result 1".to_owned()))?;
    if changed_rows(row_flip.meta()?) == 0 {
        // Lost the race; the blob uploaded above is at worst an
        // unreferenced orphan (see docs/runbook.md), which the kept
        // reservation represents conservatively.
        return Ok(Persist::Lost);
    }

    governor_client::settle(
        env,
        &commit_object(StoragePool::Primary, &key, new.archive.len() as u64),
    )
    .await;

    // Same self-heal as persist_new_version: repair the blob if a
    // reclaim delete landed between the put above and the batch commit.
    heal_blob_if_reclaimed(env, &bucket, &key, new.archive).await?;

    delete_blob_if_unreferenced(env, db, old_checksum).await?;
    Ok(Persist::Done)
}

/// Deletes `checksum`'s blob from the primary bucket when no live
/// (non-rejected) version row references it any more. Best-effort and
/// idempotent: a failed or crashed delete leaves an orphaned blob (the
/// same harmless, content-addressed garbage a crashed publish leaves -
/// see `docs/runbook.md`), and later reclaim paths retry it. Never
/// touches the meta counter - the caller accounts for the bytes when it
/// flips the row states - and never touches BACKUP, which is
/// append-only by design.
///
/// ponytail: the refcount read and the delete are not atomic with
/// publishes' R2 writes (two stores, no shared transaction). Publishers
/// close the practical window by re-checking their blob after their D1
/// batch commits and re-uploading if this delete beat them to it; a
/// delete that lands even later is loud (the artifact route's
/// missing-blob 500) and recoverable from the append-only BACKUP
/// replica. A reclaim queue with a grace period is the upgrade if
/// reclaim/publish races ever become a real operational pattern.
async fn delete_blob_if_unreferenced(
    env: &Env,
    db: &D1Database,
    checksum: &str,
) -> worker::Result<()> {
    let references: CountRecord = db
        .prepare(sql::COUNT_LIVE_BLOB_REFERENCES)
        .bind(&[checksum.into()])?
        .first(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("empty COUNT(*) result".to_owned()))?;
    if references.n > 0 {
        return Ok(());
    }
    let key = format!("blobs/sha256/{checksum}");
    // No live reference also means nothing needs a backup copy any
    // more: retire the queue row first (before the delete, so it can
    // never linger past the primary object), or a blob whose copy
    // never landed would keep the drain retrying against a deleted
    // primary object forever. The retire re-checks liveness inside
    // the statement - a verdict landing between this request's
    // refcount read and here enqueues transactionally, and its work
    // must not be lost to this stale reader.
    db.prepare(sql::RETIRE_DEAD_BACKUP_PENDING)
        .bind(&[key.as_str().into(), checksum.into()])?
        .run()
        .await?;
    // R2 deletes are not billable, so no consumption rides them. The
    // ledger entry deliberately stays committed: a successful delete is
    // NOT proof the key stays gone - a concurrent same-checksum publish
    // can recreate the content-addressed object at any moment, and a
    // release here could strand that publish's bytes outside the ledger
    // if it crashed before its own settle. Reconciliation reports the
    // entry as unreferenced, and releasing it is the operator's
    // explicit, evidence-backed action (`docs/runbook.md`, "The cost
    // governor"). The dump pool differs: its keys are cron-unique and
    // never concurrently recreated, so the dump jobs release their own.
    if let Err(err) = env.bucket("BLOBS")?.delete(&key).await {
        console_error!("reclaiming blob {key} failed (left as an orphan): {err}");
    }
    Ok(())
}

/// The idempotent no-op's self-heal: the retry holds the row's exact
/// bytes, so it repairs a primary blob a reclaim race deleted. Like
/// every repair path it is governed and opportunistic - a refusal
/// skips it without failing the (already correct) response. Backup
/// replication no longer rides retries: the verified-backup queue is
/// durable on its own.
async fn heal_blobs_on_retry(env: &Env, checksum_hex: &str, archive: &[u8]) -> worker::Result<()> {
    let key = format!("blobs/sha256/{checksum_hex}");
    let bucket = env.bucket("BLOBS")?;
    heal_blob_if_reclaimed(env, &bucket, &key, archive).await
}

#[derive(Deserialize)]
struct BackupPendingRecord {
    key: String,
}

/// Drains the verified-artifact backup queue (`docs/runbook.md`,
/// "Disaster recovery"): for each due row, replicate the blob from
/// BLOBS to the append-only BACKUP bucket and delete the row on
/// success. Runs off the verdict response path and on every breaker
/// cron pass; the queue row is the durable record, so any failure
/// here just leaves the work for the next pass (and the stale-row
/// alert if it keeps failing). Every billable call is charged to the
/// infrastructure pools first, and a governor refusal stops the drain
/// - fail closed, retry next pass.
async fn drain_backup_queue(env: &Env) {
    let (Ok(db), Ok(blobs), Ok(backup)) = (env.d1("DB"), env.bucket("BLOBS"), env.bucket("BACKUP"))
    else {
        console_error!("backup drain: a binding is missing");
        return;
    };
    // Bounded per call: keyset-paginated pages of 10, so rows a pass
    // keeps (missing primary blob, unconfirmed commit) are walked
    // past instead of re-read forever.
    let mut cursor = String::new();
    for _ in 0..5 {
        let rows: Vec<BackupPendingRecord> = match db
            .prepare(sql::LIST_BACKUP_PENDING)
            .bind(&[cursor.as_str().into()])
        {
            Ok(statement) => match statement.all().await {
                Ok(result) => match result.results() {
                    Ok(rows) => rows,
                    Err(err) => {
                        console_error!("backup drain: queue rows did not parse: {err}");
                        return;
                    }
                },
                Err(err) => {
                    console_error!("backup drain: queue listing failed: {err}");
                    return;
                }
            },
            Err(err) => {
                console_error!("backup drain: queue listing failed to bind: {err}");
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        let page = rows.len();
        for row in &rows {
            if !backup_one(env, &db, &blobs, &backup, row).await {
                return;
            }
        }
        if page < 10 {
            return;
        }
        if let Some(last) = rows.last() {
            cursor.clone_from(&last.key);
        }
    }
}

/// Replicates one queue row; `false` stops the drain pass (governor
/// refusal or a storage error worth backing off from).
async fn backup_one(
    env: &Env,
    db: &D1Database,
    blobs: &worker::Bucket,
    backup: &worker::Bucket,
    row: &BackupPendingRecord,
) -> bool {
    let delete_row = |key: String| async move {
        if let Ok(statement) = db.prepare(sql::DELETE_BACKUP_PENDING).bind(&[key.into()])
            && statement.run().await.is_err()
        {
            console_error!("backup drain: deleting a queue row failed");
        }
    };
    let Some(checksum) = row.key.strip_prefix("blobs/sha256/") else {
        console_error!("backup drain: malformed queue key {}", row.key);
        delete_row(row.key.clone()).await;
        return true;
    };
    // Only blobs the registry still serves as verified content are
    // worth a backup copy: a rejection (or replacement) that landed
    // after the enqueue retires the row instead.
    let live: Result<Option<CountRecord>, _> = match db
        .prepare(sql::COUNT_LIVE_VERIFIED_BLOB_REFERENCES)
        .bind(&[checksum.into()])
    {
        Ok(statement) => statement.first(None).await,
        Err(err) => Err(err),
    };
    match live {
        Ok(Some(count)) if count.n > 0 => {}
        Ok(_) => {
            // Dead row: retire under the in-statement liveness
            // re-check, so a verdict racing this pass cannot lose its
            // just-enqueued work.
            if let Ok(statement) = db
                .prepare(sql::RETIRE_DEAD_BACKUP_PENDING)
                .bind(&[row.key.as_str().into(), checksum.into()])
                && statement.run().await.is_err()
            {
                console_error!("backup drain: retiring a dead queue row failed");
            }
            return true;
        }
        Err(err) => {
            console_error!("backup drain: liveness check for {} failed: {err}", row.key);
            return false;
        }
    }

    match governor_client::decide(env, &consume_one(OpPool::BInfra)).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_error!("backup drain: governor refused the backup head; stopping");
            return false;
        }
    }
    match backup.head(&row.key).await {
        Ok(Some(object)) => {
            // Already replicated (a shared checksum, a retry after a
            // lost acknowledgement, or an out-of-band backfill copy):
            // settle the ledger at the size the head OBSERVED - the
            // object is reality, the queue row only expected it. The
            // row is retired only once the commit is CONFIRMED: for an
            // out-of-band copy there is no reservation behind it, so
            // deleting the row on an unacknowledged commit would leave
            // a real backup object unledgered with nothing left to
            // retry.
            let commit = commit_object(StoragePool::Backup, &row.key, object.size());
            match governor_client::decide(env, &commit).await {
                Gate::Allowed => {
                    delete_row(row.key.clone()).await;
                    true
                }
                Gate::Refused(_) => {
                    console_error!(
                        "backup drain: ledger commit for {} unconfirmed; keeping the row",
                        row.key
                    );
                    false
                }
            }
        }
        Ok(None) => match copy_blob_to_backup(env, blobs, backup, &row.key).await {
            CopyOutcome::Copied(len) => {
                // Same confirmed-commit rule as the head arm. A lost
                // commit here would still be covered by the copy's
                // reservation, but keeping the row costs one retry and
                // keeps the settled/reserved distinction honest.
                let commit = commit_object(StoragePool::Backup, &row.key, len);
                match governor_client::decide(env, &commit).await {
                    Gate::Allowed => {
                        delete_row(row.key.clone()).await;
                        true
                    }
                    Gate::Refused(_) => {
                        console_error!(
                            "backup drain: ledger commit for {} unconfirmed; keeping the row",
                            row.key
                        );
                        false
                    }
                }
            }
            CopyOutcome::KeepRow => true,
            CopyOutcome::Stop => false,
        },
        Err(err) => {
            console_error!("backup drain: head for {} failed: {err}", row.key);
            false
        }
    }
}

enum CopyOutcome {
    /// The copy landed; the value is the object's byte length.
    Copied(u64),
    /// This row cannot be copied right now (missing primary blob);
    /// keep it so the stale-row alert keeps firing.
    KeepRow,
    /// Back off: governor refusal or a storage error.
    Stop,
}

/// The charged copy itself: one infrastructure Class B read of the
/// primary blob, then one Class A put with a backup-storage
/// reservation, both taken before their calls.
async fn copy_blob_to_backup(
    env: &Env,
    blobs: &worker::Bucket,
    backup: &worker::Bucket,
    key: &str,
) -> CopyOutcome {
    match governor_client::decide(env, &consume_one(OpPool::BInfra)).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_error!("backup drain: governor refused the source read; stopping");
            return CopyOutcome::Stop;
        }
    }
    let object = match blobs.get(key).execute().await {
        Ok(Some(object)) => object,
        Ok(None) => {
            // A verified version's primary blob is missing: loud, and
            // the row stays so the alert keeps firing until an
            // operator intervenes.
            console_error!("backup drain: primary blob {key} is missing");
            return CopyOutcome::KeepRow;
        }
        Err(err) => {
            console_error!("backup drain: reading {key} failed: {err}");
            return CopyOutcome::Stop;
        }
    };
    let Some(body) = object.body() else {
        console_error!("backup drain: blob {key} has no body");
        return CopyOutcome::KeepRow;
    };
    let bytes = match body.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            console_error!("backup drain: buffering {key} failed: {err}");
            return CopyOutcome::Stop;
        }
    };

    let admit = Decision {
        consume: vec![Consume {
            pool: OpPool::AInfra,
            n: 1,
            principal: None,
            principal_cap: None,
        }],
        reserve: vec![Reserve {
            pool: StoragePool::Backup,
            key: key.to_owned(),
            bytes: bytes.len() as u64,
        }],
        ..Decision::default()
    };
    match governor_client::decide(env, &admit).await {
        Gate::Allowed => {}
        Gate::Refused(_) => {
            console_error!("backup drain: governor refused the backup put; stopping");
            return CopyOutcome::Stop;
        }
    }
    let len = bytes.len() as u64;
    if let Err(err) = backup.put(key, bytes).execute().await {
        // The put's outcome is uncertain: the reservation stays held
        // (conservative) and the queue row retries next pass.
        console_error!("backup drain: replicating {key} failed: {err}");
        return CopyOutcome::Stop;
    }
    CopyOutcome::Copied(len)
}

#[derive(Deserialize)]
struct LiveBlobRecord {
    checksum: String,
    size: i64,
}

/// The governor reconciliation pass (`docs/architecture.md`, "The cost
/// governor"): pushes D1's authoritative live-blob set so the ledger
/// records every referenced blob as committed usage. Increase-only by
/// construction - the governor adds and settles, but a ledger entry D1
/// does not name is only reported here (a candidate orphan or leaked
/// reservation), and releasing it is the operator's explicit,
/// evidence-backed action (`docs/runbook.md`, "Governor ledger").
async fn reconcile_governor(env: &Env, db: &D1Database) {
    // Operator visibility: one summary line per pass, so `wrangler
    // tail` shows the ledger next to the analytics-based evaluation.
    if let Some(snapshot) = governor_client::usage(env).await {
        let storage: Vec<String> = snapshot
            .storage
            .iter()
            .map(|row| format!("{}/{}={}B", row.pool, row.state, row.bytes))
            .collect();
        let ops: Vec<String> = snapshot
            .ops
            .iter()
            .map(|row| format!("{}[{}]={}", row.pool, row.window, row.used))
            .collect();
        console_log!(
            "governor usage: storage {}; ops {}",
            if storage.is_empty() {
                "-".to_owned()
            } else {
                storage.join(" ")
            },
            if ops.is_empty() {
                "-".to_owned()
            } else {
                ops.join(" ")
            },
        );
    }
    let rows: Vec<LiveBlobRecord> = match db.prepare(sql::LIVE_BLOB_SIZES).all().await {
        Ok(result) => match result.results() {
            Ok(rows) => rows,
            Err(err) => {
                console_error!("governor reconciliation: live set did not parse: {err}");
                return;
            }
        },
        Err(err) => {
            console_error!("governor reconciliation: live-set query failed: {err}");
            return;
        }
    };
    let live = rows
        .into_iter()
        .map(|row| governor::LiveObject {
            key: format!("blobs/sha256/{}", row.checksum),
            bytes: non_negative(row.size),
        })
        .collect();
    let request = governor::ReconcileRequest {
        pool: StoragePool::Primary,
        live,
    };
    match governor_client::reconcile(env, &request).await {
        Some(report) => {
            if !report.added.is_empty() {
                console_log!(
                    "governor reconciliation recorded {} previously unledgered blob(s)",
                    report.added.len()
                );
            }
            if !report.unreferenced.is_empty() || !report.mismatched.is_empty() {
                console_error!(
                    "governor ledger divergence: {} unreferenced entr(ies), {} byte \
                     mismatch(es); see docs/runbook.md, \"Governor ledger\"",
                    report.unreferenced.len(),
                    report.mismatched.len()
                );
            }
        }
        None => console_error!("governor reconciliation: the governor did not answer"),
    }
}

/// What the publish handler found for an already-existing
/// `(scope, name, version)` row.
enum ExistingVersion {
    /// The request is answered directly: byte-identical metadata means
    /// a byte-identical archive too (the metadata embeds the checksum
    /// and both uploads passed the digest check), so over a pending or
    /// verified row a re-publish is the `200` no-op reporting the row's
    /// current verification status, and different bytes hit the `409`
    /// immutability wall.
    Answered(Response),
    /// A rejected row never became part of the registry: any bytes are
    /// an accepted replacement ([`replace_rejected_version`]). The old
    /// checksum drives the self-healing blob cleanup.
    Rejected { old_checksum: String },
}

/// Idempotency, immutability, and the rejected-replacement carve-out
/// for an already-published `(scope, name, version)`. `None` means the
/// version is new.
async fn existing_version(
    db: &D1Database,
    scope: &str,
    name: &str,
    version: &str,
    metadata_text: &str,
) -> worker::Result<Option<ExistingVersion>> {
    let existing: Option<StoredVersionRecord> = db
        .prepare(sql::EXISTING_VERSION)
        .bind(&[scope.into(), name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    let Some(status) = verify::Status::parse(&existing.verification) else {
        // An invariant break (the schema never writes other values);
        // fail safe by refusing rather than guessing a transition.
        console_error!(
            "stored verification for {scope}/{name}@{version} is invalid: {}",
            existing.verification
        );
        return error_response(500, error::INTERNAL)
            .map(ExistingVersion::Answered)
            .map(Some);
    };
    if status == verify::Status::Rejected {
        return Ok(Some(ExistingVersion::Rejected {
            old_checksum: existing.checksum,
        }));
    }
    if existing.metadata_json == metadata_text {
        return json_response_with_status(
            200,
            &serde_json::json!({ "ok": true, "no_op": true, "verification": status.as_str() })
                .to_string(),
        )
        .map(ExistingVersion::Answered)
        .map(Some);
    }
    error_response(409, error::VERSION_IMMUTABLE)
        .map(ExistingVersion::Answered)
        .map(Some)
}

/// The publish token bucket (`429`), charged per publish attempt - valid
/// or not - before the body is even buffered. On an allowed take the new
/// bucket state is persisted as a compare-and-swap against the state the
/// take was computed from, so concurrent requests on one token cannot
/// all spend the same snapshot; a loser re-reads the row and retries
/// once. On a denial the stored state is left untouched, so refill keeps
/// accruing from the last persisted take, and the response carries
/// `Retry-After`.
async fn publish_rate_limit(
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    quotas: &quota::ClassQuotas,
) -> worker::Result<Option<Response>> {
    // Enough attempts to drain a full burst even when every one of them
    // loses a race to a parallel publisher on the same token.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small quota constant
    let attempts = quotas.publish_burst.ceil() as usize + 1;
    let mut bucket = auth.bucket;
    for _ in 0..attempts {
        let outcome = quota::take_publish_token(bucket, now_epoch_ms(), quotas);
        if !outcome.allowed {
            return Ok(Some(denial_response(
                env,
                &quota::RATE_LIMITED,
                Some(outcome.retry_after_secs),
            )?));
        }
        if cas_bucket(db, &auth.token_id, bucket, outcome.bucket).await? {
            return Ok(None);
        }
        bucket = read_bucket(db, &auth.token_id).await?;
    }
    // Losing a burst's worth of races in a row means the token is being
    // spent concurrently right now; refusing the attempt is the limiter
    // working. The bucket refills within a minute, hence the short
    // Retry-After.
    denial_response(env, &quota::RATE_LIMITED, Some(1)).map(Some)
}

#[derive(Deserialize)]
struct BucketRecord {
    rl_tokens: Option<f64>,
    rl_updated_at: Option<String>,
}

/// The current bucket state straight from the token row.
async fn read_bucket(db: &D1Database, token_id: &str) -> worker::Result<Option<quota::Bucket>> {
    let record: Option<BucketRecord> = db
        .prepare(sql::TOKEN_BUCKET)
        .bind(&[token_id.into()])?
        .first(None)
        .await?;
    Ok(record
        .and_then(|record| bucket_from_columns(record.rl_tokens, record.rl_updated_at.as_deref())))
}

/// Persists a bucket take iff the row still holds `prev` (`IS` makes the
/// comparison NULL-safe for a token that has never published). `false`
/// means a concurrent request won the race. Round-trip exactness holds:
/// the stored text and REAL came from these same f64 values.
async fn cas_bucket(
    db: &D1Database,
    token_id: &str,
    prev: Option<quota::Bucket>,
    next: quota::Bucket,
) -> worker::Result<bool> {
    use worker::wasm_bindgen::JsValue;
    let (prev_tokens, prev_updated_at) = match prev {
        Some(prev) => (
            JsValue::from_f64(prev.tokens),
            prev.updated_at_ms.to_string().into(),
        ),
        None => (JsValue::NULL, JsValue::NULL),
    };
    let result = db
        .prepare(sql::CAS_TOKEN_BUCKET)
        .bind(&[
            next.tokens.into(),
            next.updated_at_ms.to_string().into(),
            token_id.into(),
            prev_tokens,
            prev_updated_at,
        ])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

/// Gathers the [`quota::PublishCounts`] for one prospective publish in a
/// single D1 batch - every statement is a point lookup or an aggregate
/// over an indexed column - plus whether the name has a `-`/`_` twin in
/// the scope (the deterministic reject the caller renders when the
/// package would be new; a preflight only - the persistence batch
/// repeats the guard transactionally).
async fn publish_counts(
    db: &D1Database,
    user_id: i64,
    scope: &str,
    name: &str,
    day_prefix: &str,
) -> worker::Result<(quota::PublishCounts, bool)> {
    let results = db
        .batch(vec![
            // Rejected versions are excluded: their bytes were refunded
            // when the verdict landed.
            db.prepare(sql::USER_STORED_BYTES)
                .bind(&[js_int(user_id)])?,
            // Both package quotas key on creation (`created_by`), so a
            // version published into someone else's package never counts
            // against the publisher's package quotas.
            db.prepare(sql::USER_PACKAGE_COUNTS)
                .bind(&[js_int(user_id), day_prefix.into()])?,
            db.prepare(sql::COUNT_PACKAGE_VERSIONS_SINCE).bind(&[
                scope.into(),
                name.into(),
                day_prefix.into(),
            ])?,
            db.prepare(sql::PACKAGE_EXISTS)
                .bind(&[scope.into(), name.into()])?,
            db.prepare(sql::TWIN_PACKAGE_EXISTS)
                .bind(&[scope.into(), name.into()])?,
        ])
        .await?;
    let user_usage: UserUsageRecord = first_row(&results, 0)?;
    let user_packages: PackageCountsRecord = first_row(&results, 1)?;
    let versions_today: CountRecord = first_row(&results, 2)?;
    let package_rows: CountRecord = first_row(&results, 3)?;
    let twin_rows: CountRecord = first_row(&results, 4)?;
    let counts = quota::PublishCounts {
        user_stored_bytes: non_negative(user_usage.stored_bytes),
        user_package_count: non_negative(user_packages.package_count),
        user_new_packages_today: non_negative(user_packages.new_today),
        package_versions_today: non_negative(versions_today.n),
        package_exists: package_rows.n > 0,
    };
    Ok((counts, twin_rows.n > 0))
}

/// The single row of one aggregate statement in a batch result.
fn first_row<T: serde::de::DeserializeOwned>(
    results: &[worker::D1Result],
    index: usize,
) -> worker::Result<T> {
    results
        .get(index)
        .ok_or_else(|| worker::Error::RustError(format!("missing batch result {index}")))?
        .results::<T>()?
        .into_iter()
        .next()
        .ok_or_else(|| worker::Error::RustError(format!("empty batch result {index}")))
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

fn consume_one(pool: OpPool) -> Decision {
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

fn commit_object(pool: StoragePool, key: &str, bytes: u64) -> Decision {
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
        drain_backup_queue(&env).await;
    } else if let Err(err) = crate::backup_glue::run_nightly_dump(&env).await {
        console_error!("nightly backup failed: {err}");
    }
}

/// One usage snapshot: the exact self-accounted storage plus the
/// analytics-sourced metrics.
#[allow(clippy::similar_names)] // r2_class_{a,b}_month mirror the Usage fields
async fn gather_usage(env: &Env, db: &D1Database, now: &str) -> worker::Result<breaker::Usage> {
    // Storage is the exact self-accounted meta row, never analytics. A
    // missing or non-numeric row is unavailable data - never zero - so a
    // corrupt counter can only keep or escalate the mode, not reopen
    // writes.
    let stored_bytes = read_meta(db, "total_stored_bytes")
        .await?
        .and_then(|value| value.parse::<u64>().ok());
    if stored_bytes.is_none() {
        console_error!(
            "meta.total_stored_bytes is missing or non-numeric; treating as unavailable"
        );
    }

    let account = env
        .var("CF_ACCOUNT_ID")
        .map(|var| var.to_string())
        .unwrap_or_default();
    let workers_requests_today = match analytics::utc_day_start(now) {
        Some(start) => {
            fetch_metric(
                env,
                analytics::workers_requests_query(&account, &start),
                analytics::WORKERS_DATASET,
                "requests",
            )
            .await
        }
        None => None,
    };
    let r2_class_a_month = match analytics::utc_month_start(now) {
        Some(start) => {
            fetch_metric(
                env,
                analytics::r2_class_a_query(&account, &start),
                analytics::R2_DATASET,
                "requests",
            )
            .await
        }
        None => None,
    };
    let d1_rows_read_today = match analytics::utc_date(now) {
        Some(date) => {
            fetch_metric(
                env,
                analytics::d1_rows_read_query(&account, date),
                analytics::D1_DATASET,
                "rowsRead",
            )
            .await
        }
        None => None,
    };
    let r2_class_b_month = match analytics::utc_month_start(now) {
        Some(start) => {
            fetch_metric(
                env,
                analytics::r2_class_b_query(&account, &start),
                analytics::R2_DATASET,
                "requests",
            )
            .await
        }
        None => None,
    };

    Ok(breaker::Usage {
        stored_bytes,
        workers_requests_today,
        r2_class_a_month,
        d1_rows_read_today,
        r2_class_b_month,
    })
}

async fn evaluate_budgets(env: &Env) -> worker::Result<()> {
    let db = env.d1("DB")?;
    let now = now_iso8601();

    let usage = gather_usage(env, &db, &now).await?;
    let defaults = breaker::Budgets::default();
    // Presence arms `reads_blocked`: an operator who set
    // BUDGET_R2_CLASS_B_MONTH meant to cap read spend, so a value that
    // does not parse still arms the breaker - loudly, at the built-in
    // default budget - rather than silently reverting to warn-only
    // monitoring, which on a paid plan would be uncapped spend behind a
    // typo.
    let r2_class_b_env: Option<u64> = env
        .var("BUDGET_R2_CLASS_B_MONTH")
        .ok()
        .map(|var| var.to_string())
        .map(|value| {
            value.parse().unwrap_or_else(|_| {
                console_error!(
                    "BUDGET_R2_CLASS_B_MONTH is not a number ({value}); \
                     keeping the read breaker armed with the default budget"
                );
                defaults.r2_class_b_month
            })
        });
    let budgets = breaker::Budgets {
        r2_storage_bytes: env_budget(env, "BUDGET_R2_STORAGE_BYTES", defaults.r2_storage_bytes),
        r2_class_a_month: env_budget(env, "BUDGET_R2_CLASS_A_MONTH", defaults.r2_class_a_month),
        workers_requests_day: env_budget(
            env,
            "BUDGET_WORKERS_REQ_DAY",
            defaults.workers_requests_day,
        ),
        d1_rows_read_day: env_budget(env, "BUDGET_D1_ROWS_READ_DAY", defaults.d1_rows_read_day),
        r2_class_b_month: r2_class_b_env.unwrap_or(defaults.r2_class_b_month),
        r2_class_b_ceiling: if r2_class_b_env.is_some() {
            breaker::Mode::ReadsBlocked
        } else {
            defaults.r2_class_b_ceiling
        },
    };

    let (candidate, reason) = breaker::evaluate(&usage, &budgets);
    // A missing or corrupt stored mode is WritesBlocked, matching the
    // request path's fail-closed reading: partial analytics data must
    // never flip such a state back to normal (complete data still wins
    // outright below).
    let current = read_meta(&db, "service_mode")
        .await?
        .and_then(|value| breaker::Mode::parse(&value))
        .unwrap_or(breaker::Mode::WritesBlocked);
    let next = breaker::next_mode(
        current,
        candidate,
        usage.write_complete(),
        usage.read_complete(budgets.r2_class_b_ceiling == breaker::Mode::ReadsBlocked),
    );
    let reason = if next == candidate {
        reason
    } else {
        format!(
            "kept {} on incomplete analytics data (fresh evaluation said {}: {reason})",
            next.as_str(),
            candidate.as_str()
        )
    };

    // Persist mode and reason every pass so operators always see the
    // latest evaluation.
    db.batch(vec![
        upsert_meta(&db, "service_mode", next.as_str())?,
        upsert_meta(&db, "service_mode_reason", &reason)?,
    ])
    .await?;

    // Backup health rides every pass (docs/runbook.md, "Disaster
    // recovery"): an unhealthy backup logs and notifies on every pass
    // until resolved - a backup system's classic failure mode is
    // stopping silently - while mode changes notify once.
    let health = match read_backup_health(&db, &now).await {
        Ok(health) => health,
        Err(_) => BackupHealth::unreadable(),
    };
    if let Some(alert) = &health.alert {
        console_error!("backup health: {alert}");
    }
    // Verification health rides every pass too: versions pending for
    // over an hour mean the verifier is stuck or absent, and the
    // fail-safe direction (nothing pending ever becomes resolvable on
    // its own) makes that invisible to users unless it alerts here.
    let stale_pending = read_stale_pending(&db).await.ok();
    let verification_alert = verify::stale_pending_alert(stale_pending);
    if let Some(alert) = &verification_alert {
        console_error!("verification health: {alert}");
    }
    if next != current {
        console_log!(
            "service_mode {} -> {}: {reason}",
            current.as_str(),
            next.as_str()
        );
    }
    if next != current || health.alert.is_some() || verification_alert.is_some() {
        notify_webhook(
            env,
            current,
            next,
            &reason,
            &usage,
            &health,
            stale_pending,
            verification_alert.as_deref(),
        )
        .await;
    }
    Ok(())
}

/// How many versions have sat `pending` for over an hour. The cutoff is
/// rendered by `SQLite` in the same ISO 8601 shape `published_at` is
/// stored in (`%fZ` gives the fractional seconds and the `Z` the JS
/// clock writes), so the comparison stays lexicographic.
async fn read_stale_pending(db: &D1Database) -> worker::Result<u64> {
    let record: CountRecord = db
        .prepare(sql::COUNT_STALE_PENDING)
        .first(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("empty COUNT(*) result".to_owned()))?;
    Ok(non_negative(record.n))
}

/// One breaker pass's view of backup health, for the log line and the
/// webhook payload.
struct BackupHealth {
    last_backup_at: Option<String>,
    freshness: backup::Freshness,
    overdue_backups: Option<u64>,
    alert: Option<String>,
}

impl BackupHealth {
    /// Fail closed when D1 would not answer: alert rather than report
    /// an unknown state as healthy.
    fn unreadable() -> BackupHealth {
        BackupHealth {
            last_backup_at: None,
            freshness: backup::Freshness::Never,
            overdue_backups: None,
            alert: Some("backup health could not be read from d1".to_owned()),
        }
    }
}

async fn read_backup_health(db: &D1Database, now: &str) -> worker::Result<BackupHealth> {
    let last_backup_at = read_meta(db, "last_backup_at").await?;
    let overdue: CountRecord = db
        .prepare(sql::COUNT_STALE_BACKUP_PENDING)
        .first(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("empty COUNT(*) result".to_owned()))?;
    let overdue_backups = non_negative(overdue.n);
    let freshness = backup::freshness(now, last_backup_at.as_deref());
    Ok(BackupHealth {
        last_backup_at,
        freshness,
        overdue_backups: Some(overdue_backups),
        alert: backup::alert(freshness, overdue_backups),
    })
}

fn env_budget(env: &Env, name: &str, default: u64) -> u64 {
    env.var(name)
        .ok()
        .and_then(|var| var.to_string().parse().ok())
        .unwrap_or(default)
}

/// One analytics metric via the GraphQL Analytics API; `None` (with a
/// log line) on any failure, so a rejected dataset or a missing token
/// degrades that metric instead of failing the whole cron pass.
async fn fetch_metric(
    env: &Env,
    query: Option<String>,
    dataset: &str,
    metric: &str,
) -> Option<u64> {
    let Ok(token) = env.secret("ANALYTICS_API_TOKEN") else {
        console_log!("ANALYTICS_API_TOKEN is not set; skipping {dataset}");
        return None;
    };
    let query = query?;
    let response = post_json(
        analytics::GRAPHQL_ENDPOINT,
        &query,
        Some(&token.to_string()),
    )
    .await;
    let Ok(mut response) = response else {
        console_error!("analytics {dataset} request failed");
        return None;
    };
    if response.status_code() != 200 {
        console_error!("analytics {dataset} answered {}", response.status_code());
        return None;
    }
    let body = response.text().await.ok()?;
    let sum = analytics::parse_sum(&body, dataset, metric);
    if sum.is_none() {
        console_error!("analytics {dataset} response did not parse; treating as unavailable");
    }
    sum
}

/// POSTs a summary to `NOTIFY_WEBHOOK_URL` when it is configured, on
/// service-mode changes (`from != to`), backup alerts, and
/// stuck-verifier alerts alike; failures only log.
#[allow(clippy::too_many_arguments)] // one cron pass's full snapshot
async fn notify_webhook(
    env: &Env,
    from: breaker::Mode,
    to: breaker::Mode,
    reason: &str,
    usage: &breaker::Usage,
    health: &BackupHealth,
    stale_pending: Option<u64>,
    verification_alert: Option<&str>,
) {
    let Ok(url) = env.secret("NOTIFY_WEBHOOK_URL") else {
        return;
    };
    let body = serde_json::json!({
        "service": "cabin-registry",
        "from": from.as_str(),
        "to": to.as_str(),
        "reason": reason,
        "stored_bytes": usage.stored_bytes,
        "workers_requests_today": usage.workers_requests_today,
        "r2_class_a_month": usage.r2_class_a_month,
        "d1_rows_read_today": usage.d1_rows_read_today,
        "r2_class_b_month": usage.r2_class_b_month,
        "backup": {
            "last_backup_at": health.last_backup_at,
            "freshness": health.freshness.as_str(),
            "overdue_backups": health.overdue_backups,
            "alert": health.alert,
        },
        "verification": {
            "stale_pending": stale_pending,
            "alert": verification_alert,
        },
    })
    .to_string();
    if post_json(&url.to_string(), &body, None).await.is_err() {
        console_error!("state-change webhook POST failed");
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

/// Renders a quota or rate-limit [`quota::Denial`]; the quota family's
/// detail embeds the dashboard URL built from `WEB_ORIGIN`
/// ([`quota::detail_with_usage_url`]).
fn denial_response(
    env: &Env,
    denial: &quota::Denial,
    retry_after_secs: Option<u64>,
) -> worker::Result<Response> {
    let detail = quota::detail_with_usage_url(denial, &web_origin(env)?);
    error_response_with_code(denial.status, &detail, denial.code, retry_after_secs)
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
