use std::io::Read;
use std::time::Duration;

use cabin_credentials::{CredentialsError, Token};

use crate::error::IndexHttpError;

/// Default per-request timeout for the sparse HTTP client.  Static
/// registries are usually fast (cached object stores or local
/// servers); a long timeout is rarely useful and surfaces broken
/// links quickly.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum body size we will read for a single response.  Generous
/// enough for a per-package metadata document or a typical source
/// archive, conservative enough to refuse a runaway response.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Cap on how much of a non-2xx body is read looking for the error
/// envelope's `code`.  Envelopes are tiny; a body bigger than this is a
/// proxy error page, not one - and [`MAX_BODY_BYTES`] is three orders of
/// magnitude too generous to spend on a refusal.  Matches the write
/// client's cap in `cabin-registry-api`.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// The envelope `code` the registry's budget breaker sets
/// (`docs/remote-registry.md`, "Error envelope").  It is what separates
/// a breaker refusal from every other `503` on the wire - Cloudflare's
/// edge and the Workers runtime emit bare `503`s of their own.
const OVER_BUDGET_CODE: &str = "registry_over_budget";

/// A bearer credential the client may attach to registry requests:
/// the normalized origin the token is scoped to plus the token
/// itself.  Constructed via [`RegistryAuth::for_index_url`], which
/// normalizes the origin, so a token can never be scoped to a URL
/// with a path or with userinfo.
#[derive(Clone)]
pub struct RegistryAuth {
    origin: String,
    token: Token,
}

impl RegistryAuth {
    /// Scope `token` to the origin of `index_url`.
    ///
    /// # Errors
    /// Returns [`IndexHttpError::InvalidUrl`] when `index_url` is
    /// not a valid `http(s)` URL or carries userinfo credentials.
    pub fn for_index_url(index_url: &str, token: Token) -> Result<Self, IndexHttpError> {
        let origin = cabin_credentials::normalize_origin(index_url).map_err(|err| match err {
            CredentialsError::InvalidOrigin { url, message } => {
                IndexHttpError::InvalidUrl { url, message }
            }
            other => IndexHttpError::InvalidUrl {
                url: index_url.to_owned(),
                message: other.to_string(),
            },
        })?;
        Ok(Self { origin, token })
    }

    /// Whether the credential may be attached to a request for
    /// `url`: the URL's origin must equal the credential's origin
    /// exactly, and plain `http` is refused unless the host is
    /// loopback (`127.0.0.0/8`, `::1`, `localhost`) so a token is
    /// never sent in cleartext beyond local testing.
    fn applies_to(&self, url: &url::Url) -> bool {
        let same_origin = url.origin().ascii_serialization() == self.origin;
        same_origin && (url.scheme() == "https" || cabin_credentials::url_is_loopback(url.as_str()))
    }
}

impl std::fmt::Debug for RegistryAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Token`'s own `Debug` redacts; keep the origin visible so
        // debug output still says which registry the auth targets.
        f.debug_struct("RegistryAuth")
            .field("origin", &self.origin)
            .field("token", &self.token)
            .finish()
    }
}

/// Thin blocking HTTP client used by the sparse index source.  Wraps
/// `ureq::Agent` so callers do not have to mention `ureq` directly.
#[derive(Clone)]
pub struct HttpClient {
    agent: ureq::Agent,
    max_body_bytes: usize,
    auth: Option<RegistryAuth>,
}

impl HttpClient {
    /// Build a client with sensible defaults: 30 s timeout, body
    /// reads capped at 64 MiB, the default `ureq` TLS configuration,
    /// and redirects disabled.  Disabling redirects keeps fetches
    /// pinned to the operator-configured registry origin; the module
    /// docs already promise this behavior.
    pub fn new() -> Self {
        Self::with_redirect_budget(0)
    }

    /// Build a client whose agent follows up to `max_redirects`
    /// HTTP 3xx responses.  Use only for downloads whose
    /// integrity is established by an out-of-band pin (SHA-256 in
    /// a foundation-port recipe); the sparse-HTTP-index read path
    /// must keep using [`HttpClient::new`] so a registry cannot
    /// redirect metadata fetches to a different origin.
    pub fn with_redirect_budget(max_redirects: u32) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(DEFAULT_TIMEOUT)
            .redirects(max_redirects)
            .build();
        Self {
            agent,
            max_body_bytes: MAX_BODY_BYTES,
            auth: None,
        }
    }

    /// Attach a registry credential: every request whose URL is on
    /// the credential's exact origin (and satisfies the cleartext
    /// rule, see [`RegistryAuth`]) carries `Authorization: Bearer
    /// <token>`; requests to any other origin never do.  Callers
    /// must combine this with the redirect-free [`HttpClient::new`]
    /// client - the redirect-following variant is reserved for
    /// unauthenticated pinned downloads.
    #[must_use]
    pub fn with_auth(mut self, auth: RegistryAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// `GET` `url` and return the raw response body. `package` is
    /// embedded into errors so HTTP failures surface a useful
    /// caller-provided context.
    ///
    /// When a credential is attached ([`HttpClient::with_auth`]) and
    /// [`RegistryAuth`]'s origin / cleartext rules allow it for this
    /// URL, the request carries `Authorization: Bearer <token>`.
    ///
    /// # Errors
    /// Returns [`IndexHttpError::PackageNotFound`] on a 404, and
    /// [`IndexHttpError::ServerError`] on a 3xx (redirects are not
    /// followed) or any other non-success status.  A 401 maps to
    /// [`IndexHttpError::AuthRequired`] when the request carried no
    /// token and [`IndexHttpError::TokenRejected`] when it did; a 403
    /// maps to [`IndexHttpError::MissingScope`] only when the request
    /// carried a token (a tokenless 403 stays a
    /// [`IndexHttpError::ServerError`]); a 503 whose envelope carries
    /// the `registry_over_budget` code maps to
    /// [`IndexHttpError::RegistryOverBudget`] with the response's
    /// `Retry-After` seconds when usable (the registry's read-side
    /// budget breaker), while any other 503 stays a
    /// [`IndexHttpError::ServerError`].  Returns
    /// [`IndexHttpError::Transport`] when reading the body fails,
    /// when the body exceeds the 64 MiB cap, or on a `ureq` transport
    /// error.
    pub fn get_bytes(&self, url: &str, package: &str) -> Result<Vec<u8>, IndexHttpError> {
        // The auth decision is per request URL, not per client: the
        // token is only ever sent to the exact origin it is stored
        // under, and never in cleartext beyond loopback.
        let auth = self
            .auth
            .as_ref()
            .filter(|auth| url::Url::parse(url).is_ok_and(|parsed| auth.applies_to(&parsed)));
        let mut request = self.agent.get(url);
        if let Some(auth) = auth {
            request = request.set("Authorization", &format!("Bearer {}", auth.token.expose()));
        }
        match request.call() {
            Ok(response) => {
                // `.redirects(0)` on the agent means redirects are not
                // followed, but ureq still returns the 3xx response as
                // `Ok`.  Reject it explicitly so a registry that 3xx's
                // out to a different origin surfaces as an error
                // instead of silently producing an empty body.
                let status = response.status();
                if (300..400).contains(&status) {
                    return Err(IndexHttpError::ServerError {
                        name: package.to_owned(),
                        status,
                    });
                }
                let mut reader = response.into_reader().take(self.max_body_bytes as u64 + 1);
                let mut body = Vec::new();
                reader
                    .read_to_end(&mut body)
                    .map_err(|err| IndexHttpError::Transport {
                        name: package.to_owned(),
                        message: err.to_string(),
                    })?;
                if body.len() > self.max_body_bytes {
                    return Err(IndexHttpError::Transport {
                        name: package.to_owned(),
                        message: format!("response body exceeded {} bytes", self.max_body_bytes),
                    });
                }
                Ok(body)
            }
            Err(ureq::Error::Status(404, _)) => Err(IndexHttpError::PackageNotFound {
                name: package.to_owned(),
            }),
            // Auth statuses are mapped on whether *this request*
            // carried a token: a 401 without one means the registry
            // wants a login, a 401 despite one means the stored token
            // is no longer valid, and a 403 despite one means the
            // token is valid but lacks the scope the route requires.
            // The tokenless 401 advice applies even without
            // `-Z remote-registry` on the command line - a 401 can
            // only mean the registry wants auth, and the message
            // itself names the experimental flag the user must opt
            // into.  A tokenless 403 is *not* the protocol's
            // missing-scope case (no scope was presented), so it
            // keeps the generic status mapping below.
            Err(ureq::Error::Status(401, _)) => Err(if auth.is_some() {
                IndexHttpError::TokenRejected {
                    origin: origin_for_error(url),
                }
            } else {
                IndexHttpError::AuthRequired {
                    origin: origin_for_error(url),
                }
            }),
            Err(ureq::Error::Status(403, _)) if auth.is_some() => {
                Err(IndexHttpError::MissingScope {
                    origin: origin_for_error(url),
                })
            }
            // The registry's read-side budget breaker
            // (`registry/docs/architecture.md`, "Billing model and the
            // budget breaker"): reads are refused service-wide until the
            // budget window resets.  It answers `503`, not the `402` it
            // used before ("Why 503, not 402"), and `503` is a status
            // Cloudflare's own edge and runtime also emit - so the
            // envelope `code`, not the status, is what identifies the
            // breaker.  Without it this stays the generic server error
            // it was before the breaker existed, rather than blaming a
            // platform outage on the registry's budget.  `Retry-After`
            // (delta seconds) rides on the refusal and is read before
            // the body consumes the response; a missing or non-numeric
            // value (an HTTP date, say) degrades to no hint, mirroring
            // the publish-side mapping in `cabin-registry-api`.
            Err(ureq::Error::Status(503, response)) => {
                let retry_after_secs = response
                    .header("Retry-After")
                    .and_then(|value| value.trim().parse::<u64>().ok());
                Err(
                    if envelope_code(response).as_deref() == Some(OVER_BUDGET_CODE) {
                        IndexHttpError::RegistryOverBudget { retry_after_secs }
                    } else {
                        IndexHttpError::ServerError {
                            name: package.to_owned(),
                            status: 503,
                        }
                    },
                )
            }
            Err(ureq::Error::Status(status, _)) => Err(IndexHttpError::ServerError {
                name: package.to_owned(),
                status,
            }),
            Err(ureq::Error::Transport(transport)) => Err(IndexHttpError::Transport {
                name: package.to_owned(),
                message: transport.to_string(),
            }),
        }
    }

    /// `GET` `url` and return the raw response body.  Used by the CLI
    /// to download artifacts; checksum verification happens later in
    /// `cabin-artifact`.
    ///
    /// # Errors
    /// Mirrors [`HttpClient::get_bytes`] but remaps a 404 into
    /// [`IndexHttpError::Transport`] ("artifact not found (404)"), so
    /// it never returns [`IndexHttpError::PackageNotFound`].  All other
    /// errors ([`IndexHttpError::ServerError`],
    /// [`IndexHttpError::Transport`]) are propagated unchanged.
    pub fn download(&self, url: &str, label: &str) -> Result<Vec<u8>, IndexHttpError> {
        // Download paths share the same plumbing as metadata
        // requests: the `label` field of the error tells the user
        // *which* package's archive failed to download.
        self.get_bytes(url, label).map_err(|err| match err {
            IndexHttpError::PackageNotFound { name } => IndexHttpError::Transport {
                name,
                message: "artifact not found (404)".to_owned(),
            },
            other => other,
        })
    }
}

/// Serde shape of the registry's error envelope
/// (`docs/remote-registry.md`, "Error envelope").  Only the
/// machine-readable `code` is read here - the rendered message is the
/// client's own, and unknown fields are ignored by contract.
#[derive(serde::Deserialize)]
struct ErrorEnvelope {
    errors: Vec<ErrorEntry>,
}

#[derive(serde::Deserialize)]
struct ErrorEntry {
    #[serde(default)]
    code: Option<String>,
}

/// The first envelope entry's `code`, read from a capped body.  Missing,
/// oversized, malformed, and code-less bodies all yield `None`, so an
/// intermediary that replaced the response with its own error page can
/// never be mistaken for a coded refusal.
///
/// The cap is a rejection, not a truncation: reading one byte past it and
/// refusing what overflows is what stops an oversized body whose first
/// 64 KiB happens to parse - a coded envelope followed by padding - from
/// being accepted as the envelope it is not.
fn envelope_code(response: ureq::Response) -> Option<String> {
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_ERROR_BODY_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .ok()?;
    if body.len() > MAX_ERROR_BODY_BYTES {
        return None;
    }
    serde_json::from_slice::<ErrorEnvelope>(&body)
        .ok()?
        .errors
        .into_iter()
        .next()?
        .code
}

/// Origin string used in auth error messages: the normalized origin
/// of `url`, falling back to a userinfo-redacted spelling for the
/// (unreachable through the index paths, which reject credential
/// URLs) case where normalization refuses the URL - a raw fallback
/// could echo a `user:pw@` credential into the diagnostic.
fn origin_for_error(url: &str) -> String {
    cabin_credentials::normalize_origin(url)
        .unwrap_or_else(|_| crate::source::redact_raw_url_userinfo(url))
}

/// Deadline for the login-URL discovery probe.  Deliberately far below
/// [`DEFAULT_TIMEOUT`]: the probe is advisory (`cabin login` prints a
/// generic hint without it), so an offline machine must fail it in a
/// couple of seconds, never hang the login for the full read timeout.
const LOGIN_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Discover an `auth-required` registry's token-creation page: one
/// unauthenticated `GET` of `<index_url>/config.json`, expecting the
/// uniform 401 whose `WWW-Authenticate` header carries the
/// `Cabin login_url="<url>"` challenge (`docs/remote-registry.md`,
/// "Authentication").  Every other outcome - a 2xx (the registry does
/// not require auth), any other status, a missing or malformed
/// challenge, an implausible URL, or a transport failure (offline) -
/// is `None`: the probe must never block `cabin login`.
///
/// The probe never carries a credential (no `Authorization` header,
/// whatever `CABIN_REGISTRY_TOKEN` or `credentials.toml` hold): it
/// exists to see what an unauthenticated caller is told.
pub fn fetch_login_url(index_url: &str) -> Option<String> {
    let base = crate::source::parse_base_url(index_url).ok()?;
    let config_url = base.join("config.json").ok()?;
    let agent = ureq::AgentBuilder::new()
        .timeout(LOGIN_PROBE_TIMEOUT)
        .redirects(0)
        .build();
    match agent.get(config_url.as_str()).call() {
        Err(ureq::Error::Status(401, response)) => response
            .header("www-authenticate")
            .and_then(parse_login_url),
        _ => None,
    }
}

/// Parses the `Cabin login_url="<url>"` challenge.  Deliberately strict
/// to the one challenge the protocol defines (scheme token `Cabin`,
/// parameter `login_url`), not a general RFC 7235 parser.  The value
/// must parse as an absolute `http(s)` URL without userinfo or control
/// characters - anything else is not worth printing to a terminal.
fn parse_login_url(header: &str) -> Option<String> {
    let header = header.trim();
    let scheme_len = "Cabin".len();
    if header.len() < scheme_len || !header.is_char_boundary(scheme_len) {
        return None;
    }
    let (scheme, params) = header.split_at(scheme_len);
    if !scheme.eq_ignore_ascii_case("Cabin") || !params.starts_with(char::is_whitespace) {
        return None;
    }
    // The parameter name must sit on a boundary: `not_login_url="..."`
    // is a different parameter, not ours. `params` starts with
    // whitespace, so every match has a preceding byte.
    let marker = "login_url=\"";
    let start = params
        .match_indices(marker)
        .find(|(idx, _)| matches!(params.as_bytes()[idx - 1], b' ' | b'\t' | b','))?
        .0;
    let value = &params[start + marker.len()..];
    let url = &value[..value.find('"')?];
    let parsed = url::Url::parse(url).ok()?;
    let plausible = matches!(parsed.scheme(), "http" | "https")
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && !url.chars().any(char::is_control);
    plausible.then(|| url.to_owned())
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("max_body_bytes", &self.max_body_bytes)
            // `RegistryAuth`'s Debug redacts the token itself.
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread::JoinHandle;

    /// Tiny HTTP server that answers `/from` with a 302 redirect to
    /// `/to`, `/to` with `200 OK` carrying a known body, `/boom` with
    /// a 500, and anything else with a 404.  Used to exercise each
    /// `get_bytes` response branch without external network access.
    struct RedirectServer {
        server: Arc<tiny_http::Server>,
        thread: Option<JoinHandle<()>>,
        url: String,
    }

    impl RedirectServer {
        fn start() -> Self {
            let server = Arc::new(
                tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
            );
            let addr = server.server_addr().to_ip().expect("loopback addr");
            let url = format!("http://{addr}");
            let target = url.clone();
            let server_for_thread = Arc::clone(&server);
            let thread = std::thread::spawn(move || {
                while let Ok(req) = server_for_thread.recv() {
                    let path = req.url().to_string();
                    if path == "/from" {
                        let location = format!("{target}/to");
                        let header =
                            tiny_http::Header::from_bytes(&b"Location"[..], location.as_bytes())
                                .expect("header");
                        let _ = req.respond(tiny_http::Response::empty(302).with_header(header));
                    } else if path == "/to" {
                        let _ = req.respond(tiny_http::Response::from_string("followed"));
                    } else if path == "/boom" {
                        let _ = req.respond(tiny_http::Response::empty(500));
                    } else {
                        let _ = req.respond(tiny_http::Response::empty(404));
                    }
                }
            });
            Self {
                server,
                thread: Some(thread),
                url,
            }
        }

        fn url(&self) -> &str {
            &self.url
        }
    }

    impl Drop for RedirectServer {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }

    #[test]
    fn get_bytes_does_not_follow_redirects() {
        let server = RedirectServer::start();
        let client = HttpClient::new();

        let result = client.get_bytes(&format!("{}/from", server.url()), "pkg");

        match result {
            Err(IndexHttpError::ServerError { status, .. }) => {
                assert_eq!(
                    status, 302,
                    "expected 302 status surfaced as ServerError, got {status}"
                );
            }
            Ok(body) => panic!(
                "redirect should not be followed, but body was: {:?}",
                String::from_utf8_lossy(&body)
            ),
            Err(other) => panic!("expected ServerError(302), got {other:?}"),
        }
    }

    #[test]
    fn get_bytes_returns_body_on_success() {
        let server = RedirectServer::start();
        let client = HttpClient::new();

        let body = client
            .get_bytes(&format!("{}/to", server.url()), "pkg")
            .expect("2xx response with a small body succeeds");

        assert_eq!(body, b"followed");
    }

    #[test]
    fn get_bytes_maps_404_to_package_not_found() {
        let server = RedirectServer::start();
        let client = HttpClient::new();

        let result = client.get_bytes(&format!("{}/missing", server.url()), "pkg");

        match result {
            Err(IndexHttpError::PackageNotFound { name }) => assert_eq!(name, "pkg"),
            other => panic!("expected PackageNotFound, got {other:?}"),
        }
    }

    #[test]
    fn get_bytes_maps_5xx_to_server_error() {
        let server = RedirectServer::start();
        let client = HttpClient::new();

        let result = client.get_bytes(&format!("{}/boom", server.url()), "pkg");

        match result {
            Err(IndexHttpError::ServerError { name, status }) => {
                assert_eq!(name, "pkg");
                assert_eq!(status, 500);
            }
            other => panic!("expected ServerError(500), got {other:?}"),
        }
    }

    #[test]
    fn get_bytes_rejects_body_exceeding_cap() {
        let server = RedirectServer::start();
        // Shrink the cap below the 8-byte "followed" body so the
        // test does not have to stream 64 MiB through loopback.
        let client = HttpClient {
            agent: ureq::AgentBuilder::new()
                .timeout(DEFAULT_TIMEOUT)
                .redirects(0)
                .build(),
            max_body_bytes: 4,
            auth: None,
        };

        let result = client.get_bytes(&format!("{}/to", server.url()), "pkg");

        match result {
            Err(IndexHttpError::Transport { name, message }) => {
                assert_eq!(name, "pkg");
                assert!(
                    message.contains("exceeded 4 bytes"),
                    "message should mention the cap, got: {message}"
                );
            }
            other => panic!("expected Transport error, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Authenticated reads (`-Z remote-registry` client plumbing)
    // -----------------------------------------------------------------

    const TEST_TOKEN: &str = "cabin_testToken12345";

    /// Tiny server for the auth tests: `/echo-auth` answers 200 with
    /// the request's `Authorization` header value (or `none`),
    /// `/needs-auth` answers 401, and `/needs-scope` answers 403.
    struct AuthServer {
        server: Arc<tiny_http::Server>,
        thread: Option<JoinHandle<()>>,
        url: String,
    }

    impl AuthServer {
        fn start() -> Self {
            let server = Arc::new(
                tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
            );
            let addr = server.server_addr().to_ip().expect("loopback addr");
            let url = format!("http://{addr}");
            let server_for_thread = Arc::clone(&server);
            let thread = std::thread::spawn(move || {
                while let Ok(req) = server_for_thread.recv() {
                    let path = req.url().to_string();
                    if path == "/echo-auth" {
                        let value = req
                            .headers()
                            .iter()
                            .find(|h| h.field.equiv("Authorization"))
                            .map_or_else(|| "none".to_owned(), |h| h.value.to_string());
                        let _ = req.respond(tiny_http::Response::from_string(value));
                    } else if path == "/needs-auth" {
                        let _ = req.respond(tiny_http::Response::empty(401));
                    } else if path == "/needs-scope" {
                        let _ = req.respond(tiny_http::Response::empty(403));
                    } else {
                        let _ = req.respond(tiny_http::Response::empty(404));
                    }
                }
            });
            Self {
                server,
                thread: Some(thread),
                url,
            }
        }
    }

    impl Drop for AuthServer {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }

    fn auth_for(url: &str) -> RegistryAuth {
        RegistryAuth::for_index_url(url, Token::parse(TEST_TOKEN).unwrap()).unwrap()
    }

    /// The header is absent without a credential and present (with
    /// the exact `Bearer <token>` shape) when one is attached for
    /// the request origin.  Loopback `http` is the documented
    /// cleartext exception, which is what lets this test observe
    /// the header without a TLS server.
    #[test]
    fn authorization_header_present_exactly_when_credential_attached() {
        let server = AuthServer::start();
        let url = format!("{}/echo-auth", server.url);

        let body = HttpClient::new().get_bytes(&url, "pkg").unwrap();
        assert_eq!(body, b"none", "no credential must mean no header");

        let client = HttpClient::new().with_auth(auth_for(&server.url));
        let body = client.get_bytes(&url, "pkg").unwrap();
        assert_eq!(body, format!("Bearer {TEST_TOKEN}").as_bytes());
    }

    /// A credential scoped to a different origin is never attached,
    /// even though the request itself succeeds.
    #[test]
    fn authorization_header_absent_for_other_origins() {
        let server = AuthServer::start();
        let client = HttpClient::new().with_auth(auth_for("http://localhost:1/registry"));
        let body = client
            .get_bytes(&format!("{}/echo-auth", server.url), "pkg")
            .unwrap();
        assert_eq!(body, b"none");
    }

    /// The cleartext rule, exercised as a pure predicate: plain
    /// `http` is refused for non-loopback hosts even when the origin
    /// matches, while `https` and the loopback spellings pass.
    #[test]
    fn credential_refuses_cleartext_http_beyond_loopback() {
        for (index_url, request_url, expected) in [
            (
                "https://registry.example.com",
                "https://registry.example.com/config.json",
                true,
            ),
            (
                "http://registry.example.com",
                "http://registry.example.com/config.json",
                false,
            ),
            (
                "http://127.0.0.1:8080",
                "http://127.0.0.1:8080/config.json",
                true,
            ),
            ("http://127.5.6.7", "http://127.5.6.7/config.json", true),
            ("http://[::1]:8080", "http://[::1]:8080/config.json", true),
            (
                "http://localhost:8080",
                "http://localhost:8080/config.json",
                true,
            ),
            // Exact-origin rule: scheme, host, and port all match.
            (
                "https://registry.example.com",
                "https://registry.example.com:444/x",
                false,
            ),
            (
                "https://registry.example.com",
                "https://other.example.com/x",
                false,
            ),
            (
                "https://registry.example.com",
                "http://registry.example.com/x",
                false,
            ),
        ] {
            let auth = auth_for(index_url);
            let parsed = url::Url::parse(request_url).unwrap();
            assert_eq!(
                auth.applies_to(&parsed),
                expected,
                "auth for {index_url} against {request_url}"
            );
        }
    }

    /// 401 without a token advises `cabin login` for the origin.
    #[test]
    fn get_bytes_maps_401_without_credential_to_auth_required() {
        let server = AuthServer::start();
        let err = HttpClient::new()
            .get_bytes(&format!("{}/needs-auth", server.url), "pkg")
            .unwrap_err();
        match err {
            IndexHttpError::AuthRequired { ref origin } => {
                assert_eq!(origin, &server.url);
                let message = err.to_string();
                assert!(
                    message.contains(&format!("cabin login --index-url {}", server.url)),
                    "message must advise cabin login: {message}"
                );
                assert!(
                    message.contains("-Z remote-registry"),
                    "message must name the experimental flag: {message}"
                );
            }
            other => panic!("expected AuthRequired, got {other:?}"),
        }
    }

    /// 401 despite a token means the token is no longer valid.
    #[test]
    fn get_bytes_maps_401_with_credential_to_token_rejected() {
        let server = AuthServer::start();
        let client = HttpClient::new().with_auth(auth_for(&server.url));
        let err = client
            .get_bytes(&format!("{}/needs-auth", server.url), "pkg")
            .unwrap_err();
        match err {
            IndexHttpError::TokenRejected { ref origin } => {
                assert_eq!(origin, &server.url);
                let message = err.to_string();
                assert!(
                    message.contains("revoked or expired"),
                    "message must explain the rejection: {message}"
                );
                assert!(
                    message.contains("re-run `cabin login"),
                    "message must advise re-running cabin login: {message}"
                );
                assert!(
                    !message.contains(TEST_TOKEN),
                    "token bytes must never surface in errors: {message}"
                );
            }
            other => panic!("expected TokenRejected, got {other:?}"),
        }
    }

    /// 403 means the token is valid but under-scoped.
    #[test]
    fn get_bytes_maps_403_to_missing_scope() {
        let server = AuthServer::start();
        let client = HttpClient::new().with_auth(auth_for(&server.url));
        let err = client
            .get_bytes(&format!("{}/needs-scope", server.url), "pkg")
            .unwrap_err();
        match err {
            IndexHttpError::MissingScope { ref origin } => {
                assert_eq!(origin, &server.url);
                let message = err.to_string();
                assert!(
                    message.contains("scope"),
                    "message must mention the missing scope: {message}"
                );
            }
            other => panic!("expected MissingScope, got {other:?}"),
        }
    }

    /// A 503 carrying the `registry_over_budget` code is the registry's
    /// read-side budget breaker: the message says reads are paused and
    /// carries the `Retry-After` seconds when the header is usable,
    /// degrading to "try again later" otherwise. The mapping is shared by
    /// metadata reads and artifact downloads (`download` remaps only the
    /// 404).
    #[test]
    fn get_bytes_maps_a_coded_503_to_registry_over_budget() {
        const ENVELOPE: &str = r#"{"errors":[{"detail":"registry downloads are temporarily disabled: the registry's read budget is exhausted","code":"registry_over_budget"}]}"#;

        let (server, url, thread) = challenge_server(503, ENVELOPE, &[("Retry-After", "900")]);
        let err = HttpClient::new()
            .get_bytes(&format!("{url}/packages/smoke/withdep.json"), "pkg")
            .unwrap_err();
        match &err {
            IndexHttpError::RegistryOverBudget {
                retry_after_secs: Some(900),
            } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("downloads"), "{message}");
        assert!(message.contains("budget"), "{message}");
        assert!(
            message.contains("try again in 900 seconds"),
            "expected the Retry-After hint in: {message}"
        );
        server.unblock();
        let _ = thread.join();

        let (server, url, thread) = challenge_server(503, ENVELOPE, &[]);
        let err = HttpClient::new()
            .download(&format!("{url}/artifacts/smoke/withdep/a.zip"), "pkg")
            .unwrap_err();
        match &err {
            IndexHttpError::RegistryOverBudget {
                retry_after_secs: None,
            } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
        assert!(err.to_string().contains("try again later"), "{err}");
        server.unblock();
        let _ = thread.join();

        // A body exactly at the read cap is still an envelope; the
        // rejection starts one byte later.
        let (server, url, thread) =
            challenge_server(503, padded_envelope(MAX_ERROR_BODY_BYTES), &[]);
        let err = HttpClient::new()
            .get_bytes(&format!("{url}/packages/smoke/withdep.json"), "pkg")
            .unwrap_err();
        match &err {
            IndexHttpError::RegistryOverBudget { .. } => {}
            other => panic!("expected RegistryOverBudget, got {other:?}"),
        }
        server.unblock();
        let _ = thread.join();
    }

    /// A coded envelope padded out to `len` bytes: valid JSON on its own,
    /// which is exactly what a truncating cap would let through.
    fn padded_envelope(len: usize) -> String {
        let mut body =
            r#"{"errors":[{"detail":"over budget","code":"registry_over_budget"}]}"#.to_owned();
        body.extend(std::iter::repeat_n(' ', len - body.len()));
        body
    }

    /// The code, not the status, identifies the breaker: Cloudflare's
    /// edge and the Workers runtime emit bare 503s of their own, and a
    /// platform outage must not be reported as the registry's budget.
    /// A different code, a near-miss code, no entry, no code, an
    /// unparsable body, and a body past the read cap all stay the
    /// generic server error - as does the breaker's old 402, which has
    /// no mapping left.
    #[test]
    fn get_bytes_keeps_uncoded_503s_and_the_old_402_generic() {
        for (status, body) in [
            (
                503,
                r#"{"errors":[{"detail":"origin is unreachable"}]}"#.to_owned(),
            ),
            (
                503,
                r#"{"errors":[{"detail":"nope","code":"something_else"}]}"#.to_owned(),
            ),
            // The comparison is exact: a code the real one is a prefix
            // of must not match.
            (
                503,
                r#"{"errors":[{"detail":"nope","code":"registry_over_budgets"}]}"#.to_owned(),
            ),
            (503, r#"{"errors":[]}"#.to_owned()),
            (
                503,
                "<html><body>Service Unavailable</body></html>".to_owned(),
            ),
            // One byte past the cap.  The envelope prefix parses on its
            // own, so only a rejecting (not truncating) cap keeps this
            // generic.
            (503, padded_envelope(MAX_ERROR_BODY_BYTES + 1)),
            (
                402,
                r#"{"errors":[{"detail":"over budget","code":"registry_over_budget"}]}"#.to_owned(),
            ),
        ] {
            let label = format!("{status} {}", &body[..body.len().min(72)]);
            let (server, url, thread) = challenge_server(status, body, &[("Retry-After", "900")]);
            let err = HttpClient::new()
                .get_bytes(&format!("{url}/packages/smoke/withdep.json"), "pkg")
                .unwrap_err();
            match &err {
                IndexHttpError::ServerError { name, status: got } => {
                    assert_eq!(name, "pkg");
                    assert_eq!(*got, status, "case: {label}");
                }
                other => panic!("expected ServerError({status}), got {other:?}"),
            }
            assert!(!err.to_string().contains("infrastructure budget"), "{err}");
            server.unblock();
            let _ = thread.join();
        }
    }

    /// A 403 on a request that carried no token is not the
    /// protocol's missing-scope case: it keeps the pre-existing
    /// generic status mapping, so unauthenticated flows against a
    /// 403-ing host are not told to debug a nonexistent stored
    /// token.
    #[test]
    fn get_bytes_maps_403_without_credential_to_server_error() {
        let server = AuthServer::start();
        let err = HttpClient::new()
            .get_bytes(&format!("{}/needs-scope", server.url), "pkg")
            .unwrap_err();
        match err {
            IndexHttpError::ServerError { name, status } => {
                assert_eq!(name, "pkg");
                assert_eq!(status, 403);
            }
            other => panic!("expected ServerError(403), got {other:?}"),
        }
    }

    /// The auth-error origin fallback never echoes userinfo: a URL
    /// that fails origin normalization is redacted, not passed
    /// through raw.
    #[test]
    fn origin_for_error_redacts_userinfo_on_fallback() {
        // Userinfo URLs fail `normalize_origin`, so this exercises
        // the fallback arm.
        let rendered = origin_for_error("http://user:pw@registry.example.com/x");
        assert!(
            !rendered.contains("user:pw"),
            "credentials must be redacted: {rendered}"
        );
        // The happy path still yields the normalized origin.
        assert_eq!(
            origin_for_error("https://registry.example.com/x"),
            "https://registry.example.com"
        );
    }

    /// The redaction contract holds through the client's own Debug
    /// output too.
    #[test]
    fn client_debug_output_redacts_the_token() {
        let client = HttpClient::new().with_auth(auth_for("https://registry.example.com"));
        let rendered = format!("{client:?}");
        assert!(
            !rendered.contains("testToken"),
            "token bytes leaked through Debug: {rendered}"
        );
        assert!(
            rendered.contains("https://registry.example.com"),
            "the origin should stay visible for debugging: {rendered}"
        );
    }

    // -----------------------------------------------------------------
    // Login-URL discovery (`cabin login`'s advisory probe)
    // -----------------------------------------------------------------

    #[test]
    fn parse_login_url_accepts_the_documented_challenge() {
        assert_eq!(
            parse_login_url(r#"Cabin login_url="https://cabinpkg.com/settings/tokens""#).as_deref(),
            Some("https://cabinpkg.com/settings/tokens")
        );
        // The scheme token is case-insensitive (RFC 7235), loopback http
        // is plausible (local testing), and other parameters may precede.
        assert_eq!(
            parse_login_url(r#"cabin login_url="http://127.0.0.1:8787/settings/tokens""#)
                .as_deref(),
            Some("http://127.0.0.1:8787/settings/tokens")
        );
        assert_eq!(
            parse_login_url(r#"Cabin realm="reg", login_url="https://cabinpkg.com/t""#).as_deref(),
            Some("https://cabinpkg.com/t")
        );
        // A comma boundary without a space still counts.
        assert_eq!(
            parse_login_url(r#"Cabin realm="reg",login_url="https://cabinpkg.com/t""#).as_deref(),
            Some("https://cabinpkg.com/t")
        );
    }

    #[test]
    fn parse_login_url_rejects_other_challenges_and_implausible_urls() {
        for header in [
            "",
            "Cabin",
            "Cabin ",
            r#"Basic realm="x""#,
            // No whitespace after the scheme token.
            r#"Cabinlogin_url="https://x/y""#,
            // A different scheme's login_url is not ours.
            r#"Cargo login_url="https://x/y""#,
            // A different parameter that merely ends in our name is not
            // ours either.
            r#"Cabin not_login_url="https://attacker.example/""#,
            // Unquoted, relative, non-http(s), userinfo, control chars.
            "Cabin login_url=https://x/y",
            r#"Cabin login_url="/settings/tokens""#,
            r#"Cabin login_url="ftp://x/y""#,
            r#"Cabin login_url="https://user:pw@x/y""#,
            "Cabin login_url=\"https://x/\u{7}y\"",
        ] {
            assert_eq!(parse_login_url(header), None, "header: {header:?}");
        }
    }

    /// Server answering `/config.json` with a fixed status, body, and
    /// headers, recording whether the request carried an
    /// `Authorization` header.
    fn challenge_server(
        status: u16,
        body: impl Into<String>,
        headers: &'static [(&'static str, &'static str)],
    ) -> (Arc<tiny_http::Server>, String, JoinHandle<bool>) {
        let body = body.into();
        let server =
            Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"));
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let url = format!("http://{addr}");
        let server_for_thread = Arc::clone(&server);
        let thread = std::thread::spawn(move || {
            let mut saw_authorization = false;
            while let Ok(req) = server_for_thread.recv() {
                saw_authorization |= req.headers().iter().any(|h| h.field.equiv("Authorization"));
                let mut response =
                    tiny_http::Response::from_string(body.clone()).with_status_code(status);
                for (name, value) in headers {
                    response.add_header(
                        tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                            .expect("valid test header"),
                    );
                }
                let _ = req.respond(response);
            }
            saw_authorization
        });
        (server, url, thread)
    }

    /// The probe returns the challenge's URL on a 401 and stays
    /// credential-less on the wire.
    #[test]
    fn fetch_login_url_reads_the_challenge_from_a_401() {
        let (server, url, thread) = challenge_server(
            401,
            "{}",
            &[(
                "WWW-Authenticate",
                r#"Cabin login_url="https://cabinpkg.com/settings/tokens""#,
            )],
        );
        assert_eq!(
            fetch_login_url(&url).as_deref(),
            Some("https://cabinpkg.com/settings/tokens")
        );
        server.unblock();
        assert!(
            !thread.join().unwrap(),
            "the probe must never send Authorization"
        );
    }

    /// Every non-challenge outcome degrades to `None`: a 401 without (or
    /// with a foreign) challenge, a registry that answers 200 (auth not
    /// required), other statuses, and a machine that is offline.
    #[test]
    fn fetch_login_url_degrades_to_none_on_everything_else() {
        for (status, headers) in [
            (401, &[][..]),
            (401, &[("WWW-Authenticate", r#"Basic realm="reg""#)][..]),
            (200, &[][..]),
            (500, &[][..]),
        ] {
            let (server, url, thread) = challenge_server(status, "{}", headers);
            assert_eq!(fetch_login_url(&url), None, "status: {status}");
            server.unblock();
            let _ = thread.join();
        }

        // Offline: a closed loopback port fails the probe, quickly and
        // silently.
        let addr = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            listener.local_addr().expect("loopback addr")
        };
        assert_eq!(fetch_login_url(&format!("http://{addr}")), None);
    }

    #[test]
    fn get_bytes_surfaces_transport_errors() {
        // Bind an ephemeral loopback port, then drop the listener so
        // the port is closed; connecting must fail at the transport
        // layer rather than with an HTTP status.
        let addr = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            listener.local_addr().expect("loopback addr")
        };
        let client = HttpClient::new();

        let result = client.get_bytes(&format!("http://{addr}/pkg.json"), "pkg");

        match result {
            Err(IndexHttpError::Transport { name, .. }) => assert_eq!(name, "pkg"),
            other => panic!("expected Transport error, got {other:?}"),
        }
    }
}
