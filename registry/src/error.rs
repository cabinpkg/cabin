//! The crates.io-style error envelope: every non-2xx response body is
//! `{"errors":[{"detail":"..."}]}` (`docs/remote-registry.md`, "Error
//! envelope").

/// The one detail string the client contract fixes verbatim: every
/// unauthenticated response carries exactly this envelope, so callers cannot
/// probe package existence without a token.
pub const AUTH_REQUIRED: &str = "authentication required";
pub const NOT_FOUND: &str = "not found";
pub const METHOD_NOT_ALLOWED: &str = "method not allowed";
pub const INTERNAL: &str = "internal error";
pub const PUBLISH_SCOPE_REQUIRED: &str = "the token does not have the publish scope";
pub const YANK_SCOPE_REQUIRED: &str = "the token does not have the yank scope";
pub const VERSION_IMMUTABLE: &str = "published versions are immutable";
pub const INVALID_YANK_BODY: &str = r#"the yank body must be exactly {"yanked": <bool>}"#;

/// Renders `detail` into the error envelope.
pub fn envelope(detail: &str) -> String {
    serde_json::json!({ "errors": [{ "detail": detail }] }).to_string()
}

/// Renders `detail` plus a machine-readable `code` into the envelope:
/// `{"errors":[{"detail":"...","code":"..."}]}`. Quota, rate-limit, and
/// budget refusals carry codes; the pre-existing errors stay detail-only
/// (clients ignore the extra field either way).
pub fn envelope_with_code(detail: &str, code: &str) -> String {
    serde_json::json!({ "errors": [{ "detail": detail, "code": code }] }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_matches_the_contract_byte_for_byte() {
        assert_eq!(
            envelope(AUTH_REQUIRED),
            r#"{"errors":[{"detail":"authentication required"}]}"#
        );
    }

    #[test]
    fn envelope_escapes_details() {
        assert_eq!(
            envelope(r#"a "quoted" detail"#),
            r#"{"errors":[{"detail":"a \"quoted\" detail"}]}"#
        );
    }

    #[test]
    fn envelope_with_code_appends_the_code_field() {
        assert_eq!(
            envelope_with_code(
                "archive exceeds the plan's per-archive size limit",
                "archive_too_large"
            ),
            r#"{"errors":[{"detail":"archive exceeds the plan's per-archive size limit","code":"archive_too_large"}]}"#
        );
    }

    #[test]
    fn refusal_envelopes_carry_the_documented_status_and_code() {
        use crate::{breaker, quota};

        let denials = [
            (&quota::RATE_LIMITED, 429, "rate_limited"),
            (&quota::ARCHIVE_TOO_LARGE, 413, "archive_too_large"),
            (&quota::QUOTA_STORAGE, 403, "quota_storage"),
            (&quota::QUOTA_PACKAGES_DAILY, 403, "quota_packages_daily"),
            (&quota::QUOTA_PACKAGES_TOTAL, 403, "quota_packages_total"),
            (&quota::QUOTA_VERSIONS_DAILY, 403, "quota_versions_daily"),
        ];
        for (denial, status, code) in denials {
            assert_eq!(denial.status, status, "code: {code}");
            assert_eq!(denial.code, code);
            assert_eq!(
                envelope_with_code(denial.detail, denial.code),
                format!(
                    r#"{{"errors":[{{"detail":"{}","code":"{}"}}]}}"#,
                    denial.detail, denial.code
                ),
                "details are fixed strings that never need escaping"
            );
        }
        // The 402 budget refusal uses the same envelope shape.
        assert_eq!(
            envelope_with_code(breaker::OVER_BUDGET_DETAIL, breaker::OVER_BUDGET_CODE),
            r#"{"errors":[{"detail":"registry writes are temporarily disabled: the free-plan budget is exhausted","code":"registry_over_budget"}]}"#
        );
    }
}
