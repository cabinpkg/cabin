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
    /// [`IndexHttpError::ServerError`]).  Returns
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

/// Origin string used in auth error messages: the normalized origin
/// of `url`, falling back to a userinfo-redacted spelling for the
/// (unreachable through the index paths, which reject credential
/// URLs) case where normalization refuses the URL - a raw fallback
/// could echo a `user:pw@` credential into the diagnostic.
fn origin_for_error(url: &str) -> String {
    cabin_credentials::normalize_origin(url)
        .unwrap_or_else(|_| crate::source::redact_raw_url_userinfo(url))
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
