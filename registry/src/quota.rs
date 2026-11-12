//! Per-user publish quotas: the class -> quota map, the publish token
//! bucket, and the pure enforcement checks the publish handler runs in
//! order.
//!
//! Everything here is pure and host-testable; the wasm32 glue supplies
//! the clock, the D1 counts, and the archive size. Daily windows are UTC
//! calendar days, compared lexicographically on the stored ISO 8601
//! timestamps via [`utc_day_prefix`].

/// The quotas one class grants. The map from class name to quotas lives
/// in [`quotas_for_class`]; there is deliberately no quota-class table
/// in D1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassQuotas {
    pub max_archive_bytes: u64,
    pub max_total_bytes_per_user: u64,
    pub max_new_packages_per_day: u64,
    pub max_packages_total: u64,
    pub max_versions_per_package_per_day: u64,
    /// Archive bytes one user may have admitted per UTC day, as
    /// uploaded. An admitted upload stays charged whatever becomes of
    /// it - verified, rejected, or expired.
    pub max_upload_bytes_per_day: u64,
    /// Uploads one user may have admitted per UTC day, charged like
    /// [`Self::max_upload_bytes_per_day`].
    pub max_uploads_per_day: u64,
    /// Uploads one user may have awaiting verification at once,
    /// counting those admitted but not yet stored.
    pub max_pending_uploads: u64,
    /// Archive bytes and uploads the whole class may have admitted per
    /// UTC day ([`shared_class`]): what bounds many accounts uploading
    /// at once. Raised only by editing this map.
    pub shared_upload_bytes_per_day: u64,
    pub shared_uploads_per_day: u64,
    /// Lifetime cap on *successful* scope claims, counted over the
    /// append-only `scope_claims` history: releasing or transferring a
    /// scope never restores capacity, and failed claims never consume
    /// it. An anti-squatting bound - scopes are cheap to claim and
    /// permanent, so without this one account could sweep the
    /// namespace.
    pub max_scope_claims_total: u64,
    /// Publish token bucket: burst capacity in tokens.
    pub publish_burst: f64,
    /// Publish token bucket: refill rate in tokens per minute.
    pub publish_refill_per_minute: f64,
    /// Governor fairness cap: R2-charged artifact reads (cache misses)
    /// per user per UTC day. Cache hits are free and uncounted; the
    /// cap only bounds how fast one user can drain the shared read
    /// pool, never grants allowance.
    pub artifact_reads_per_day: u64,
    /// Governor fairness cap: source-viewer ranged reads per user per
    /// UTC day (every one is an R2 read; the viewer path is uncached).
    pub source_reads_per_day: u64,
}

/// The `users.quota_class` value every account starts on, and the
/// class an anonymous caller is measured against.
pub const DEFAULT_CLASS_NAME: &str = "default";

const DEFAULT: ClassQuotas = ClassQuotas {
    max_archive_bytes: 16 * 1024 * 1024,
    max_total_bytes_per_user: 128 * 1024 * 1024,
    max_new_packages_per_day: 5,
    max_packages_total: 20,
    max_versions_per_package_per_day: 30,
    max_upload_bytes_per_day: 256 * 1024 * 1024,
    max_uploads_per_day: 30,
    max_pending_uploads: 3,
    shared_upload_bytes_per_day: 5 * 1024 * 1024 * 1024,
    shared_uploads_per_day: 500,
    max_scope_claims_total: 3,
    publish_burst: 5.0,
    publish_refill_per_minute: 1.0,
    artifact_reads_per_day: 5_000,
    source_reads_per_day: 2_000,
};

/// Bulk-publishing tier for the operator's own accounts, granted
/// manually via `users.quota_class`; the operator's own account is
/// seeded pre-promoted by the baseline migration. Sized so the
/// cabin-ports publish pipeline can seed thousands of curated ports
/// (whole library collections such as Boost) in serial runs without a
/// per-user limit binding. The storage cap and the breaker - which no
/// quota class raises - are what bound this tier's stored bytes. One
/// republish of the ports corpus sits pending all at once while the
/// verifier works through it, so the pending limit equals a day's
/// uploads; the daily byte allowances equal the storage cap, since a
/// day cannot usefully admit more than may be stored.
const OPERATOR: ClassQuotas = ClassQuotas {
    max_archive_bytes: 16 * 1024 * 1024,
    max_total_bytes_per_user: 4 * 1024 * 1024 * 1024,
    max_new_packages_per_day: 1_000,
    max_packages_total: 10_000,
    max_versions_per_package_per_day: 1_000,
    max_upload_bytes_per_day: 4 * 1024 * 1024 * 1024,
    max_uploads_per_day: 1_000,
    max_pending_uploads: 1_000,
    shared_upload_bytes_per_day: 4 * 1024 * 1024 * 1024,
    shared_uploads_per_day: 1_000,
    max_scope_claims_total: 30,
    publish_burst: 100.0,
    publish_refill_per_minute: 60.0,
    artifact_reads_per_day: 5_000,
    source_reads_per_day: 2_000,
};

/// Quotas for a `users.quota_class` value. Unknown class names get the
/// default class: deny-by-default, a typo never grants more.
pub fn quotas_for_class(class: &str) -> ClassQuotas {
    match class {
        "operator" => OPERATOR,
        _ => DEFAULT,
    }
}

/// The shared daily allowance a class's uploads are charged to: the
/// operator's own, so bulk port publishing never spends what ordinary
/// users share, and the default class's for every other name, as
/// [`quotas_for_class`] measures them.
pub fn shared_class(class: &str) -> &'static str {
    match class {
        "operator" => "operator",
        _ => DEFAULT_CLASS_NAME,
    }
}

/// Publish token-bucket state, as stored on the user row (`rl_tokens`,
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

/// Takes one publish token from `prev` (or a full bucket for a user
/// that has never published), refilling first from the elapsed time.
pub fn take_publish_token(prev: Option<Bucket>, now_ms: f64, quotas: &ClassQuotas) -> TakeOutcome {
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
/// of the same UTC calendar day starts with it, every earlier day
/// compares lexicographically below it, and every later day at or above
/// its successor `YYYY-MM-DDU`, so the SQL day window is
/// `col >= ?day AND col < substr(?day, 1, 10) || 'U'`. The upper bound
/// matters: a publish sampled just before midnight must not count rows
/// stamped after it. `None` when the input is not ISO-shaped.
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
/// The governor's per-user read-fairness refusal
/// ([`ClassQuotas::artifact_reads_per_day`],
/// [`ClassQuotas::source_reads_per_day`]); `Retry-After` carries the
/// seconds until the UTC day rolls over.
pub const READ_RATE_LIMITED: Denial = Denial {
    status: 429,
    code: "read_rate_limited",
    detail: "read rate limit exceeded; retry after the daily read allowance resets",
};
/// The pre-credential admission refusal on the two public OIDC
/// endpoints and the session mint (`glue::bearer::oidc_admission`): one
/// fixed shape for all three, decided before any credential is read, so
/// it carries no validity signal.
pub const OIDC_RATE_LIMITED: Denial = Denial {
    status: 429,
    code: "rate_limited",
    detail: "request rate limit exceeded; retry later",
};
pub const ARCHIVE_TOO_LARGE: Denial = Denial {
    status: 413,
    code: "archive_too_large",
    detail: "archive exceeds the per-archive size limit",
};
pub const QUOTA_STORAGE: Denial = Denial {
    status: 403,
    code: "quota_storage",
    detail: "publishing this archive would exceed the total storage quota",
};
pub const QUOTA_PACKAGES_DAILY: Denial = Denial {
    status: 403,
    code: "quota_packages_daily",
    detail: "daily new-package quota exhausted",
};
pub const QUOTA_PACKAGES_TOTAL: Denial = Denial {
    status: 403,
    code: "quota_packages_total",
    detail: "total package quota exhausted",
};
pub const QUOTA_VERSIONS_DAILY: Denial = Denial {
    status: 403,
    code: "quota_versions_daily",
    detail: "daily per-package version quota exhausted",
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
/// [`ARCHIVE_TOO_LARGE`] when the archive exceeds the class's cap.
pub fn check_archive_size(archive_bytes: u64, quotas: &ClassQuotas) -> Result<(), Denial> {
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
    quotas: &ClassQuotas,
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

/// What one prospective upload's admission is measured against: the
/// allowance rows of the request's UTC day and the user's uploads
/// awaiting verification.
#[derive(Debug, Clone, Copy)]
pub struct UploadUsage {
    pub user_bytes_today: u64,
    pub user_uploads_today: u64,
    pub shared_bytes_today: u64,
    pub shared_uploads_today: u64,
    /// Reservations included.
    pub pending: u64,
}

/// Which daily allowance an upload ran into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    UserBytes,
    UserCount,
    SharedBytes,
    SharedCount,
}

impl LimitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserBytes => "user_bytes",
            Self::UserCount => "user_count",
            Self::SharedBytes => "shared_bytes",
            Self::SharedCount => "shared_count",
        }
    }
}

/// An upload admission refusal (`429`). `used` is the usage before this
/// upload: an archive too large for what is left of a byte allowance is
/// refused without that allowance reading as spent.
#[derive(Debug, PartialEq, Eq)]
pub enum UploadDenial {
    /// A daily allowance; it reopens at the next UTC midnight.
    Daily {
        kind: LimitKind,
        limit: u64,
        used: u64,
    },
    /// The pending limit; it reopens when a verdict lands, which no
    /// clock predicts.
    Pending { limit: u64, used: u64 },
}

impl UploadDenial {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Daily { .. } => "daily_upload_limit_exceeded",
            Self::Pending { .. } => "pending_limit_exceeded",
        }
    }

    /// The envelope `detail`. The client prints it verbatim and reads
    /// none of the other fields, so it says what to do and when.
    pub fn detail(&self, archive_bytes: u64, reset_at: &str) -> String {
        match *self {
            Self::Daily {
                kind: LimitKind::UserBytes,
                limit,
                used,
            } => format!(
                "this {archive_bytes}-byte upload would exceed your daily upload allowance \
                 of {limit} bytes ({used} used); the allowance resets at {reset_at}"
            ),
            Self::Daily {
                kind: LimitKind::UserCount,
                limit,
                ..
            } => {
                format!("your daily allowance of {limit} uploads is spent; it resets at {reset_at}")
            }
            Self::Daily {
                kind: LimitKind::SharedBytes,
                limit,
                used,
            } => format!(
                "this {archive_bytes}-byte upload would exceed the registry's shared daily \
                 upload capacity of {limit} bytes ({used} used); the capacity resets at \
                 {reset_at}"
            ),
            Self::Daily {
                kind: LimitKind::SharedCount,
                limit,
                ..
            } => format!(
                "the registry's shared daily capacity of {limit} uploads is spent; it \
                 resets at {reset_at}"
            ),
            Self::Pending { limit, used } => format!(
                "{used} of your uploads are awaiting verification and at most {limit} may \
                 at once; publish again once one has been verified"
            ),
        }
    }

    /// The envelope's machine-readable fields beside `detail` and
    /// `code`.
    pub fn fields(&self, reset_at: &str) -> Vec<(&'static str, serde_json::Value)> {
        match *self {
            Self::Daily { kind, limit, used } => vec![
                ("limit_kind", kind.as_str().into()),
                ("limit", limit.into()),
                ("used", used.into()),
                ("reset_at", reset_at.into()),
            ],
            Self::Pending { limit, used } => {
                vec![("limit", limit.into()), ("used", used.into())]
            }
        }
    }
}

/// The daily allowance checks of a newly charged upload, the user's
/// before the class's. Limits are exact thresholds like
/// [`check_publish`]'s. With `archive_bytes` zero - before the body is
/// read - only an allowance already spent refuses.
///
/// # Errors
///
/// The first allowance the upload would exceed.
pub fn check_daily_uploads(
    archive_bytes: u64,
    usage: &UploadUsage,
    quotas: &ClassQuotas,
) -> Result<(), UploadDenial> {
    let daily = |kind, limit, used| Err(UploadDenial::Daily { kind, limit, used });
    if usage.user_bytes_today + archive_bytes > quotas.max_upload_bytes_per_day {
        return daily(
            LimitKind::UserBytes,
            quotas.max_upload_bytes_per_day,
            usage.user_bytes_today,
        );
    }
    if usage.user_uploads_today >= quotas.max_uploads_per_day {
        return daily(
            LimitKind::UserCount,
            quotas.max_uploads_per_day,
            usage.user_uploads_today,
        );
    }
    if usage.shared_bytes_today + archive_bytes > quotas.shared_upload_bytes_per_day {
        return daily(
            LimitKind::SharedBytes,
            quotas.shared_upload_bytes_per_day,
            usage.shared_bytes_today,
        );
    }
    if usage.shared_uploads_today >= quotas.shared_uploads_per_day {
        return daily(
            LimitKind::SharedCount,
            quotas.shared_uploads_per_day,
            usage.shared_uploads_today,
        );
    }
    Ok(())
}

/// The pending-limit check of an upload about to take a pending slot.
///
/// # Errors
///
/// [`UploadDenial::Pending`] when every slot is taken.
pub fn check_pending_uploads(
    usage: &UploadUsage,
    quotas: &ClassQuotas,
) -> Result<(), UploadDenial> {
    if usage.pending >= quotas.max_pending_uploads {
        return Err(UploadDenial::Pending {
            limit: quotas.max_pending_uploads,
            used: usage.pending,
        });
    }
    Ok(())
}

/// The fairness window a charged artifact read is billed to, as a
/// `(principal, cap)` pair for the governor's per-principal daily
/// window.
///
/// Tokened callers key on their registry user id.  Anonymous callers -
/// the read plane is public - key on the client IP Cloudflare stamps on
/// every edge request (`CF-Connecting-IP`, which the edge overwrites,
/// so it is not caller-controlled).  They must be bounded by
/// *something*: the deployed WAF rate limiter deliberately covers only
/// the website origin's write/auth surface, never
/// `registry.cabinpkg.com` reads ([`runbook.md`](../docs/runbook.md),
/// "Zone rate limiting (WAF)"), so without this one unauthenticated
/// caller could drain the shared `b_ordinary` pool and 503 everyone
/// else's uncached downloads.
///
/// A request with no client-IP header (local dev; the edge always
/// sets it in production) falls back to one shared anonymous window
/// rather than no window at all - bounding total anonymous drain is
/// the fail-safe direction.
#[must_use]
pub fn artifact_read_fairness(
    user_id: Option<i64>,
    quotas: &ClassQuotas,
    client_ip: Option<&str>,
) -> (String, u64) {
    if let Some(user_id) = user_id {
        return (user_id.to_string(), quotas.artifact_reads_per_day);
    }
    let principal = match client_ip {
        Some(ip) if !ip.is_empty() => format!("ip:{ip}"),
        _ => "ip:unknown".to_owned(),
    };
    (principal, DEFAULT.artifact_reads_per_day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_classes_fall_back_to_the_default() {
        assert_eq!(quotas_for_class("default"), DEFAULT);
        assert_eq!(quotas_for_class("enterprise"), DEFAULT);
        assert_eq!(quotas_for_class(""), DEFAULT);
        // Case-sensitive on purpose: a hand-typed 'Operator' must not
        // silently grant the bulk tier.
        assert_eq!(quotas_for_class("Operator"), DEFAULT);
    }

    /// The cabin-ports pipeline seeding an empty registry serially at
    /// one publish a second: a thousand new packages in one UTC day go
    /// through without a single 429 or 403, and only the next new
    /// package is refused.
    #[test]
    fn the_operator_class_seeds_a_thousand_new_ports_in_a_day() {
        const ARCHIVE_BYTES: u64 = 1024 * 1024;
        let quotas = quotas_for_class("operator");
        let mut bucket = None;
        let mut now_ms = 0.0;
        let mut counts = healthy_counts();
        for n in 0..1_000 {
            let take = take_publish_token(bucket, now_ms, &quotas);
            assert!(take.allowed, "publish {n} was rate limited");
            bucket = Some(take.bucket);
            assert_eq!(
                check_publish(ARCHIVE_BYTES, &counts, &quotas),
                Ok(()),
                "publish {n}"
            );
            counts.user_stored_bytes += ARCHIVE_BYTES;
            counts.user_package_count += 1;
            counts.user_new_packages_today += 1;
            now_ms += 1_000.0;
        }
        assert_eq!(
            check_publish(ARCHIVE_BYTES, &counts, &quotas),
            Err(QUOTA_PACKAGES_DAILY)
        );
        // The curated set publishes under the single `cabin-ports`
        // scope, but the bulk tier keeps claim headroom beyond the
        // default anti-squatting cap.
        assert!(quotas.max_scope_claims_total > DEFAULT.max_scope_claims_total);
    }

    /// The operator's other raised limits at their exact boundaries: the
    /// 10,000th package, the 1,000th version of one package in a day,
    /// and the byte that lands on 4 GiB are allowed; one more is not.
    #[test]
    fn the_operator_limits_are_exact_at_the_raised_boundaries() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let quotas = quotas_for_class("operator");
        let packages = |user_package_count| PublishCounts {
            user_package_count,
            ..healthy_counts()
        };
        assert_eq!(check_publish(1, &packages(9_999), &quotas), Ok(()));
        assert_eq!(
            check_publish(1, &packages(10_000), &quotas),
            Err(QUOTA_PACKAGES_TOTAL)
        );
        let versions = |package_versions_today| PublishCounts {
            package_exists: true,
            package_versions_today,
            ..healthy_counts()
        };
        assert_eq!(check_publish(1, &versions(999), &quotas), Ok(()));
        assert_eq!(
            check_publish(1, &versions(1_000), &quotas),
            Err(QUOTA_VERSIONS_DAILY)
        );
        let stored = PublishCounts {
            user_stored_bytes: 4 * GIB - 100,
            ..healthy_counts()
        };
        assert_eq!(check_publish(100, &stored, &quotas), Ok(()));
        assert_eq!(check_publish(101, &stored, &quotas), Err(QUOTA_STORAGE));
    }

    #[test]
    fn fresh_bucket_allows_a_full_burst_then_denies() {
        // The empty bucket's Retry-After is one token's refill: 60 s at
        // 1 token/minute, 1 s at 60.
        for (class, burst, retry_after_secs) in [("default", 5, 60), ("operator", 100, 1)] {
            let quotas = quotas_for_class(class);
            let mut bucket = None;
            for take in 0..burst {
                let outcome = take_publish_token(bucket, 1_000.0, &quotas);
                assert!(outcome.allowed, "{class} take {take}");
                bucket = Some(outcome.bucket);
            }
            let denied = take_publish_token(bucket, 1_000.0, &quotas);
            assert!(!denied.allowed, "{class}");
            assert_eq!(denied.retry_after_secs, retry_after_secs, "{class}");
        }
    }

    #[test]
    fn bucket_refills_at_the_class_rate_and_caps_at_burst() {
        let quotas = quotas_for_class("default");
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
        let quotas = quotas_for_class("default");
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
        // ... and the next day compares at or above the successor.
        assert!("2026-07-09T23:59:59.999Z" < "2026-07-09U");
        assert!("2026-07-10T00:00:00.000Z" >= "2026-07-09U");
        assert_eq!(utc_day_prefix("not a timestamp"), None);
        assert_eq!(utc_day_prefix("2026-07-09"), None);
        assert_eq!(utc_day_prefix(""), None);
    }

    #[test]
    fn quota_denials_embed_the_dashboard_url_and_others_do_not() {
        assert_eq!(
            detail_with_usage_url(&QUOTA_STORAGE, "https://cabinpkg.com"),
            "publishing this archive would exceed the total storage quota; \
             see https://cabinpkg.com/dashboard for current usage"
        );
        // A trailing slash on the env var never doubles the separator.
        assert_eq!(
            detail_with_usage_url(&QUOTA_PACKAGES_TOTAL, "https://cabinpkg.com/"),
            "total package quota exhausted; \
             see https://cabinpkg.com/dashboard for current usage"
        );
        for denial in [&RATE_LIMITED, &OIDC_RATE_LIMITED, &ARCHIVE_TOO_LARGE] {
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
        let quotas = quotas_for_class("default");
        assert_eq!(
            check_archive_size(quotas.max_archive_bytes, &quotas),
            Ok(())
        );
        assert_eq!(
            check_archive_size(quotas.max_archive_bytes + 1, &quotas),
            Err(ARCHIVE_TOO_LARGE)
        );
    }

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

    fn no_usage() -> UploadUsage {
        UploadUsage {
            user_bytes_today: 0,
            user_uploads_today: 0,
            shared_bytes_today: 0,
            shared_uploads_today: 0,
            pending: 0,
        }
    }

    #[test]
    fn the_daily_upload_allowances_are_exact_thresholds() {
        let quotas = quotas_for_class("default");
        assert_eq!(
            (
                quotas.max_upload_bytes_per_day,
                quotas.max_uploads_per_day,
                quotas.max_pending_uploads,
                quotas.shared_upload_bytes_per_day,
                quotas.shared_uploads_per_day,
            ),
            (256 * MIB, 30, 3, 5 * GIB, 500)
        );
        let daily = |kind, limit, used| Err(UploadDenial::Daily { kind, limit, used });

        // The byte that lands on an allowance is admitted, the next is not,
        // and the refusal reports the usage before the refused upload.
        let user_bytes = UploadUsage {
            user_bytes_today: 256 * MIB - 100,
            ..no_usage()
        };
        assert_eq!(check_daily_uploads(100, &user_bytes, &quotas), Ok(()));
        assert_eq!(
            check_daily_uploads(101, &user_bytes, &quotas),
            daily(LimitKind::UserBytes, 256 * MIB, 256 * MIB - 100)
        );
        let shared_bytes = UploadUsage {
            shared_bytes_today: 5 * GIB - 100,
            ..no_usage()
        };
        assert_eq!(check_daily_uploads(100, &shared_bytes, &quotas), Ok(()));
        assert_eq!(
            check_daily_uploads(101, &shared_bytes, &quotas),
            daily(LimitKind::SharedBytes, 5 * GIB, 5 * GIB - 100)
        );

        // The thirtieth and the five-hundredth upload are admitted.
        let user_count = |user_uploads_today| UploadUsage {
            user_uploads_today,
            ..no_usage()
        };
        assert_eq!(check_daily_uploads(1, &user_count(29), &quotas), Ok(()));
        assert_eq!(
            check_daily_uploads(1, &user_count(30), &quotas),
            daily(LimitKind::UserCount, 30, 30)
        );
        let shared_count = |shared_uploads_today| UploadUsage {
            shared_uploads_today,
            ..no_usage()
        };
        assert_eq!(check_daily_uploads(1, &shared_count(499), &quotas), Ok(()));
        assert_eq!(
            check_daily_uploads(1, &shared_count(500), &quotas),
            daily(LimitKind::SharedCount, 500, 500)
        );

        // Before the body is read, only an allowance already spent refuses.
        assert_eq!(check_daily_uploads(0, &user_bytes, &quotas), Ok(()));
        assert!(check_daily_uploads(0, &user_count(30), &quotas).is_err());

        let pending = |pending| UploadUsage {
            pending,
            ..no_usage()
        };
        assert_eq!(check_pending_uploads(&pending(2), &quotas), Ok(()));
        assert_eq!(
            check_pending_uploads(&pending(3), &quotas),
            Err(UploadDenial::Pending { limit: 3, used: 3 })
        );
    }

    /// The operator's allowances are its own row and its own values:
    /// spending the ordinary users' whole shared day leaves it untouched.
    #[test]
    fn the_operator_class_has_its_own_shared_allowance() {
        assert_eq!(shared_class("operator"), "operator");
        for ordinary in ["default", "enterprise", "", "Operator"] {
            assert_eq!(shared_class(ordinary), "default", "{ordinary:?}");
        }
        let quotas = quotas_for_class("operator");
        assert_eq!(
            (
                quotas.max_upload_bytes_per_day,
                quotas.max_uploads_per_day,
                quotas.max_pending_uploads,
                quotas.shared_upload_bytes_per_day,
                quotas.shared_uploads_per_day,
            ),
            (4 * GIB, 1_000, 1_000, 4 * GIB, 1_000)
        );
        // A whole republish of the ports corpus waits for the verifier
        // at once.
        let republish = UploadUsage {
            user_bytes_today: 200 * MIB,
            user_uploads_today: 319,
            shared_bytes_today: 200 * MIB,
            shared_uploads_today: 319,
            pending: 319,
        };
        assert_eq!(check_daily_uploads(MIB, &republish, &quotas), Ok(()));
        assert_eq!(check_pending_uploads(&republish, &quotas), Ok(()));
    }

    #[test]
    fn upload_denials_render_the_contract_fields() {
        let reset_at = "2026-11-12T00:00:00Z";
        let daily = UploadDenial::Daily {
            kind: LimitKind::SharedBytes,
            limit: 5 * GIB,
            used: 5 * GIB - 100,
        };
        assert_eq!(daily.code(), "daily_upload_limit_exceeded");
        assert_eq!(
            serde_json::Value::Object(
                daily
                    .fields(reset_at)
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect()
            ),
            serde_json::json!({
                "limit_kind": "shared_bytes",
                "limit": 5 * GIB,
                "used": 5 * GIB - 100,
                "reset_at": reset_at,
            })
        );
        // The detail is what the client shows: it names the reopening
        // time, and does not call a partly used allowance spent.
        let detail = daily.detail(101, reset_at);
        assert!(detail.contains(reset_at), "{detail}");
        assert!(detail.contains("101-byte"), "{detail}");
        assert!(!detail.contains("spent"), "{detail}");

        let pending = UploadDenial::Pending { limit: 3, used: 3 };
        assert_eq!(pending.code(), "pending_limit_exceeded");
        let fields = pending.fields(reset_at);
        assert_eq!(
            fields.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            ["limit", "used"]
        );
        // A pending slot does not reopen at midnight.
        assert!(!pending.detail(1, reset_at).contains(reset_at));
        for kind in [
            LimitKind::UserBytes,
            LimitKind::UserCount,
            LimitKind::SharedBytes,
            LimitKind::SharedCount,
        ] {
            let denial = UploadDenial::Daily {
                kind,
                limit: 1,
                used: 1,
            };
            assert!(denial.detail(1, reset_at).contains(reset_at), "{kind:?}");
            // A detail is one line of prose: a lost line continuation
            // would leave the source's indentation in it.
            assert!(!denial.detail(1, reset_at).contains("  "), "{kind:?}");
        }
        assert!(!pending.detail(1, reset_at).contains("  "));
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
        let quotas = quotas_for_class("default");
        assert_eq!(check_publish(1024, &healthy_counts(), &quotas), Ok(()));
    }

    #[test]
    fn storage_quota_is_exact_to_the_byte() {
        let quotas = quotas_for_class("default");
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
        let quotas = quotas_for_class("default");
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
    fn total_package_quota_is_an_exact_threshold() {
        let quotas = quotas_for_class("default");
        // One below the limit: creating the package that lands exactly
        // on the limit is allowed, the next one is not.
        let one_below = PublishCounts {
            user_package_count: quotas.max_packages_total - 1,
            ..healthy_counts()
        };
        assert_eq!(check_publish(1, &one_below, &quotas), Ok(()));
        let at_limit = PublishCounts {
            user_package_count: quotas.max_packages_total,
            ..healthy_counts()
        };
        assert_eq!(
            check_publish(1, &at_limit, &quotas),
            Err(QUOTA_PACKAGES_TOTAL)
        );
        // A user already over the limit (e.g. after a quota reduction)
        // still publishes new versions of existing packages.
        let over_limit_existing = PublishCounts {
            user_package_count: quotas.max_packages_total + 5,
            package_exists: true,
            ..healthy_counts()
        };
        assert_eq!(check_publish(1, &over_limit_existing, &quotas), Ok(()));
    }

    #[test]
    fn versions_per_day_quota_is_an_exact_threshold() {
        let quotas = quotas_for_class("default");
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
        let quotas = quotas_for_class("default");
        let counts = PublishCounts {
            user_stored_bytes: quotas.max_total_bytes_per_user,
            user_new_packages_today: quotas.max_new_packages_per_day,
            package_versions_today: quotas.max_versions_per_package_per_day,
            ..healthy_counts()
        };
        assert_eq!(check_publish(1, &counts, &quotas), Err(QUOTA_STORAGE));
    }

    /// Anonymous readers must still land in a per-caller window: the
    /// WAF rate limiter does not cover the read host, so an unbounded
    /// anonymous miss path would let one caller drain the shared pool.
    #[test]
    fn anonymous_artifact_reads_key_on_the_client_ip() {
        let quotas = quotas_for_class("operator");
        let (principal, cap) = artifact_read_fairness(None, &quotas, Some("203.0.113.7"));
        assert_eq!(principal, "ip:203.0.113.7");
        // Anonymous callers never inherit a promoted class's cap - the
        // class belongs to an account, and there is no account here.
        assert_eq!(cap, DEFAULT.artifact_reads_per_day);
    }

    /// A tokened caller keys on its user id and its own class cap.
    #[test]
    fn tokened_artifact_reads_key_on_the_user() {
        let quotas = quotas_for_class("operator");
        let (principal, cap) = artifact_read_fairness(Some(42), &quotas, Some("203.0.113.7"));
        assert_eq!(principal, "42");
        assert_eq!(cap, quotas.artifact_reads_per_day);
    }

    /// No client-IP header (local dev): one shared anonymous window,
    /// never an unbounded path.
    #[test]
    fn a_missing_client_ip_still_lands_in_a_window() {
        let quotas = quotas_for_class("default");
        for ip in [None, Some("")] {
            let (principal, cap) = artifact_read_fairness(None, &quotas, ip);
            assert_eq!(principal, "ip:unknown");
            assert_eq!(cap, quotas.artifact_reads_per_day);
        }
    }
}

/// Kani proof harness for [`check_publish`] (`cargo kani` from
/// `registry/`; the module is invisible to every other build). The
/// specification below is written from the documented policy, not from
/// the function body, and compares bytes in `u128` so the storage limit
/// is the mathematical sum, never the `u64` expression under proof.
#[cfg(kani)]
mod verification {
    use super::*;

    /// Any quota class, not just the two the map grants: the proof must
    /// not depend on the constants' sizes.
    fn any_quotas() -> ClassQuotas {
        ClassQuotas {
            max_archive_bytes: kani::any(),
            max_total_bytes_per_user: kani::any(),
            max_new_packages_per_day: kani::any(),
            max_packages_total: kani::any(),
            max_versions_per_package_per_day: kani::any(),
            max_scope_claims_total: kani::any(),
            publish_burst: kani::any(),
            publish_refill_per_minute: kani::any(),
            artifact_reads_per_day: kani::any(),
            source_reads_per_day: kani::any(),
        }
    }

    /// Every count the publish handler passes is a D1 `INTEGER` - an
    /// aggregate or a user-row tally - mapped through
    /// `glue::non_negative` (`i64 -> u64`, so at most `i64::MAX`).
    fn any_d1_count() -> u64 {
        let n: u64 = kani::any();
        kani::assume(n <= i64::MAX as u64);
        n
    }

    /// The publish handler's actual input domain: the archive is a slice
    /// of a body `publish::decode_frame` already capped at
    /// [`crate::publish::MAX_BODY_BYTES`], and each count is a D1
    /// aggregate. Quotas and `package_exists` are unconstrained.
    // Comparing a `Denial` compares its `&str`s, a `memcmp` CBMC cannot
    // bound on its own: the longest is 60 bytes, and the unwinding
    // assertion fails verification if this bound ever gets too small.
    #[kani::proof]
    #[kani::unwind(64)]
    fn check_publish_is_the_documented_policy() {
        let quotas = any_quotas();
        let archive_bytes: u64 = kani::any();
        kani::assume(archive_bytes <= crate::publish::MAX_BODY_BYTES as u64);
        let counts = PublishCounts {
            user_stored_bytes: any_d1_count(),
            user_package_count: any_d1_count(),
            user_new_packages_today: any_d1_count(),
            package_versions_today: any_d1_count(),
            package_exists: kani::any(),
        };

        // The policy as [`check_publish`]'s contract states it: exact
        // thresholds, package quotas only for a new package, denials in
        // the documented order.
        let stored_after = u128::from(counts.user_stored_bytes) + u128::from(archive_bytes);
        let storage_ok = stored_after <= u128::from(quotas.max_total_bytes_per_user);
        let daily_packages_ok = counts.user_new_packages_today < quotas.max_new_packages_per_day;
        let total_packages_ok = counts.user_package_count < quotas.max_packages_total;
        let versions_ok = counts.package_versions_today < quotas.max_versions_per_package_per_day;
        let new_package = !counts.package_exists;

        let result = check_publish(archive_bytes, &counts, &quotas);

        match result {
            // Soundness: success never authorizes a publish over any
            // applicable limit.
            Ok(()) => {
                assert!(storage_ok, "success over the storage quota");
                assert!(versions_ok, "success over the daily version quota");
                assert!(
                    counts.package_exists || (daily_packages_ok && total_packages_ok),
                    "a new package over a package quota"
                );
            }
            // Every denial names a limit that is really exceeded, keeps
            // the documented precedence, and package denials only hit
            // new packages.
            Err(QUOTA_STORAGE) => {
                assert!(!storage_ok);
            }
            Err(QUOTA_PACKAGES_DAILY) => {
                assert!(new_package && !daily_packages_ok && storage_ok);
            }
            Err(QUOTA_PACKAGES_TOTAL) => {
                assert!(new_package && !total_packages_ok && storage_ok && daily_packages_ok);
            }
            Err(QUOTA_VERSIONS_DAILY) => {
                assert!(!versions_ok && storage_ok);
                assert!(counts.package_exists || (daily_packages_ok && total_packages_ok));
            }
            Err(other) => panic!("undocumented denial {other:?}"),
        }
        // Completeness: a publish within every applicable limit is
        // allowed.
        if storage_ok
            && versions_ok
            && (counts.package_exists || (daily_packages_ok && total_packages_ok))
        {
            assert_eq!(result, Ok(()));
        }

        // Non-vacuity: the domain reaches each outcome, each exact
        // boundary on both sides, and the edges of the input domain.
        // Kani stays green on an unsatisfied cover; CI's
        // `cargo check-kani-report` fails the run on one.
        kani::cover!(result == Ok(()));
        kani::cover!(result == Err(QUOTA_STORAGE));
        kani::cover!(result == Err(QUOTA_PACKAGES_DAILY));
        kani::cover!(result == Err(QUOTA_PACKAGES_TOTAL));
        kani::cover!(result == Err(QUOTA_VERSIONS_DAILY));
        kani::cover!(
            stored_after == u128::from(quotas.max_total_bytes_per_user) && result == Ok(())
        );
        kani::cover!(
            stored_after == u128::from(quotas.max_total_bytes_per_user) + 1
                && result == Err(QUOTA_STORAGE)
        );
        kani::cover!(
            new_package
                && counts.user_new_packages_today + 1 == quotas.max_new_packages_per_day
                && result == Ok(())
        );
        kani::cover!(
            new_package
                && counts.user_new_packages_today == quotas.max_new_packages_per_day
                && result == Err(QUOTA_PACKAGES_DAILY)
        );
        kani::cover!(
            new_package
                && counts.user_package_count + 1 == quotas.max_packages_total
                && result == Ok(())
        );
        kani::cover!(
            new_package
                && counts.user_package_count == quotas.max_packages_total
                && result == Err(QUOTA_PACKAGES_TOTAL)
        );
        kani::cover!(
            counts.package_versions_today + 1 == quotas.max_versions_per_package_per_day
                && result == Ok(())
        );
        kani::cover!(
            counts.package_versions_today == quotas.max_versions_per_package_per_day
                && result == Err(QUOTA_VERSIONS_DAILY)
        );
        // An existing package publishes past both package quotas.
        kani::cover!(
            counts.package_exists && !daily_packages_ok && !total_packages_ok && result == Ok(())
        );
        kani::cover!(
            archive_bytes == crate::publish::MAX_BODY_BYTES as u64
                && counts.user_stored_bytes == i64::MAX as u64
        );
    }

    /// The production clock (`glue::now_epoch_ms`) is an integer count of
    /// Unix epoch milliseconds cast to `f64`, exact below 2^53; bounding
    /// it at 2^52 (year 144,683) keeps the Retry-After addition exact too.
    #[allow(clippy::cast_precision_loss)]
    fn any_epoch_ms() -> f64 {
        let ms: u64 = kani::any();
        kani::assume(ms <= 1 << 52);
        ms as f64
    }

    /// The bucket's production input domain. The class is one of the two
    /// the map grants. A stored bucket was written by an *allowed* take
    /// of this very function (`glue::bearer::package::cas_bucket`; NULL
    /// columns, the documented operator reset, read as `None`), so its
    /// balance is a non-negative balance under some class and its
    /// timestamp a past clock reading - the balance bound is what the
    /// proof below establishes for the next write, the inductive
    /// invariant. A row edited by hand to other values is outside the
    /// domain.
    #[kani::proof]
    fn take_publish_token_is_the_documented_bucket() {
        let quotas = if kani::any() { OPERATOR } else { DEFAULT };
        let now_ms = any_epoch_ms();
        let prev = if kani::any() {
            let tokens: f64 = kani::any();
            kani::assume(
                tokens >= 0.0 && tokens <= DEFAULT.publish_burst.max(OPERATOR.publish_burst),
            );
            Some(Bucket {
                tokens,
                updated_at_ms: any_epoch_ms(),
            })
        } else {
            None
        };

        let outcome = take_publish_token(prev, now_ms, &quotas);

        // The returned state is stamped with this take's clock, and the
        // balance is always a finite number (CBMC's NaN check covers the
        // arithmetic itself).
        assert!(outcome.bucket.updated_at_ms == now_ms);
        assert!(outcome.bucket.tokens.is_finite());

        match prev {
            // A user who has never published gets exactly a full burst
            // minus this take.
            None => {
                assert!(outcome.allowed);
                assert!(outcome.bucket.tokens == quotas.publish_burst - 1.0);
            }
            Some(prev) => {
                // Policy, in reals: refill never lowers a balance, the
                // balance never exceeds the burst, and one take costs
                // one token. The balance a demoted class inherits above
                // its burst is clamped first.
                let carried = prev.tokens.min(quotas.publish_burst);
                if outcome.allowed {
                    assert!(outcome.bucket.tokens >= carried - 1.0, "time lost tokens");
                    assert!(
                        outcome.bucket.tokens <= quotas.publish_burst - 1.0,
                        "over burst"
                    );
                    assert!(outcome.bucket.tokens >= 0.0);
                    // No elapsed time (or a clock that went backwards):
                    // no refill, and the take is exact in IEEE 754.
                    if now_ms <= prev.updated_at_ms {
                        assert!(outcome.bucket.tokens == carried - 1.0);
                    }
                } else {
                    // A bucket holding a whole token is never refused.
                    assert!(carried < 1.0, "refused with a token in hand");
                    assert!(outcome.bucket.tokens < 1.0);
                }
            }
        }

        if outcome.allowed {
            assert!(outcome.retry_after_secs == 0);
        } else {
            // Retry-After: at least a second, at most one whole token's
            // refill. Sufficiency - that the same stored bucket (a denial
            // persists nothing) taken again once the wait has elapsed is
            // allowed - is deliberately not asserted: proving it costs
            // CBMC several minutes on the float arithmetic (8-15 min
            // here), and it is false by one ulp in rare cases anyway, see
            // the cover below.
            assert!(outcome.retry_after_secs >= 1);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let one_token_secs = (60.0 / quotas.publish_refill_per_minute) as u64;
            assert!(outcome.retry_after_secs <= one_token_secs);
            // In reals, the advertised wait at the class rate refills at
            // least the missing fraction of a token (rounding slack far
            // below one refill second).
            #[allow(clippy::cast_precision_loss)]
            let retry_secs = outcome.retry_after_secs as f64;
            assert!(
                outcome.bucket.tokens + retry_secs / 60.0 * quotas.publish_refill_per_minute
                    >= 1.0 - 1e-12,
                "Retry-After does not cover the missing token"
            );
            let retry_ms = retry_secs * 1000.0;
            let retried = take_publish_token(prev, now_ms + retry_ms, &quotas);
            // The rounding shortfall is real: a balance a few ulps under
            // 1/60, retried exactly 59 s later with the same clock,
            // refills to 0.9999999999999999 and is refused once more
            // with `Retry-After: 1`. Fail-safe, and below a second.
            kani::cover!(!retried.allowed && prev.is_some_and(|prev| now_ms >= prev.updated_at_ms));
        }

        // Non-vacuity (reported, never enforced): the fresh bucket, both
        // verdicts, refill, the burst cap, both Retry-After extremes of
        // each class (the operator's coincide: one token refills in a
        // second), and the backwards clock.
        kani::cover!(outcome.allowed && prev.is_none());
        kani::cover!(outcome.allowed && prev.is_some_and(|prev| now_ms > prev.updated_at_ms));
        kani::cover!(
            outcome.allowed
                && prev.is_some_and(|prev| {
                    prev.tokens < quotas.publish_burst - 1.0
                        && outcome.bucket.tokens == quotas.publish_burst - 1.0
                })
        );
        kani::cover!(outcome.allowed && prev.is_some_and(|prev| now_ms < prev.updated_at_ms));
        kani::cover!(!outcome.allowed && outcome.retry_after_secs == 1);
        kani::cover!(
            !outcome.allowed
                && outcome.retry_after_secs == 60
                && quotas.publish_burst == DEFAULT.publish_burst
        );
        kani::cover!(
            !outcome.allowed
                && outcome.retry_after_secs == 1
                && quotas.publish_burst == OPERATOR.publish_burst
        );
        kani::cover!(!outcome.allowed && prev.is_some_and(|prev| now_ms < prev.updated_at_ms));
    }

    /// The refill *rate*: the harness above bounds a balance but not how
    /// fast it grows (a symbolic elapsed time costs CBMC many minutes), so
    /// this pins the documented tokens-per-minute at exactly one minute
    /// with a concrete clock - a bounded check of the rate, not a proof
    /// over every elapsed time.
    #[kani::proof]
    fn take_publish_token_refills_the_class_rate_per_minute() {
        let quotas = if kani::any() { OPERATOR } else { DEFAULT };
        let tokens: f64 = kani::any();
        kani::assume(tokens >= 0.0 && tokens <= quotas.publish_burst - 1.0);
        let prev = Bucket {
            tokens,
            updated_at_ms: 0.0,
        };
        let outcome = take_publish_token(Some(prev), 60_000.0, &quotas);
        // One minute refills one rate's worth (both classes refill at
        // least a token a minute), capped at the burst, less this take.
        let expected = (tokens + quotas.publish_refill_per_minute).min(quotas.publish_burst) - 1.0;
        assert!(outcome.allowed);
        assert!(
            (outcome.bucket.tokens - expected).abs() <= 1e-12,
            "wrong refill rate"
        );
        kani::cover!(outcome.bucket.tokens == quotas.publish_burst - 1.0);
        kani::cover!(outcome.bucket.tokens < quotas.publish_burst - 1.0);
    }
}
