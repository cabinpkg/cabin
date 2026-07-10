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
pub const VERIFY_SCOPE_REQUIRED: &str = "the token does not have the verify scope";
pub const INVALID_VERDICT_BODY: &str =
    r#"the verdict body must be {"verdict": "verified" | "rejected", "reason": <string>}"#;
pub const VERDICT_REASON_REQUIRED: &str = "a rejection verdict requires a non-empty reason";
pub const VERDICT_BINDING_REQUIRED: &str =
    "a verified verdict requires the checksum and published_at the admin listing reported";
pub const VERSION_REJECTED_REVERDICT: &str =
    "the version was rejected; republishing it is the recovery path";
pub const VERDICT_TARGET_CHANGED: &str =
    "the version changed since it was listed; fetch the pending list again";
pub const INVALID_STATUS_QUERY: &str =
    "the status query parameter must be pending, verified, or rejected";
pub const CSRF_REQUIRED: &str = "the request must declare Content-Type: application/json and \
     carry the X-CSRF-Protection header";
pub const INVALID_TOKEN_NAME_OR_SCOPES: &str = "invalid token name or scopes";

/// The `WWW-Authenticate` challenge every Bearer-plane 401 carries,
/// mirroring Cargo's `login_url` challenge: byte-identical on every path
/// and failure reason, so unauthenticated responses stay
/// indistinguishable and leak nothing about package existence.
///
/// The token page it names ships with the website step; until that
/// lands, the challenged URL 404s on the dev deployment and the interim
/// token flow is `docs/runbook.md` ("Route management"). That gap is
/// deliberate sequencing on the operator-only dev registry - `cabin
/// login` never depends on the URL resolving, only on the grammar.
pub fn www_authenticate(web_origin: &str) -> String {
    format!(
        "Cabin login_url=\"{origin}/settings/tokens\"",
        origin = web_origin.trim_end_matches('/'),
    )
}

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
    fn www_authenticate_matches_the_challenge_grammar() {
        assert_eq!(
            www_authenticate("https://cabinpkg.com"),
            r#"Cabin login_url="https://cabinpkg.com/settings/tokens""#
        );
        // A trailing slash on the env var never doubles the separator.
        assert_eq!(
            www_authenticate("https://cabinpkg.com/"),
            r#"Cabin login_url="https://cabinpkg.com/settings/tokens""#
        );
    }

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
