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
use crate::{analytics, backup, breaker, quota};

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
struct ChecksumRecord {
    checksum: String,
}

#[derive(Deserialize)]
struct MetaRecord {
    value: String,
}

#[derive(Deserialize)]
struct StoredMetadataRecord {
    metadata_json: String,
}

#[derive(Deserialize)]
struct YankedRecord {
    yanked: i64,
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

/// Routes one request. Returns the response plus the authenticated token row
/// id for logging.
async fn handle(
    req: &mut Request,
    env: &Env,
    ctx: &Context,
) -> worker::Result<(Response, Option<String>)> {
    let path = req.path();

    // The only unauthenticated route; 200 with no body.
    if path == "/healthz" {
        return Ok((Response::empty()?, None));
    }

    // The browser plane: GitHub sign-in and the /me token page use
    // session-cookie auth (`crate::web_glue`) and never bearer tokens; the
    // data routes below never accept the session cookie. Web paths serve
    // no package data, so dispatching them before the bearer gate keeps
    // the data plane's uniform 401 intact.
    if let Some(web_route) = match_web_route(&path) {
        return Ok((web_glue::respond(req, env, web_route).await?, None));
    }

    // Deny by default: the uniform 401 is emitted before any route matching
    // or D1/R2 data lookup, so non-callers cannot probe package existence.
    let db = env.d1("DB")?;
    let Some(auth) = authenticate(req, &db, ctx).await? else {
        return Ok((error_response(401, error::AUTH_REQUIRED)?, None));
    };

    let mut response = match req.method() {
        Method::Get => match match_route(&path) {
            Some(Route::Config) => json_response(&documents::config_json(
                &env.var("REGISTRY_ORIGIN")?.to_string(),
            ))?,
            Some(Route::Package { name }) => package_response(&db, name).await?,
            Some(Route::Artifact { name, version }) => {
                artifact_response(env, &db, name, version).await?
            }
            // `/healthz` was handled above; anything else is an
            // authenticated 404.
            Some(Route::Healthz) | None => error_response(404, error::NOT_FOUND)?,
        },
        Method::Put => match match_api_route(&path) {
            Some(ApiRoute::Publish { name, version }) => {
                let (name, version) = (name.to_owned(), version.to_owned());
                publish_response(req, env, ctx, &db, &auth, &name, &version).await?
            }
            Some(ApiRoute::Yank { .. }) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        Method::Patch => match match_api_route(&path) {
            Some(ApiRoute::Yank { name, version }) => {
                let (name, version) = (name.to_owned(), version.to_owned());
                yank_response(req, env, &db, &auth, &name, &version).await?
            }
            Some(ApiRoute::Publish { .. }) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        _ => error_response(405, error::METHOD_NOT_ALLOWED)?,
    };

    // Debug aid for the disposable dev environment (see docs/runbook.md):
    // stamp every authenticated response with the registry generation.
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

async fn package_response(db: &D1Database, name: &str) -> worker::Result<Response> {
    let records: Vec<VersionRecord> = db
        .prepare("SELECT version, metadata_json, yanked FROM versions WHERE name = ?1")
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
/// versions only - by the archive-size cap (`413`) and the per-user
/// quota checks (`403`); on success the archive lands in R2 first (an
/// orphaned blob from a crash between the two writes is harmless,
/// content-addressed garbage - see `docs/runbook.md`), then one atomic
/// D1 batch inserts the package and version rows and bumps the storage
/// self-accounting.
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
    if let Some(limited) = publish_rate_limit(db, auth, &quotas).await? {
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

    if let Some(response) = existing_version_response(db, name, version, metadata_text).await? {
        // The idempotent no-op (200) is a retry of a committed publish;
        // if the original isolate died before its replication ran (and
        // before any failure row was recorded), this is the retry's one
        // chance to heal the backup copy - head-first, so the common
        // already-replicated case costs a single head. The 409 arm gets
        // no copy: its uploaded bytes were rejected.
        if response.status_code() == 200 {
            let key = format!("blobs/sha256/{computed_hex}");
            replicate_blob(env, ctx, &key, frame.archive);
        }
        return Ok(response);
    }

    // The archive-size cap and the per-user quotas gate genuinely new
    // versions only: a byte-identical re-publish of an already-stored
    // archive (even one grandfathered above the current cap) stays the
    // idempotent no-op above and never consumes quota.
    if let Err(denial) = quota::check_archive_size(archive_bytes, &quotas) {
        return denial_response(&denial, None);
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
        return denial_response(&denial, None);
    }

    persist_new_version(
        env,
        ctx,
        db,
        &NewVersion {
            name,
            version,
            checksum_hex: &computed_hex,
            metadata_text,
            published_at: &now,
            archive: frame.archive,
            user_id: auth.user_id,
        },
    )
    .await?;

    json_response_with_status(
        201,
        &serde_json::json!({
            "ok": true,
            "name": name,
            "version": version,
            "checksum": metadata.checksum,
        })
        .to_string(),
    )
}

/// `PATCH /api/v1/packages/<name>/<version>/yank`
/// (`docs/remote-registry.md`, "Yank"): idempotent, and the row's
/// `yanked` column is the single home of yank state - the read path
/// overrides the stored metadata's field from it. Gated by the budget
/// breaker (`402`) like every write; yank has no rate limit or quota.
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
        .prepare("SELECT yanked FROM versions WHERE name = ?1 AND version = ?2")
        .bind(&[name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(existing) = existing else {
        return error_response(404, error::NOT_FOUND);
    };
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
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    let record: Option<ChecksumRecord> = db
        .prepare("SELECT checksum FROM versions WHERE name = ?1 AND version = ?2")
        .bind(&[name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(record) = record else {
        return error_response(404, error::NOT_FOUND);
    };

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
/// version rows plus the storage self-accounting.
///
/// The accounting decision lives inside the batch (one transaction): the
/// meta bump counts the archive only when the row just inserted is the
/// checksum's sole reference. That way the crash-retry path - blob
/// already uploaded but never counted - still accounts for it, a second
/// name sharing the blob never double-counts it, and two concurrent
/// first publishes of the same archive serialize on the transaction so
/// exactly one of them counts it. Once the batch commits, the blob is
/// replicated to the BACKUP bucket off the response path
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
             published_at, archive_size, published_by) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)",
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
             CASE WHEN (SELECT COUNT(*) FROM versions WHERE checksum = ?1) = 1 \
                  THEN CAST(?2 AS INTEGER) ELSE 0 END) \
             ON CONFLICT (key) DO UPDATE SET \
             value = CAST(value AS INTEGER) + \
             CASE WHEN (SELECT COUNT(*) FROM versions WHERE checksum = ?3) = 1 \
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

    replicate_blob(env, ctx, &key, new.archive);
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

/// Idempotency and immutability for an already-published `(name,
/// version)`: byte-identical metadata means a byte-identical archive too
/// (the metadata embeds the checksum and both uploads passed the digest
/// check), so a re-publish is a `200` no-op that never touches R2;
/// anything else hits the `409` immutability wall. `None` means the
/// version is new.
async fn existing_version_response(
    db: &D1Database,
    name: &str,
    version: &str,
    metadata_text: &str,
) -> worker::Result<Option<Response>> {
    let existing: Option<StoredMetadataRecord> = db
        .prepare("SELECT metadata_json FROM versions WHERE name = ?1 AND version = ?2")
        .bind(&[name.into(), version.into()])?
        .first(None)
        .await?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    if existing.metadata_json == metadata_text {
        return json_response_with_status(
            200,
            &serde_json::json!({ "ok": true, "no_op": true }).to_string(),
        )
        .map(Some);
    }
    error_response(409, error::VERSION_IMMUTABLE).map(Some)
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
    denial_response(&quota::RATE_LIMITED, Some(1)).map(Some)
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
            db.prepare(
                "SELECT COALESCE(SUM(archive_size), 0) AS stored_bytes \
                 FROM versions WHERE published_by = ?1",
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
    if next != current {
        console_log!(
            "service_mode {} -> {}: {reason}",
            current.as_str(),
            next.as_str()
        );
    }
    if next != current || health.alert.is_some() {
        notify_webhook(env, current, next, &reason, &usage, &health).await;
    }
    Ok(())
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
/// service-mode changes (`from != to`) and backup alerts alike;
/// failures only log.
async fn notify_webhook(
    env: &Env,
    from: breaker::Mode,
    to: breaker::Mode,
    reason: &str,
    usage: &breaker::Usage,
    health: &BackupHealth,
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

/// Renders a quota or rate-limit [`quota::Denial`].
fn denial_response(
    denial: &quota::Denial,
    retry_after_secs: Option<u64>,
) -> worker::Result<Response> {
    error_response_with_code(denial.status, denial.detail, denial.code, retry_after_secs)
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
