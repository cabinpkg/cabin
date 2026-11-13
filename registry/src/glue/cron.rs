//! The breaker cron's budget evaluation, upload lifecycle pass, and
//! website catalog refresh.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use worker::{D1Database, Env, Fetch, Method, Request, RequestInit, console_error, console_log};

use crate::catalog_refresh::{self, Target};
use crate::governor::OpPool;
use crate::governor_client::{self, Gate};
use crate::routes::Route;
use crate::{analytics, auth, breaker, publication, sql, upload, verify};

use super::{
    CountRecord, changed_rows, consume_one, iso8601_from_ms, non_negative, now_iso8601, post_json,
    read_meta, upsert_meta, web_origin,
};

/// One summary line of the governor's operation windows per pass, next
/// to the analytics-based evaluation.
pub(super) async fn log_governor_usage(env: &Env) {
    let Some(snapshot) = governor_client::usage(env).await else {
        return;
    };
    let ops: Vec<String> = snapshot
        .ops
        .iter()
        .map(|row| format!("{}[{}]={}", row.pool, row.window, row.used))
        .collect();
    console_log!(
        "governor usage: ops {}",
        if ops.is_empty() {
            "-".to_owned()
        } else {
            ops.join(" ")
        },
    );
}

/// The upload lifecycle pass. Failures only log; the next pass retries.
pub(super) async fn settle_uploads(env: &Env, db: &D1Database) {
    let now = now_iso8601();
    // The sweeps that need only D1 run first, so an R2 or governor
    // outage in the reservation pass cannot hold them back.
    if let Err(err) = sweep_uploads(db, &now).await {
        console_error!("upload sweep: {err}");
    }
    if let Err(err) = settle_reservations(env, db, &now).await {
        console_error!("reservation pass: {err}");
    }
}

/// Times out the uploads whose objects are about to expire and prunes
/// what the admission and the publish pass no longer need.
async fn sweep_uploads(db: &D1Database, now: &str) -> worker::Result<()> {
    let timed_out = [now.into(), upload::PENDING_TIMEOUT.into()];
    db.batch(vec![
        db.prepare(sql::REFUND_TIMED_OUT_UPLOADS).bind(&timed_out)?,
        db.prepare(sql::TIME_OUT_PENDING_UPLOADS).bind(&timed_out)?,
        db.prepare(sql::PRUNE_USER_DAILY_UPLOADS)
            .bind(&[now.into()])?,
        db.prepare(sql::PRUNE_SHARED_DAILY_UPLOADS)
            .bind(&[now.into()])?,
        db.prepare(sql::PRUNE_SETTLED_UPLOAD_IDS)
            .bind(&[now.into(), upload::UPLOAD_ID_RETENTION.into()])?,
        db.prepare(sql::PRUNE_COMPLETED_PUBLICATION_JOBS)
            .bind(&[now.into(), publication::JOB_RETENTION.into()])?,
    ])
    .await?;
    Ok(())
}

#[derive(Deserialize)]
struct ReservationRecord {
    scope: String,
    name: String,
    version: String,
    revision: String,
    upload_id: String,
    published_at: String,
}

/// Settles the reservations whose request never confirmed its store:
/// one whose object is in UPLOADS becomes a stored upload, as of the
/// object's own upload time; one without an object is released and its
/// storage refunded. Both writes are guarded on the listed generation,
/// so a request that confirms or retakes the reservation meanwhile
/// wins.
async fn settle_reservations(env: &Env, db: &D1Database, now: &str) -> worker::Result<()> {
    let expired: Vec<ReservationRecord> = db
        .prepare(sql::EXPIRED_RESERVATIONS)
        .bind(&[now.into(), upload::RESERVATION_TTL.into()])?
        .all()
        .await?
        .results()?;
    if expired.is_empty() {
        return Ok(());
    }
    let bucket = env.bucket("UPLOADS")?;
    for reservation in &expired {
        if let Gate::Refused(_) = governor_client::decide(env, &consume_one(OpPool::BInfra)).await {
            console_error!("reservation pass: the governor refused the object check; stopping");
            return Ok(());
        }
        let ReservationRecord {
            scope,
            name,
            version,
            revision,
            upload_id,
            published_at,
        } = reservation;
        let settled = match bucket.head(upload::object_key(upload_id)).await? {
            Some(object) => {
                db.prepare(sql::CONFIRM_UPLOAD_STORED)
                    .bind(&[
                        iso8601_from_ms(object.uploaded().as_millis()).into(),
                        scope.into(),
                        name.into(),
                        version.into(),
                        revision.into(),
                        upload_id.into(),
                        published_at.into(),
                    ])?
                    .run()
                    .await?
            }
            None => db
                .batch(vec![
                    db.prepare(sql::RELEASE_EXPIRED_RESERVATION).bind(&[
                        now.into(),
                        scope.into(),
                        name.into(),
                        version.into(),
                        revision.into(),
                        published_at.into(),
                    ])?,
                    db.prepare(sql::REFUND_USER_STORED_BYTES).bind(&[
                        scope.into(),
                        name.into(),
                        version.into(),
                        revision.into(),
                    ])?,
                ])
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| worker::Error::RustError("missing batch result 0".to_owned()))?,
        };
        if changed_rows(settled.meta()?) == 0 {
            console_log!(
                "reservation pass: {scope}/{name}@{version}#{revision} moved on before it settled"
            );
        }
    }
    Ok(())
}

/// The website catalog refresh ([`catalog_refresh`]), dormant until the
/// `WEBSITE_DEPLOY_HOOK` secret (a Workers Builds deploy hook of
/// `website`) is set. Failures only log; the next pass retries.
pub(super) async fn refresh_website(env: &Env, db: &D1Database) {
    let Ok(hook) = env.secret("WEBSITE_DEPLOY_HOOK") else {
        return;
    };
    if let Err(err) = try_refresh_website(env, db, &hook.to_string()).await {
        console_error!("website refresh: {err}");
    }
}

async fn try_refresh_website(env: &Env, db: &D1Database, hook: &str) -> worker::Result<()> {
    // What the website's anonymous deploy fetch gets, by the read
    // plane's own admission rule.
    let target = if Route::Catalog.is_public() {
        let mut catalog = super::read::catalog_response(db).await?;
        if catalog.status_code() != 200 {
            return Err(worker::Error::RustError(
                "the catalog did not compose".to_owned(),
            ));
        }
        Target::Available(auth::hex(&Sha256::digest(catalog.bytes().await?)))
    } else {
        Target::Unavailable
    };
    let url = format!("{}/catalog-provenance.json", web_origin(env)?);
    let mut deployed = Fetch::Request(Request::new(&url, Method::Get)?)
        .send()
        .await?;
    let provenance = match deployed.status_code() {
        200 => deployed.text().await?,
        // A deployment from before the record existed is stale, not unreachable.
        404 => String::new(),
        status => {
            return Err(worker::Error::RustError(format!("{url} answered {status}")));
        }
    };
    let stored = read_meta(db, catalog_refresh::META_KEY).await?;
    if catalog_refresh::is_current(&provenance, &target) {
        if stored.as_deref().is_some_and(|value| !value.is_empty()) {
            upsert_meta(db, catalog_refresh::META_KEY, "")?
                .run()
                .await?;
        }
        return Ok(());
    }
    let Some(next) = catalog_refresh::next_trigger(stored.as_deref(), &target) else {
        console_error!(
            "website refresh: the site still lacks the current catalog after {} rebuilds; \
             waiting for the catalog to change or the site to catch up",
            catalog_refresh::MAX_TRIGGERS
        );
        return Ok(());
    };
    // Counted before the POST, so a failing meta write cannot leave
    // triggers uncounted.
    upsert_meta(db, catalog_refresh::META_KEY, &next)?
        .run()
        .await?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    // The hook URL carries its token and request errors can quote the URL,
    // so they never reach the log.
    let failed = |_| worker::Error::RustError("the deploy hook POST failed".to_owned());
    let request = Request::new_with_init(hook, &init).map_err(failed)?;
    let triggered = Fetch::Request(request).send().await.map_err(failed)?;
    if !(200..300).contains(&triggered.status_code()) {
        return Err(worker::Error::RustError(format!(
            "the deploy hook answered {}",
            triggered.status_code()
        )));
    }
    console_log!("website refresh: triggered a rebuild ({next})");
    Ok(())
}

/// One usage snapshot: the self-accounted storage plus the
/// analytics-sourced metrics.
#[allow(clippy::similar_names)] // r2_class_{a,b}_month mirror the Usage fields
async fn gather_usage(env: &Env, db: &D1Database, now: &str) -> worker::Result<breaker::Usage> {
    // Storage is the sum of what every publisher is charged, never
    // analytics. A negative sum is corrupt, and reads as unavailable.
    let stored_bytes = db
        .prepare(sql::TOTAL_STORED_BYTES)
        .first::<CountRecord>(None)
        .await?
        .and_then(|record| u64::try_from(record.n).ok());

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

pub(super) async fn evaluate_budgets(env: &Env) -> worker::Result<()> {
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

    if next != current {
        console_log!(
            "service_mode {} -> {}: {reason}",
            current.as_str(),
            next.as_str()
        );
    }
    let (pipeline, alerting) = pipeline_health(&db).await;
    if next != current || alerting {
        notify_webhook(env, current, next, &reason, &usage, &pipeline).await;
    }
    Ok(())
}

/// The webhook payload's `verification` and `publication` objects, and
/// whether either alerts. Pipeline health rides every pass, logging and
/// notifying until resolved, while mode changes notify once: revisions
/// pending for over an hour mean the verifier is stuck or absent, and
/// the fail-safe direction (nothing pending ever becomes resolvable on
/// its own) makes that invisible to users unless it alerts here. A
/// verified archive that is not placed, or a change that stays open,
/// fails as quietly: no request errors over it.
async fn pipeline_health(db: &D1Database) -> (serde_json::Value, bool) {
    let health = read_health(db)
        .await
        .inspect_err(|err| console_error!("pipeline health read: {err}"))
        .ok();
    let health = health.as_ref();
    let stale_pending = health.map(|health| non_negative(health.stale_pending));
    let verification_alert = verify::stale_pending_alert(stale_pending);
    if let Some(alert) = &verification_alert {
        console_error!("verification health: {alert}");
    }
    let publication_alert = health.and_then(|health| {
        publication::stall_alert(
            health.stalled_archive.as_deref(),
            health.stalled_job.as_deref(),
        )
    });
    if let Some(alert) = &publication_alert {
        console_error!("publication health: {alert}");
    }
    let alerting = verification_alert.is_some() || publication_alert.is_some();
    let pipeline = serde_json::json!({
        "verification": {
            "stale_pending": stale_pending,
            "oldest_pending": health.map(|health| &health.oldest_pending),
            "verifier_seen_at": health.map(|health| &health.verifier_seen_at),
            "alert": verification_alert,
        },
        "publication": {
            "stalled_archive": health.map(|health| &health.stalled_archive),
            "stalled_job": health.map(|health| &health.stalled_job),
            "last_completed_job": health.map(|health| &health.last_completed_job),
            "alert": publication_alert,
        },
    });
    (pipeline, alerting)
}

/// One row of `sql::PIPELINE_HEALTH`.
#[derive(Deserialize)]
struct Health {
    stale_pending: i64,
    oldest_pending: Option<String>,
    stalled_archive: Option<String>,
    stalled_job: Option<String>,
    last_completed_job: Option<String>,
    verifier_seen_at: Option<String>,
}

/// The pipeline's health as of now. The cutoffs are rendered by
/// `SQLite` in the ISO 8601 shape the times are stored in (`%fZ` gives
/// the fractional seconds and the `Z` the JS clock writes), so the
/// comparisons stay lexicographic.
async fn read_health(db: &D1Database) -> worker::Result<Health> {
    db.prepare(sql::PIPELINE_HEALTH)
        .bind(&[
            now_iso8601().into(),
            verify::STALE_PENDING.into(),
            publication::STALL.into(),
        ])?
        .first(None)
        .await?
        .ok_or_else(|| worker::Error::RustError("empty pipeline health result".to_owned()))
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
/// service-mode changes (`from != to`) and pipeline alerts alike
/// (`pipeline` holds the payload's `verification` and `publication`
/// objects); failures only log.
async fn notify_webhook(
    env: &Env,
    from: breaker::Mode,
    to: breaker::Mode,
    reason: &str,
    usage: &breaker::Usage,
    pipeline: &serde_json::Value,
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
        "verification": pipeline["verification"],
        "publication": pipeline["publication"],
    })
    .to_string();
    if post_json(&url.to_string(), &body, None).await.is_err() {
        console_error!("state-change webhook POST failed");
    }
}
