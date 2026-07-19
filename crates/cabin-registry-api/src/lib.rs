//! Typed HTTP client for the experimental remote registry API
//! (`-Z remote-registry`).
//!
//! This crate owns the *mutating* half of the remote-registry
//! protocol specified in `docs/remote-registry.md`.  Registry
//! packages are always scoped, so both routes address the
//! `<scope>/<name>` pair:
//!
//! - [`RegistryApi::publish`] -
//!   `PUT /api/v1/packages/<scope>/<name>/<version>` with the
//!   crates.io-style length-prefixed body
//!   (`[u32 LE metadata_len][metadata][u32 LE archive_len][archive]`);
//! - [`RegistryApi::set_yanked`] -
//!   `PATCH /api/v1/packages/<scope>/<name>/<version>/yank` with a
//!   JSON `{"yanked": bool}` body.
//!
//! Both routes live on the registry's `api` origin (the `api` field of
//! its `config.json`, fetched by `cabin-index-http` through the
//! authenticated read path) and authenticate with the same
//! `Authorization: Bearer <token>` credential as the reads.  The
//! caller resolves the token through `cabin-credentials` and hands it
//! in as the typed [`Token`]; this crate never reads `credentials.toml`
//! or the environment itself.
//!
//! Crate boundaries:
//! - no staging, validation, or lint logic - `cabin-package` /
//!   `cabin-publish` produce the archive and metadata bytes, this
//!   crate only frames and ships them;
//! - no read routes - `config.json`, package metadata, and artifact
//!   downloads stay in `cabin-index-http`;
//! - token bytes never surface through errors or `Debug` output
//!   ([`Token`] redacts).

use std::io::Read as _;
use std::time::Duration;

use cabin_credentials::Token;
use serde::Deserialize;
use thiserror::Error;

/// Per-request timeout, matching the sparse HTTP read client.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap on how much of a non-2xx response body is read while looking
/// for the error envelope.  Envelopes are tiny; anything bigger is
/// not one.
const MAX_ERROR_BODY_BYTES: u64 = 64 * 1024;

/// Client for one registry's API origin.  Construction validates the
/// URL (http(s), no userinfo) and enforces the same cleartext rule as
/// the read path: the credential-bearing mutation routes are refused
/// over plain `http` beyond loopback hosts.
pub struct RegistryApi {
    agent: ureq::Agent,
    /// Validated API base URL, always with a trailing `/`.
    base: url::Url,
    /// Normalized API origin, for error messages.
    origin: String,
    token: Option<Token>,
}

/// What a successful [`RegistryApi::publish`] meant on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// `201`: the version did not exist and was created.
    Created,
    /// `200`: byte-identical metadata and archive were already
    /// published; the request was an idempotent no-op.
    AlreadyPublished,
}

/// A successful publish: the outcome plus the response body's optional
/// `"verification"` field, read tolerantly - `Some("pending")` on a
/// registry with the asynchronous verification lifecycle, `None` on one
/// without it (or an unreadable body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishReceipt {
    pub outcome: PublishOutcome,
    pub verification: Option<String>,
}

impl RegistryApi {
    /// Build a client for the registry API at `api_url` (the `api`
    /// field of the registry's `config.json`), attaching `token` to
    /// every request when one is supplied.
    ///
    /// # Errors
    /// Returns [`RegistryApiError::InvalidApiUrl`] when `api_url` is
    /// not a valid `http(s)` URL or carries userinfo credentials, and
    /// [`RegistryApiError::CleartextApiUrl`] when it uses plain
    /// `http` beyond loopback hosts - a bearer token must never
    /// travel in cleartext, mirroring the read path's rule.
    pub fn new(api_url: &str, token: Option<Token>) -> Result<Self, RegistryApiError> {
        // `normalize_origin` performs the full hygiene check
        // (scheme, host, userinfo) with userinfo redacted from its
        // own error messages, so this crate cannot drift on the rule.
        let origin = cabin_credentials::normalize_origin(api_url).map_err(|err| {
            RegistryApiError::InvalidApiUrl {
                message: err.to_string(),
            }
        })?;
        if !origin.starts_with("https://") && !cabin_credentials::url_is_loopback(api_url) {
            return Err(RegistryApiError::CleartextApiUrl { origin });
        }
        let mut base = url::Url::parse(api_url).map_err(|err| RegistryApiError::InvalidApiUrl {
            message: err.to_string(),
        })?;
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        Ok(Self {
            // Redirects are refused so a mutation can never be
            // bounced to a different origin than the one the
            // registry's `config.json` declared.
            agent: ureq::AgentBuilder::new()
                .timeout(DEFAULT_TIMEOUT)
                .redirects(0)
                .build(),
            base,
            origin,
            token,
        })
    }

    /// `PUT <api>/api/v1/packages/<scope>/<name>/<version>` with the
    /// framed metadata + archive body.  `name` is the full scoped
    /// `<scope>/<name>` string.
    ///
    /// # Errors
    /// Returns [`RegistryApiError::UnsafePackageName`] before any
    /// request when `name` is bare or fails the hosted registry's
    /// name grammar (scope: lowercase/digits/interior `-`; name part:
    /// `[a-z0-9][a-z0-9_-]*`), and
    /// [`RegistryApiError::FrameTooLarge`] when either payload
    /// exceeds the `u32` framing limit.  Response statuses map per
    /// `docs/remote-registry.md`: `409` becomes
    /// [`RegistryApiError::VersionConflict`], `400` / `401` map like
    /// the read path, a token-authenticated `403` surfaces the
    /// server's envelope detail as [`RegistryApiError::Forbidden`]
    /// (unless a `quota_*` code marks it as
    /// [`RegistryApiError::QuotaExceeded`]),
    /// the quota and budget refusals map to
    /// [`RegistryApiError::RegistryOverBudget`] (`402`),
    /// [`RegistryApiError::ArchiveTooLarge`] (`413`), and
    /// [`RegistryApiError::RateLimited`] (`429`) - the first and last
    /// carrying the response's `Retry-After` seconds when usable - and
    /// any other non-success status surfaces as
    /// [`RegistryApiError::ServerError`] with the error envelope's
    /// `detail` when the body carries one.
    pub fn publish(
        &self,
        name: &str,
        version: &semver::Version,
        metadata_json: &[u8],
        archive: &[u8],
    ) -> Result<PublishReceipt, RegistryApiError> {
        let url = self.package_route(name, version, "")?;
        let body = encode_publish_body(metadata_json, archive)?;
        let request = self
            .request("PUT", &url)
            .set("Content-Type", "application/octet-stream");
        let (status, response) = self.send(request.send_bytes(&body), name, version)?;
        let outcome = match status {
            201 => PublishOutcome::Created,
            200 => PublishOutcome::AlreadyPublished,
            status => {
                return Err(RegistryApiError::ServerError {
                    status,
                    detail: None,
                });
            }
        };
        Ok(PublishReceipt {
            outcome,
            verification: verification_field(response),
        })
    }

    /// `PATCH <api>/api/v1/packages/<scope>/<name>/<version>/yank`
    /// with a JSON `{"yanked": bool}` body.  `true` yanks, `false`
    /// un-yanks; the route is idempotent.  `name` is the full scoped
    /// `<scope>/<name>` string.
    ///
    /// # Errors
    /// Returns [`RegistryApiError::UnsafePackageName`] before any
    /// request when `name` is bare or fails the hosted registry's
    /// name grammar (scope: lowercase/digits/interior `-`; name part:
    /// `[a-z0-9][a-z0-9_-]*`).
    /// Response
    /// statuses map per `docs/remote-registry.md`; a `404` for an
    /// unknown package or version becomes
    /// [`RegistryApiError::NotFound`].
    pub fn set_yanked(
        &self,
        name: &str,
        version: &semver::Version,
        yanked: bool,
    ) -> Result<(), RegistryApiError> {
        let url = self.package_route(name, version, "/yank")?;
        let body = serde_json::json!({ "yanked": yanked }).to_string();
        let request = self
            .request("PATCH", &url)
            .set("Content-Type", "application/json");
        match self.send(request.send_string(&body), name, version)? {
            (200, _) => Ok(()),
            (status, _) => Err(RegistryApiError::ServerError {
                status,
                detail: None,
            }),
        }
    }

    /// `<api>/api/v1/packages/<scope>/<name>/<version><suffix>`.  The
    /// hosted routes have no bare-name form, so a bare name fails
    /// here, before any request; the scoped name is re-validated
    /// against the full `PackageName` grammar plus the registry's
    /// stricter publish grammar for the name part at this URL
    /// boundary (defense in depth, mirroring `cabin-index-http`), so
    /// both segments it embeds are path-safe by construction.
    fn package_route(
        &self,
        name: &str,
        version: &semver::Version,
        suffix: &str,
    ) -> Result<url::Url, RegistryApiError> {
        let safe = cabin_core::PackageName::new(name).is_ok_and(|parsed| {
            parsed.is_scoped() && is_valid_registry_package_name(parsed.base_name())
        });
        if !safe {
            return Err(RegistryApiError::UnsafePackageName {
                name: name.to_owned(),
            });
        }
        let relative = format!("api/v1/packages/{name}/{version}{suffix}");
        self.base
            .join(&relative)
            .map_err(|err| RegistryApiError::InvalidApiUrl {
                message: format!("cannot build route `{relative}`: {err}"),
            })
    }

    fn request(&self, method: &str, url: &url::Url) -> ureq::Request {
        let mut request = self.agent.request(method, url.as_str());
        if let Some(token) = &self.token {
            request = request.set("Authorization", &format!("Bearer {}", token.expose()));
        }
        request
    }

    /// Map a `ureq` result into either a success status (2xx, with the
    /// response for the caller to interpret) or the typed error for
    /// the shared protocol statuses.
    fn send(
        &self,
        result: Result<ureq::Response, ureq::Error>,
        name: &str,
        version: &semver::Version,
    ) -> Result<(u16, ureq::Response), RegistryApiError> {
        match result {
            Ok(response) => {
                let status = response.status();
                // `.redirects(0)` refuses to follow, but ureq still
                // returns the 3xx as `Ok`; reject it explicitly.
                if (300..400).contains(&status) {
                    return Err(RegistryApiError::ServerError {
                        status,
                        detail: None,
                    });
                }
                Ok((status, response))
            }
            Err(ureq::Error::Status(status, response)) => {
                // `Retry-After` (delta seconds) rides on the 402 and 429
                // refusals; an absent or non-numeric value (an HTTP date,
                // say) degrades to no hint rather than failing the
                // mapping.  Read before the body consumes the response.
                let retry_after_secs = response
                    .header("Retry-After")
                    .and_then(|value| value.trim().parse::<u64>().ok());
                let (detail, code) = match envelope_entry(response) {
                    Some(entry) => (Some(entry.detail), entry.code),
                    None => (None, None),
                };
                Err(match status {
                    400 => RegistryApiError::BadRequest { detail },
                    401 if self.token.is_some() => RegistryApiError::TokenRejected {
                        origin: self.origin.clone(),
                    },
                    401 => RegistryApiError::AuthRequired {
                        origin: self.origin.clone(),
                    },
                    402 => RegistryApiError::RegistryOverBudget { retry_after_secs },
                    // A 403 whose envelope carries a `quota_*` code is a
                    // per-user quota refusal (`docs/remote-registry.md`,
                    // "Error envelope"), not a scope problem: the server
                    // detail - which embeds the registry's own usage URL -
                    // reaches the user verbatim. The client never derives
                    // a web URL itself.
                    403 if code
                        .as_deref()
                        .is_some_and(|code| code.starts_with("quota_")) =>
                    {
                        RegistryApiError::QuotaExceeded {
                            // The envelope requires `detail`, so a parsed
                            // `code` guarantees one.
                            detail: detail.unwrap_or_default(),
                        }
                    }
                    // A token-authenticated, code-less 403 covers two
                    // distinct server refusals that differ only in
                    // their `detail`: a token permission the user did
                    // not grant, and a scope the token's user is not a
                    // member of.  The detail is surfaced verbatim so
                    // the user fixes the right one; only an
                    // envelope-less response falls back to the generic
                    // token-permission wording.  A tokenless 403 is
                    // neither case (no credential was presented), and
                    // an unknown code falls through to the generic
                    // mapping so its detail still reaches the user.
                    403 if self.token.is_some() && code.is_none() => RegistryApiError::Forbidden {
                        origin: self.origin.clone(),
                        detail,
                    },
                    404 => RegistryApiError::NotFound {
                        name: name.to_owned(),
                        version: version.to_string(),
                    },
                    409 => RegistryApiError::VersionConflict {
                        name: name.to_owned(),
                        version: version.to_string(),
                    },
                    413 => RegistryApiError::ArchiveTooLarge { detail },
                    429 => RegistryApiError::RateLimited { retry_after_secs },
                    _ => RegistryApiError::ServerError { status, detail },
                })
            }
            Err(ureq::Error::Transport(transport)) => Err(RegistryApiError::Transport {
                message: transport.to_string(),
            }),
        }
    }
}

impl std::fmt::Debug for RegistryApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Token`'s own `Debug` redacts; keep the origin visible.
        f.debug_struct("RegistryApi")
            .field("origin", &self.origin)
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

/// Mirror of the hosted registry's package-name grammar
/// (`registry/src/routes.rs`, `is_valid_name`):
/// `^[a-z0-9][a-z0-9_-]*$`.  `PackageName`'s own grammar is looser
/// (uppercase and `.` are legal in local-only names), so without this
/// check a name the registry refuses would fail publish only after
/// staging and network work - and 404 a yank misleadingly.
fn is_valid_registry_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Encode the publish request body:
/// `[u32 LE metadata_len][metadata][u32 LE archive_len][archive]`.
///
/// # Errors
/// Returns [`RegistryApiError::FrameTooLarge`] when either payload
/// does not fit the `u32` length prefix.
pub fn encode_publish_body(
    metadata_json: &[u8],
    archive: &[u8],
) -> Result<Vec<u8>, RegistryApiError> {
    let metadata_len = u32::try_from(metadata_json.len())
        .map_err(|_| RegistryApiError::FrameTooLarge { part: "metadata" })?;
    let archive_len = u32::try_from(archive.len())
        .map_err(|_| RegistryApiError::FrameTooLarge { part: "archive" })?;
    let mut body = Vec::with_capacity(8 + metadata_json.len() + archive.len());
    body.extend_from_slice(&metadata_len.to_le_bytes());
    body.extend_from_slice(metadata_json);
    body.extend_from_slice(&archive_len.to_le_bytes());
    body.extend_from_slice(archive);
    Ok(body)
}

/// Serde shape of the protocol's error envelope:
/// `{"errors":[{"detail":"...","code":"..."}]}`; `code` is the optional
/// machine-readable refusal code quota and budget errors carry.
#[derive(Deserialize)]
struct ErrorEnvelope {
    errors: Vec<ErrorEntry>,
}

#[derive(Deserialize)]
struct ErrorEntry {
    detail: String,
    #[serde(default)]
    code: Option<String>,
}

/// Serde shape of a publish success body's optional `"verification"`
/// field; every other field is ignored on purpose - a registry without
/// the verification lifecycle simply omits it.
#[derive(Deserialize)]
struct PublishSuccessBody {
    #[serde(default)]
    verification: Option<String>,
}

/// Read a publish success body (capped like the error envelope) and
/// extract its optional `"verification"` field tolerantly: a missing,
/// oversized, or malformed body yields `None` rather than an error.
fn verification_field(response: ureq::Response) -> Option<String> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES)
        .read_to_end(&mut body)
        .ok()?;
    serde_json::from_slice::<PublishSuccessBody>(&body)
        .ok()?
        .verification
}

/// Read a non-2xx response body (capped) and extract the first error
/// envelope entry.  A malformed or missing envelope yields `None`, so
/// the caller's message degrades to the raw status.
fn envelope_entry(response: ureq::Response) -> Option<ErrorEntry> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES)
        .read_to_end(&mut body)
        .ok()?;
    let envelope: ErrorEnvelope = serde_json::from_slice(&body).ok()?;
    let mut entry = envelope.errors.into_iter().next()?;
    entry.detail = escape_control_chars(&entry.detail);
    Some(entry)
}

/// Escape terminal control characters in registry-provided diagnostics.
/// Error details are useful, but a third-party registry must not be able to
/// inject terminal commands or forge extra diagnostic lines.
fn escape_control_chars(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_control() || is_bidi_control(ch) {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

/// Unicode's bidirectional controls can reorder otherwise printable text
/// in terminal diagnostics. Keep ordinary international text intact while
/// making those invisible formatting characters explicit.
fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

/// Append the server's envelope `detail` to a base message when one
/// was present.
fn with_detail(base: String, detail: Option<&String>) -> String {
    match detail {
        Some(detail) => format!("{base}: {detail}"),
        None => base,
    }
}

/// The token-authenticated 403 message: the server's terminal-safe envelope
/// `detail` when present (it distinguishes a missing token
/// permission from missing scope membership), else the generic
/// token-permission wording for registries that answer without an
/// envelope.
fn forbidden_message(origin: &str, detail: Option<&String>) -> String {
    let reason = detail.map_or(
        "the stored token does not have the required scope",
        String::as_str,
    );
    format!("registry API `{origin}` refused the request: {reason}")
}

/// Append the retry hint: the server's `Retry-After` seconds when the
/// response carried a usable one, a plain "try again later" otherwise.
fn with_retry(base: &str, retry_after_secs: Option<u64>) -> String {
    match retry_after_secs {
        Some(1) => format!("{base}; try again in 1 second"),
        Some(secs) => format!("{base}; try again in {secs} seconds"),
        None => format!("{base}; try again later"),
    }
}

/// Errors produced by the registry API client.  No variant ever
/// embeds token bytes.
#[derive(Debug, Error)]
pub enum RegistryApiError {
    #[error("invalid registry API URL: {message}")]
    InvalidApiUrl { message: String },

    #[error(
        "refusing to send requests to registry API `{origin}` over plain `http`: bearer tokens \
         are never sent in cleartext except to loopback hosts; use an `https` API URL"
    )]
    CleartextApiUrl { origin: String },

    #[error(
        "package name `{name}` cannot be used on remote registry routes; registry packages are named `<scope>/<name>` (exactly one `/`), where the scope is lowercase ASCII letters, digits, and interior `-` (at most 39 characters) and the name part matches `[a-z0-9][a-z0-9_-]*`"
    )]
    UnsafePackageName { name: String },

    #[error("cannot frame the publish request: the {part} exceeds the u32 length prefix")]
    FrameTooLarge { part: &'static str },

    #[error("{}", with_detail("registry rejected the request (status 400)".to_owned(), .detail.as_ref()))]
    BadRequest { detail: Option<String> },

    #[error(
        "authentication required by registry API `{origin}`; run `cabin login --index-url <URL>` \
         with `-Z remote-registry` to store a token for this registry"
    )]
    AuthRequired { origin: String },

    #[error(
        "registry API `{origin}` rejected the stored token (revoked or expired); re-run `cabin \
         login --index-url <URL>` for this registry"
    )]
    TokenRejected { origin: String },

    #[error("{}", forbidden_message(.origin, .detail.as_ref()))]
    Forbidden {
        origin: String,
        detail: Option<String>,
    },

    #[error("{detail}")]
    QuotaExceeded { detail: String },

    #[error("{}", with_retry(
        "the registry is temporarily not accepting publishes (over its free budget)",
        *.retry_after_secs,
    ))]
    RegistryOverBudget { retry_after_secs: Option<u64> },

    #[error("{}", with_retry("the registry rate limited this request", *.retry_after_secs))]
    RateLimited { retry_after_secs: Option<u64> },

    #[error("{}", with_detail(
        "the package archive is too large for this registry".to_owned(),
        .detail.as_ref(),
    ))]
    ArchiveTooLarge { detail: Option<String> },

    #[error("`{name}@{version}` is not published on this registry")]
    NotFound { name: String, version: String },

    #[error(
        "`{name} {version}` is already published with different bytes; published versions are \
         immutable - bump the version and publish again"
    )]
    VersionConflict { name: String, version: String },

    #[error("{}", with_detail(format!("registry API request failed: server returned {status}"), .detail.as_ref()))]
    ServerError { status: u16, detail: Option<String> },

    #[error("registry API transport error: {message}")]
    Transport { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    const TEST_TOKEN: &str = "cabin_apiTestToken12";

    fn token() -> Token {
        Token::parse(TEST_TOKEN).unwrap()
    }

    fn version(raw: &str) -> semver::Version {
        semver::Version::parse(raw).unwrap()
    }

    /// Decode a framed publish body back into (metadata, archive).
    /// Test-side inverse of [`encode_publish_body`]; asserts the
    /// frame is exactly consumed.
    fn decode_publish_body(body: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let metadata_len = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
        let metadata = body[4..4 + metadata_len].to_vec();
        let rest = &body[4 + metadata_len..];
        let archive_len = u32::from_le_bytes(rest[0..4].try_into().unwrap()) as usize;
        let archive = rest[4..4 + archive_len].to_vec();
        assert_eq!(
            body.len(),
            8 + metadata_len + archive_len,
            "frame must be exactly consumed"
        );
        (metadata, archive)
    }

    /// The retry hint degrades to "later" without a usable
    /// `Retry-After` and pluralizes correctly ("in 1 second", not
    /// "1 seconds" - live-verified wording).
    #[test]
    fn retry_hints_degrade_and_pluralize() {
        assert_eq!(with_retry("x", None), "x; try again later");
        assert_eq!(with_retry("x", Some(1)), "x; try again in 1 second");
        assert_eq!(with_retry("x", Some(2)), "x; try again in 2 seconds");
    }

    #[test]
    fn publish_body_round_trips_through_the_decoder() {
        let metadata = br#"{"schema":1,"name":"fmtlib/fmt"}"#;
        let archive = [0x1fu8, 0x8b, 0x08, 0x00, 0xff];
        let body = encode_publish_body(metadata, &archive).unwrap();
        assert_eq!(
            &body[0..4],
            &u32::try_from(metadata.len()).unwrap().to_le_bytes()
        );
        let (decoded_metadata, decoded_archive) = decode_publish_body(&body);
        assert_eq!(decoded_metadata, metadata);
        assert_eq!(decoded_archive, archive);

        // Empty payloads still frame correctly.
        let empty = encode_publish_body(b"", b"").unwrap();
        assert_eq!(empty, vec![0u8; 8]);
    }

    #[test]
    fn new_rejects_invalid_and_cleartext_api_urls() {
        for api in [
            "ftp://registry.example.com",
            "https://user:pw@registry.example.com",
            "not a url",
        ] {
            let err = RegistryApi::new(api, None).unwrap_err();
            let message = err.to_string();
            assert!(
                matches!(err, RegistryApiError::InvalidApiUrl { .. }),
                "{api}: {err:?}"
            );
            assert!(
                !message.contains("user:pw"),
                "credentials leaked: {message}"
            );
        }
        let err = RegistryApi::new("http://registry.example.com", Some(token())).unwrap_err();
        assert!(
            matches!(err, RegistryApiError::CleartextApiUrl { .. }),
            "{err:?}"
        );
        // Loopback http is the documented local-testing exception.
        RegistryApi::new("http://127.0.0.1:8080", Some(token())).unwrap();
        RegistryApi::new("http://localhost:8080/base", None).unwrap();
    }

    #[test]
    fn unsafe_and_bare_package_names_never_reach_the_wire() {
        // No server bound: an attempted request would surface as a
        // transport error, so getting `UnsafePackageName` proves the
        // gate fires first.  Bare names are rejected alongside unsafe
        // segments: the hosted routes have no bare-name form.
        let api = RegistryApi::new("http://127.0.0.1:9", Some(token())).unwrap();
        for name in [
            "fmt",
            "../evil",
            ".hidden",
            "-flag",
            "acme/../evil",
            "../evil/fmt",
            "acme/.hidden",
            "acme/fmt/extra",
            "acme//fmt",
            "/fmt",
            "acme/",
            // The full grammar applies, not just path safety: a scope
            // is lowercase-only, and the name part follows the
            // registry's publish grammar (`[a-z0-9][a-z0-9_-]*`), so
            // uppercase or `.`-bearing local-only names are refused
            // before any request.
            "ACME/fmt",
            "acme/Foo",
            "acme/foo.bar",
            "acme/_foo",
        ] {
            let err = api
                .publish(name, &version("1.0.0"), b"{}", b"")
                .unwrap_err();
            assert!(
                matches!(err, RegistryApiError::UnsafePackageName { .. }),
                "{name}: {err:?}"
            );
            let err = api.set_yanked(name, &version("1.0.0"), true).unwrap_err();
            assert!(
                matches!(err, RegistryApiError::UnsafePackageName { .. }),
                "{name}: {err:?}"
            );
        }
    }

    #[test]
    fn debug_output_redacts_the_token() {
        let api = RegistryApi::new("https://registry.example.com", Some(token())).unwrap();
        let rendered = format!("{api:?}");
        assert!(!rendered.contains("apiTestToken"), "leaked: {rendered}");
        assert!(rendered.contains("https://registry.example.com"));
    }

    // -----------------------------------------------------------------
    // Mock registry: wire-level assertions per response status
    // -----------------------------------------------------------------

    /// One captured request: everything the protocol tests assert on.
    struct Captured {
        method: String,
        path: String,
        authorization: Option<String>,
        body: Vec<u8>,
    }

    /// Mock registry API server answering every request with a fixed
    /// status + body, capturing requests into a channel.
    struct MockApi {
        server: Arc<tiny_http::Server>,
        thread: Option<JoinHandle<()>>,
        url: String,
        captured: mpsc::Receiver<Captured>,
    }

    impl MockApi {
        fn respond_with(status: u16, body: &'static str) -> Self {
            Self::respond_with_headers(status, body, &[])
        }

        fn respond_with_headers(status: u16, body: &'static str, headers: &[(&str, &str)]) -> Self {
            let server = Arc::new(
                tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
            );
            let addr = server.server_addr().to_ip().expect("loopback addr");
            let url = format!("http://{addr}");
            let (sender, captured) = mpsc::channel();
            let server_for_thread = Arc::clone(&server);
            let response_headers: Vec<tiny_http::Header> = headers
                .iter()
                .map(|(name, value)| {
                    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                        .expect("valid test header")
                })
                .collect();
            let thread = std::thread::spawn(move || {
                while let Ok(mut req) = server_for_thread.recv() {
                    let mut body_bytes = Vec::new();
                    let _ = req.as_reader().read_to_end(&mut body_bytes);
                    let _ = sender.send(Captured {
                        method: req.method().as_str().to_owned(),
                        path: req.url().to_owned(),
                        authorization: req
                            .headers()
                            .iter()
                            .find(|h| h.field.equiv("Authorization"))
                            .map(|h| h.value.to_string()),
                        body: body_bytes,
                    });
                    let mut response =
                        tiny_http::Response::from_string(body).with_status_code(status);
                    for header in &response_headers {
                        response.add_header(header.clone());
                    }
                    let _ = req.respond(response);
                }
            });
            Self {
                server,
                thread: Some(thread),
                url,
                captured,
            }
        }

        fn client(&self, token: Option<Token>) -> RegistryApi {
            RegistryApi::new(&self.url, token).unwrap()
        }

        fn captured(&self) -> Captured {
            self.captured
                .recv_timeout(Duration::from_secs(5))
                .expect("a request should have reached the mock registry")
        }
    }

    impl Drop for MockApi {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }

    /// The 201 path: outcome, route, method, bearer header, and the
    /// exact frame bytes on the wire.
    #[test]
    fn publish_created_sends_the_framed_body_and_bearer_token() {
        let mock = MockApi::respond_with(201, r#"{"ok":true}"#);
        let metadata = br#"{"schema":1,"name":"fmtlib/fmt","version":"10.2.1"}"#;
        let archive = b"\x1f\x8b\x08\x00fake-gzip-bytes";

        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), metadata, archive)
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::Created);
        // No "verification" field: a registry without the lifecycle.
        assert_eq!(receipt.verification, None);

        let captured = mock.captured();
        assert_eq!(captured.method, "PUT");
        assert_eq!(captured.path, "/api/v1/packages/fmtlib/fmt/10.2.1");
        assert_eq!(
            captured.authorization.as_deref(),
            Some(format!("Bearer {TEST_TOKEN}").as_str())
        );
        assert_eq!(
            &captured.body[0..4],
            &u32::try_from(metadata.len()).unwrap().to_le_bytes()
        );
        let (decoded_metadata, decoded_archive) = decode_publish_body(&captured.body);
        assert_eq!(decoded_metadata, metadata);
        assert_eq!(decoded_archive, archive);
    }

    /// The 200 path: byte-identical re-publish reports the no-op.
    #[test]
    fn publish_maps_200_to_already_published() {
        let mock = MockApi::respond_with(200, r#"{"ok":true,"no_op":true}"#);
        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::AlreadyPublished);
        assert_eq!(receipt.verification, None);
    }

    /// The optional `"verification"` field is read tolerantly: present
    /// on either success status it is surfaced verbatim, and a body
    /// that is not the expected JSON degrades to `None` instead of
    /// failing the publish.
    #[test]
    fn publish_reads_the_verification_field_tolerantly() {
        let mock = MockApi::respond_with(
            201,
            r#"{"ok":true,"name":"fmtlib/fmt","version":"10.2.1","checksum":"sha256:aa","verification":"pending"}"#,
        );
        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::Created);
        assert_eq!(receipt.verification.as_deref(), Some("pending"));

        let mock =
            MockApi::respond_with(200, r#"{"ok":true,"no_op":true,"verification":"verified"}"#);
        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::AlreadyPublished);
        assert_eq!(receipt.verification.as_deref(), Some("verified"));

        // A body that is not the expected JSON shape never fails the
        // publish: the field just reads as absent.
        let mock = MockApi::respond_with(201, "not json at all");
        let receipt = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap();
        assert_eq!(receipt.outcome, PublishOutcome::Created);
        assert_eq!(receipt.verification, None);
    }

    /// 409: the version exists with different bytes and stays
    /// immutable.
    #[test]
    fn publish_maps_409_to_version_conflict() {
        let mock = MockApi::respond_with(409, r#"{"errors":[{"detail":"checksum mismatch"}]}"#);
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::VersionConflict { name, version } => {
                assert_eq!(name, "fmtlib/fmt");
                assert_eq!(version, "10.2.1");
            }
            other => panic!("expected VersionConflict, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("different bytes"), "{message}");
        assert!(message.contains("immutable"), "{message}");
    }

    /// 401 without a token asks for a login; 401 despite one reports
    /// the token as rejected.  Neither leaks token bytes.
    #[test]
    fn publish_maps_401_by_whether_a_token_was_sent() {
        let mock =
            MockApi::respond_with(401, r#"{"errors":[{"detail":"authentication required"}]}"#);
        let err = mock
            .client(None)
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        assert!(
            matches!(err, RegistryApiError::AuthRequired { .. }),
            "{err:?}"
        );
        assert_eq!(mock.captured().authorization, None);

        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        assert!(
            matches!(err, RegistryApiError::TokenRejected { .. }),
            "{err:?}"
        );
        assert!(
            !err.to_string().contains("apiTestToken"),
            "token bytes leaked: {err}"
        );
    }

    /// A token-authenticated, code-less 403 surfaces a printable server
    /// `detail` unchanged: it distinguishes a token permission the
    /// user did not grant from a scope the token's user is not a
    /// member of, and the client must not collapse the second into
    /// the first.  Without an envelope the message degrades to the
    /// generic token-permission wording.
    #[test]
    fn publish_maps_printable_403_details_unchanged() {
        for detail in [
            "the token does not have the publish scope",
            "the scope does not exist or the token's user is not a member of it",
        ] {
            let body: &'static str =
                Box::leak(format!(r#"{{"errors":[{{"detail":"{detail}"}}]}}"#).into_boxed_str());
            let mock = MockApi::respond_with(403, body);
            let err = mock
                .client(Some(token()))
                .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
                .unwrap_err();
            match &err {
                RegistryApiError::Forbidden {
                    origin,
                    detail: Some(got),
                } => {
                    assert_eq!(origin, &mock.url);
                    assert_eq!(got, detail);
                }
                other => panic!("{detail}: expected Forbidden with the detail, got {other:?}"),
            }
            assert!(err.to_string().contains(detail), "{err}");
        }

        let mock = MockApi::respond_with(403, "no envelope here");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::Forbidden { detail: None, .. } => {}
            other => panic!("expected Forbidden without detail, got {other:?}"),
        }
        assert!(
            err.to_string()
                .contains("the stored token does not have the required scope"),
            "{err}"
        );
    }

    #[test]
    fn registry_details_cannot_inject_terminal_controls() {
        let mock = MockApi::respond_with(
            400,
            r#"{"errors":[{"detail":"denied\u001b[2J\nforged\u202ereordered"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();

        let rendered = err.to_string();
        assert!(
            !rendered.contains('\u{1b}'),
            "escape reached terminal: {rendered:?}"
        );
        assert!(
            !rendered.contains('\n'),
            "newline reached terminal: {rendered:?}"
        );
        assert!(
            rendered.contains(r"denied\u{1b}[2J\nforged\u{202e}reordered"),
            "{rendered:?}"
        );
    }

    /// A 403 whose envelope carries a `quota_*` code is a per-user quota
    /// refusal, not the missing-scope case: the server detail - which
    /// embeds the registry's own usage URL - must reach the user
    /// unchanged when printable. The client never builds a web URL itself.
    #[test]
    fn publish_maps_coded_403_quota_refusals_to_the_server_detail() {
        for code in [
            "quota_storage",
            "quota_packages_daily",
            "quota_packages_total",
            "quota_versions_daily",
        ] {
            let body: &'static str = Box::leak(
                format!(
                    r#"{{"errors":[{{"detail":"the quota is exhausted; see https://cabinpkg.com/dashboard for current usage","code":"{code}"}}]}}"#
                )
                .into_boxed_str(),
            );
            let mock = MockApi::respond_with(403, body);
            let err = mock
                .client(Some(token()))
                .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
                .unwrap_err();
            match &err {
                RegistryApiError::QuotaExceeded { detail } => {
                    assert_eq!(
                        detail,
                        "the quota is exhausted; \
                         see https://cabinpkg.com/dashboard for current usage"
                    );
                }
                other => panic!("{code}: expected QuotaExceeded, got {other:?}"),
            }
            let message = err.to_string();
            assert!(
                message.contains("see https://cabinpkg.com/dashboard for current usage"),
                "{code}: expected the server-embedded usage URL verbatim in: {message}"
            );
            assert!(
                !message.contains(&mock.url),
                "{code}: the client must not derive a URL from the API origin: {message}"
            );
            assert!(!message.contains("scope"), "{code}: {message}");
        }
    }

    /// A 403 with an unknown (non-`quota_*`) code falls back to the
    /// generic mapping carrying the detail string - never the misleading
    /// scope message, never a guessed quota message.
    #[test]
    fn publish_falls_back_to_the_detail_on_unknown_codes() {
        let mock = MockApi::respond_with(
            403,
            r#"{"errors":[{"detail":"refused for a brand-new reason","code":"shiny_new_refusal"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::ServerError {
                status: 403,
                detail: Some(detail),
            } => assert_eq!(detail, "refused for a brand-new reason"),
            other => panic!("expected the generic mapping, got {other:?}"),
        }
        assert!(!err.to_string().contains("scope"), "{err}");

        // A code on a status with its own mapping does not hijack it:
        // the 400 stays a BadRequest with the detail.
        let mock = MockApi::respond_with(
            400,
            r#"{"errors":[{"detail":"metadata name mismatch","code":"quota_storage"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::BadRequest { detail: Some(_) } => {}
            other => panic!("expected BadRequest, got {other:?}"),
        }
        assert!(err.to_string().contains("metadata name mismatch"), "{err}");
    }

    /// 402: the service-wide budget breaker has writes paused. The message
    /// says so and carries the `Retry-After` seconds when present.
    #[test]
    fn publish_maps_402_to_registry_over_budget() {
        let mock = MockApi::respond_with_headers(
            402,
            r#"{"errors":[{"detail":"registry writes are temporarily disabled: the free-plan budget is exhausted","code":"registry_over_budget"}]}"#,
            &[("Retry-After", "900")],
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::RegistryOverBudget {
                retry_after_secs: Some(900),
            } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("over its free budget"), "{message}");
        assert!(
            message.contains("900"),
            "expected Retry-After in: {message}"
        );

        // Without a Retry-After header - or even without an envelope at
        // all - the mapping still holds and degrades to "try again later".
        let mock = MockApi::respond_with(402, "no envelope here");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::RegistryOverBudget {
                retry_after_secs: None,
            } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
        assert!(err.to_string().contains("try again later"), "{err}");
    }

    /// The breaker blocks yanks too: the shared mapping covers PATCH.
    #[test]
    fn set_yanked_maps_402_to_registry_over_budget() {
        let mock = MockApi::respond_with_headers(
            402,
            r#"{"errors":[{"detail":"registry writes are temporarily disabled: the free-plan budget is exhausted","code":"registry_over_budget"}]}"#,
            &[("Retry-After", "900")],
        );
        let err = mock
            .client(Some(token()))
            .set_yanked("fmtlib/fmt", &version("10.2.1"), true)
            .unwrap_err();
        match &err {
            RegistryApiError::RegistryOverBudget {
                retry_after_secs: Some(900),
            } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
    }

    /// 429: the publish token bucket is empty; `Retry-After` says when
    /// the next publish will be accepted.
    #[test]
    fn publish_maps_429_to_rate_limited() {
        let mock = MockApi::respond_with_headers(
            429,
            r#"{"errors":[{"detail":"publish rate limit exceeded; retry after the token bucket refills","code":"rate_limited"}]}"#,
            &[("Retry-After", "42")],
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::RateLimited {
                retry_after_secs: Some(42),
            } => {}
            other => panic!("expected RateLimited, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("rate limit"), "{message}");
        assert!(message.contains("42"), "expected Retry-After in: {message}");

        // A missing or non-numeric Retry-After (an HTTP date, say)
        // degrades to no hint rather than failing the mapping.
        let mock = MockApi::respond_with_headers(
            429,
            r#"{"errors":[{"detail":"publish rate limit exceeded","code":"rate_limited"}]}"#,
            &[("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT")],
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::RateLimited {
                retry_after_secs: None,
            } => {}
            other => panic!("expected RateLimited, got {other:?}"),
        }
        assert!(err.to_string().contains("try again later"), "{err}");
    }

    /// 413: the archive exceeds the per-archive size limit. The
    /// server detail (which carries the limit when the server states it)
    /// is appended; without an envelope the fixed message stands alone.
    #[test]
    fn publish_maps_413_to_archive_too_large() {
        let mock = MockApi::respond_with(
            413,
            r#"{"errors":[{"detail":"archive exceeds the per-archive size limit (16777216 bytes)","code":"archive_too_large"}]}"#,
        );
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::ArchiveTooLarge { detail: Some(_) } => {}
            other => panic!("expected ArchiveTooLarge, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("too large"), "{message}");
        assert!(
            message.contains("16777216 bytes"),
            "expected the limit from the detail in: {message}"
        );

        let mock = MockApi::respond_with(413, "not an envelope");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::ArchiveTooLarge { detail: None } => {}
            other => panic!("expected ArchiveTooLarge, got {other:?}"),
        }
        assert!(err.to_string().contains("too large"), "{err}");
    }

    /// A well-formed envelope's `detail` reaches the 400 message; a
    /// malformed one degrades to the raw status.
    #[test]
    fn error_envelope_parses_and_degrades_to_the_raw_status() {
        let mock =
            MockApi::respond_with(400, r#"{"errors":[{"detail":"metadata name mismatch"}]}"#);
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        assert!(
            err.to_string().contains("metadata name mismatch"),
            "expected the envelope detail in: {err}"
        );

        let mock = MockApi::respond_with(400, "<html>not the envelope</html>");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::BadRequest { detail: None } => {}
            other => panic!("expected BadRequest without detail, got {other:?}"),
        }
        assert!(
            err.to_string().contains("400"),
            "expected the raw status in: {err}"
        );

        let mock = MockApi::respond_with(500, "garbage");
        let err = mock
            .client(Some(token()))
            .publish("fmtlib/fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::ServerError {
                status: 500,
                detail: None,
            } => {}
            other => panic!("expected ServerError(500), got {other:?}"),
        }
    }

    /// The yank route: method, path, JSON body, idempotent 200, and
    /// the 404 mapping for unknown versions.
    #[test]
    fn set_yanked_patches_the_yank_route() {
        let mock = MockApi::respond_with(200, r#"{"ok":true}"#);
        mock.client(Some(token()))
            .set_yanked("fmtlib/fmt", &version("10.2.1"), true)
            .unwrap();
        let captured = mock.captured();
        assert_eq!(captured.method, "PATCH");
        assert_eq!(captured.path, "/api/v1/packages/fmtlib/fmt/10.2.1/yank");
        assert_eq!(captured.body, br#"{"yanked":true}"#);
        assert_eq!(
            captured.authorization.as_deref(),
            Some(format!("Bearer {TEST_TOKEN}").as_str())
        );

        mock.client(Some(token()))
            .set_yanked("fmtlib/fmt", &version("10.2.1"), false)
            .unwrap();
        assert_eq!(mock.captured().body, br#"{"yanked":false}"#);

        let mock = MockApi::respond_with(404, r#"{"errors":[{"detail":"unknown version"}]}"#);
        let err = mock
            .client(Some(token()))
            .set_yanked("fmtlib/fmt", &version("9.9.9"), true)
            .unwrap_err();
        match &err {
            RegistryApiError::NotFound { name, version } => {
                assert_eq!(name, "fmtlib/fmt");
                assert_eq!(version, "9.9.9");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// The API base's own path prefix is preserved when building
    /// routes.
    #[test]
    fn routes_join_under_a_base_path() {
        let api = RegistryApi::new("https://registry.example.com/base", None).unwrap();
        let url = api
            .package_route("fmtlib/fmt", &version("10.2.1"), "/yank")
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://registry.example.com/base/api/v1/packages/fmtlib/fmt/10.2.1/yank"
        );
    }
}
