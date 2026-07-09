//! Cloudflare-specific glue: binding access, D1/R2 I/O, and response
//! plumbing. Everything with behavior worth testing lives in the host-target
//! modules; keep this file thin.

use std::fmt::Write as _;

use serde::Deserialize;
use worker::{
    Context, D1Database, Env, Method, Request, Response, console_error, console_log, event,
};

use crate::auth::{self, AuthContext, Scope};
use crate::documents::{self, VersionRow};
use crate::error;
use crate::publish;
use crate::routes::{ApiRoute, Route, match_api_route, match_route, match_web_route};
use crate::web_glue;

const GENERATION_HEADER: &str = "x-cabin-registry-generation";

#[derive(Deserialize)]
struct TokenRecord {
    id: String,
    user_id: i64,
    scopes: String,
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
                publish_response(req, env, &db, &auth, &name, &version).await?
            }
            Some(ApiRoute::Yank { .. }) => error_response(405, error::METHOD_NOT_ALLOWED)?,
            None => error_response(404, error::NOT_FOUND)?,
        },
        Method::Patch => match match_api_route(&path) {
            Some(ApiRoute::Yank { name, version }) => {
                let (name, version) = (name.to_owned(), version.to_owned());
                yank_response(req, &db, &auth, &name, &version).await?
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
            "SELECT id, user_id, scopes FROM tokens WHERE token_hash = ?1 AND revoked_at IS NULL",
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

    Ok(Some(AuthContext {
        token_id: record.id,
        user_id: record.user_id,
        scopes: auth::parse_scopes(&record.scopes),
    }))
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
/// mapping follow `crate::publish`; on success the archive lands in R2
/// first (an orphaned blob from a crash between the two writes is
/// harmless, content-addressed garbage - see `docs/runbook.md`), then
/// one atomic D1 batch inserts the package and version rows.
async fn publish_response(
    req: &mut Request,
    env: &Env,
    db: &D1Database,
    auth: &AuthContext,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    if !auth.scopes.contains(&Scope::Publish) {
        return error_response(403, error::PUBLISH_SCOPE_REQUIRED);
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

    // Idempotency and immutability: byte-identical metadata means a
    // byte-identical archive too (the metadata embeds the checksum and
    // both uploads passed the digest check), so a re-publish is a no-op
    // that never touches R2; anything else hits the immutability wall.
    let existing: Option<StoredMetadataRecord> = db
        .prepare("SELECT metadata_json FROM versions WHERE name = ?1 AND version = ?2")
        .bind(&[name.into(), version.into()])?
        .first(None)
        .await?;
    if let Some(existing) = existing {
        if existing.metadata_json == metadata_text {
            return json_response_with_status(
                200,
                &serde_json::json!({ "ok": true, "no_op": true }).to_string(),
            );
        }
        return error_response(409, error::VERSION_IMMUTABLE);
    }

    // R2 before D1, skipping the upload when the content-addressed blob
    // is already there (e.g. the same archive published under a name it
    // was yanked from, or a retry after a crash between the two writes).
    let key = format!("blobs/sha256/{computed_hex}");
    let bucket = env.bucket("BLOBS")?;
    if bucket.head(&key).await?.is_none() {
        bucket.put(&key, frame.archive.to_vec()).execute().await?;
    }

    let now = now_iso8601();
    db.batch(vec![
        db.prepare("INSERT OR IGNORE INTO packages (name, created_at) VALUES (?1, ?2)")
            .bind(&[name.into(), now.clone().into()])?,
        db.prepare(
            "INSERT INTO versions (name, version, checksum, metadata_json, yanked, published_at) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5)",
        )
        .bind(&[
            name.into(),
            version.into(),
            computed_hex.clone().into(),
            metadata_text.into(),
            now.into(),
        ])?,
    ])
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
/// overrides the stored metadata's field from it.
async fn yank_response(
    req: &mut Request,
    db: &D1Database,
    auth: &AuthContext,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
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
/// would burn CPU budget instead.
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
