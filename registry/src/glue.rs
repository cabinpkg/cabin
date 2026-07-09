//! Cloudflare-specific glue: binding access, D1/R2 I/O, and response
//! plumbing. Everything with behavior worth testing lives in the host-target
//! modules; keep this file thin.

use serde::Deserialize;
use worker::{
    Context, D1Database, Env, Method, Request, Response, console_error, console_log, event,
};

use crate::auth::{self, AuthContext};
use crate::documents::{self, VersionRow};
use crate::error;
use crate::routes::{Route, match_route};

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

#[event(fetch)]
pub async fn fetch(req: Request, env: Env, ctx: Context) -> worker::Result<Response> {
    let request_id = request_id(&req);
    let method = req.method();
    let path = req.path();

    let (response, token_id) = match handle(&req, &env, &ctx).await {
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
    req: &Request,
    env: &Env,
    ctx: &Context,
) -> worker::Result<(Response, Option<String>)> {
    let path = req.path();

    // The only unauthenticated route; 200 with no body.
    if path == "/healthz" {
        return Ok((Response::empty()?, None));
    }

    // Deny by default: the uniform 401 is emitted before any route matching
    // or D1/R2 data lookup, so non-callers cannot probe package existence.
    let db = env.d1("DB")?;
    let Some(auth) = authenticate(req, &db, ctx).await? else {
        return Ok((error_response(401, error::AUTH_REQUIRED)?, None));
    };

    let mut response = if req.method() == Method::Get {
        match match_route(&path) {
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

fn error_response(status: u16, detail: &str) -> worker::Result<Response> {
    let mut response = Response::ok(error::envelope(detail))?.with_status(status);
    response
        .headers_mut()
        .set("content-type", "application/json")?;
    Ok(response)
}

fn now_iso8601() -> String {
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
