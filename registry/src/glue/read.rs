//! The registry domain's machine read plane: the public read routes,
//! the edge blob cache, and download counting.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::Deserialize;
use worker::{Context, D1Database, Delay, Env, Method, Request, Response, console_error};

use crate::auth::AuthContext;
use crate::documents::{self, VersionRow};
use crate::error;
use crate::governor::{Consume, Decision, OpPool};
use crate::governor_client::{self, Gate};
use crate::routes::{Route, match_route};
use crate::{breaker, quota, sql, telemetry, verify};

use super::{
    GENERATION_HEADER, authenticate, error_response, error_response_with_code,
    governor_refusal_response, has_verify_scope, js_int, json_response, now_epoch_ms,
    registry_generation, service_mode, unauthorized, web_origin,
};

#[derive(Deserialize)]
struct VersionRecord {
    version: String,
    revision: String,
    metadata_json: String,
    yanked: i64,
}

#[derive(Deserialize)]
struct RevisionListRecord {
    version: String,
    revision: String,
    checksum: String,
    published_at: String,
}

#[derive(Deserialize)]
struct ArtifactRecord {
    checksum: String,
    verification: String,
}

/// The registry custom domain: only the machine read plane exists here,
/// and it is public - unauthenticated GETs read verified content.
/// Every other path - including all of `/api/*` - answers the uniform
/// 401 without consulting the `Authorization` header at all, so a
/// misdirected credential or a probe of the mutation routes is
/// indistinguishable from any unknown path.
pub(super) async fn handle_registry(
    req: &Request,
    env: &Env,
    ctx: &Context,
    path: &str,
) -> worker::Result<(Response, Option<String>)> {
    // The only bodyless health route; 200 with no body.
    if path == "/healthz" {
        return Ok((Response::empty()?, None));
    }
    let Some(route) = match_route(path) else {
        return Ok((unauthorized(env)?, None));
    };

    let db = env.d1("DB")?;
    // The read plane is public, so its method discipline is public
    // too: a non-`GET` answers 405 whatever credential rides along,
    // before the header is read at all. Deciding it on the presented
    // credential instead would make the refusal a token-validity
    // oracle on a route that exists for everyone.
    if req.method() != Method::Get {
        let mut response = error_response(405, error::METHOD_NOT_ALLOWED)?;
        if let Some(generation) = registry_generation(&db).await {
            response.headers_mut().set(GENERATION_HEADER, &generation)?;
        }
        return Ok((response, None));
    }

    // Public verified reads (`docs/architecture.md`, "Origins and
    // roles"): a request with no `Authorization` header is an anonymous
    // reader. A presented credential is still a claim - one that fails
    // to validate answers the uniform 401 rather than silently
    // degrading to anonymous, so the verifier's pending fetches fail
    // loudly on a rotated token instead of reading as missing rows.
    let auth = if req.headers().get("authorization")?.is_some() {
        match authenticate(req, &db, ctx).await? {
            Some(auth) => Some(auth),
            None => return Ok((unauthorized(env)?, None)),
        }
    } else {
        None
    };

    // The read-side budget gate (`docs/architecture.md`, "Billing
    // model and the budget breaker"). Anonymous readers receive the
    // same refusal as authenticated ones - a public over-budget
    // answer necessarily reveals service state, which is inherent
    // to public reads (the recorded revision in "Origins and
    // roles"). Fail-open on the mode lookup (`.ok()`): only an
    // affirmatively read `reads_blocked` refuses, so downloads keep
    // working through an outage of the breaker itself. The
    // verifier's fetches - the config it discovers the api origin
    // from and the artifacts it inspects, never the package
    // documents - are exempt: it must be able to drain the pending
    // queue while reads are blocked, and its spend is negligible.
    let verify_exempt = auth.as_ref().is_some_and(has_verify_scope)
        && matches!(route, Route::Config | Route::Artifact { .. });
    let mode = service_mode(env, &db).await.ok();
    let mut response = if breaker::read_gate_refuses(mode, verify_exempt) {
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
                revision,
            } => {
                // Cloudflare stamps `CF-Connecting-IP` on every edge
                // request (it overwrites any client-supplied value),
                // so it is the one caller identity an anonymous
                // reader has - see `quota::artifact_read_fairness`.
                let client_ip = req.headers().get("cf-connecting-ip")?;
                artifact_response(
                    env,
                    &db,
                    ctx,
                    auth.as_ref(),
                    client_ip.as_deref(),
                    scope,
                    name,
                    version,
                    revision,
                )
                .await?
            }
            // Answered above before the credential check.
            Route::Healthz => Response::empty()?,
        }
    };

    // Debug aid for the pre-launch registry (see docs/runbook.md):
    // stamp every read-plane response with the registry generation.
    if let Some(generation) = registry_generation(&db).await {
        response.headers_mut().set(GENERATION_HEADER, &generation)?;
    }
    Ok((response, auth.map(|auth| auth.token_id)))
}

/// `GET /packages/<scope>/<name>.json`: composed from **verified**
/// revisions only - each version's entry carries its current
/// revision's metadata plus the full verified `revisions` map, so
/// pending and rejected rows never reach composition (a pending respin
/// leaves the served revision untouched) and a package with no
/// verified revisions is indistinguishable from an unknown one (fail
/// safe: if the verifier never runs, nothing new ever becomes
/// resolvable).
async fn package_response(db: &D1Database, scope: &str, name: &str) -> worker::Result<Response> {
    let records: Vec<VersionRecord> = db
        .prepare(sql::CURRENT_REVISIONS_BY_PACKAGE)
        .bind(&[scope.into(), name.into()])?
        .all()
        .await?
        .results()?;
    if records.is_empty() {
        return error_response(404, error::NOT_FOUND);
    }
    let revision_records: Vec<RevisionListRecord> = db
        .prepare(sql::VERIFIED_REVISIONS_BY_PACKAGE)
        .bind(&[scope.into(), name.into()])?
        .all()
        .await?
        .results()?;
    let rows: Vec<VersionRow> = records
        .into_iter()
        .map(|record| VersionRow {
            version: record.version,
            revision: record.revision,
            metadata_json: record.metadata_json,
            yanked: record.yanked != 0,
        })
        .collect();
    let revisions: Vec<documents::RevisionRow> = revision_records
        .into_iter()
        .map(|record| documents::RevisionRow {
            version: record.version,
            revision: record.revision,
            checksum: record.checksum,
            published_at: record.published_at,
        })
        .collect();
    match documents::package_json(scope, name, &rows, &revisions) {
        Ok(body) => json_response(&body),
        Err(detail) => {
            console_error!("package document for {scope}/{name}: {detail}");
            error_response(500, error::INTERNAL)
        }
    }
}

/// The synthetic edge-cache identity for immutable verified archives:
/// derived from the content checksum only, never from the outward URL
/// or query string, so no request input can alias or bust an entry.
/// The path exists on no route (the registry host answers its uniform
/// 401 there), and the Worker runs on every request to its hostnames,
/// so the entry is reachable only through this handler - after Bearer
/// auth and the D1 verified-version gate.
fn blob_cache_url(checksum: &str) -> String {
    format!(
        "https://registry.cabinpkg.com/__cache/blobs/sha256/{}",
        crate::checksum::hex(checksum)
    )
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

#[allow(clippy::too_many_arguments)] // the revision quad plus the request plumbing
async fn artifact_response(
    env: &Env,
    db: &D1Database,
    ctx: &Context,
    auth: Option<&AuthContext>,
    client_ip: Option<&str>,
    scope: &str,
    name: &str,
    version: &str,
    revision: &str,
) -> worker::Result<Response> {
    let record: Option<ArtifactRecord> = db
        .prepare(sql::ARTIFACT_BY_REVISION)
        .bind(&[scope.into(), name.into(), version.into(), revision.into()])?
        .first(None)
        .await?;
    let Some(record) = record else {
        return error_response(404, error::NOT_FOUND);
    };
    // Verified versions download for everyone; pending ones only with
    // the `verify` scope (the verifier fetches the bytes it inspects);
    // rejected ones - whose blob is reclaimed - and rows with an
    // unreadable status gate like missing rows.
    let status = verify::Status::parse(&record.verification);
    let readable = status.is_some_and(|status| {
        verify::artifact_readable(status, auth.is_some_and(has_verify_scope))
    });
    if !readable {
        return error_response(404, error::NOT_FOUND);
    }

    // Archives are immutable and content-addressed; yanked versions stay
    // downloadable on purpose (docs/remote-registry.md, "Yank").
    let key = format!("blobs/sha256/{}", crate::checksum::hex(&record.checksum));
    if status == Some(verify::Status::Verified) {
        let response = verified_artifact_response(
            env,
            auth,
            client_ip,
            &key,
            &record.checksum,
            scope,
            name,
            version,
        )
        .await?;
        if response.status_code() == 200 {
            count_download(env, ctx, scope, name, version);
        }
        return Ok(response);
    }

    // Pending is readable only with the `verify` scope, so a credential
    // is present here by construction; gate like a missing row if not.
    let Some(auth) = auth else {
        return error_response(404, error::NOT_FOUND);
    };

    // The verifier's pending fetch: never cached (the bytes are not yet
    // part of the registry), charged to the isolated verifier pool so
    // ordinary traffic can never starve verification - and vice versa.
    // The per-user cap rides along: every verify-scoped credential -
    // the operator's session, the trustpub verify arm - resolves to the
    // operator's own account today, and one account must not be able
    // to drain the whole verifier pool either.
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
    // the outward answer must not (it would let the edge re-serve the
    // body without the Worker, dropping the download count).
    response.headers_mut().set("cache-control", "no-store")?;
    Ok(response)
}

#[allow(clippy::too_many_arguments)] // the caller identity plus the artifact triple
async fn verified_artifact_response(
    env: &Env,
    auth: Option<&AuthContext>,
    client_ip: Option<&str>,
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
    let result =
        charged_blob_read(env, auth, client_ip, key, &cache_url, scope, name, version).await;
    if owns_marker {
        INFLIGHT_BLOB_READS.with(|set| set.borrow_mut().remove(checksum));
    }
    result
}

/// The cache-miss path: charge one ordinary Class B read immediately
/// before the R2 `get`, then serve and fill the edge cache. Every
/// miss lands in a per-caller daily fairness window - the token's
/// user for a tokened caller, the edge client IP for an anonymous
/// one ([`quota::artifact_read_fairness`]) - so no single caller can
/// drain the shared pool and turn everyone else's uncached downloads
/// into `503`s (`docs/architecture.md`, "The cost governor").
#[allow(clippy::too_many_arguments)] // the caller identity plus the artifact triple
async fn charged_blob_read(
    env: &Env,
    auth: Option<&AuthContext>,
    client_ip: Option<&str>,
    key: &str,
    cache_url: &str,
    scope: &str,
    name: &str,
    version: &str,
) -> worker::Result<Response> {
    let quotas = quota::quotas_for_class(
        auth.map_or(quota::DEFAULT_CLASS_NAME, |auth| auth.quota_class.as_str()),
    );
    let (principal, principal_cap) =
        quota::artifact_read_fairness(auth.map(|auth| auth.user_id), &quotas, client_ip);
    let decision = Decision {
        consume: vec![Consume {
            pool: OpPool::BOrdinary,
            n: 1,
            principal: Some(principal),
            principal_cap: Some(principal_cap),
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
    // The freshness directives go on the internal cache copy ONLY. An
    // outward `public` would license Cloudflare's own edge layer (and
    // any shared cache) to re-serve the body without running the
    // Worker, which would stop counting downloads and bypass the
    // verified-version gate; the Worker-internal Cache API copy is the
    // one caching layer that keeps both (`docs/architecture.md`,
    // "Download counts").
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
