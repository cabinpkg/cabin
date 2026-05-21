use std::io::Read;
use std::time::Duration;

use crate::error::IndexHttpError;

/// Default per-request timeout for the sparse HTTP client. Static
/// registries are usually fast (cached object stores or local
/// servers); a long timeout is rarely useful and surfaces broken
/// links quickly.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum body size we will read for a single response. Generous
/// enough for a per-package metadata document or a typical source
/// archive, conservative enough to refuse a runaway response.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// Thin blocking HTTP client used by the sparse index source. Wraps
/// `ureq::Agent` so callers do not have to mention `ureq` directly.
#[derive(Clone)]
pub struct HttpClient {
    agent: ureq::Agent,
    max_body_bytes: usize,
}

impl HttpClient {
    /// Build a client with sensible defaults: 30 s timeout, body
    /// reads capped at 64 MiB, the default `ureq` TLS configuration,
    /// and redirects disabled. Disabling redirects keeps fetches
    /// pinned to the operator-configured registry origin; the module
    /// docs already promise this behaviour.
    pub fn new() -> Self {
        Self::with_redirect_budget(0)
    }

    /// Build a client whose agent follows up to `max_redirects`
    /// HTTP 3xx responses. Use only for downloads whose
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
        }
    }

    /// Variant useful for tests that want to inject a different
    /// ureq agent (e.g. one with a longer timeout). Gated to test
    /// builds so a production caller cannot bypass the redirect
    /// rejection wired into [`HttpClient::new`].
    #[cfg(test)]
    pub fn with_agent(agent: ureq::Agent) -> Self {
        Self {
            agent,
            max_body_bytes: MAX_BODY_BYTES,
        }
    }

    /// `GET` `url` and return the raw response body. `package` is
    /// embedded into errors so HTTP failures surface a useful
    /// caller-provided context.
    pub fn get_bytes(&self, url: &str, package: &str) -> Result<Vec<u8>, IndexHttpError> {
        match self.agent.get(url).call() {
            Ok(response) => {
                // `.redirects(0)` on the agent means redirects are not
                // followed, but ureq still returns the 3xx response as
                // `Ok`. Reject it explicitly so a registry that 3xx's
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

    /// `GET` `url` and return the raw response body. Used by the CLI
    /// to download artifacts; checksum verification happens later in
    /// `cabin-artifact`.
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

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("max_body_bytes", &self.max_body_bytes)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread::JoinHandle;

    /// Tiny HTTP server that answers `/from` with a 302 redirect to
    /// `/to`, and `/to` with `200 OK` carrying a known body. Used to
    /// verify the client does not silently follow registry redirects.
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
}
