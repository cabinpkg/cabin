//! Shared file-registry `config.json` contract.
//!
//! The schema version, the `kind` discriminant, and the
//! relative-subdirectory safety rule for a file registry's
//! `config.json` live here so the readers (`cabin-index`,
//! `cabin-index-http`) and the writer (`cabin-registry-file`)
//! validate one identical contract instead of three drifting copies.
//! `cabin-core` carries no I/O - each crate keeps its own error type
//! and maps the shared predicates and message helpers into its own
//! diagnostic.

use std::path::{Component, Path};

/// Supported `config.json` `schema` version.
pub const REGISTRY_CONFIG_SCHEMA: u32 = 1;

/// Required `config.json` `kind` discriminant for a file registry.
pub const REGISTRY_KIND: &str = "file-registry";

/// Error message for a remote-registry `config.json` field
/// (`auth-required` / `api`) encountered while the experimental
/// remote-registry client is disabled.  Shared so the local and
/// HTTP readers reject a gated field with identical wording.
/// Silently ignoring the field is not an option: dropping
/// `auth-required` would surface later as a confusing `401`.
#[must_use]
pub fn remote_registry_field_error(field: &str) -> String {
    format!(
        "`{field}` requires the experimental remote-registry client; run with `-Z remote-registry` \
         to enable it"
    )
}

/// Validate a registry config `api` value: the absolute `http(s)`
/// base URL of the registry's web/API origin.  Returns `None` when
/// valid and `Some(message)` naming what is wrong.  Uses the same
/// URL parser as the sparse HTTP client's index-URL hygiene, so the
/// acceptance rules cannot drift: `http` / `https` schemes only, a
/// well-formed host, and no `userinfo` credentials.  The message
/// never echoes the raw value (`url::ParseError` renders a static
/// description), so a credential-bearing URL cannot leak into logs.
#[must_use]
pub fn api_url_error(value: &str) -> Option<String> {
    let parsed = match url::Url::parse(value) {
        Ok(parsed) => parsed,
        Err(err) => return Some(format!("`api` is not a valid absolute URL: {err}")),
    };
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Some(format!("`api` uses unsupported URL scheme {other:?}")),
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Some("`api` must not contain credentials (userinfo)".to_owned());
    }
    None
}

/// Whether `value` is a safe relative subdirectory for a registry
/// config field (`packages` / `artifacts`): non-empty, not absolute,
/// and composed only of normal path components (a leading / interior
/// `.` is tolerated).  Rejects `..`, absolute paths, and OS root /
/// prefix components so a config cannot point outside the registry.
pub fn relative_subdir_is_safe(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let candidate = Path::new(value);
    if candidate.is_absolute() {
        return false;
    }
    candidate
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_relative_subdirs() {
        assert!(relative_subdir_is_safe("packages"));
        assert!(relative_subdir_is_safe("artifacts"));
        assert!(relative_subdir_is_safe("a/b"));
    }

    #[test]
    fn rejects_empty_absolute_and_traversal() {
        assert!(!relative_subdir_is_safe(""));
        assert!(!relative_subdir_is_safe("/abs"));
        assert!(!relative_subdir_is_safe("../escape"));
        assert!(!relative_subdir_is_safe("a/../b"));
    }

    #[test]
    fn gated_field_error_names_field_and_flag() {
        let message = remote_registry_field_error("auth-required");
        assert!(message.contains("`auth-required`"), "{message}");
        assert!(message.contains("-Z remote-registry"), "{message}");
    }

    #[test]
    fn api_url_accepts_http_and_https_origins() {
        for value in [
            "https://registry.cabinpkg.com",
            "http://localhost:8080",
            "HTTPS://example.com/base/",
        ] {
            assert_eq!(api_url_error(value), None, "{value}");
        }
    }

    #[test]
    fn api_url_rejects_relative_and_non_http_schemes() {
        let relative = api_url_error("registry.example.com").unwrap();
        assert!(relative.contains("absolute"), "{relative}");
        let scheme = api_url_error("file:///tmp/registry").unwrap();
        assert!(scheme.contains("\"file\""), "{scheme}");
        let hostless = api_url_error("https://:443").unwrap();
        assert!(hostless.contains("host"), "{hostless}");
    }

    /// A syntactically broken authority is rejected at load time
    /// instead of failing later when API routes are built: an empty
    /// host with a bare port, whitespace inside the host, and an
    /// unparsable port are all parse errors.
    #[test]
    fn api_url_rejects_malformed_hosts_and_ports() {
        for value in [
            "https://:443",
            "https://exa mple.com",
            "https://example.com:port",
        ] {
            assert!(api_url_error(value).is_some(), "{value} must be rejected");
        }
    }

    #[test]
    fn api_url_rejects_userinfo_without_echoing_it() {
        let message = api_url_error("https://user:pw@example.com").unwrap();
        assert!(message.contains("userinfo"), "{message}");
        assert!(
            !message.contains("user:pw"),
            "credentials must not leak into the message: {message}"
        );
    }

    /// Parse failures also never echo the raw value, so credentials
    /// in an unparsable URL cannot leak into the message either.
    #[test]
    fn api_url_parse_error_does_not_echo_the_value() {
        let message = api_url_error("https://user:pw@exa mple.com").unwrap();
        assert!(
            !message.contains("user:pw"),
            "credentials must not leak into the message: {message}"
        );
    }
}
