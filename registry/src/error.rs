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
}
