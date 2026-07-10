//! Cloudflare-specific glue: binding access, D1/R2 I/O, and response
//! plumbing. Everything with behavior worth testing lives in the host-target
//! modules; keep this file thin.

use std::cell::Cell;
use std::fmt::Write as _;

use serde::Deserialize;
use worker::{
    Context, D1Database, Env, Fetch, Headers, Method, Request, RequestInit, Response,
    ScheduleContext, ScheduledEvent, console_error, console_log, event,
};

use crate::auth::{self, AuthContext, Scope};
use crate::documents::{self, VersionRow};
use crate::error;
use crate::publish;
use crate::routes::{ApiRoute, Route, match_api_route, match_route, match_web_route};
use crate::web_glue;
use crate::{analytics, backup, breaker, quota, verify};

const GENERATION_HEADER: &str = "x-cabin-registry-generation";

#[derive(Deserialize)]
struct TokenRecord {
    id: String,
    user_id: i64,
    scopes: String,
    plan: String,
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
        match route {
            Route::Config => json_response(&documents::config_json(&web_origin(env)?))?,
            Route::Package { name } => package_response(&db, name).await?,
            Route::Artifact { name, version } => {
                artifact_response(env, &db, &auth, name, version).await?
            }
            // Answered above before the auth gate.
            Route::Healthz => Response::empty()?,
        }
    } else {
        error_response(405, error::METHOD_NOT_ALLOWED)?
    };

    // Debug aid for the disposable dev environment (see docs/runbook.md):
    // stamp every authenticated response with the registry generation.
    if let Some(generation) = registry_generation(&db).await {
        response.headers_mut().set(GENERATION_HEADER, &generation)?;
    }
    Ok((response, Some(auth.token_id)))
}

/// The website origin: the OAuth plane (`/login`, `/callback`), the
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

    // Everything else is the Bearer plane: deny by default, the uniform
    // 401 before any route matching or data lookup.
    let db = env.d1("DB")?;
    let Some(auth) = authenticate(req, &db, ctx).await? else {
        return Ok((unauthorized(env)?, None));
    };

    let mut response = match req.method() {
        // The admin listing is the one API route read with GET; anything
        // else is an authenticated 404.
        Method::Get => match match_api_route(path) {
            Some(ApiRoute::AdminVersions) => admin_versions_response(req, &db, &auth).await?,
            _ => error_response(404, error::NOT_FOUND)?,
        },
        Method::Put => match match_api_route(path) {
            Some(ApiRoute::Publish { name, version }) => {
                let (name, version) = (name.to_owned(), version.to_owned());
                publish_response(req, env, ctx, &db, &auth, &name, &version).await?
            }
            Some(
                ApiRoute::Yank { .. } | ApiRoute::AdminVersions | ApiRoute::AdminVerdict { .. },
            ) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        Method::Patch => match match_api_route(path) {
            Some(ApiRoute::Yank { name, version }) => {
                let (name, version) = (name.to_owned(), version.to_owned());
                yank_response(req, env, &db, &auth, &name, &version).await?
            }
            Some(ApiRoute::AdminVerdict { name, version }) => {
                let (name, version) = (name.to_owned(), version.to_owned());
                verdict_response(req, env, &db, &auth, &name, &version).await?
            }
            Some(ApiRoute::Publish { .. } | ApiRoute::AdminVersions) => {
                error_response(405, error::METHOD_NOT_ALLOWED)?
            }
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
        .prepare(
            "SELECT t.id, t.user_id, t.scopes, u.plan, t.rl_tokens, t.rl_updated_at \
             FROM tokens t JOIN users u ON u.github_id = t.user_id \
             WHERE t.token_hash = ?1 AND t.revoked_at IS NULL",
        )
        .bind(&[hash.into()])?
        .first(None)
        .await?;
    let Some(record) = record else {
        return Ok(None);
    };

    // Best-effort bookkeeping: never fail or delay the request over it.
    if let Ok(update) = db
        .prepare("UPDATE tokens SET last_used_at = ?1 WHERE id = ?2")
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
        plan: record.plan,
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

/// `GET /packages/<name>.json`: composed from **verified** versions
/// only - the filter is in the query, so pending and rejected rows
/// never reach composition, and a package with no verified versions is
/// indistinguishable from an unknown one (fail safe: if the verifier
/// never runs, nothing new ever becomes resolvable).
async fn package_response(db: &D1Database, name: &str) -> worker::Result<Response> {
    let records: Vec<VersionRecord> = db
        .prepare(
            "SELECT version, metadata_json, yanked FROM versions \
             WHERE name = ?1 AND verification = 'verified'",
        )
        .bind(&[name.into()])?
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
    match documents::package_json(name, &rows) {
        Ok(body) => json_response(&body),
        Err(detail) => {
            console_error!("package document for {name}: {detail}");
            error_response(500, error::INTERNAL)
        }
    }
}

/// `PUT /api/v1/packages/<name>/<version>`: the publish route
/// (`docs/remote-registry.md`, "Publish"). Validation order and status
/// mapping follow `crate::publish`, preceded by the budget gate (`402`)
/// and the publish rate limit (`429`), and followed - for genuinely new
/// versions and replacements of rejected ones only - by the
/// archive-size cap (`413`) and the per-user quota checks (`403`); on
/// success the archive lands in R2 first (an orphaned blob from a crash
/// between the two writes is harmless, content-addressed garbage - see
/// `docs/runbook.md`), then one atomic D1 batch inserts (or, for a
/// rejected row, replaces) the package and version rows and bumps the
/// storage self-accounting. New rows start `pending` and the `201`
/// reports it: nothing becomes resolvable before the verifier says so.
async fn publish_response(
    req: &mut Request,
    env: &Env,
    ctx: &Context,
    db: &D1Database,
    auth: &AuthContext,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    if let Some(blocked) = write_gate(env, db).await? {
        return Ok(blocked);
    }
    if !auth.scopes.contains(&Scope::Publish) {
        return error_response(403, error::PUBLISH_SCOPE_REQUIRED);
    }

    let quotas = quota::quotas_for_plan(&auth.plan);
    if let Some(limited) = publish_rate_limit(env, db, auth, &quotas).await? {
        return Ok(limited);
    }

    // Reject oversized uploads before buffering when the client declared
    // a length; `decode_frame` re-checks the buffered size regardless.
    if let Some(length) = req.headers().get("content-length")?
        && length
            .parse::<u64>()
            .is_ok_and(|n| n > publish::MAX_BODY_BYTES as u64)
    {
        return error_response(400, publish::BODY_TOO_LARGE);
    }

    let body = req.bytes().await?;
    let frame = match publish::decode_frame(&body) {
        Ok(frame) => frame,
        Err(detail) => return error_response(400, detail),
    };
    let archive_bytes = frame.archive.len() as u64;
    let metadata = match publish::validate_metadata(name, version, frame.metadata) {
        Ok(metadata) => metadata,
        Err(detail) => return error_response(400, detail),
    };
    let computed_hex = sha256_hex(frame.archive).await?;
    if let Err(detail) = publish::verify_checksum(&metadata, &computed_hex) {
        return error_response(400, detail);
    }
    // The frame parsed as JSON, so it is valid UTF-8; the stored column
    // is the uploaded document verbatim.
    let Ok(metadata_text) = std::str::from_utf8(frame.metadata) else {
        return error_response(400, publish::METADATA_NOT_JSON);
    };

    let replaced = match existing_version(db, name, version, metadata_text).await? {
        Some(ExistingVersion::Answered(response)) => {
            // The idempotent no-op (200) is a retry of a committed
            // publish that still holds the row's exact bytes, so it is
            // the one chance to self-heal both stores: a primary blob a
            // reclaim race deleted, and a backup copy a crashed
            // original's replication never wrote. The 409 arm gets
            // neither: its uploaded bytes were rejected.
            if response.status_code() == 200 {
                heal_blobs_on_retry(env, ctx, &computed_hex, frame.archive).await?;
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
    let counts = publish_counts(db, auth.user_id, name, &day_prefix).await?;
    if let Err(denial) = quota::check_publish(archive_bytes, &counts, &quotas) {
        return denial_response(env, &denial, None);
    }

    let new = NewVersion {
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
            if !replace_rejected_version(env, ctx, db, &new, old_checksum).await? {
                // A concurrent replacement or verdict moved the rejected
                // row first; answer for the row's new state exactly as if
                // this request had arrived after the winner.
                return match existing_version(db, name, version, metadata_text).await? {
                    Some(ExistingVersion::Answered(response)) => {
                        if response.status_code() == 200 {
                            heal_blobs_on_retry(env, ctx, &computed_hex, frame.archive).await?;
                        }
                        Ok(response)
                    }
                    // Rejected again (a third racer) or gone: the
                    // conservative refusal; a retry resolves it.
                    _ => error_response(409, error::VERSION_IMMUTABLE),
                };
            }
        }
        None => persist_new_version(env, ctx, db, &new).await?,
    }

    json_response_with_status(
        201,
        &serde_json::json!({
            "ok": true,
            "name": name,
            "version": version,
            "checksum": metadata.checksum,
            "verification": "pending",
        })
        .to_string(),
    )
}

/// `PATCH /api/v1/packages/<name>/<version>/yank`
/// (`docs/remote-registry.md`, "Yank"): idempotent, and the row's
/// `yanked` column is the single home of yank state - the read path
/// overrides the stored metadata's field from it. Gated by the budget
/// breaker (`402`) like every write; yank has no rate limit or quota.
/// Yank applies to **verified** versions only: a pending or rejected
/// version was never part of the registry's resolvable surface, so
/// there is nothing to retract and the pair answers an authenticated
/// 404.
async fn yank_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    if let Some(blocked) = write_gate(env, db).await? {
        return Ok(blocked);
    }
    if !auth.scopes.contains(&Scope::Yank) {
        return error_response(403, error::YANK_SCOPE_REQUIRED);
    }
    let body = req.bytes().await?;
    let Ok(YankBody { yanked }) = serde_json::from_slice(&body) else {
        return error_response(400, error::INVALID_YANK_BODY);
    };

    let existing: Option<YankedRecord> = db
        .prepare("SELECT yanked, verification FROM versions WHERE name = ?1 AND version = ?2")
        .bind(&[name.into(), version.into()])?
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
        db.prepare("UPDATE versions SET yanked = ?1 WHERE name = ?2 AND version = ?3")
            .bind(&[i32::from(yanked).into(), name.into(), version.into()])?
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
    name: String,
    version: String,
    checksum: String,
    published_by: i64,
    published_at: String,
    metadata_json: String,
}

/// `GET /api/v1/admin/versions?status=<status>` (`verify` scope): the
/// verifier's work list. Each entry carries the stored canonical
/// metadata document (parsed, so the response is one JSON value), and
/// the listing is deterministic: ordered by name, then version.
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
        .prepare(
            "SELECT name, version, checksum, published_by, published_at, metadata_json \
             FROM versions WHERE verification = ?1 ORDER BY name, version",
        )
        .bind(&[status.as_str().into()])?
        .all()
        .await?
        .results()?;
    let mut versions = Vec::with_capacity(records.len());
    for record in records {
        let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&record.metadata_json) else {
            console_error!(
                "stored metadata for {}@{} is not valid JSON",
                record.name,
                record.version
            );
            return error_response(500, error::INTERNAL);
        };
        versions.push(serde_json::json!({
            "name": record.name,
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
struct VerdictTargetRecord {
    verification: String,
    checksum: String,
    published_at: String,
    archive_size: i64,
}

/// `PATCH /api/v1/admin/versions/<name>/<version>` (`verify` scope):
/// the verifier's verdict. Pending versions accept either verdict; a
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
/// cannot refund twice), and then reclaims the blob itself. Gated by
/// the budget breaker like every write. The response reports the
/// resulting state plus whether this request changed it.
async fn verdict_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    if let Some(blocked) = write_gate(env, db).await? {
        return Ok(blocked);
    }
    if !has_verify_scope(auth) {
        return error_response(403, error::VERIFY_SCOPE_REQUIRED);
    }
    let body = req.bytes().await?;
    let parsed = match verify::parse_verdict(&body) {
        Ok(parsed) => parsed,
        Err(detail) => return error_response(400, detail),
    };

    let target: Option<VerdictTargetRecord> = db
        .prepare(
            "SELECT verification, checksum, published_at, archive_size FROM versions \
             WHERE name = ?1 AND version = ?2",
        )
        .bind(&[name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(target) = target else {
        return error_response(404, error::NOT_FOUND);
    };
    let Some(current) = verify::Status::parse(&target.verification) else {
        console_error!(
            "stored verification for {name}@{version} is invalid: {}",
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
            if !apply_verdict(env, db, name, version, &parsed, &target).await? {
                // The row moved between this request's read and its
                // guarded update: a concurrent conflicting verdict or a
                // replacement won the race.
                return error_response(409, error::VERDICT_TARGET_CHANGED);
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
            "name": name,
            "version": version,
            "verification": resulting.as_str(),
            "changed": changed,
        })
        .to_string(),
    )
}

/// Rows changed by a statement, from its result metadata.
fn changed_rows(meta: Option<worker::D1ResultMeta>) -> usize {
    meta.and_then(|meta| meta.changes).unwrap_or(0)
}

/// Applies a verdict to a pending row under the transactional guards
/// (still pending, still the checksum and `published_at` this request
/// read); `false` means the row moved first and nothing was changed.
async fn apply_verdict(
    env: &Env,
    db: &D1Database,
    name: &str,
    version: &str,
    parsed: &verify::ParsedVerdict,
    target: &VerdictTargetRecord,
) -> worker::Result<bool> {
    match parsed.verdict {
        verify::Verdict::Verified => {
            let result = db
                .prepare(
                    "UPDATE versions SET verification = 'verified', verified_at = ?1 \
                     WHERE name = ?2 AND version = ?3 \
                     AND verification = 'pending' AND checksum = ?4 \
                     AND published_at = ?5",
                )
                .bind(&[
                    now_iso8601().into(),
                    name.into(),
                    version.into(),
                    target.checksum.as_str().into(),
                    target.published_at.as_str().into(),
                ])?
                .run()
                .await?;
            Ok(changed_rows(result.meta()?) > 0)
        }
        verify::Verdict::Rejected => {
            let applied = apply_rejection(
                db,
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
    name: &str,
    version: &str,
    reason: &str,
    target: &VerdictTargetRecord,
) -> worker::Result<bool> {
    let archive_size = js_int(target.archive_size);
    let results = db
        .batch(vec![
            db.prepare(
                "UPDATE meta SET value = MAX(CAST(value AS INTEGER) - \
                 CASE WHEN (SELECT COUNT(*) FROM versions \
                            WHERE checksum = ?1 AND verification != 'rejected') = 1 \
                      AND (SELECT verification FROM versions \
                           WHERE name = ?2 AND version = ?3) = 'pending' \
                      AND (SELECT checksum FROM versions \
                           WHERE name = ?2 AND version = ?3) = ?1 \
                      AND (SELECT published_at FROM versions \
                           WHERE name = ?2 AND version = ?3) = ?5 \
                      THEN CAST(?4 AS INTEGER) ELSE 0 END, 0) \
                 WHERE key = 'total_stored_bytes'",
            )
            .bind(&[
                target.checksum.as_str().into(),
                name.into(),
                version.into(),
                archive_size,
                target.published_at.as_str().into(),
            ])?,
            db.prepare(
                "UPDATE versions SET verification = 'rejected', verification_reason = ?1, \
                 verified_at = NULL \
                 WHERE name = ?2 AND version = ?3 \
                 AND verification = 'pending' AND checksum = ?4 AND published_at = ?5",
            )
            .bind(&[
                reason.into(),
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
    Ok(Uint8Array::new(&buffer)
        .to_vec()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

async fn artifact_response(
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    let record: Option<ArtifactRecord> = db
        .prepare("SELECT checksum, verification FROM versions WHERE name = ?1 AND version = ?2")
        .bind(&[name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(record) = record else {
        return error_response(404, error::NOT_FOUND);
    };
    // Verified versions download with any valid token; pending ones
    // only with the `verify` scope (the verifier fetches the bytes it
    // inspects); rejected ones - whose blob is reclaimed - and rows
    // with an unreadable status gate like missing rows.
    let readable = verify::Status::parse(&record.verification)
        .is_some_and(|status| verify::artifact_readable(status, has_verify_scope(auth)));
    if !readable {
        return error_response(404, error::NOT_FOUND);
    }

    // Archives are immutable and content-addressed; yanked versions stay
    // downloadable on purpose (docs/remote-registry.md, "Yank").
    let key = format!("blobs/sha256/{}", record.checksum);
    let Some(object) = env.bucket("BLOBS")?.get(&key).execute().await? else {
        console_error!("blob {key} for {name}@{version} is missing from R2");
        return error_response(500, error::INTERNAL);
    };
    let size = object.size();
    let Some(body) = object.body() else {
        console_error!("blob {key} for {name}@{version} has no body");
        return error_response(500, error::INTERNAL);
    };

    let mut response = Response::from_stream(body.stream()?)?;
    response
        .headers_mut()
        .set("content-type", "application/gzip")?;
    response
        .headers_mut()
        .set("content-length", &size.to_string())?;
    Ok(response)
}

/// Reads `meta.registry_generation`; best-effort (the header is a debug
/// aid, not part of the client contract).
async fn registry_generation(db: &D1Database) -> Option<String> {
    let record: Option<MetaRecord> = db
        .prepare("SELECT value FROM meta WHERE key = 'registry_generation'")
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
    name: &'a str,
    version: &'a str,
    checksum_hex: &'a str,
    metadata_text: &'a str,
    published_at: &'a str,
    archive: &'a [u8],
    user_id: i64,
}

/// The publish write phase: R2 before D1, skipping the upload when the
/// content-addressed blob is already there (e.g. the same archive
/// published under a name it was yanked from, or a retry after a crash
/// between the two writes), then one atomic D1 batch for the package and
/// version rows plus the storage self-accounting. The row starts
/// `pending`: it becomes resolvable only once the verifier says so.
///
/// The accounting decision lives inside the batch (one transaction): the
/// meta bump counts the archive only when the row just inserted is the
/// checksum's sole **live** (non-rejected) reference - a rejected row's
/// bytes were refunded when its blob was reclaimed, so it must not
/// suppress re-counting a re-uploaded blob. That way the crash-retry
/// path - blob already uploaded but never counted - still accounts for
/// it, a second name sharing the blob never double-counts it, and two
/// concurrent first publishes of the same archive serialize on the
/// transaction so exactly one of them counts it. Once the batch commits,
/// the blob is replicated to the BACKUP bucket off the response path
/// ([`replicate_blob`]).
async fn persist_new_version(
    env: &Env,
    ctx: &Context,
    db: &D1Database,
    new: &NewVersion<'_>,
) -> worker::Result<()> {
    let key = format!("blobs/sha256/{}", new.checksum_hex);
    let bucket = env.bucket("BLOBS")?;
    if bucket.head(&key).await?.is_none() {
        bucket.put(&key, new.archive.to_vec()).execute().await?;
    }

    let archive_size = js_int(i64::try_from(new.archive.len()).unwrap_or(i64::MAX));
    db.batch(vec![
        db.prepare(
            "INSERT OR IGNORE INTO packages (name, created_at, created_by) \
             VALUES (?1, ?2, ?3)",
        )
        .bind(&[
            new.name.into(),
            new.published_at.into(),
            js_int(new.user_id),
        ])?,
        db.prepare(
            "INSERT INTO versions (name, version, checksum, metadata_json, yanked, \
             published_at, archive_size, published_by, verification) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, 'pending')",
        )
        .bind(&[
            new.name.into(),
            new.version.into(),
            new.checksum_hex.into(),
            new.metadata_text.into(),
            new.published_at.into(),
            archive_size.clone(),
            js_int(new.user_id),
        ])?,
        // The CASTs keep the TEXT-affinity meta value integer-shaped: D1
        // binds numbers as floats, and INTEGER + REAL would otherwise
        // store "254.0", which the breaker's strict u64 parse rejects.
        db.prepare(
            "INSERT INTO meta (key, value) VALUES ('total_stored_bytes', \
             CASE WHEN (SELECT COUNT(*) FROM versions \
                        WHERE checksum = ?1 AND verification != 'rejected') = 1 \
                  THEN CAST(?2 AS INTEGER) ELSE 0 END) \
             ON CONFLICT (key) DO UPDATE SET \
             value = CAST(value AS INTEGER) + \
             CASE WHEN (SELECT COUNT(*) FROM versions \
                        WHERE checksum = ?3 AND verification != 'rejected') = 1 \
                  THEN CAST(?4 AS INTEGER) ELSE 0 END",
        )
        .bind(&[
            new.checksum_hex.into(),
            archive_size.clone(),
            new.checksum_hex.into(),
            archive_size,
        ])?,
    ])
    .await?;

    // Self-heal the head-skip race: a reclaim delete whose refcount was
    // read before this batch committed can land between the head above
    // and here, leaving the just-inserted row's blob missing. This
    // request still holds the bytes, so one more head buys the repair.
    if bucket.head(&key).await?.is_none() {
        bucket.put(&key, new.archive.to_vec()).execute().await?;
    }

    replicate_blob(env, ctx, &key, new.archive);
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
    ctx: &Context,
    db: &D1Database,
    new: &NewVersion<'_>,
    old_checksum: &str,
) -> worker::Result<bool> {
    let key = format!("blobs/sha256/{}", new.checksum_hex);
    let bucket = env.bucket("BLOBS")?;
    // Unconditional put, unlike persist_new_version's head-first skip:
    // when the replacement re-uses the rejected bytes, the rejecting
    // verdict's reclaim delete may still be in flight, and a head could
    // observe the object right before that delete lands - skipping the
    // upload would then leave a pending row whose blob is gone.
    // ponytail: a delete decided before this batch can still land after
    // this put (two stores, no shared transaction); that residual
    // window needs the same version's verdict and replacement in flight
    // simultaneously, fails loudly (the artifact route's missing-blob
    // 500), and the append-only BACKUP replica holds the bytes for
    // recovery.
    bucket.put(&key, new.archive.to_vec()).execute().await?;

    let archive_size = js_int(i64::try_from(new.archive.len()).unwrap_or(i64::MAX));
    let results = db
        .batch(vec![
            db.prepare(
                "UPDATE meta SET value = CAST(value AS INTEGER) + \
                 CASE WHEN (SELECT verification FROM versions \
                            WHERE name = ?1 AND version = ?2) = 'rejected' \
                      AND (SELECT checksum FROM versions \
                           WHERE name = ?1 AND version = ?2) = ?3 \
                      AND (SELECT COUNT(*) FROM versions \
                           WHERE checksum = ?4 AND verification != 'rejected') = 0 \
                      THEN CAST(?5 AS INTEGER) ELSE 0 END \
                 WHERE key = 'total_stored_bytes'",
            )
            .bind(&[
                new.name.into(),
                new.version.into(),
                old_checksum.into(),
                new.checksum_hex.into(),
                archive_size.clone(),
            ])?,
            db.prepare(
                "UPDATE versions SET checksum = ?1, metadata_json = ?2, yanked = 0, \
                 published_at = ?3, archive_size = ?4, published_by = ?5, \
                 verification = 'pending', verification_reason = NULL, verified_at = NULL \
                 WHERE name = ?6 AND version = ?7 \
                 AND verification = 'rejected' AND checksum = ?8",
            )
            .bind(&[
                new.checksum_hex.into(),
                new.metadata_text.into(),
                new.published_at.into(),
                archive_size,
                js_int(new.user_id),
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
        // unreferenced orphan (see docs/runbook.md).
        return Ok(false);
    }

    // Same self-heal as persist_new_version: repair the blob if a
    // reclaim delete landed between the put above and the batch commit.
    if bucket.head(&key).await?.is_none() {
        bucket.put(&key, new.archive.to_vec()).execute().await?;
    }

    delete_blob_if_unreferenced(env, db, old_checksum).await?;
    replicate_blob(env, ctx, &key, new.archive);
    Ok(true)
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
        .prepare(
            "SELECT COUNT(*) AS n FROM versions \
             WHERE checksum = ?1 AND verification != 'rejected'",
        )
        .bind(&[checksum.into()])?
        .first(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("empty COUNT(*) result".to_owned()))?;
    if references.n > 0 {
        return Ok(());
    }
    let key = format!("blobs/sha256/{checksum}");
    // No live reference also means nothing needs a backup copy any
    // more: clear the replication-failure bookkeeping first (before
    // the delete, so it can never linger past the primary object), or
    // a blob whose backup copy never landed would keep the breaker
    // alerting - and abort scripts/backup-backfill.sh against a
    // deleted primary object - forever.
    db.prepare("DELETE FROM backup_replication_failures WHERE key = ?1")
        .bind(&[key.as_str().into()])?
        .run()
        .await?;
    if let Err(err) = env.bucket("BLOBS")?.delete(&key).await {
        console_error!("reclaiming blob {key} failed (left as an orphan): {err}");
    }
    Ok(())
}

/// The idempotent no-op's self-heal: the retry holds the row's exact
/// bytes, so it repairs a primary blob a reclaim race deleted (the
/// head-first check keeps the common healthy case at one head), then
/// re-schedules the backup copy as usual.
async fn heal_blobs_on_retry(
    env: &Env,
    ctx: &Context,
    checksum_hex: &str,
    archive: &[u8],
) -> worker::Result<()> {
    let key = format!("blobs/sha256/{checksum_hex}");
    let bucket = env.bucket("BLOBS")?;
    if bucket.head(&key).await?.is_none() {
        bucket.put(&key, archive.to_vec()).execute().await?;
    }
    replicate_blob(env, ctx, &key, archive);
    Ok(())
}

/// Best-effort blob replication to the BACKUP bucket, scheduled off the
/// response path once the D1 batch has committed - and again on the
/// idempotent re-publish no-op, so a retry of a publish whose isolate
/// died before replicating heals the gap - which keeps every logged
/// failure referring to a referenced blob. Nothing ever deletes from BACKUP
/// here or anywhere else in the service - the primary's reclaim paths do
/// not propagate - so the backup is append-only. The head-first copy
/// also self-heals a blob a crashed earlier publish never replicated. A
/// failed copy is logged with its key and recorded in
/// `backup_replication_failures` for `scripts/backup-backfill.sh` to
/// re-run; the breaker cron alerts while such rows exist.
fn replicate_blob(env: &Env, ctx: &Context, key: &str, archive: &[u8]) {
    let (Ok(backup), Ok(db)) = (env.bucket("BACKUP"), env.d1("DB")) else {
        console_error!("backup replication for {key}: BACKUP or DB binding is missing");
        return;
    };
    let key = key.to_owned();
    let archive = archive.to_vec();
    ctx.wait_until(async move {
        let outcome = match backup.head(&key).await {
            Ok(Some(_)) => Ok(()),
            Ok(None) => backup.put(&key, archive).execute().await.map(|_| ()),
            Err(err) => Err(err),
        };
        let bookkeeping = match outcome {
            Ok(()) => db
                .prepare("DELETE FROM backup_replication_failures WHERE key = ?1")
                .bind(&[key.clone().into()]),
            Err(err) => {
                console_error!(
                    "backup replication failed for {key}: {err}; \
                     recorded for scripts/backup-backfill.sh"
                );
                db.prepare(
                    "INSERT INTO backup_replication_failures (key, failed_at) \
                     VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET \
                     failed_at = excluded.failed_at",
                )
                .bind(&[key.clone().into(), now_iso8601().into()])
            }
        };
        match bookkeeping {
            Ok(statement) => {
                if statement.run().await.is_err() {
                    console_error!("backup replication bookkeeping for {key} failed");
                }
            }
            Err(_) => {
                console_error!("backup replication bookkeeping for {key} could not be prepared");
            }
        }
    });
}

/// What the publish handler found for an already-existing `(name,
/// version)` row.
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
/// for an already-published `(name, version)`. `None` means the version
/// is new.
async fn existing_version(
    db: &D1Database,
    name: &str,
    version: &str,
    metadata_text: &str,
) -> worker::Result<Option<ExistingVersion>> {
    let existing: Option<StoredVersionRecord> = db
        .prepare(
            "SELECT metadata_json, checksum, verification FROM versions \
             WHERE name = ?1 AND version = ?2",
        )
        .bind(&[name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    let Some(status) = verify::Status::parse(&existing.verification) else {
        // An invariant break (the schema never writes other values);
        // fail safe by refusing rather than guessing a transition.
        console_error!(
            "stored verification for {name}@{version} is invalid: {}",
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
    quotas: &quota::PlanQuotas,
) -> worker::Result<Option<Response>> {
    // Enough attempts to drain a full burst even when every one of them
    // loses a race to a parallel publisher on the same token.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small plan constant
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
        .prepare("SELECT rl_tokens, rl_updated_at FROM tokens WHERE id = ?1")
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
        .prepare(
            "UPDATE tokens SET rl_tokens = ?1, rl_updated_at = ?2 \
             WHERE id = ?3 AND rl_tokens IS ?4 AND rl_updated_at IS ?5",
        )
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
/// single D1 batch; every statement is a point lookup or an aggregate
/// over an indexed column.
async fn publish_counts(
    db: &D1Database,
    user_id: i64,
    name: &str,
    day_prefix: &str,
) -> worker::Result<quota::PublishCounts> {
    let results = db
        .batch(vec![
            // Rejected versions are excluded: their bytes were refunded
            // when the verdict landed.
            db.prepare(
                "SELECT COALESCE(SUM(archive_size), 0) AS stored_bytes \
                 FROM versions WHERE published_by = ?1 AND verification != 'rejected'",
            )
            .bind(&[js_int(user_id)])?,
            // Both package quotas key on creation (`created_by`), so a
            // version published into someone else's package never counts
            // against the publisher's package quotas.
            db.prepare(
                "SELECT COUNT(*) AS package_count, \
                 COALESCE(SUM(created_at >= ?2), 0) AS new_today \
                 FROM packages WHERE created_by = ?1",
            )
            .bind(&[js_int(user_id), day_prefix.into()])?,
            db.prepare("SELECT COUNT(*) AS n FROM versions WHERE name = ?1 AND published_at >= ?2")
                .bind(&[name.into(), day_prefix.into()])?,
            db.prepare("SELECT COUNT(*) AS n FROM packages WHERE name = ?1")
                .bind(&[name.into()])?,
        ])
        .await?;
    let user_usage: UserUsageRecord = first_row(&results, 0)?;
    let user_packages: PackageCountsRecord = first_row(&results, 1)?;
    let versions_today: CountRecord = first_row(&results, 2)?;
    let package_rows: CountRecord = first_row(&results, 3)?;
    Ok(quota::PublishCounts {
        user_stored_bytes: non_negative(user_usage.stored_bytes),
        user_package_count: non_negative(user_packages.package_count),
        user_new_packages_today: non_negative(user_packages.new_today),
        package_versions_today: non_negative(versions_today.n),
        package_exists: package_rows.n > 0,
    })
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

/// The service mode for the write routes, cached in isolate memory for
/// ~60 s (one cheap D1 point read on expiry; the `SERVICE_MODE_TTL_SECS`
/// env var overrides the TTL, and dev pins it to 0 so the smoke test can
/// flip modes without waiting it out). Fail closed: a missing or unknown
/// `meta.service_mode` is `WritesBlocked`, and a D1 failure propagates
/// into the caller's 500. Reads never call this - they fail open by
/// construction.
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

/// `Some(402)` when the budget breaker has writes blocked
/// (`docs/architecture.md`, "Billing model and the budget breaker").
async fn write_gate(env: &Env, db: &D1Database) -> worker::Result<Option<Response>> {
    if service_mode(env, db).await? == breaker::Mode::WritesBlocked {
        return Ok(Some(error_response_with_code(
            402,
            breaker::OVER_BUDGET_DETAIL,
            breaker::OVER_BUDGET_CODE,
            Some(breaker::OVER_BUDGET_RETRY_AFTER_SECS),
        )?));
    }
    Ok(None)
}

/// Reads one `meta` row.
pub(crate) async fn read_meta(db: &D1Database, key: &str) -> worker::Result<Option<String>> {
    let record: Option<MetaRecord> = db
        .prepare("SELECT value FROM meta WHERE key = ?1")
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
    db.prepare(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(&[key.into(), value.into()])
}

/// The budget-breaker schedule (`wrangler.jsonc` `triggers`); the cron
/// entry point routes on this exact expression.
const BREAKER_CRON: &str = "*/15 * * * *";

/// The cron entry point. The breaker's [`BREAKER_CRON`] runs the budget
/// evaluation (every 15 minutes: gather usage, evaluate it against the
/// budgets, persist the resulting service mode - failed analytics
/// queries leave their metric unset, which can escalate but never
/// unblock writes, [`breaker::next_mode`]). Any other trigger - the
/// nightly `0 3 * * *`, or a temporary schedule added for an ops
/// rehearsal - runs the D1 dump job, so exercising the backup path
/// never needs a recompile.
#[event(scheduled)]
pub async fn scheduled(event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    if event.cron() == BREAKER_CRON {
        if let Err(err) = evaluate_budgets(&env).await {
            console_error!("budget evaluation failed; keeping the last service mode: {err}");
        }
    } else if let Err(err) = crate::backup_glue::run_nightly_dump(&env).await {
        console_error!("nightly backup failed: {err}");
    }
}

/// One usage snapshot: the exact self-accounted storage plus the
/// analytics-sourced metrics.
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

    Ok(breaker::Usage {
        stored_bytes,
        workers_requests_today,
        r2_class_a_month,
        d1_rows_read_today,
    })
}

async fn evaluate_budgets(env: &Env) -> worker::Result<()> {
    let db = env.d1("DB")?;
    let now = now_iso8601();

    let usage = gather_usage(env, &db, &now).await?;
    let defaults = breaker::Budgets::default();
    let budgets = breaker::Budgets {
        r2_storage_bytes: env_budget(env, "BUDGET_R2_STORAGE_BYTES", defaults.r2_storage_bytes),
        r2_class_a_month: env_budget(env, "BUDGET_R2_CLASS_A_MONTH", defaults.r2_class_a_month),
        workers_requests_day: env_budget(
            env,
            "BUDGET_WORKERS_REQ_DAY",
            defaults.workers_requests_day,
        ),
        d1_rows_read_day: env_budget(env, "BUDGET_D1_ROWS_READ_DAY", defaults.d1_rows_read_day),
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
    let next = breaker::next_mode(current, candidate, usage.complete());
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
        .prepare(
            "SELECT COUNT(*) AS n FROM versions WHERE verification = 'pending' \
             AND published_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 hour')",
        )
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
    replication_failures: Option<u64>,
    alert: Option<String>,
}

impl BackupHealth {
    /// Fail closed when D1 would not answer: alert rather than report
    /// an unknown state as healthy.
    fn unreadable() -> BackupHealth {
        BackupHealth {
            last_backup_at: None,
            freshness: backup::Freshness::Never,
            replication_failures: None,
            alert: Some("backup health could not be read from d1".to_owned()),
        }
    }
}

async fn read_backup_health(db: &D1Database, now: &str) -> worker::Result<BackupHealth> {
    let last_backup_at = read_meta(db, "last_backup_at").await?;
    let failures: CountRecord = db
        .prepare("SELECT COUNT(*) AS n FROM backup_replication_failures")
        .first(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("empty COUNT(*) result".to_owned()))?;
    let replication_failures = non_negative(failures.n);
    let freshness = backup::freshness(now, last_backup_at.as_deref());
    Ok(BackupHealth {
        last_backup_at,
        freshness,
        replication_failures: Some(replication_failures),
        alert: backup::alert(freshness, replication_failures),
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
        "backup": {
            "last_backup_at": health.last_backup_at,
            "freshness": health.freshness.as_str(),
            "replication_failures": health.replication_failures,
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
/// as a float; everything bound this way (GitHub ids, byte counts, row
/// counts) sits far below 2^53, where f64 is exact.
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
