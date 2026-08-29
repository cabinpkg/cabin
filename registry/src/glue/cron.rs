//! The breaker cron's budget evaluation and governor reconciliation.

use serde::Deserialize;
use worker::{D1Database, Env, console_error, console_log};

use crate::governor::{self, StoragePool};
use crate::governor_client::{self};
use crate::{analytics, backup, breaker, sql, verify};

use super::{CountRecord, non_negative, now_iso8601, post_json, read_meta, upsert_meta};

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
pub(super) async fn reconcile_governor(env: &Env, db: &D1Database) {
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
    match push_live_set_to_governor(env, db).await {
        Err(err) => console_error!("governor reconciliation: live-set query failed: {err}"),
        Ok(None) => console_error!("governor reconciliation: the governor did not answer"),
        Ok(Some(report)) => {
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
    }
}

/// The reconciliation core the cron pass and the admin endpoint's
/// on-demand `{"reconcile":true}` action share: pushes D1's
/// authoritative live-blob set (primary pool only - operation windows,
/// backup, and dump accounting have their own recovery paths;
/// `docs/runbook.md`, "Known ceilings") and returns the governor's
/// report. `None` means the governor did not answer.
pub(super) async fn push_live_set_to_governor(
    env: &Env,
    db: &D1Database,
) -> worker::Result<Option<governor::ReconcileReport>> {
    let rows: Vec<LiveBlobRecord> = db.prepare(sql::LIVE_BLOB_SIZES).all().await?.results()?;
    let live = rows
        .into_iter()
        .map(|row| governor::LiveObject {
            key: format!("blobs/sha256/{}", crate::checksum::hex(&row.checksum)),
            bytes: non_negative(row.size),
        })
        .collect();
    let request = governor::ReconcileRequest {
        pool: StoragePool::Primary,
        live,
    };
    Ok(governor_client::reconcile(env, &request).await)
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
