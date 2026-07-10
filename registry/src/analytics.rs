//! Cloudflare GraphQL Analytics API shapes for the budget breaker's cron
//! pass: dataset names, query construction, and response parsing. Pure and
//! host-testable; the wasm32 glue performs the HTTP calls.
//!
//! Dataset and field names below are external contracts - verify them
//! against the current Cloudflare GraphQL Analytics documentation
//! (<https://developers.cloudflare.com/analytics/graphql-api/>) when a
//! query starts coming back rejected. The cron path degrades gracefully:
//! a rejected dataset just yields `None` for that metric.

use serde_json::Value;

pub const GRAPHQL_ENDPOINT: &str = "https://api.cloudflare.com/client/v4/graphql";

/// Workers request counts (account-wide, matching the account-wide free
/// limit), summed as `sum.requests`.
pub const WORKERS_DATASET: &str = "workersInvocationsAdaptive";
/// R2 operation counts by `actionType`, summed as `sum.requests`.
pub const R2_DATASET: &str = "r2OperationsAdaptiveGroups";
/// D1 query analytics, summed as `sum.rowsRead`.
pub const D1_DATASET: &str = "d1AnalyticsAdaptiveGroups";

/// The R2 action types billed as Class A operations - the only R2 ops
/// with real overage exposure. Class B (reads) is not budgeted: its free
/// allowance is 10x larger and the read path is D1-gated anyway.
pub const R2_CLASS_A_ACTIONS: &[&str] = &[
    "ListBuckets",
    "PutBucket",
    "ListObjects",
    "PutObject",
    "CopyObject",
    "CompleteMultipartUpload",
    "CreateMultipartUpload",
    "ListMultipartUploads",
    "UploadPart",
    "UploadPartCopy",
    "LifecycleStorageTierTransition",
];

/// Guards values interpolated into a GraphQL document: account tags are
/// hex, timestamps are ISO 8601. Anything else never reaches the API.
fn graphql_safe(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'.' | b'T' | b'Z'))
}

/// Wraps one dataset selection in the account-scoped query envelope and
/// serializes it as the POST body.
fn query_body(account: &str, selection: &str) -> String {
    serde_json::json!({
        "query": format!(
            "query {{ viewer {{ accounts(filter: {{accountTag: \"{account}\"}}) {{ {selection} }} }} }}"
        )
    })
    .to_string()
}

/// POST body for today's account-wide Workers request count.
pub fn workers_requests_query(account: &str, day_start_iso: &str) -> Option<String> {
    (graphql_safe(account) && graphql_safe(day_start_iso)).then(|| {
        query_body(
            account,
            &format!(
                "{WORKERS_DATASET}(limit: 100, filter: {{datetime_geq: \"{day_start_iso}\"}}) \
                 {{ sum {{ requests }} }}"
            ),
        )
    })
}

/// POST body for this calendar month's Class A R2 operation count.
pub fn r2_class_a_query(account: &str, month_start_iso: &str) -> Option<String> {
    (graphql_safe(account) && graphql_safe(month_start_iso)).then(|| {
        let actions = R2_CLASS_A_ACTIONS
            .iter()
            .map(|action| format!("\"{action}\""))
            .collect::<Vec<_>>()
            .join(", ");
        query_body(
            account,
            &format!(
                "{R2_DATASET}(limit: 100, filter: {{datetime_geq: \"{month_start_iso}\", \
                 actionType_in: [{actions}]}}) {{ sum {{ requests }} }}"
            ),
        )
    })
}

/// POST body for today's D1 rows-read count.
pub fn d1_rows_read_query(account: &str, date: &str) -> Option<String> {
    (graphql_safe(account) && graphql_safe(date)).then(|| {
        query_body(
            account,
            &format!(
                "{D1_DATASET}(limit: 100, filter: {{date_geq: \"{date}\"}}) \
                 {{ sum {{ rowsRead }} }}"
            ),
        )
    })
}

/// Extracts and sums `sum.<metric>` across a dataset's groups from one
/// GraphQL response body. `None` on GraphQL errors, a missing dataset, or
/// a shape mismatch - the caller treats that metric as unavailable. An
/// empty group list is genuine zero usage.
pub fn parse_sum(body: &str, dataset: &str, metric: &str) -> Option<u64> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value
        .get("errors")
        .is_some_and(|errors| errors.as_array().is_some_and(|list| !list.is_empty()))
    {
        return None;
    }
    let groups = value
        .get("data")?
        .get("viewer")?
        .get("accounts")?
        .as_array()?
        .first()?
        .get(dataset)?
        .as_array()?;
    let mut total: u64 = 0;
    for group in groups {
        total = total.checked_add(group.get("sum")?.get(metric)?.as_u64()?)?;
    }
    Some(total)
}

/// `YYYY-MM-DDT00:00:00Z` for the UTC day of an ISO 8601 timestamp.
pub fn utc_day_start(now_iso: &str) -> Option<String> {
    crate::quota::utc_day_prefix(now_iso).map(|prefix| format!("{prefix}00:00:00Z"))
}

/// `YYYY-MM-01T00:00:00Z` for the UTC month of an ISO 8601 timestamp.
pub fn utc_month_start(now_iso: &str) -> Option<String> {
    let month = now_iso.get(..7)?;
    (crate::quota::utc_day_prefix(now_iso).is_some()).then(|| format!("{month}-01T00:00:00Z"))
}

/// `YYYY-MM-DD` for the UTC day of an ISO 8601 timestamp.
pub fn utc_date(now_iso: &str) -> Option<&str> {
    crate::quota::utc_day_prefix(now_iso).map(|prefix| &prefix[..10])
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-07-09T12:34:56.789Z";

    #[test]
    fn window_helpers_slice_the_utc_timestamp() {
        assert_eq!(utc_day_start(NOW).as_deref(), Some("2026-07-09T00:00:00Z"));
        assert_eq!(
            utc_month_start(NOW).as_deref(),
            Some("2026-07-01T00:00:00Z")
        );
        assert_eq!(utc_date(NOW), Some("2026-07-09"));
        for helper in [utc_day_start, utc_month_start] {
            assert_eq!(helper("garbage"), None);
        }
        assert_eq!(utc_date("garbage"), None);
    }

    #[test]
    fn queries_embed_the_account_window_and_dataset() {
        let body = workers_requests_query("abc123", "2026-07-09T00:00:00Z").unwrap();
        assert!(body.contains(r#"accountTag: \"abc123\""#), "body: {body}");
        assert!(body.contains(WORKERS_DATASET), "body: {body}");
        assert!(
            body.contains(r#"datetime_geq: \"2026-07-09T00:00:00Z\""#),
            "body: {body}"
        );

        let body = r2_class_a_query("abc123", "2026-07-01T00:00:00Z").unwrap();
        assert!(body.contains(R2_DATASET), "body: {body}");
        assert!(body.contains(r#"\"PutObject\""#), "body: {body}");
        assert!(!body.contains("GetObject"), "body: {body}");

        let body = d1_rows_read_query("abc123", "2026-07-09").unwrap();
        assert!(body.contains(D1_DATASET), "body: {body}");
        assert!(body.contains("rowsRead"), "body: {body}");
    }

    #[test]
    fn queries_refuse_hostile_interpolations() {
        for hostile in ["", "a\"b", "a{b}", "a b"] {
            assert_eq!(
                workers_requests_query(hostile, "2026-07-09T00:00:00Z"),
                None
            );
            assert_eq!(r2_class_a_query("abc", hostile), None);
            assert_eq!(d1_rows_read_query(hostile, "2026-07-09"), None);
        }
    }

    fn response(dataset: &str, groups: &str) -> String {
        format!(
            r#"{{"data":{{"viewer":{{"accounts":[{{"{dataset}":[{groups}]}}]}}}},"errors":null}}"#
        )
    }

    #[test]
    fn parse_sum_totals_the_groups() {
        let body = response(
            WORKERS_DATASET,
            r#"{"sum":{"requests":120}},{"sum":{"requests":3}}"#,
        );
        assert_eq!(parse_sum(&body, WORKERS_DATASET, "requests"), Some(123));
    }

    #[test]
    fn parse_sum_treats_an_empty_group_list_as_zero_usage() {
        let body = response(D1_DATASET, "");
        assert_eq!(parse_sum(&body, D1_DATASET, "rowsRead"), Some(0));
    }

    #[test]
    fn parse_sum_rejects_errors_and_shape_mismatches() {
        // A rejected dataset: GraphQL errors present.
        let rejected = r#"{"data":null,"errors":[{"message":"unknown field"}]}"#;
        assert_eq!(parse_sum(rejected, WORKERS_DATASET, "requests"), None);
        // The wrong dataset key.
        let body = response(WORKERS_DATASET, r#"{"sum":{"requests":1}}"#);
        assert_eq!(parse_sum(&body, R2_DATASET, "requests"), None);
        // A group missing the metric.
        let body = response(WORKERS_DATASET, r#"{"sum":{"errors":1}}"#);
        assert_eq!(parse_sum(&body, WORKERS_DATASET, "requests"), None);
        // Not JSON at all.
        assert_eq!(parse_sum("<html>", WORKERS_DATASET, "requests"), None);
    }
}
