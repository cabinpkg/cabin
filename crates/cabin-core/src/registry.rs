//! Registry contract constants and predicates shared by Cabin's
//! registry clients and writers.
//!
//! The schema version, the `kind` discriminant, and the
//! relative-subdirectory safety rule for a file registry's
//! `config.json` live here so the readers (`cabin-index`,
//! `cabin-index-http`) and the writer (`cabin-registry-file`)
//! validate one contract.
//! `cabin-core` carries no I/O - each crate keeps its own error type
//! and maps the shared predicates and message helpers into its own
//! diagnostic.

use std::path::Path;

/// Supported `config.json` `schema` version.
pub const REGISTRY_CONFIG_SCHEMA: u32 = 1;

/// Length, in lowercase hex characters, of a packaging-revision
/// identifier: the leading prefix of the canonical archive's SHA-256.
/// Deriving the id from the archive bytes maps a byte-identical
/// republication onto the existing revision.  16 hex chars (64 bits)
/// keep filenames and display short; a same-version collision of
/// *different* archives is negligible, and writers reject it because
/// the full checksum stored next to the id disagrees.
pub const PACKAGING_REVISION_HEX_LEN: usize = 16;

/// Whether `value` is a well-formed packaging-revision identifier
/// (exactly [`PACKAGING_REVISION_HEX_LEN`] lowercase hex characters).
#[must_use]
pub fn is_valid_packaging_revision(value: &str) -> bool {
    value.len() == PACKAGING_REVISION_HEX_LEN
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Required `config.json` `kind` discriminant for a file registry.
pub const REGISTRY_KIND: &str = "file-registry";

/// Cabin's default hosted-registry index origin: the sparse HTTP
/// index a command falls back to when it needs an index and neither
/// the CLI (`--index-path` / `--index-url`) nor the config
/// (`[registry]`) names one.
pub const DEFAULT_INDEX_URL: &str = "https://registry.cabinpkg.com";

/// Error message for a registry *mutation* surface (`cabin publish
/// --index-url` / `cabin yank`) used while the experimental
/// remote-registry client is disabled.  Shared so every gated
/// command rejects with identical wording.
#[must_use]
pub fn remote_registry_command_error(command: &str) -> String {
    format!(
        "`{command}` requires the experimental remote-registry client; run with `-Z remote-registry` \
         to enable it"
    )
}

/// Validate a registry config `api` value: the absolute `http(s)`
/// base URL of the registry's web/API origin.  Returns `None` when
/// valid and `Some(message)` naming what is wrong.  Applies the same
/// `url` parser as the sparse HTTP client's index-URL check
/// (`cabin-index-http`'s `parse_base_url`): `http` / `https` schemes
/// only, a well-formed host, and no `userinfo` credentials.  Plain
/// `http` beyond loopback is refused later, where a request would be
/// sent (`cabin-registry-api`'s `RegistryApi::new`).  The message
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
/// prefix components so the value, joined onto the registry root,
/// stays lexically inside it.  This is a filesystem rule only:
/// `cabin-index-http` adds URL checks before joining it onto a URL.
pub fn relative_subdir_is_safe(value: &str) -> bool {
    !value.is_empty() && cabin_fs::path::is_safe_relative_path(Path::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaging_revision_is_exactly_sixteen_lowercase_hex() {
        assert!(is_valid_packaging_revision("9a93b2b7dfdac77c"));
        assert!(!is_valid_packaging_revision("9a93b2b7dfdac77")); // 15 chars
        assert!(!is_valid_packaging_revision("9a93b2b7dfdac77cf")); // 17 chars
        assert!(!is_valid_packaging_revision("9A93B2B7DFDAC77C")); // uppercase
    }

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
    fn gated_command_error_names_command_and_flag() {
        let message = remote_registry_command_error("cabin yank");
        assert!(message.contains("`cabin yank`"), "{message}");
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
