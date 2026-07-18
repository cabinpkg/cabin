//! Per-user publish quotas: the plan -> quota map, the publish token
//! bucket, and the pure enforcement checks the publish handler runs in
//! order (`docs/architecture.md`, "Billing model and the budget breaker").
//!
//! Everything here is pure and host-testable; the wasm32 glue supplies
//! the clock, the D1 counts, and the archive size. Daily windows are UTC
//! calendar days, compared lexicographically on the stored ISO 8601
//! timestamps via [`utc_day_prefix`].

/// The quotas one plan grants. The map from plan name to quotas lives in
/// [`quotas_for_plan`]; there is deliberately no plan table in D1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanQuotas {
    pub max_archive_bytes: u64,
    pub max_total_bytes_per_user: u64,
    pub max_new_packages_per_day: u64,
    pub max_packages_total: u64,
    pub max_versions_per_package_per_day: u64,
    /// Publish token bucket: burst capacity in tokens.
    pub publish_burst: f64,
    /// Publish token bucket: refill rate in tokens per minute.
    pub publish_refill_per_minute: f64,
}

const FREE: PlanQuotas = PlanQuotas {
    max_archive_bytes: 16 * 1024 * 1024,
    max_total_bytes_per_user: 128 * 1024 * 1024,
    max_new_packages_per_day: 5,
    max_packages_total: 50,
    max_versions_per_package_per_day: 30,
    publish_burst: 5.0,
    publish_refill_per_minute: 1.0,
};

/// Quotas for a `users.plan` value. Unknown plan names get the `free`
/// quotas: deny-by-default, a typo never grants more.
pub fn quotas_for_plan(_plan: &str) -> PlanQuotas {
    // 'free' is the only plan today; a second plan adds a match here, not
    // new columns.
    FREE
}

/// Publish token-bucket state, as stored on the token row (`rl_tokens`,
/// `rl_updated_at`; the timestamp is Unix epoch milliseconds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    pub tokens: f64,
    pub updated_at_ms: f64,
}

/// The outcome of one bucket take. On `allowed`, the caller persists
/// `bucket`; on denial the stored state is left untouched (refill keeps
/// accruing from the last persisted timestamp) and `retry_after_secs`
/// says when one full token will be available.
#[derive(Debug, PartialEq)]
pub struct TakeOutcome {
    pub allowed: bool,
    pub bucket: Bucket,
    pub retry_after_secs: u64,
}

/// Takes one publish token from `prev` (or a full bucket for a token row
/// that has never published), refilling first from the elapsed time.
pub fn take_publish_token(prev: Option<Bucket>, now_ms: f64, quotas: &PlanQuotas) -> TakeOutcome {
    let tokens = match prev {
        Some(prev) => {
            let elapsed_ms = (now_ms - prev.updated_at_ms).max(0.0);
            let refill = elapsed_ms / 60_000.0 * quotas.publish_refill_per_minute;
            (prev.tokens + refill).min(quotas.publish_burst)
        }
        None => quotas.publish_burst,
    };
    if tokens >= 1.0 {
        TakeOutcome {
            allowed: true,
            bucket: Bucket {
                tokens: tokens - 1.0,
                updated_at_ms: now_ms,
            },
            retry_after_secs: 0,
        }
    } else {
        let wait_secs = (1.0 - tokens) / quotas.publish_refill_per_minute * 60.0;
        TakeOutcome {
            allowed: false,
            bucket: Bucket {
                tokens,
                updated_at_ms: now_ms,
            },
            // ceil() of a small positive wait always fits u64.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            retry_after_secs: wait_secs.ceil().max(1.0) as u64,
        }
    }
}

/// The `YYYY-MM-DDT` prefix of an ISO 8601 UTC timestamp. Every timestamp
/// of the same UTC calendar day starts with it, and every earlier day
/// compares lexicographically below it, so `published_at >= prefix` is
/// the "today" window in SQL. `None` when the input is not ISO-shaped.
pub fn utc_day_prefix(now_iso: &str) -> Option<&str> {
    let prefix = now_iso.get(..11)?;
    (prefix.as_bytes()[10] == b'T').then_some(prefix)
}

/// A quota or rate-limit refusal: the response status, the machine-readable
/// `code` for the error envelope, and the fixed human-readable detail.
#[derive(Debug, PartialEq, Eq)]
pub struct Denial {
    pub status: u16,
    pub code: &'static str,
    pub detail: &'static str,
}

pub const RATE_LIMITED: Denial = Denial {
    status: 429,
    code: "rate_limited",
    detail: "publish rate limit exceeded; retry after the token bucket refills",
};
pub const ARCHIVE_TOO_LARGE: Denial = Denial {
    status: 413,
    code: "archive_too_large",
    detail: "archive exceeds the plan's per-archive size limit",
};
pub const QUOTA_STORAGE: Denial = Denial {
    status: 403,
    code: "quota_storage",
    detail: "publishing this archive would exceed the plan's total storage quota",
};
pub const QUOTA_PACKAGES_DAILY: Denial = Denial {
    status: 403,
    code: "quota_packages_daily",
    detail: "the plan's daily new-package quota is exhausted",
};
pub const QUOTA_PACKAGES_TOTAL: Denial = Denial {
    status: 403,
    code: "quota_packages_total",
    detail: "the plan's total package quota is exhausted",
};
pub const QUOTA_VERSIONS_DAILY: Denial = Denial {
    status: 403,
    code: "quota_versions_daily",
    detail: "the plan's daily per-package version quota is exhausted",
};

/// The envelope `detail` for a denial: the per-user quota family
/// (`quota_*`) appends the dashboard URL built from `WEB_ORIGIN`, so
/// clients print a server-embedded usage pointer instead of deriving a
/// web URL from the index origin themselves; the rate-limit, size, and
/// budget refusals stay the fixed strings.
pub fn detail_with_usage_url(denial: &Denial, web_origin: &str) -> String {
    if denial.code.starts_with("quota_") {
        format!(
            "{}; see {}/dashboard for current usage",
            denial.detail,
            web_origin.trim_end_matches('/'),
        )
    } else {
        denial.detail.to_owned()
    }
}

/// The `413` check, run as soon as the frame is decoded.
///
/// # Errors
///
/// [`ARCHIVE_TOO_LARGE`] when the archive exceeds the plan's cap.
pub fn check_archive_size(archive_bytes: u64, quotas: &PlanQuotas) -> Result<(), Denial> {
    if archive_bytes > quotas.max_archive_bytes {
        return Err(ARCHIVE_TOO_LARGE);
    }
    Ok(())
}

/// The D1 counts the publish handler gathers for a genuinely new version
/// (the idempotent no-op and the immutability conflict never reach the
/// quota checks).
#[derive(Debug, Clone, Copy)]
pub struct PublishCounts {
    /// `SUM(archive_size)` over the user's versions.
    pub user_stored_bytes: u64,
    /// Packages the user created (`packages.created_by`), ever.
    pub user_package_count: u64,
    /// Packages the user created (first published) today.
    pub user_new_packages_today: u64,
    /// Versions of the target package published today (by anyone).
    pub package_versions_today: u64,
    /// Whether the target package already exists.
    pub package_exists: bool,
}

/// The `403` quota checks, in the documented order: storage, then - when
/// the publish would create a new package - the daily and total package
/// quotas, then the daily per-package version quota. Limits are exact
/// thresholds: a publish that lands exactly on a byte or count limit is
/// allowed, the next one is not.
///
/// # Errors
///
/// The first quota [`Denial`] that fails.
pub fn check_publish(
    archive_bytes: u64,
    counts: &PublishCounts,
    quotas: &PlanQuotas,
) -> Result<(), Denial> {
    if counts.user_stored_bytes + archive_bytes > quotas.max_total_bytes_per_user {
        return Err(QUOTA_STORAGE);
    }
    if !counts.package_exists {
        if counts.user_new_packages_today >= quotas.max_new_packages_per_day {
            return Err(QUOTA_PACKAGES_DAILY);
        }
        if counts.user_package_count >= quotas.max_packages_total {
            return Err(QUOTA_PACKAGES_TOTAL);
        }
    }
    if counts.package_versions_today >= quotas.max_versions_per_package_per_day {
        return Err(QUOTA_VERSIONS_DAILY);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_plans_fall_back_to_free() {
        assert_eq!(quotas_for_plan("free"), FREE);
        assert_eq!(quotas_for_plan("enterprise"), FREE);
        assert_eq!(quotas_for_plan(""), FREE);
    }

    #[test]
    fn fresh_bucket_allows_a_full_burst_then_denies() {
        let quotas = quotas_for_plan("free");
        let mut bucket = None;
        for take in 0..5 {
            let outcome = take_publish_token(bucket, 1_000.0, &quotas);
            assert!(outcome.allowed, "take {take}");
            bucket = Some(outcome.bucket);
        }
        let denied = take_publish_token(bucket, 1_000.0, &quotas);
        assert!(!denied.allowed);
        // Empty bucket, 1 token/minute: a full token is 60 s away.
        assert_eq!(denied.retry_after_secs, 60);
    }

    #[test]
    fn bucket_refills_at_the_plan_rate_and_caps_at_burst() {
        let quotas = quotas_for_plan("free");
        let empty = Bucket {
            tokens: 0.0,
            updated_at_ms: 0.0,
        };
        // 30 s = half a token: still denied, half the wait left.
        let denied = take_publish_token(Some(empty), 30_000.0, &quotas);
        assert!(!denied.allowed);
        assert_eq!(denied.retry_after_secs, 30);
        // 60 s = exactly one token: allowed, bucket drained again.
        let allowed = take_publish_token(Some(empty), 60_000.0, &quotas);
        assert!(allowed.allowed);
        assert!(allowed.bucket.tokens.abs() < 1e-9);
        assert!((allowed.bucket.updated_at_ms - 60_000.0).abs() < f64::EPSILON);
        // A long idle refills to burst, never beyond.
        let idle = take_publish_token(Some(empty), 3_600_000.0, &quotas);
        assert!((idle.bucket.tokens - (quotas.publish_burst - 1.0)).abs() < 1e-9);
    }

    #[test]
    fn bucket_ignores_a_clock_that_went_backwards() {
        let quotas = quotas_for_plan("free");
        let prev = Bucket {
            tokens: 2.0,
            updated_at_ms: 100_000.0,
        };
        let outcome = take_publish_token(Some(prev), 50_000.0, &quotas);
        assert!(outcome.allowed);
        // No negative refill: 2.0 - 1.0, not less.
        assert!((outcome.bucket.tokens - 1.0).abs() < 1e-9);
    }

    #[test]
    fn utc_day_prefix_is_the_lexicographic_day_window() {
        assert_eq!(
            utc_day_prefix("2026-07-09T12:34:56.789Z"),
            Some("2026-07-09T")
        );
        // Same day compares >= the prefix, the day before compares below,
        // including midnight timestamps with fractional seconds.
        assert!("2026-07-09T00:00:00.000Z" >= "2026-07-09T");
        assert!("2026-07-09T23:59:59.999Z" >= "2026-07-09T");
        assert!("2026-07-08T23:59:59.999Z" < "2026-07-09T");
        assert_eq!(utc_day_prefix("not a timestamp"), None);
        assert_eq!(utc_day_prefix("2026-07-09"), None);
        assert_eq!(utc_day_prefix(""), None);
    }

    #[test]
    fn quota_denials_embed_the_dashboard_url_and_others_do_not() {
        assert_eq!(
            detail_with_usage_url(&QUOTA_STORAGE, "https://cabinpkg.com"),
            "publishing this archive would exceed the plan's total storage quota; \
             see https://cabinpkg.com/dashboard for current usage"
        );
        // A trailing slash on the env var never doubles the separator.
        assert_eq!(
            detail_with_usage_url(&QUOTA_PACKAGES_TOTAL, "https://cabinpkg.com/"),
            "the plan's total package quota is exhausted; \
             see https://cabinpkg.com/dashboard for current usage"
        );
        for denial in [&RATE_LIMITED, &ARCHIVE_TOO_LARGE] {
            assert_eq!(
                detail_with_usage_url(denial, "https://cabinpkg.com"),
                denial.detail,
                "code: {}",
                denial.code
            );
        }
    }

    #[test]
    fn archive_size_is_an_exact_threshold() {
        let quotas = quotas_for_plan("free");
        assert_eq!(
            check_archive_size(quotas.max_archive_bytes, &quotas),
            Ok(())
        );
        assert_eq!(
            check_archive_size(quotas.max_archive_bytes + 1, &quotas),
            Err(ARCHIVE_TOO_LARGE)
        );
    }

    fn healthy_counts() -> PublishCounts {
        PublishCounts {
            user_stored_bytes: 0,
            user_package_count: 0,
            user_new_packages_today: 0,
            package_versions_today: 0,
            package_exists: false,
        }
    }

    #[test]
    fn publish_quotas_pass_when_everything_is_under_the_limits() {
        let quotas = quotas_for_plan("free");
        assert_eq!(check_publish(1024, &healthy_counts(), &quotas), Ok(()));
    }

    #[test]
    fn storage_quota_is_exact_to_the_byte() {
        let quotas = quotas_for_plan("free");
        let counts = PublishCounts {
            user_stored_bytes: quotas.max_total_bytes_per_user - 100,
            ..healthy_counts()
        };
        // Landing exactly on the limit is allowed; one byte over is not.
        assert_eq!(check_publish(100, &counts, &quotas), Ok(()));
        assert_eq!(check_publish(101, &counts, &quotas), Err(QUOTA_STORAGE));
    }

    #[test]
    fn package_quotas_only_gate_new_packages() {
        let quotas = quotas_for_plan("free");
        let at_limits = PublishCounts {
            user_new_packages_today: quotas.max_new_packages_per_day,
            user_package_count: quotas.max_packages_total,
            package_exists: false,
            ..healthy_counts()
        };
        assert_eq!(
            check_publish(1, &at_limits, &quotas),
            Err(QUOTA_PACKAGES_DAILY)
        );
        let daily_ok = PublishCounts {
            user_new_packages_today: quotas.max_new_packages_per_day - 1,
            ..at_limits
        };
        assert_eq!(
            check_publish(1, &daily_ok, &quotas),
            Err(QUOTA_PACKAGES_TOTAL)
        );
        // The same counts pass once the package already exists.
        let existing = PublishCounts {
            package_exists: true,
            ..at_limits
        };
        assert_eq!(check_publish(1, &existing, &quotas), Ok(()));
    }

    #[test]
    fn versions_per_day_quota_is_an_exact_threshold() {
        let quotas = quotas_for_plan("free");
        let counts = PublishCounts {
            package_exists: true,
            package_versions_today: quotas.max_versions_per_package_per_day - 1,
            ..healthy_counts()
        };
        assert_eq!(check_publish(1, &counts, &quotas), Ok(()));
        let at_limit = PublishCounts {
            package_versions_today: quotas.max_versions_per_package_per_day,
            ..counts
        };
        assert_eq!(
            check_publish(1, &at_limit, &quotas),
            Err(QUOTA_VERSIONS_DAILY)
        );
    }

    #[test]
    fn storage_denial_wins_over_the_later_checks() {
        let quotas = quotas_for_plan("free");
        let counts = PublishCounts {
            user_stored_bytes: quotas.max_total_bytes_per_user,
            user_new_packages_today: quotas.max_new_packages_per_day,
            package_versions_today: quotas.max_versions_per_package_per_day,
            ..healthy_counts()
        };
        assert_eq!(check_publish(1, &counts, &quotas), Err(QUOTA_STORAGE));
    }
}
