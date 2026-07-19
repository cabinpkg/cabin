//! The free-plan budget circuit breaker: pure evaluation of usage against
//! budgets (`docs/architecture.md`, "Billing model and the budget
//! breaker").
//!
//! The wasm32 glue's cron handler gathers usage (exact self-accounted R2
//! storage from `meta.total_stored_bytes`, the rest from the Cloudflare
//! GraphQL Analytics API via `crate::analytics`), evaluates it here, and
//! persists the resulting mode to `meta.service_mode`; the request
//! handlers only ever read that row - writes fail closed on it, reads
//! fail open ([`read_gate_refuses`]).

/// The persisted `meta.service_mode` value. Ordered by severity so partial
/// analytics data can escalate but never de-escalate ([`next_mode`]);
/// [`Mode::ReadsBlocked`] sits above [`Mode::WritesBlocked`] because it
/// blocks writes too - write gates compare `>=`, never `==`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    Normal,
    Warn,
    WritesBlocked,
    ReadsBlocked,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Normal => "normal",
            Mode::Warn => "warn",
            Mode::WritesBlocked => "writes_blocked",
            Mode::ReadsBlocked => "reads_blocked",
        }
    }

    /// Parses a stored `meta.service_mode` value. `None` (an unknown or
    /// corrupted value) must be treated as [`Mode::WritesBlocked`] by
    /// write handlers: fail closed. Read handlers gate only on an
    /// affirmatively parsed [`Mode::ReadsBlocked`] ([`read_gate_refuses`]),
    /// so the same `None` leaves reads serving: fail open.
    pub fn parse(value: &str) -> Option<Mode> {
        match value {
            "normal" => Some(Mode::Normal),
            "warn" => Some(Mode::Warn),
            "writes_blocked" => Some(Mode::WritesBlocked),
            "reads_blocked" => Some(Mode::ReadsBlocked),
            _ => None,
        }
    }
}

/// Whether the data-plane read gate refuses a request. Reads obey only an
/// affirmatively read `reads_blocked`: `None` - a failed mode lookup -
/// serves, and so does every lesser mode (a corrupt or missing stored
/// value parses to [`Mode::WritesBlocked`] upstream, which also serves),
/// so downloads keep working through an outage of the breaker itself.
/// `verify_exempt` (the verifier's data-plane fetches) wins over the mode:
/// verification must be able to drain the pending queue while reads are
/// blocked, and its spend is negligible.
pub fn read_gate_refuses(mode: Option<Mode>, verify_exempt: bool) -> bool {
    mode == Some(Mode::ReadsBlocked) && !verify_exempt
}

/// The envelope `code` and fixed details for refused writes and reads,
/// plus the `Retry-After` matching the cron cadence (the next evaluation
/// that could unblock is at most ~15 minutes away). One code for both
/// planes; only the detail names which surface is paused.
pub const OVER_BUDGET_CODE: &str = "registry_over_budget";
pub const OVER_BUDGET_DETAIL: &str =
    "registry writes are temporarily disabled: the free-plan budget is exhausted";
pub const OVER_BUDGET_READS_DETAIL: &str =
    "registry downloads are temporarily disabled: the registry's read budget is exhausted";
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
    /// `BUDGET_R2_CLASS_B_MONTH`; free limit 10 million/month.
    pub r2_class_b_month: u64,
    /// How far the Class B metric may escalate the mode: [`Mode::Warn`]
    /// while `BUDGET_R2_CLASS_B_MONTH` is unset - visibility without
    /// blocking, because a write block cannot fix read-driven spend -
    /// and [`Mode::ReadsBlocked`] once the operator configures a read
    /// budget. Keeping the ceiling here (not inferred inside
    /// [`evaluate`]) is what makes `reads_blocked` unreachable until
    /// the env var is set.
    pub r2_class_b_ceiling: Mode,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets {
            r2_storage_bytes: 4 * 1024 * 1024 * 1024,
            r2_class_a_month: 800_000,
            workers_requests_day: 80_000,
            d1_rows_read_day: 4_000_000,
            r2_class_b_month: 8_000_000,
            r2_class_b_ceiling: Mode::Warn,
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
    pub r2_class_b_month: Option<u64>,
}

impl Usage {
    /// Whether every write-side metric was gathered this pass; gates
    /// de-escalation at and below `writes_blocked` ([`next_mode`]).
    pub fn write_complete(&self) -> bool {
        self.stored_bytes.is_some()
            && self.workers_requests_today.is_some()
            && self.r2_class_a_month.is_some()
            && self.d1_rows_read_today.is_some()
    }

    /// Whether the read-side metric was gathered this pass; gates
    /// lifting `reads_blocked` ([`next_mode`]). Class B counts only
    /// while the read breaker is armed (its ceiling is
    /// [`Mode::ReadsBlocked`]): unarmed it is warn-only monitoring, and
    /// an outage of just its query must not count as partial data -
    /// that would keep a stale block pinned over a metric that could
    /// never have caused it.
    pub fn read_complete(&self, class_b_armed: bool) -> bool {
        !class_b_armed || self.r2_class_b_month.is_some()
    }
}

/// Evaluates one usage snapshot: any metric at or over its budget
/// escalates to that metric's ceiling ([`Mode::WritesBlocked`] for
/// every metric except Class B, whose ceiling is configuration-driven -
/// see [`Budgets::r2_class_b_ceiling`]), any at or over 80% warns,
/// otherwise normal. Missing metrics contribute nothing - [`next_mode`]
/// decides what missing data means for the persisted state. The reason
/// names the worst offending metric.
pub fn evaluate(usage: &Usage, budgets: &Budgets) -> (Mode, String) {
    let metrics: [(&str, Option<u64>, u64, Mode); 5] = [
        (
            "r2 storage bytes",
            usage.stored_bytes,
            budgets.r2_storage_bytes,
            Mode::WritesBlocked,
        ),
        (
            "r2 class A operations this month",
            usage.r2_class_a_month,
            budgets.r2_class_a_month,
            Mode::WritesBlocked,
        ),
        (
            "workers requests today",
            usage.workers_requests_today,
            budgets.workers_requests_day,
            Mode::WritesBlocked,
        ),
        (
            "d1 rows read today",
            usage.d1_rows_read_today,
            budgets.d1_rows_read_day,
            Mode::WritesBlocked,
        ),
        (
            "r2 class B operations this month",
            usage.r2_class_b_month,
            budgets.r2_class_b_month,
            budgets.r2_class_b_ceiling,
        ),
    ];
    let mut worst = (Mode::Normal, String::from("all budgets under 80%"));
    for (label, used, budget, ceiling) in metrics {
        let Some(used) = used else { continue };
        // 80% in integer math, widened so huge env-var budgets cannot
        // overflow the comparison.
        let mode = if used >= budget {
            ceiling
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

/// Combines the persisted mode with a fresh evaluation, per plane.
/// Complete data moves a plane in both directions; missing data keeps
/// that plane's current state ("never flip modes on missing data"):
/// `write_complete` gates de-escalation at and below `writes_blocked`,
/// and `read_complete` gates lifting `reads_blocked`. The planes are
/// independent on purpose - a write-side analytics outage drops a
/// `reads_blocked` whose read data proves recovery to `writes_blocked`
/// (never below), while a read-side outage never reopens reads and a
/// write-side outage never unblocks writes.
pub fn next_mode(
    current: Mode,
    candidate: Mode,
    write_complete: bool,
    read_complete: bool,
) -> Mode {
    let write_floor = if write_complete {
        Mode::Normal
    } else {
        current.min(Mode::WritesBlocked)
    };
    let read_floor = if read_complete || current != Mode::ReadsBlocked {
        Mode::Normal
    } else {
        Mode::ReadsBlocked
    };
    candidate.max(write_floor).max(read_floor)
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
            r2_class_b_month: Some(0),
        }
    }

    #[test]
    fn mode_round_trips_and_rejects_garbage() {
        for mode in [
            Mode::Normal,
            Mode::Warn,
            Mode::WritesBlocked,
            Mode::ReadsBlocked,
        ] {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(Mode::parse(""), None);
        assert_eq!(Mode::parse("blocked"), None);
    }

    #[test]
    fn the_ladder_orders_reads_blocked_worst() {
        // next_mode and the write gates lean on this ordering.
        assert!(Mode::Normal < Mode::Warn);
        assert!(Mode::Warn < Mode::WritesBlocked);
        assert!(Mode::WritesBlocked < Mode::ReadsBlocked);
    }

    #[test]
    fn reads_are_refused_only_on_an_affirmative_reads_blocked() {
        // The outage-resilience invariant: a failed lookup (None) and
        // every lesser mode serve; only reads_blocked itself refuses,
        // and the verifier's fetches are exempt even then.
        for mode in [
            None,
            Some(Mode::Normal),
            Some(Mode::Warn),
            Some(Mode::WritesBlocked),
        ] {
            assert!(!read_gate_refuses(mode, false), "mode: {mode:?}");
        }
        assert!(read_gate_refuses(Some(Mode::ReadsBlocked), false));
        assert!(!read_gate_refuses(Some(Mode::ReadsBlocked), true));
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
    fn class_b_stops_at_warn_without_a_configured_read_budget() {
        // The default (env unset) ceiling: even far over budget, the
        // read-driven metric warns and goes no higher - a write block
        // cannot fix read spend.
        let budgets = Budgets::default();
        for used in [
            budgets.r2_class_b_month * 4 / 5,
            budgets.r2_class_b_month,
            u64::MAX,
        ] {
            let usage = Usage {
                r2_class_b_month: Some(used),
                ..healthy()
            };
            let (mode, reason) = evaluate(&usage, &budgets);
            assert_eq!(mode, Mode::Warn, "used: {used}");
            assert!(
                reason.contains("r2 class B operations this month"),
                "reason: {reason}"
            );
        }
        let under_warn = Usage {
            r2_class_b_month: Some(budgets.r2_class_b_month * 4 / 5 - 1),
            ..healthy()
        };
        assert_eq!(evaluate(&under_warn, &budgets).0, Mode::Normal);
    }

    #[test]
    fn reads_blocked_is_reachable_only_with_a_configured_read_budget() {
        // Dormant until configured: with the defaults (env unset), no
        // input at all can produce reads_blocked - not even every
        // metric maxed out.
        let saturated = Usage {
            stored_bytes: Some(u64::MAX),
            workers_requests_today: Some(u64::MAX),
            r2_class_a_month: Some(u64::MAX),
            d1_rows_read_today: Some(u64::MAX),
            r2_class_b_month: Some(u64::MAX),
        };
        assert_eq!(
            evaluate(&saturated, &Budgets::default()).0,
            Mode::WritesBlocked
        );

        // With the env var set the glue raises the ceiling, and the
        // metric escalates like the others: 80% warns, at budget blocks
        // reads.
        let budgets = Budgets {
            r2_class_b_ceiling: Mode::ReadsBlocked,
            ..Budgets::default()
        };
        let at_warn = Usage {
            r2_class_b_month: Some(budgets.r2_class_b_month * 4 / 5),
            ..healthy()
        };
        assert_eq!(evaluate(&at_warn, &budgets).0, Mode::Warn);
        let at_budget = Usage {
            r2_class_b_month: Some(budgets.r2_class_b_month),
            ..healthy()
        };
        let (mode, reason) = evaluate(&at_budget, &budgets);
        assert_eq!(mode, Mode::ReadsBlocked);
        assert!(
            reason.contains("r2 class B operations this month"),
            "reason: {reason}"
        );
    }

    #[test]
    fn missing_metrics_contribute_nothing() {
        let budgets = Budgets::default();
        let missing = Usage {
            stored_bytes: Some(0),
            workers_requests_today: None,
            r2_class_a_month: None,
            d1_rows_read_today: None,
            r2_class_b_month: None,
        };
        assert!(!missing.write_complete());
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
        assert!(!missing_storage.write_complete());
        assert_eq!(
            next_mode(
                Mode::WritesBlocked,
                evaluate(&missing_storage, &Budgets::default()).0,
                missing_storage.write_complete(),
                missing_storage.read_complete(false)
            ),
            Mode::WritesBlocked
        );
    }

    #[test]
    fn a_dormant_class_b_outage_never_pins_a_write_block() {
        // With the read breaker unarmed, a pass where only the Class B
        // query failed is still complete on both planes: the write-side
        // metrics all recovered, so a stale writes_blocked de-escalates
        // instead of being held hostage by a metric that could never
        // have caused it. Armed, the same outage is partial read data -
        // missing read usage must never reopen reads.
        let class_b_missing = Usage {
            r2_class_b_month: None,
            ..healthy()
        };
        assert!(class_b_missing.write_complete());
        assert!(class_b_missing.read_complete(false));
        assert!(!class_b_missing.read_complete(true));
        let candidate = evaluate(&class_b_missing, &Budgets::default()).0;
        assert_eq!(
            next_mode(Mode::WritesBlocked, candidate, true, true),
            Mode::Normal
        );
        assert_eq!(
            next_mode(Mode::ReadsBlocked, candidate, true, false),
            Mode::ReadsBlocked
        );
    }

    #[test]
    fn a_write_side_outage_drops_reads_blocked_to_writes_blocked() {
        // The planes are independent: read data proving the read budget
        // recovered lifts reads_blocked even while a write-side
        // analytics outage keeps writes conservatively blocked - and
        // only that far down.
        assert_eq!(
            next_mode(Mode::ReadsBlocked, Mode::Normal, false, true),
            Mode::WritesBlocked
        );
        // A candidate still at reads_blocked stays there regardless.
        assert_eq!(
            next_mode(Mode::ReadsBlocked, Mode::ReadsBlocked, false, true),
            Mode::ReadsBlocked
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
            next_mode(Mode::WritesBlocked, Mode::Normal, true, true),
            Mode::Normal
        );
        assert_eq!(
            next_mode(Mode::Normal, Mode::WritesBlocked, true, true),
            Mode::WritesBlocked
        );
        assert_eq!(
            next_mode(Mode::ReadsBlocked, Mode::Normal, true, true),
            Mode::Normal
        );
    }

    #[test]
    fn partial_data_escalates_but_never_de_escalates() {
        assert_eq!(
            next_mode(Mode::WritesBlocked, Mode::Normal, false, true),
            Mode::WritesBlocked
        );
        assert_eq!(next_mode(Mode::Warn, Mode::Normal, false, true), Mode::Warn);
        assert_eq!(
            next_mode(Mode::Normal, Mode::WritesBlocked, false, true),
            Mode::WritesBlocked
        );
        // The read plane rides the same rule: missing read data never
        // reopens reads, whatever the write metrics say.
        assert_eq!(
            next_mode(Mode::ReadsBlocked, Mode::Normal, true, false),
            Mode::ReadsBlocked
        );
        assert_eq!(
            next_mode(Mode::ReadsBlocked, Mode::Normal, false, false),
            Mode::ReadsBlocked
        );
    }
}
