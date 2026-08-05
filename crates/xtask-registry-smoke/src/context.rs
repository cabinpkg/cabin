//! The request plumbing the whole run shares: the two dev bases, the
//! credential state, and the two response buffers.
//!
//! The shell kept `$body` and `$headers` as two temporary files that
//! nearly every request overwrote, and several assertions read a buffer
//! an *earlier* request wrote (`L994` even redirects the header block
//! into `$body`).  They are modeled here as two fields on one value,
//! written at exactly the points the shell wrote them; no method
//! returns a response, because a per-request response object would
//! quietly fix the staleness the checks depend on.
//!
//! `curl_args` - the shell's one mutable credential array - becomes
//! [`Smoke::auth`], a list of extra request headers: empty is
//! anonymous, one `Authorization` entry is a token, and the session
//! legs push their own `Cookie` and CSRF headers.

use std::fmt::Write as _;
use std::io::Read as _;
use std::io::Write as _;

use anyhow::{Context as _, Result, bail};

use crate::bytes::sha256_hex;

/// Which of the two `wrangler dev` instances a request goes to: the
/// registry host, or the website role (`--host cabinpkg.com`).  Each
/// instance pins the Host header the Worker's role dispatch reads, so
/// the base is what selects the role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Base {
    Registry,
    Web,
}

/// `WEB_ORIGIN` from `wrangler.jsonc`, which the challenge, the
/// `config.json` api field and the quota details all embed.
const WEB_ORIGIN: &str = "https://cabinpkg.com";

/// What `curl` labels a `--data-binary` body when the caller names no
/// type of its own.  Preserved because the routes see it.
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// The file the differential harness reads, when it is running.
const TRACE_ENV: &str = "CABIN_SMOKE_TRACE";

/// The run's shared state: where the two dev instances are, which
/// credential the next request carries, and what the last one answered.
pub struct Smoke {
    agent: ureq::Agent,
    base: String,
    web_base: String,
    web_origin: &'static str,
    token: String,
    /// The last response body, as raw bytes - never a `String`: the
    /// byte-identity comparisons (`cmp -s`) and the zip reads both run
    /// over this buffer.
    pub body: Vec<u8>,
    /// The last response's header block, as `curl -D` wrote it.
    pub headers: Vec<u8>,
    /// The shell's `curl_args`: extra headers on every `check`/`request`.
    pub auth: Vec<(String, String)>,
}

impl Smoke {
    #[must_use]
    pub fn new(port: u16, web_port: u16, token: String) -> Self {
        Self {
            // Neither `curl` carried `-L`, so a redirect is the answer
            // and never a step on the way to one; the assertions are
            // over the 3xx itself.
            //
            // No connection reuse: every `curl` was its own process,
            // so no connection ever outlived its request.  Pooling
            // would be this port's own invention, and a leg whose
            // server answers BEFORE draining the request body (the
            // oversized chunked verdict, the body caps) leaves unread
            // bytes on a pooled connection - the next request on it
            // then hangs forever waiting for a response the server
            // will never parse out of the leftovers.  Found wedged,
            // not by review.
            // The five-minute ceiling exists only to turn a wedged
            // local dev server into a loud failure naming its URL;
            // curl ran with no timeout, and no healthy leg comes
            // within an order of magnitude of it.
            agent: ureq::AgentBuilder::new()
                .redirects(0)
                .max_idle_connections(0)
                .timeout(std::time::Duration::from_mins(5))
                .build(),
            base: format!("http://127.0.0.1:{port}"),
            web_base: format!("http://127.0.0.1:{web_port}"),
            web_origin: WEB_ORIGIN,
            token,
            body: Vec::new(),
            headers: Vec::new(),
            auth: Vec::new(),
        }
    }

    #[must_use]
    pub fn web_origin(&self) -> &'static str {
        self.web_origin
    }

    /// The absolute URL a path resolves to on one of the two roles.
    #[must_use]
    pub fn url(&self, at: Base, path: &str) -> String {
        let base = match at {
            Base::Registry => &self.base,
            Base::Web => &self.web_base,
        };
        format!("{base}{path}")
    }

    /// The shell's `curl_args=()`: the anonymous state.
    pub fn anonymous(&mut self) {
        self.auth.clear();
    }

    /// `as_publisher`: the ordinary publish/yank token.
    pub fn as_publisher(&mut self) {
        self.auth = bearer(&self.token);
    }

    /// `as_verifier`: the verify-scoped token, which the seeding block
    /// derives from the publisher's by suffix.
    pub fn as_verifier(&mut self) {
        self.auth = bearer(&format!("{}-verify", self.token));
    }

    /// One request, writing [`Smoke::body`] and [`Smoke::headers`] -
    /// the primitive the checks below are built on, and the escape
    /// hatch for the legs that need their own method, headers or range.
    ///
    /// `headers` are the only extra headers sent: an empty list sends
    /// none, which is what the shell's `${arr[@]+"${arr[@]}"}` guard
    /// bought it under `set -u` (an empty expansion, not an
    /// empty-valued header).
    ///
    /// # Errors
    ///
    /// If the request cannot be made at all, or its body cannot be
    /// read.  A non-2xx status is an answer, not an error: it is
    /// returned as the status, exactly as `curl -w '%{http_code}'`
    /// reported it.
    pub fn http(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
    ) -> Result<u16> {
        // Every one of the shell's ~500 requests paid a forked curl's
        // startup (~30 ms), and the local dev servers have never been
        // driven faster than that.  In-process HTTP without the pause
        // wedges workerd nondeterministically (an admin-plane GET
        // right after the early-answered oversized PATCH hangs with
        // no response, on a FRESH connection - twice in three runs),
        // so the pacing is part of the environment the run was
        // written for, not an optimization to strip.
        std::thread::sleep(std::time::Duration::from_millis(25));
        let mut request = self.agent.request(method, url);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        trace(&format!(
            "HTTP\t{method}\t{url}\tbody={}",
            body.map_or_else(|| "-".to_owned(), sha256_hex)
        ));

        let sent = match body {
            Some(data) => {
                if !headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                {
                    request = request.set("Content-Type", FORM_CONTENT_TYPE);
                }
                request.send_bytes(data)
            }
            None => request.call(),
        };
        let response = match sent {
            // A refusal is the assertion's subject everywhere in this
            // run, so a non-2xx status is unwrapped rather than raised.
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(error) => return Err(error).with_context(|| format!("{method} {url} failed")),
        };

        let status = response.status();
        self.headers = header_block(&response);
        self.body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut self.body)
            .with_context(|| format!("reading the body of {method} {url}"))?;
        trace(&format!("RESP\t{status}\tbody={}", sha256_hex(&self.body)));
        Ok(status)
    }

    /// `check_at`: a GET with the current credential whose status must
    /// be one of `expected`; the body lands in [`Smoke::body`].
    ///
    /// # Errors
    ///
    /// If the status is none of `expected`, worded as the shell worded
    /// it.
    pub fn check_at(&mut self, at: Base, path: &str, expected: &[u16]) -> Result<()> {
        let url = self.url(at, path);
        let auth = self.auth.clone();
        let status = self.http("GET", &url, &auth, None)?;
        self.expect_status(path, status, expected)
    }

    /// `check`: the registry host.
    ///
    /// # Errors
    ///
    /// As [`Smoke::check_at`].
    pub fn check(&mut self, path: &str, expected: &[u16]) -> Result<()> {
        self.check_at(Base::Registry, path, expected)
    }

    /// `wcheck`: the website origin.
    ///
    /// # Errors
    ///
    /// As [`Smoke::check_at`].
    pub fn wcheck(&mut self, path: &str, expected: &[u16]) -> Result<()> {
        self.check_at(Base::Web, path, expected)
    }

    /// `request_at`: `curl -X <method> --data-binary` with the current
    /// credential; the body lands in [`Smoke::body`].
    ///
    /// # Errors
    ///
    /// If the status is none of `expected`.
    pub fn request_at(
        &mut self,
        at: Base,
        method: &str,
        path: &str,
        data: &[u8],
        expected: &[u16],
    ) -> Result<()> {
        let url = self.url(at, path);
        let auth = self.auth.clone();
        let status = self.http(method, &url, &auth, Some(data))?;
        self.expect_status(&format!("{method} {path}"), status, expected)
    }

    /// `wrequest`: the mutation routes live on the website origin only.
    ///
    /// # Errors
    ///
    /// As [`Smoke::request_at`].
    pub fn wrequest(
        &mut self,
        method: &str,
        path: &str,
        data: &[u8],
        expected: &[u16],
    ) -> Result<()> {
        self.request_at(Base::Web, method, path, data, expected)
    }

    /// `expect_body`: the last body must contain `fixed`.  `grep -qF`,
    /// so a fixed string and never a pattern.
    ///
    /// # Errors
    ///
    /// If the body does not contain it.
    pub fn expect_body(&self, fixed: &str) -> Result<()> {
        let needle = fixed.as_bytes();
        // grep -qF '' matches anything with a line in it; the shell
        // never passes an empty needle, and `windows(0)` would panic.
        let found = needle.is_empty()
            || self
                .body
                .windows(needle.len())
                .any(|window| window == needle);
        if found {
            return Ok(());
        }
        bail!(
            "response body missing {fixed}: {}",
            String::from_utf8_lossy(&self.body)
        )
    }

    fn expect_status(&self, subject: &str, status: u16, expected: &[u16]) -> Result<()> {
        if expected.contains(&status) {
            println!("{}", ok_line(subject, status));
            return Ok(());
        }
        bail!(mismatch(subject, status, expected, &self.body))
    }
}

fn bearer(token: &str) -> Vec<(String, String)> {
    vec![("Authorization".to_owned(), format!("Bearer {token}"))]
}

/// The shell's `printf '    %s -> %s\n'`: four leading spaces, and
/// `<method> <path>` or a bare path as the subject.
fn ok_line(subject: &str, status: u16) -> String {
    format!("    {subject} -> {status}")
}

/// The `fail` wording for an unexpected status, byte for byte.
fn mismatch(subject: &str, status: u16, expected: &[u16], body: &[u8]) -> String {
    format!(
        "{subject} returned {status}, expected one of: {} (body: {})",
        statuses(expected),
        String::from_utf8_lossy(body)
    )
}

/// `"$*"` over the expected statuses: space separated.
fn statuses(expected: &[u16]) -> String {
    expected
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The header block as `curl -D` wrote it: the status line, then every
/// header line, CRLF throughout, terminated by a blank line.  The
/// assertions over it are line-wise, anchored and duplicate-sensitive
/// (`grep -i '^www-authenticate:'` must see a second copy), so it stays
/// raw text rather than a map.
///
/// Two fidelity ceilings, neither observable here: `ureq` reports
/// header names lowercased and does not expose the wire ORDER across
/// different names, so lines are grouped by name in first-appearance
/// order with each name's duplicates in order; and a header whose value
/// is not UTF-8 is dropped.  Every assertion in the run is `grep -i`
/// over ASCII headers.
fn header_block(response: &ureq::Response) -> Vec<u8> {
    let mut block = format!(
        "{} {} {}\r\n",
        response.http_version(),
        response.status(),
        response.status_text()
    );
    let mut written: Vec<String> = Vec::new();
    for name in response.headers_names() {
        if written.contains(&name) {
            continue;
        }
        for value in response.all(&name) {
            let _ = write!(block, "{name}: {value}\r\n");
        }
        written.push(name);
    }
    block.push_str("\r\n");
    block.into_bytes()
}

/// One normalized record per request and response when `CABIN_SMOKE_TRACE`
/// names a file, for the differential harness to diff against the same
/// records a `curl` shim logs on the shell side.  A no-op otherwise, and
/// a failed append never fails a check: the trace is an observer.
fn trace(record: &str) {
    let Ok(path) = std::env::var(TRACE_ENV) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{record}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead as _, BufReader};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// A loopback server answering `responses` in order and handing
    /// back each request's head.  The repository pulls in no HTTP mock
    /// crate, and the header block and the sent headers are only
    /// observable through a real response.
    fn serve(responses: Vec<&'static str>) -> (u16, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut head = String::new();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if let Some(value) = line
                        .to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                    {
                        length = value;
                    }
                    head.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let _ = sender.send(head);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (port, receiver)
    }

    fn smoke(port: u16) -> Smoke {
        Smoke::new(port, port, "cabin_smoke".to_owned())
    }

    #[test]
    fn the_success_line_carries_four_leading_spaces() {
        assert_eq!(ok_line("/healthz", 200), "    /healthz -> 200");
        assert_eq!(ok_line("PUT /api", 405), "    PUT /api -> 405");
    }

    #[test]
    fn the_mismatch_wording_matches_the_shell() {
        assert_eq!(
            mismatch("/config.json", 500, &[200, 404], b"nope"),
            "/config.json returned 500, expected one of: 200 404 (body: nope)"
        );
        assert_eq!(
            mismatch("PATCH /a", 409, &[200], b""),
            "PATCH /a returned 409, expected one of: 200 (body: )"
        );
    }

    #[test]
    fn the_header_block_keeps_duplicates_and_the_status_line() {
        let (port, _requests) = serve(vec![concat!(
            "HTTP/1.1 401 Unauthorized\r\n",
            "WWW-Authenticate: Cabin login_url=\"https://cabinpkg.com/settings/tokens\"\r\n",
            "Set-Cookie: a=1\r\n",
            "Set-Cookie: b=2\r\n",
            "Content-Length: 5\r\n",
            "Connection: close\r\n",
            "\r\n",
            "hello",
        )]);
        let mut smoke = smoke(port);
        let status = smoke
            .http("GET", &smoke.url(Base::Registry, "/config.json"), &[], None)
            .expect("request");

        assert_eq!(status, 401);
        assert_eq!(smoke.body, b"hello");
        let headers = String::from_utf8(smoke.headers).expect("utf8");
        assert!(
            headers.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
            "{headers}"
        );
        assert!(
            headers.contains("set-cookie: a=1\r\nset-cookie: b=2\r\n"),
            "a map would have dropped one: {headers}"
        );
        assert!(headers.ends_with("\r\n\r\n"), "{headers}");
    }

    #[test]
    fn an_empty_header_list_sends_no_extra_headers() {
        let (port, requests) = serve(vec!["HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"]);
        let mut smoke = smoke(port);
        smoke
            .http("GET", &smoke.url(Base::Registry, "/healthz"), &[], None)
            .expect("request");

        let head = requests.recv().expect("head");
        assert!(
            !head.to_ascii_lowercase().contains("authorization"),
            "{head}"
        );
        assert!(
            !head.to_ascii_lowercase().contains("content-type"),
            "{head}"
        );
    }

    #[test]
    fn a_body_defaults_to_the_urlencoded_type_unless_named() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        let (port, requests) = serve(vec![ok, ok]);
        let mut smoke = smoke(port);
        let url = smoke.url(Base::Web, "/api/v1/packages");
        smoke
            .http("POST", &url, &[], Some(b"payload"))
            .expect("post");
        let named = vec![("Content-Type".to_owned(), "application/json".to_owned())];
        smoke
            .http("POST", &url, &named, Some(b"{}"))
            .expect("post json");

        let default = requests.recv().expect("head").to_ascii_lowercase();
        assert!(
            default.contains("content-type: application/x-www-form-urlencoded"),
            "{default}"
        );
        let overridden = requests.recv().expect("head").to_ascii_lowercase();
        assert!(
            overridden.contains("content-type: application/json"),
            "{overridden}"
        );
        assert!(
            !overridden.contains("x-www-form-urlencoded"),
            "{overridden}"
        );
    }

    #[test]
    fn the_credential_state_rides_every_check() {
        let refused = concat!(
            "HTTP/1.1 401 Unauthorized\r\n",
            "Content-Length: 5\r\n",
            "Connection: close\r\n",
            "\r\n",
            "nope!",
        );
        let (port, requests) = serve(vec![refused, refused]);
        let mut smoke = smoke(port);
        smoke.as_publisher();
        smoke.check("/config.json", &[401]).expect("expected 401");
        assert!(
            requests
                .recv()
                .expect("head")
                .contains("Authorization: Bearer cabin_smoke")
        );

        let failure = smoke
            .check("/config.json", &[200, 404])
            .expect_err("a 401 is not one of the expected statuses");
        assert_eq!(
            failure.to_string(),
            "/config.json returned 401, expected one of: 200 404 (body: nope!)"
        );
        let _ = requests.recv();
    }

    #[test]
    fn expect_body_is_a_fixed_substring_over_the_shared_buffer() {
        let mut smoke = smoke(0);
        smoke.body = br#"{"github_id":0,"login":"smoke"}"#.to_vec();
        smoke.expect_body(r#""github_id":0"#).expect("present");
        assert_eq!(
            smoke
                .expect_body(r#""login":"ghost""#)
                .expect_err("absent")
                .to_string(),
            r#"response body missing "login":"ghost": {"github_id":0,"login":"smoke"}"#
        );
    }
}
