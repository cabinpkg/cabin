//! Typed HTTP client for the experimental remote registry API
//! (`-Z remote-registry`).
//!
//! This crate owns the *mutating* half of the remote-registry
//! protocol specified in `docs/remote-registry.md`:
//!
//! - [`RegistryApi::publish`] - `PUT /api/v1/packages/<name>/<version>`
//!   with the crates.io-style length-prefixed body
//!   (`[u32 LE metadata_len][metadata][u32 LE archive_len][archive]`);
//! - [`RegistryApi::set_yanked`] -
//!   `PATCH /api/v1/packages/<name>/<version>/yank` with a JSON
//!   `{"yanked": bool}` body.
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

    /// `PUT <api>/api/v1/packages/<name>/<version>` with the framed
    /// metadata + archive body.
    ///
    /// # Errors
    /// Returns [`RegistryApiError::UnsafePackageName`] before any
    /// request when `name` fails the path-safety gate, and
    /// [`RegistryApiError::FrameTooLarge`] when either payload
    /// exceeds the `u32` framing limit.  Response statuses map per
    /// `docs/remote-registry.md`: `409` becomes
    /// [`RegistryApiError::VersionConflict`], `400` / `401` / `403`
    /// map like the read path, and any other non-success status
    /// surfaces as [`RegistryApiError::ServerError`] with the error
    /// envelope's `detail` when the body carries one.
    pub fn publish(
        &self,
        name: &str,
        version: &semver::Version,
        metadata_json: &[u8],
        archive: &[u8],
    ) -> Result<PublishOutcome, RegistryApiError> {
        let url = self.package_route(name, version, "")?;
        let body = encode_publish_body(metadata_json, archive)?;
        let request = self
            .request("PUT", &url)
            .set("Content-Type", "application/octet-stream");
        match self.send(request.send_bytes(&body), name, version)? {
            201 => Ok(PublishOutcome::Created),
            200 => Ok(PublishOutcome::AlreadyPublished),
            status => Err(RegistryApiError::ServerError {
                status,
                detail: None,
            }),
        }
    }

    /// `PATCH <api>/api/v1/packages/<name>/<version>/yank` with a
    /// JSON `{"yanked": bool}` body.  `true` yanks, `false` un-yanks;
    /// the route is idempotent.
    ///
    /// # Errors
    /// Returns [`RegistryApiError::UnsafePackageName`] before any
    /// request when `name` fails the path-safety gate.  Response
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
            200 => Ok(()),
            status => Err(RegistryApiError::ServerError {
                status,
                detail: None,
            }),
        }
    }

    /// `<api>/api/v1/packages/<name>/<version><suffix>`, with the
    /// package name re-validated at the URL boundary (defense in
    /// depth, mirroring `cabin-index-http`).
    fn package_route(
        &self,
        name: &str,
        version: &semver::Version,
        suffix: &str,
    ) -> Result<url::Url, RegistryApiError> {
        if !cabin_core::is_path_safe_package_name(name) {
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

    /// Map a `ureq` result into either a success status (2xx, for the
    /// caller to interpret) or the typed error for the shared
    /// protocol statuses.
    fn send(
        &self,
        result: Result<ureq::Response, ureq::Error>,
        name: &str,
        version: &semver::Version,
    ) -> Result<u16, RegistryApiError> {
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
                Ok(status)
            }
            Err(ureq::Error::Status(status, response)) => {
                let detail = envelope_detail(response);
                Err(match status {
                    400 => RegistryApiError::BadRequest { detail },
                    401 if self.token.is_some() => RegistryApiError::TokenRejected {
                        origin: self.origin.clone(),
                    },
                    401 => RegistryApiError::AuthRequired {
                        origin: self.origin.clone(),
                    },
                    // A tokenless 403 is not the protocol's
                    // missing-scope case (no scope was presented), so
                    // it keeps the generic mapping - same rule as the
                    // read path.
                    403 if self.token.is_some() => RegistryApiError::MissingScope {
                        origin: self.origin.clone(),
                    },
                    404 => RegistryApiError::NotFound {
                        name: name.to_owned(),
                        version: version.to_string(),
                    },
                    409 => RegistryApiError::VersionConflict {
                        name: name.to_owned(),
                        version: version.to_string(),
                    },
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
/// `{"errors":[{"detail":"..."}]}`.
#[derive(Deserialize)]
struct ErrorEnvelope {
    errors: Vec<ErrorEntry>,
}

#[derive(Deserialize)]
struct ErrorEntry {
    detail: String,
}

/// Read a non-2xx response body (capped) and extract the first error
/// envelope `detail`.  A malformed or missing envelope yields `None`,
/// so the caller's message degrades to the raw status.
fn envelope_detail(response: ureq::Response) -> Option<String> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES)
        .read_to_end(&mut body)
        .ok()?;
    let envelope: ErrorEnvelope = serde_json::from_slice(&body).ok()?;
    envelope.errors.into_iter().next().map(|entry| entry.detail)
}

/// Append the server's envelope `detail` to a base message when one
/// was present.
fn with_detail(base: String, detail: Option<&String>) -> String {
    match detail {
        Some(detail) => format!("{base}: {detail}"),
        None => base,
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
        "package name `{name}` is not valid; package names must consist only of ASCII letters, ASCII digits, `_`, `-`, and `.`, must be non-empty, must not start with `.` or `-`, and must not be `.` or `..`"
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

    #[error(
        "registry API `{origin}` refused the request: the stored token does not have the \
         required scope"
    )]
    MissingScope { origin: String },

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

    #[test]
    fn publish_body_round_trips_through_the_decoder() {
        let metadata = br#"{"schema":1,"name":"fmt"}"#;
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
    fn unsafe_package_names_never_reach_the_wire() {
        // No server bound: an attempted request would surface as a
        // transport error, so getting `UnsafePackageName` proves the
        // gate fires first.
        let api = RegistryApi::new("http://127.0.0.1:9", Some(token())).unwrap();
        for name in ["../evil", "foo/bar", ".hidden", "-flag"] {
            let err = api
                .publish(name, &version("1.0.0"), b"{}", b"")
                .unwrap_err();
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
            let server = Arc::new(
                tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
            );
            let addr = server.server_addr().to_ip().expect("loopback addr");
            let url = format!("http://{addr}");
            let (sender, captured) = mpsc::channel();
            let server_for_thread = Arc::clone(&server);
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
                    let _ = req
                        .respond(tiny_http::Response::from_string(body).with_status_code(status));
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
        let metadata = br#"{"schema":1,"name":"fmt","version":"10.2.1"}"#;
        let archive = b"\x1f\x8b\x08\x00fake-gzip-bytes";

        let outcome = mock
            .client(Some(token()))
            .publish("fmt", &version("10.2.1"), metadata, archive)
            .unwrap();
        assert_eq!(outcome, PublishOutcome::Created);

        let captured = mock.captured();
        assert_eq!(captured.method, "PUT");
        assert_eq!(captured.path, "/api/v1/packages/fmt/10.2.1");
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
        let outcome = mock
            .client(Some(token()))
            .publish("fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap();
        assert_eq!(outcome, PublishOutcome::AlreadyPublished);
    }

    /// 409: the version exists with different bytes and stays
    /// immutable.
    #[test]
    fn publish_maps_409_to_version_conflict() {
        let mock = MockApi::respond_with(409, r#"{"errors":[{"detail":"checksum mismatch"}]}"#);
        let err = mock
            .client(Some(token()))
            .publish("fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::VersionConflict { name, version } => {
                assert_eq!(name, "fmt");
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
            .publish("fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        assert!(
            matches!(err, RegistryApiError::AuthRequired { .. }),
            "{err:?}"
        );
        assert_eq!(mock.captured().authorization, None);

        let err = mock
            .client(Some(token()))
            .publish("fmt", &version("10.2.1"), b"{}", b"bytes")
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

    /// 403 with a token is the protocol's missing-scope case.
    #[test]
    fn publish_maps_403_to_missing_scope() {
        let mock = MockApi::respond_with(403, r#"{"errors":[{"detail":"missing publish scope"}]}"#);
        let err = mock
            .client(Some(token()))
            .publish("fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        match &err {
            RegistryApiError::MissingScope { origin } => assert_eq!(origin, &mock.url),
            other => panic!("expected MissingScope, got {other:?}"),
        }
        assert!(err.to_string().contains("scope"), "{err}");
    }

    /// A well-formed envelope's `detail` reaches the 400 message; a
    /// malformed one degrades to the raw status.
    #[test]
    fn error_envelope_parses_and_degrades_to_the_raw_status() {
        let mock =
            MockApi::respond_with(400, r#"{"errors":[{"detail":"metadata name mismatch"}]}"#);
        let err = mock
            .client(Some(token()))
            .publish("fmt", &version("10.2.1"), b"{}", b"bytes")
            .unwrap_err();
        assert!(
            err.to_string().contains("metadata name mismatch"),
            "expected the envelope detail in: {err}"
        );

        let mock = MockApi::respond_with(400, "<html>not the envelope</html>");
        let err = mock
            .client(Some(token()))
            .publish("fmt", &version("10.2.1"), b"{}", b"bytes")
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
            .publish("fmt", &version("10.2.1"), b"{}", b"bytes")
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
            .set_yanked("fmt", &version("10.2.1"), true)
            .unwrap();
        let captured = mock.captured();
        assert_eq!(captured.method, "PATCH");
        assert_eq!(captured.path, "/api/v1/packages/fmt/10.2.1/yank");
        assert_eq!(captured.body, br#"{"yanked":true}"#);
        assert_eq!(
            captured.authorization.as_deref(),
            Some(format!("Bearer {TEST_TOKEN}").as_str())
        );

        mock.client(Some(token()))
            .set_yanked("fmt", &version("10.2.1"), false)
            .unwrap();
        assert_eq!(mock.captured().body, br#"{"yanked":false}"#);

        let mock = MockApi::respond_with(404, r#"{"errors":[{"detail":"unknown version"}]}"#);
        let err = mock
            .client(Some(token()))
            .set_yanked("fmt", &version("9.9.9"), true)
            .unwrap_err();
        match &err {
            RegistryApiError::NotFound { name, version } => {
                assert_eq!(name, "fmt");
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
            .package_route("fmt", &version("10.2.1"), "/yank")
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://registry.example.com/base/api/v1/packages/fmt/10.2.1/yank"
        );
    }
}
