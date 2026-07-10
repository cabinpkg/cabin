//! The free-plan budget circuit breaker: pure evaluation of usage against
//! budgets (`docs/architecture.md`, "Billing model and the budget
//! breaker").
//!
//! The wasm32 glue's cron handler gathers usage (exact self-accounted R2
//! storage from `meta.total_stored_bytes`, the rest from the Cloudflare
//! GraphQL Analytics API via `crate::analytics`), evaluates it here, and
//! persists the resulting mode to `meta.service_mode`; the write handlers
//! only ever read that row.

/// The persisted `meta.service_mode` value. Ordered by severity so partial
/// analytics data can escalate but never de-escalate ([`next_mode`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    Normal,
    Warn,
    WritesBlocked,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Warn => "warn",
            Mode::WritesBlocked => "writes_blocked",
        }
    }

    /// Parses a stored `meta.service_mode` value. `None` (an unknown or
    /// corrupted value) must be treated as [`Mode::WritesBlocked`] by
    /// write handlers: fail closed.
    pub fn parse(value: &str) -> Option<Mode> {
        match value {
            "normal" => Some(Mode::Normal),
            "warn" => Some(Mode::Warn),
            "writes_blocked" => Some(Mode::WritesBlocked),
            _ => None,
        }
    }
}

/// The envelope `code` and fixed detail for refused writes, plus the
/// `Retry-After` matching the cron cadence (the next evaluation that could
/// unblock is at most ~15 minutes away).
pub const OVER_BUDGET_CODE: &str = "registry_over_budget";
pub const OVER_BUDGET_DETAIL: &str =
    "registry writes are temporarily disabled: the free-plan budget is exhausted";
pub const OVER_BUDGET_RETRY_AFTER_SECS: u64 = 900;

/// Budget ceilings, each comfortably below the matching Cloudflare free
/// limit so the service blocks itself before Cloudflare does. Overridable
/// per environment via the same-named env vars.
#[derive(Debug, Clone, Copy)]
pub struct Budgets {
    /// `BUDGET_R2_STORAGE_BYTES`; free limit 10 GiB-month across the
    /// account. The metric counts primary (BLOBS) bytes only, but
    /// publish-time replication stores every blob a second time in
    /// BACKUP and the nightly D1 dumps add metadata copies there, so
    /// the default budget stays under half the free limit.
    pub r2_storage_bytes: u64,
    /// `BUDGET_R2_CLASS_A_MONTH`; free limit 1 million/month.
    pub r2_class_a_month: u64,
    /// `BUDGET_WORKERS_REQ_DAY`; free limit 100,000/day.
    pub workers_requests_day: u64,
    /// `BUDGET_D1_ROWS_READ_DAY`; free limit 5 million/day.
    pub d1_rows_read_day: u64,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets {
            r2_storage_bytes: 4 * 1024 * 1024 * 1024,
            r2_class_a_month: 800_000,
            workers_requests_day: 80_000,
            d1_rows_read_day: 4_000_000,
        }
    }
}

/// One cron pass's usage snapshot. Storage is exact (self-accounted in
/// `meta.total_stored_bytes`, never analytics) but still `None` when the
/// meta row is missing or non-numeric - unavailable data, never zero, so
/// a corrupt counter can never unblock writes. The analytics-sourced
/// metrics are `None` when their query failed or the dataset was
/// rejected.
#[derive(Debug, Clone, Copy)]
pub struct Usage {
    pub stored_bytes: Option<u64>,
    pub workers_requests_today: Option<u64>,
    pub r2_class_a_month: Option<u64>,
    pub d1_rows_read_today: Option<u64>,
}

impl Usage {
    /// Whether every metric was gathered this pass.
    pub fn complete(&self) -> bool {
        self.stored_bytes.is_some()
            && self.workers_requests_today.is_some()
            && self.r2_class_a_month.is_some()
            && self.d1_rows_read_today.is_some()
    }
}

/// Evaluates one usage snapshot: any metric at or over its budget blocks
/// writes, any at or over 80% warns, otherwise normal. Missing metrics
/// contribute nothing - [`next_mode`] decides what missing data means for
/// the persisted state. The reason names the worst offending metric.
pub fn evaluate(usage: &Usage, budgets: &Budgets) -> (Mode, String) {
    let metrics: [(&str, Option<u64>, u64); 4] = [
        (
            "r2 storage bytes",
            usage.stored_bytes,
            budgets.r2_storage_bytes,
        ),
        (
            "r2 class A operations this month",
            usage.r2_class_a_month,
            budgets.r2_class_a_month,
        ),
        (
            "workers requests today",
            usage.workers_requests_today,
            budgets.workers_requests_day,
        ),
        (
            "d1 rows read today",
            usage.d1_rows_read_today,
            budgets.d1_rows_read_day,
        ),
    ];
    let mut worst = (Mode::Normal, String::from("all budgets under 80%"));
    for (label, used, budget) in metrics {
        let Some(used) = used else { continue };
        // 80% in integer math, widened so huge env-var budgets cannot
        // overflow the comparison.
        let mode = if used >= budget {
            Mode::WritesBlocked
        } else if u128::from(used) * 5 >= u128::from(budget) * 4 {
            Mode::Warn
        } else {
            Mode::Normal
        };
        if mode > worst.0 {
            worst = (mode, format!("{label}: {used} of budget {budget}"));
        }
    }
    worst
}

/// Combines the persisted mode with a fresh evaluation. Complete data
/// wins outright; partial data may escalate but never de-escalate, so a
/// failed analytics query can never unblock writes ("never flip modes on
/// missing data").
pub fn next_mode(current: Mode, candidate: Mode, complete: bool) -> Mode {
    if complete {
        candidate
    } else {
        current.max(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Usage {
        Usage {
            stored_bytes: Some(0),
            workers_requests_today: Some(0),
            r2_class_a_month: Some(0),
            d1_rows_read_today: Some(0),
        }
    }

    #[test]
    fn mode_round_trips_and_rejects_garbage() {
        for mode in [Mode::Normal, Mode::Warn, Mode::WritesBlocked] {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(Mode::parse(""), None);
        assert_eq!(Mode::parse("blocked"), None);
    }

    #[test]
    fn healthy_usage_is_normal() {
        let (mode, reason) = evaluate(&healthy(), &Budgets::default());
        assert_eq!(mode, Mode::Normal);
        assert_eq!(reason, "all budgets under 80%");
    }

    #[test]
    fn thresholds_are_exact_at_80_and_100_percent() {
        let budgets = Budgets::default();
        let under_warn = Usage {
            workers_requests_today: Some(budgets.workers_requests_day * 4 / 5 - 1),
            ..healthy()
        };
        assert_eq!(evaluate(&under_warn, &budgets).0, Mode::Normal);
        let at_warn = Usage {
            workers_requests_today: Some(budgets.workers_requests_day * 4 / 5),
            ..healthy()
        };
        assert_eq!(evaluate(&at_warn, &budgets).0, Mode::Warn);
        let at_budget = Usage {
            workers_requests_today: Some(budgets.workers_requests_day),
            ..healthy()
        };
        let (mode, reason) = evaluate(&at_budget, &budgets);
        assert_eq!(mode, Mode::WritesBlocked);
        assert!(
            reason.contains("workers requests today"),
            "reason: {reason}"
        );
    }

    #[test]
    fn storage_uses_the_exact_self_accounted_bytes() {
        let budgets = Budgets::default();
        let over = Usage {
            stored_bytes: Some(budgets.r2_storage_bytes),
            ..healthy()
        };
        let (mode, reason) = evaluate(&over, &budgets);
        assert_eq!(mode, Mode::WritesBlocked);
        assert!(reason.contains("r2 storage bytes"), "reason: {reason}");
    }

    #[test]
    fn missing_metrics_contribute_nothing() {
        let budgets = Budgets::default();
        let missing = Usage {
            stored_bytes: Some(0),
            workers_requests_today: None,
            r2_class_a_month: None,
            d1_rows_read_today: None,
        };
        assert!(!missing.complete());
        assert_eq!(evaluate(&missing, &budgets).0, Mode::Normal);
    }

    #[test]
    fn unavailable_storage_marks_the_snapshot_incomplete() {
        // A corrupt total_stored_bytes must ride the never-de-escalate
        // path, not read as zero usage.
        let missing_storage = Usage {
            stored_bytes: None,
            ..healthy()
        };
        assert!(!missing_storage.complete());
        assert_eq!(
            next_mode(
                Mode::WritesBlocked,
                evaluate(&missing_storage, &Budgets::default()).0,
                missing_storage.complete()
            ),
            Mode::WritesBlocked
        );
    }

    #[test]
    fn the_worst_metric_wins_the_reason() {
        let budgets = Budgets::default();
        let usage = Usage {
            workers_requests_today: Some(budgets.workers_requests_day * 4 / 5),
            d1_rows_read_today: Some(budgets.d1_rows_read_day + 1),
            ..healthy()
        };
        let (mode, reason) = evaluate(&usage, &budgets);
        assert_eq!(mode, Mode::WritesBlocked);
        assert!(reason.contains("d1 rows read today"), "reason: {reason}");
    }

    #[test]
    fn complete_data_moves_the_mode_in_both_directions() {
        assert_eq!(
            next_mode(Mode::WritesBlocked, Mode::Normal, true),
            Mode::Normal
        );
        assert_eq!(
            next_mode(Mode::Normal, Mode::WritesBlocked, true),
            Mode::WritesBlocked
        );
    }

    #[test]
    fn partial_data_escalates_but_never_de_escalates() {
        assert_eq!(
            next_mode(Mode::WritesBlocked, Mode::Normal, false),
            Mode::WritesBlocked
        );
        assert_eq!(next_mode(Mode::Warn, Mode::Normal, false), Mode::Warn);
        assert_eq!(
            next_mode(Mode::Normal, Mode::WritesBlocked, false),
            Mode::WritesBlocked
        );
    }
}
