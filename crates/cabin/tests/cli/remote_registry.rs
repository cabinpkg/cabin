use super::*;

use super::standard_compat::flat_contains;

/// Minimal app manifest with one versioned dependency, so a resolve
/// run must load the index.
fn write_app_manifest(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "needs-fmt"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
}

/// Registry-root index whose `config.json` carries the given extra
/// JSON fields (after the four base fields) and one resolvable `fmt`
/// entry.
fn write_registry(root: &Path, extra_config_fields: &str) {
    assert_fs::fixture::ChildPath::new(root.join("config.json"))
        .write_str(&format!(
            r#"{{
    "schema": 1,
    "kind": "file-registry",
    "packages": "packages",
    "artifacts": "artifacts"{extra_config_fields}
}}"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/fmt.json"))
        .write_str(
            r#"{
    "schema": 1,
    "name": "fmt",
    "versions": { "10.2.1": { "dependencies": {} } }
}"#,
        )
        .unwrap();
}

/// `-Z remote-registry` is a recognized feature: it parses at
/// argument time instead of being rejected as unknown.
#[test]
fn remote_registry_feature_is_recognized() {
    cabin()
        .args(["-Z", "remote-registry", "--list"])
        .assert()
        .success();
}

/// An unknown `-Z` value is rejected with the full recognized list,
/// which now names `remote-registry`.
#[test]
fn unknown_feature_error_lists_remote_registry() {
    let assertion = cabin()
        .args(["build", "-Z", "frobnicate"])
        .assert()
        .failure()
        .code(2);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(
            "unknown experimental feature 'frobnicate'; expected one of: remote-registry"
        ),
        "expected the recognized-feature list in: {stderr}"
    );
}

/// The hosted-registry `config.json` fields are ordinary
/// configuration: a local mirror (or vendored copy) of a hosted
/// registry that carries `auth-required` / `api` resolves without
/// any experimental flag, and `-Z remote-registry` changes nothing.
#[test]
fn registry_config_fields_need_no_experimental_flag() {
    let dir = TempDir::new().unwrap();
    write_app_manifest(dir.path());
    let registry = dir.path().join("registry");
    write_registry(
        &registry,
        r#",
    "auth-required": true,
    "api": "https://registry.cabinpkg.com""#,
    );

    let mut outputs = Vec::new();
    for unstable in [None, Some(["-Z", "remote-registry"])] {
        let mut cmd = cabin();
        if let Some(flags) = unstable {
            cmd.args(flags);
        }
        let assertion = cmd
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--index-path")
            .arg(&registry)
            .assert()
            .success();
        outputs.push(String::from_utf8_lossy(&assertion.get_output().stdout).to_string());
    }
    assert_eq!(
        outputs[0], outputs[1],
        "resolution output must be byte-identical with and without the flag"
    );
    assert!(
        outputs[0].contains("fmt"),
        "expected fmt in the resolution output: {}",
        outputs[0]
    );
}

/// The same registry without the remote-registry fields resolves
/// identically with and without the flag: enabling the feature
/// never changes behavior for existing registries.
#[test]
fn existing_registries_resolve_identically_with_the_flag() {
    let dir = TempDir::new().unwrap();
    write_app_manifest(dir.path());
    let registry = dir.path().join("registry");
    write_registry(&registry, "");

    let mut outputs = Vec::new();
    for unstable in [None, Some(["-Z", "remote-registry"])] {
        let mut cmd = cabin();
        if let Some(flags) = unstable {
            cmd.args(flags);
        }
        let assertion = cmd
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--index-path")
            .arg(&registry)
            .assert()
            .success();
        outputs.push(String::from_utf8_lossy(&assertion.get_output().stdout).to_string());
    }
    assert_eq!(
        outputs[0], outputs[1],
        "resolution output must be byte-identical with and without the flag"
    );
    assert!(outputs[0].contains("fmt"), "{}", outputs[0]);
}

// -----------------------------------------------------------------
// cabin login / cabin logout + authenticated reads
// -----------------------------------------------------------------

const TEST_TOKEN: &str = "cabin_integrationTok1";

/// File server over `root` that 401s (with the protocol's error
/// envelope) every request not carrying `Authorization: Bearer
/// <token>` - the shape of an `auth-required` registry.  `config.json`
/// alone is served to anyone, the protocol's login-bootstrap rule.
struct AuthRegistryServer {
    server: std::sync::Arc<tiny_http::Server>,
    thread: Option<std::thread::JoinHandle<()>>,
    url: String,
}

impl AuthRegistryServer {
    fn serve(root: PathBuf, token: &'static str) -> Self {
        let server = std::sync::Arc::new(
            tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
        );
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let url = format!("http://{addr}");
        let server_for_thread = std::sync::Arc::clone(&server);
        let thread = std::thread::spawn(move || {
            while let Ok(req) = server_for_thread.recv() {
                let authorized = req.headers().iter().any(|h| {
                    h.field.equiv("Authorization") && h.value == format!("Bearer {token}")
                });
                let path = req.url().trim_start_matches('/').to_owned();
                if !authorized && path != "config.json" {
                    let _ = req.respond(
                        tiny_http::Response::from_string(
                            r#"{"errors":[{"detail":"authentication required"}]}"#,
                        )
                        .with_status_code(401),
                    );
                    continue;
                }
                if path.contains("..") {
                    let _ = req.respond(tiny_http::Response::empty(400));
                    continue;
                }
                let file_path = root.join(&path);
                match fs::read(&file_path) {
                    Ok(bytes) => {
                        let _ = req.respond(tiny_http::Response::from_data(bytes));
                    }
                    Err(_) => {
                        let _ = req.respond(tiny_http::Response::empty(404));
                    }
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

impl Drop for AuthRegistryServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Far-future expiry every seeded test session carries.
const TEST_EXPIRES_AT: &str = "2999-01-01T00:00:00.000Z";

/// A lapsed expiry, for the expired-session UX tests.
const PAST_EXPIRES_AT: &str = "2000-01-01T00:00:00.000Z";

/// The session token the mock registry's mint answers.
const MINTED_SESSION: &str = "cabin_ses_mintedSession-Token_0123456789abcdefghij";

/// The GitHub access token the mock OAuth server's poll answers.
const GITHUB_ACCESS_TOKEN: &str = "gho_testAccessSecret";

/// The user code the mock OAuth server hands out.
const DEVICE_USER_CODE: &str = "ABCD-1234";

/// Seed `home/credentials.toml` with a session for `origin`, exactly
/// as a `cabin login` against it would leave it, with `api_url` as
/// the revocation origin and mode 0600.
fn write_session_credentials_full(home: &Path, origin: &str, token: &str, expires_at: &str) {
    fs::create_dir_all(home).unwrap();
    let credentials_path = home.join("credentials.toml");
    fs::write(
        &credentials_path,
        format!(
            "[registries.\"{origin}\"]\ntoken = \"{token}\"\nexpires-at = \"{expires_at}\"\n\
             api-url = \"{origin}\"\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&credentials_path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// [`write_session_credentials_full`] with the far-future expiry.
fn write_session_credentials(home: &Path, origin: &str, token: &str) {
    write_session_credentials_full(home, origin, token, TEST_EXPIRES_AT);
}

/// GitHub OAuth mock for the device flow: `POST /login/device/code`
/// answers an immediate one-second-interval grant (with the extra
/// fields GitHub sends), and `POST /login/oauth/access_token` answers
/// the access token plus the refresh-token fields the OAuth app's
/// token expiration adds - which the client must tolerate and
/// discard.
struct GithubOauthServer {
    server: std::sync::Arc<tiny_http::Server>,
    thread: Option<std::thread::JoinHandle<()>>,
    url: String,
}

impl GithubOauthServer {
    fn start() -> Self {
        let server = std::sync::Arc::new(
            tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
        );
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let url = format!("http://{addr}");
        let server_for_thread = std::sync::Arc::clone(&server);
        let device_code_body = format!(
            r#"{{"device_code":"dc-test","user_code":"{DEVICE_USER_CODE}","verification_uri":"{url}/login/device","expires_in":900,"interval":1}}"#
        );
        let token_body = format!(
            r#"{{"access_token":"{GITHUB_ACCESS_TOKEN}","token_type":"bearer","scope":"","expires_in":28800,"refresh_token":"ghr_testRefreshSecret","refresh_token_expires_in":15811200}}"#
        );
        let thread = std::thread::spawn(move || {
            while let Ok(req) = server_for_thread.recv() {
                let body = match req.url() {
                    "/login/device/code" => device_code_body.clone(),
                    "/login/oauth/access_token" => token_body.clone(),
                    _ => {
                        let _ = req.respond(tiny_http::Response::empty(404));
                        continue;
                    }
                };
                let _ = req.respond(tiny_http::Response::from_string(body));
            }
        });
        Self {
            server,
            thread: Some(thread),
            url,
        }
    }
}

impl Drop for GithubOauthServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// One captured session-route request.
struct CapturedSessionRequest {
    method: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

/// Mock registry for the login flow: serves `config.json` under any
/// path prefix (declaring this server as its own `api` origin) and
/// answers `/api/v1/sessions/tokens` with the configured status -
/// a `200` mint, the uniform `401`, or a `204` revocation - capturing
/// each session-route request.
struct SessionRegistryServer {
    server: std::sync::Arc<tiny_http::Server>,
    thread: Option<std::thread::JoinHandle<()>>,
    url: String,
    captured: std::sync::Arc<std::sync::Mutex<Vec<CapturedSessionRequest>>>,
}

impl SessionRegistryServer {
    fn start() -> Self {
        Self::start_with_mint_body(
            200,
            format!(r#"{{"token":"{MINTED_SESSION}","expires_at":"{TEST_EXPIRES_AT}"}}"#),
        )
    }

    fn start_with_mint_body(mint_status: u16, mint_body: String) -> Self {
        Self::start_inner(mint_status, mint_body, false)
    }

    /// Declare `auth-required: true` and 401 every route other than
    /// the public `config.json` and the session routes - the protocol
    /// shape whose login bootstrap the docs guarantee.
    fn start_auth_required() -> Self {
        Self::start_inner(
            200,
            format!(r#"{{"token":"{MINTED_SESSION}","expires_at":"{TEST_EXPIRES_AT}"}}"#),
            true,
        )
    }

    fn start_inner(mint_status: u16, mint_body: String, auth_required: bool) -> Self {
        let server = std::sync::Arc::new(
            tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
        );
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let url = format!("http://{addr}");
        let auth_field = if auth_required {
            r#""auth-required":true,"#
        } else {
            ""
        };
        let config = format!(
            r#"{{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts",{auth_field}"api":"{url}"}}"#
        );
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_for_thread = std::sync::Arc::clone(&captured);
        let server_for_thread = std::sync::Arc::clone(&server);
        let thread = std::thread::spawn(move || {
            while let Ok(mut req) = server_for_thread.recv() {
                let path = req.url().to_owned();
                if path.ends_with("/config.json") {
                    let _ = req.respond(tiny_http::Response::from_string(config.clone()));
                } else if path == "/api/v1/sessions/tokens" {
                    let mut body = Vec::new();
                    let _ = req.as_reader().read_to_end(&mut body);
                    let method = req.method().as_str().to_owned();
                    captured_for_thread
                        .lock()
                        .unwrap()
                        .push(CapturedSessionRequest {
                            method: method.clone(),
                            authorization: req
                                .headers()
                                .iter()
                                .find(|h| h.field.equiv("Authorization"))
                                .map(|h| h.value.to_string()),
                            body,
                        });
                    let (status, body) = if method == "DELETE" {
                        (204, String::new())
                    } else {
                        (mint_status, mint_body.clone())
                    };
                    let _ = req
                        .respond(tiny_http::Response::from_string(body).with_status_code(status));
                } else if auth_required {
                    let _ = req.respond(
                        tiny_http::Response::from_string(
                            r#"{"errors":[{"detail":"authentication required"}]}"#,
                        )
                        .with_status_code(401),
                    );
                } else {
                    let _ = req.respond(tiny_http::Response::empty(404));
                }
            }
        });
        Self {
            server,
            thread: Some(thread),
            url,
            captured,
        }
    }

    fn captured(&self) -> Vec<CapturedSessionRequest> {
        std::mem::take(&mut *self.captured.lock().unwrap())
    }
}

impl Drop for SessionRegistryServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// The credential commands are part of the stable read path: login
/// and logout work without any experimental flag.  End to end against
/// the mocks: the device flow's user code is shown, the GitHub access
/// token reaches exactly the mint's body (no `Authorization` header),
/// the minted session lands in `credentials.toml`, and logout
/// self-revokes with the session as the bearer before removing it.
#[test]
fn login_and_logout_need_no_experimental_flag() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let github = GithubOauthServer::start();
    let registry = SessionRegistryServer::start();
    let assertion = cabin()
        .args(["login", "--index-url", &registry.url])
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "session for `{}` saved",
            registry.url
        )));
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains(DEVICE_USER_CODE),
        "the user code must be shown: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    for secret in [GITHUB_ACCESS_TOKEN, MINTED_SESSION] {
        assert!(
            !stdout.contains(secret) && !stderr.contains(secret),
            "secrets must never be echoed; stdout: {stdout}; stderr: {stderr}"
        );
    }

    let captured = registry.captured();
    assert_eq!(captured.len(), 1, "exactly one mint request");
    assert_eq!(captured[0].method, "PUT");
    assert_eq!(captured[0].authorization, None);
    assert_eq!(
        captured[0].body,
        format!(r#"{{"github_token":"{GITHUB_ACCESS_TOKEN}"}}"#).as_bytes()
    );
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert!(body.contains(MINTED_SESSION), "session stored: {body}");
    assert!(body.contains(TEST_EXPIRES_AT), "expiry stored: {body}");

    cabin()
        .args(["logout", "--index-url", &registry.url])
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "session for `{}` removed",
            registry.url
        )));
    let captured = registry.captured();
    assert_eq!(captured.len(), 1, "exactly one revocation request");
    assert_eq!(captured[0].method, "DELETE");
    assert_eq!(
        captured[0].authorization.as_deref(),
        Some(format!("Bearer {MINTED_SESSION}").as_str()),
        "the session revokes itself"
    );
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert!(!body.contains(MINTED_SESSION), "entry removed: {body}");
}

/// A repeat `cabin login` revokes the session it displaces - after
/// the new one is stored, so the mint lands first.  The registry
/// keeps one row per mint and revocation deletes only the presented
/// token, so the overwritten session would otherwise stay live
/// server-side until it expired, beyond any later `cabin logout`.
#[test]
fn login_revokes_the_session_it_displaces() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let github = GithubOauthServer::start();
    let registry = SessionRegistryServer::start();
    write_session_credentials_full(&home, &registry.url, TEST_TOKEN, TEST_EXPIRES_AT);
    cabin()
        .args(["login", "--index-url", &registry.url])
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .success();
    let captured = registry.captured();
    assert_eq!(captured.len(), 2, "one mint, then one revocation");
    assert_eq!(captured[0].method, "PUT", "the mint lands first");
    assert_eq!(captured[1].method, "DELETE");
    assert_eq!(
        captured[1].authorization.as_deref(),
        Some(format!("Bearer {TEST_TOKEN}").as_str()),
        "the displaced token is the one revoked"
    );
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert!(body.contains(MINTED_SESSION), "new session stored: {body}");
    assert!(!body.contains(TEST_TOKEN), "old session replaced: {body}");
}

/// A first `cabin login` against an `auth-required` registry
/// bootstraps from the public `config.json` alone: with sessions the
/// only credential, discovery must precede authentication, so a login
/// that grew a dependency on any protected route would 401 here with
/// no way to ever obtain a token.
#[test]
fn login_bootstraps_an_auth_required_registry() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let github = GithubOauthServer::start();
    let registry = SessionRegistryServer::start_auth_required();
    cabin()
        .args(["login", "--index-url", &registry.url])
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "session for `{}` saved",
            registry.url
        )));
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert!(body.contains(MINTED_SESSION), "session stored: {body}");
}

/// With no `--index-url` and no config, the credential commands
/// target the default hosted registry.  `cabin logout` resolves the
/// origin without any network traffic, so it is the hermetic probe
/// for the default: it names `https://registry.cabinpkg.com`.
#[test]
fn bare_logout_targets_the_default_registry() {
    let dir = TempDir::new().unwrap();
    cabin()
        .args(["logout"])
        .env("CABIN_CONFIG_HOME", dir.path().join("empty-home"))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "no session was stored for `https://registry.cabinpkg.com`",
        ));
}

/// `cabin yank` never falls back to the default registry: a mutation
/// must not target a registry the user did not name.  With nothing
/// configured it fails before any credential or network work.
#[test]
fn yank_requires_an_explicit_index_source() {
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .args(["-Z", "remote-registry", "yank", "acme/demo@1.0.0"])
        .current_dir(dir.path())
        .env("CABIN_CONFIG_HOME", dir.path().join("empty-home"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            "`cabin yank` requires --index-url or a `[registry] index-url` config setting"
        ),
        "expected the explicit-source requirement in: {stderr}"
    );
    assert!(
        !stderr.contains("registry.cabinpkg.com"),
        "yank must not mention (or target) the default registry: {stderr}"
    );
}

/// The device-flow login needs github.com and the registry, so under
/// `CABIN_NET_OFFLINE` it refuses up front - after naming its target
/// - and the registry receives zero requests.
#[test]
fn login_refuses_to_run_offline() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let dir = TempDir::new().unwrap();
    let server = std::sync::Arc::new(
        tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
    );
    let addr = server.server_addr().to_ip().expect("loopback addr");
    let url = format!("http://{addr}");
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let server_for_thread = std::sync::Arc::clone(&server);
    let hits_for_thread = std::sync::Arc::clone(&hits);
    let thread = std::thread::spawn(move || {
        while let Ok(req) = server_for_thread.recv() {
            hits_for_thread.fetch_add(1, Ordering::SeqCst);
            let _ = req.respond(tiny_http::Response::empty(401));
        }
    });

    cabin()
        .args(["login", "--index-url", &url])
        .env("CABIN_CONFIG_HOME", dir.path().join("config-home"))
        .env("CABIN_NET_OFFLINE", "1")
        .assert()
        .failure()
        .stderr(predicate::str::contains("needs the network"));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "offline login must not touch the network"
    );
    server.unblock();
    let _ = thread.join();
}

/// A checked-out project's `.cabin/config.toml` cannot steer where a
/// minted credential is stored: the credential commands resolve their
/// registry from user-level config only, so a project-declared
/// `[registry] index-url` is ignored and bare `cabin login` targets
/// the default registry.  (`CABIN_NET_OFFLINE` keeps the run
/// hermetic - login names its target, then refuses to go online.)
#[test]
fn login_ignores_project_config_when_choosing_the_registry() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let dir = TempDir::new().unwrap();
    let attacker = std::sync::Arc::new(
        tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
    );
    let attacker_addr = attacker.server_addr().to_ip().expect("loopback addr");
    let attacker_url = format!("http://{attacker_addr}");
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let attacker_for_thread = std::sync::Arc::clone(&attacker);
    let hits_for_thread = std::sync::Arc::clone(&hits);
    let thread = std::thread::spawn(move || {
        while let Ok(req) = attacker_for_thread.recv() {
            hits_for_thread.fetch_add(1, Ordering::SeqCst);
            let _ = req.respond(tiny_http::Response::empty(200));
        }
    });

    // The "hostile checkout": a project whose workspace config points
    // the registry at the attacker's server.
    assert_fs::fixture::ChildPath::new(dir.path().join("proj/cabin.toml"))
        .write_str("[package]\nname = \"proj\"\nversion = \"0.1.0\"\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("proj/.cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{attacker_url}\"\n"))
        .unwrap();

    let home = dir.path().join("config-home");
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["login"])
        .current_dir(dir.path().join("proj"))
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_NET_OFFLINE", "1")
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "logging in to `https://registry.cabinpkg.com`",
        ));
    assert!(
        !home.join("credentials.toml").exists(),
        "the refused login must store nothing"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the project-picked registry must never be contacted"
    );
    attacker.unblock();
    let _ = thread.join();
}

/// A loopback address whose port was just released: connecting fails
/// immediately, which is how the login-probe tests exercise the
/// offline path without external DNS or timeouts.
fn dead_loopback_url() -> String {
    let addr = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        listener.local_addr().expect("loopback addr")
    };
    format!("http://{addr}")
}

/// Two distinct dead loopback URLs: both listeners are held bound
/// while the ports are read, so the second bind can never reuse the
/// first port the way two consecutive `dead_loopback_url` calls can.
fn dead_loopback_url_pair() -> (String, String) {
    let first = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let second = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addrs = (
        first.local_addr().expect("loopback addr"),
        second.local_addr().expect("loopback addr"),
    );
    (format!("http://{}", addrs.0), format!("http://{}", addrs.1))
}

/// The minted session is stored keyed by the normalized index
/// origin (path and trailing slash stripped) with the mint's expiry
/// and the discovered `api` origin alongside, in a 0600 file.
#[test]
fn login_stores_the_session_keyed_by_normalized_origin() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let github = GithubOauthServer::start();
    let registry = SessionRegistryServer::start();
    let base = registry.url.clone();
    cabin()
        .args(["login", "--index-url", &format!("{base}/some/path/")])
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "session for `{base}` saved"
        )));

    let credentials_path = home.join("credentials.toml");
    let body = fs::read_to_string(&credentials_path).unwrap();
    assert_eq!(
        body,
        format!(
            "[registries.\"{base}\"]\ntoken = \"{MINTED_SESSION}\"\n\
             expires-at = \"{TEST_EXPIRES_AT}\"\napi-url = \"{base}\"\n"
        )
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&credentials_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:03o}", mode & 0o777);
    }
}

/// The registry's mint endpoint refuses with its deliberately
/// uniform 401: login fails with the uniform-refusal wording and
/// stores nothing.
#[test]
fn login_reports_the_uniform_mint_refusal() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let github = GithubOauthServer::start();
    let registry = SessionRegistryServer::start_with_mint_body(
        401,
        r#"{"errors":[{"detail":"authentication required"}]}"#.to_owned(),
    );
    let assertion = cabin()
        .args(["login", "--index-url", &registry.url])
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "refused the session request"),
        "expected the uniform-refusal wording in: {stderr}"
    );
    assert!(!home.join("credentials.toml").exists());
}

/// A mint response whose token is not a Cabin token is rejected
/// before anything is written, and the error never echoes the value.
#[test]
fn login_rejects_invalid_tokens_without_writing() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let github = GithubOauthServer::start();
    let registry = SessionRegistryServer::start_with_mint_body(
        200,
        r#"{"token":"ghp_notACabinToken12345","expires_at":"2999-01-01T00:00:00.000Z"}"#.to_owned(),
    );
    let assertion = cabin()
        .args(["login", "--index-url", &registry.url])
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "without a usable token"),
        "expected the unusable-token error in: {stderr}"
    );
    assert!(
        !stderr.contains("notACabinToken"),
        "the minted value must not be echoed: {stderr}"
    );
    assert!(!home.join("credentials.toml").exists());
}

/// Without `--index-url` the `[registry] index-url` config setting
/// applies; a config-supplied local `index-path` is rejected because
/// a token has no local-path counterpart; and with no index source
/// at all, login targets the default hosted registry (here rerouted
/// through `[source-replacement]` to stay hermetic).
#[test]
fn login_resolves_the_registry_from_config_and_rejects_local_paths() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let github = GithubOauthServer::start();
    let registry = SessionRegistryServer::start();
    let base = registry.url.clone();
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{base}/index/\"\n"))
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["-Z", "remote-registry", "login"])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "session for `{base}` saved"
        )));

    // Same setup with a local index-path: refused.
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str("[registry]\nindex-path = \"registry\"\n")
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    let assertion = cmd
        .args(["-Z", "remote-registry", "login"])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "requires an HTTP registry"),
        "expected the local-path rejection in: {stderr}"
    );

    // No index source anywhere: login falls back to the default
    // hosted registry.  A `[source-replacement]` entry for the
    // default origin applies to it exactly like a config-supplied
    // source, which also keeps this hermetic - the replacement wins
    // before any request could contact the real registry.
    let mirror_registry = SessionRegistryServer::start();
    let mirror = mirror_registry.url.clone();
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!(
            "[source-replacement]\n\"https://registry.cabinpkg.com\" = \
             {{ index-url = \"{mirror}/index\" }}\n",
        ))
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["login"])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "session for `{mirror}` saved"
        )));
}

/// `cabin logout` removes exactly the effective origin's entry and
/// reports whether one existed.
#[test]
fn logout_removes_the_entry_and_reports_absence() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    fs::create_dir_all(&home).unwrap();
    let credentials_path = home.join("credentials.toml");
    fs::write(
        &credentials_path,
        format!(
            "[registries.\"https://keep.example.com\"]\ntoken = \"{TEST_TOKEN}\"\n\
             expires-at = \"{TEST_EXPIRES_AT}\"\napi-url = \"https://keep.example.com\"\n\
             [registries.\"https://registry.example.com\"]\ntoken = \"{TEST_TOKEN}\"\n\
             expires-at = \"{TEST_EXPIRES_AT}\"\napi-url = \"https://registry.example.com\"\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&credentials_path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    // The revocation call to the (unreachable) stored `api-url` is
    // best-effort: the local removal succeeds regardless.
    cabin()
        .args([
            "-Z",
            "remote-registry",
            "logout",
            "--index-url",
            "https://registry.example.com",
        ])
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_NET_OFFLINE", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "session for `https://registry.example.com` removed",
        ));
    let body = fs::read_to_string(&credentials_path).unwrap();
    assert!(body.contains("keep.example.com"), "{body}");
    assert!(!body.contains("registry.example.com"), "{body}");

    cabin()
        .args([
            "-Z",
            "remote-registry",
            "logout",
            "--index-url",
            "https://registry.example.com",
        ])
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "no session was stored for `https://registry.example.com`",
        ));
}

/// End-to-end authenticated read path, with no experimental flag: an
/// `auth-required` registry resolves only when a credential is
/// available - via `CABIN_REGISTRY_TOKEN` or a prior `cabin login` -
/// and the tokenless failure advises `cabin login` for the origin
/// without mentioning any `-Z` flag.
#[test]
fn resolve_against_an_auth_required_registry_uses_the_credential() {
    let dir = TempDir::new().unwrap();
    write_app_manifest(dir.path());
    let registry = dir.path().join("registry");
    write_registry(&registry, r#", "auth-required": true"#);
    let server = AuthRegistryServer::serve(registry, TEST_TOKEN);

    // Tokenless: the very first request (config.json) is refused and
    // the error advises `cabin login --index-url <origin>`.
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "authentication required by registry"),
        "expected the auth-required error in: {stderr}"
    );
    assert!(
        flat_contains(
            &stderr,
            &format!("cabin login --index-url {}", server.url())
        ),
        "expected the login advice in: {stderr}"
    );
    assert!(
        !stderr.contains("-Z remote-registry"),
        "the login advice must not name the experimental flag: {stderr}"
    );

    // The env override authenticates every request this invocation
    // makes (a loopback origin, so the override is eligible).
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(stdout.contains("fmt"), "{stdout}");

    // A stored session works the same way.
    let home = dir.path().join("config-home");
    write_session_credentials(&home, server.url(), TEST_TOKEN);
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .success()
        .stdout(predicate::str::contains("fmt"));

    // The env override outranks the stored session: with a session
    // stored that the server rejects, the env token still wins and
    // the resolve succeeds.
    let overridden = dir.path().join("config-home-overridden");
    write_session_credentials(&overridden, server.url(), "cabin_storedButOutranked1");
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .env("CABIN_CONFIG_HOME", &overridden)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success()
        .stdout(predicate::str::contains("fmt"));

    // A wrong stored token surfaces the revoked/expired wording.
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .env("CABIN_REGISTRY_TOKEN", "cabin_wrongToken12345")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "revoked or expired"),
        "expected the token-rejected error in: {stderr}"
    );
    assert!(
        !stderr.contains("cabin_wrongToken12345"),
        "token bytes must never surface: {stderr}"
    );
}

/// A login-session token (`cabin_ses_<base64url>`, carrying `-` and `_`)
/// is a first-class bearer at the CLI seam: a stored session survives
/// the credentials round-trip with those bytes intact, and a later
/// `resolve` loads it and authenticates with no env override - the
/// user-facing half of the `Token::parse` widening that lets
/// `cabin_ses_` tokens through.
#[test]
fn a_stored_session_token_authenticates_reads() {
    const SESSION_TOKEN: &str = "cabin_ses_abcdefghij-klmnopqrst_uvwxyzABCDEFGHIJKLMNO";
    let dir = TempDir::new().unwrap();
    write_app_manifest(dir.path());
    let registry = dir.path().join("registry");
    write_registry(&registry, r#", "auth-required": true"#);
    let server = AuthRegistryServer::serve(registry, SESSION_TOKEN);

    let home = dir.path().join("config-home");
    write_session_credentials(&home, server.url(), SESSION_TOKEN);
    // Persisted with its `-`/`_` bytes intact through the credentials
    // round-trip.
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert!(
        body.contains(SESSION_TOKEN),
        "the session token must be stored verbatim: {body}"
    );

    // Reusable: the stored credential alone authenticates the resolve.
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .success()
        .stdout(predicate::str::contains("fmt"));
}

/// An expired stored session is withheld from reads: the client warns
/// with the cause and the fix, proceeds tokenless, and the
/// `auth-required` registry's own 401 stands - the registry's uniform
/// refusal could never name the expiry.
#[test]
fn an_expired_session_warns_and_reads_proceed_tokenless() {
    let dir = TempDir::new().unwrap();
    write_app_manifest(dir.path());
    let registry = dir.path().join("registry");
    write_registry(&registry, r#", "auth-required": true"#);
    let server = AuthRegistryServer::serve(registry, TEST_TOKEN);

    let home = dir.path().join("config-home");
    write_session_credentials_full(&home, server.url(), TEST_TOKEN, PAST_EXPIRES_AT);
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            &format!(
                "the stored session for `{}` has expired (at {PAST_EXPIRES_AT})",
                server.url()
            )
        ),
        "expected the expiry warning in: {stderr}"
    );
    assert!(
        flat_contains(&stderr, "authentication required by registry"),
        "the tokenless request must still reach the server's 401: {stderr}"
    );
}

/// A pre-session `token`-only credentials file (whose long-lived keys
/// no registry accepts any more) must not break reads: the file reads
/// as absent with a warning naming it, the resolve proceeds
/// tokenless, and the auth-required registry's own 401 stands with
/// the login advice.
#[test]
fn an_unreadable_credentials_file_does_not_break_reads() {
    let dir = TempDir::new().unwrap();
    write_app_manifest(dir.path());
    let registry = dir.path().join("registry");
    write_registry(&registry, r#", "auth-required": true"#);
    let server = AuthRegistryServer::serve(registry, TEST_TOKEN);

    let home = dir.path().join("config-home");
    fs::create_dir_all(&home).unwrap();
    fs::write(
        home.join("credentials.toml"),
        format!(
            "[registries.\"{}\"]\ntoken = \"{TEST_TOKEN}\"\n",
            server.url()
        ),
    )
    .unwrap();
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "ignoring unreadable credentials file"),
        "expected the unreadable-file warning in: {stderr}"
    );
    assert!(
        flat_contains(&stderr, "authentication required by registry"),
        "the tokenless request must still reach the server's 401: {stderr}"
    );
}

/// `cabin login` over a pre-session `token`-only credentials file
/// replaces it wholesale - the actionable advice the unreadable-file
/// errors give must actually work.
#[test]
fn login_replaces_an_unreadable_legacy_credentials_file() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    fs::create_dir_all(&home).unwrap();
    let github = GithubOauthServer::start();
    let registry = SessionRegistryServer::start();
    let base = registry.url.clone();
    fs::write(
        home.join("credentials.toml"),
        format!("[registries.\"{base}\"]\ntoken = \"{TEST_TOKEN}\"\n"),
    )
    .unwrap();

    cabin()
        .args(["login", "--index-url", &base])
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "session for `{base}` saved"
        )));
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert_eq!(
        body,
        format!(
            "[registries.\"{base}\"]\ntoken = \"{MINTED_SESSION}\"\n\
             expires-at = \"{TEST_EXPIRES_AT}\"\napi-url = \"{base}\"\n"
        )
    );
}

/// A mutation with only an expired session, non-interactive: publish
/// fails with the expiry as the cause before any network request -
/// the dead index port would fail differently if one were made.
#[test]
fn publish_with_an_expired_session_fails_actionably() {
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());
    let base = dead_loopback_url();
    let home = dir.path().join("config-home");
    write_session_credentials_full(&home, &base, TEST_TOKEN, PAST_EXPIRES_AT);
    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-url", &base])
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            &format!(
                "the stored session for `{base}` has expired (at {PAST_EXPIRES_AT}); run `cabin \
                 login --index-url"
            )
        ),
        "expected the expired-session error in: {stderr}"
    );
}

/// The same expired-session bail on the yank path, likewise before
/// any network request.  The index URL carries a path here to pin
/// the advice's URL choice: sessions are origin-keyed, but the
/// recommended `cabin login` command must quote the full index URL -
/// the origin alone would point it at the wrong `config.json`.
#[test]
fn yank_with_an_expired_session_fails_actionably() {
    let dir = TempDir::new().unwrap();
    let base = dead_loopback_url();
    let index_url = format!("{base}/idx");
    let home = dir.path().join("config-home");
    write_session_credentials_full(&home, &base, TEST_TOKEN, PAST_EXPIRES_AT);
    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "yank",
            "acme/demo@0.1.0",
            "--index-url",
            &index_url,
        ])
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            &format!(
                "the stored session for `{base}` has expired (at {PAST_EXPIRES_AT}); run `cabin \
                 login --index-url {index_url}` to start a new one"
            )
        ),
        "expected the expired-session error in: {stderr}"
    );
}

/// A scoped package fetches end to end from an `auth-required`
/// registry with no experimental flag: the credential rides the
/// `config.json`, metadata, and artifact requests alike, and the
/// verified archive lands extracted in the cache.
#[test]
fn fetch_scoped_package_from_an_auth_required_registry() {
    let dir = TempDir::new().unwrap();
    // Publish the scoped fixture into a local registry, then mark
    // the registry auth-required like the hosted one.
    let pkg_root = dir.path().join("pkg");
    write_scoped_publishable_package(&pkg_root);
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();
    let config = fs::read_to_string(registry.join("config.json")).unwrap();
    fs::write(
        registry.join("config.json"),
        config.replace(
            "\"kind\"",
            "\"auth-required\": true, \"api\": \"https://cabinpkg.com\", \"kind\"",
        ),
    )
    .unwrap();

    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
"acme/demo" = "0.1.0"
"#,
        )
        .unwrap();
    let server = AuthRegistryServer::serve(registry, TEST_TOKEN);
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .arg("--cache-dir")
        .arg(&cache)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
    // The extracted source tree is in the cache, manifest included.
    let sources = cache.join("sources/sha256");
    assert!(sources.is_dir());
    let found_cabin_toml = fs::read_dir(&sources)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry.path().join("cabin.toml").is_file());
    assert!(
        found_cabin_toml,
        "expected an extracted cabin.toml in cache"
    );
}

/// The tokenless-read error for exactly `origin`.  Naming the origin
/// is what makes the provenance assertions below structural: a run
/// that silently fell back to some *other* registry cannot satisfy
/// them by producing the same generic wording.
fn auth_required_for(origin: &str) -> String {
    format!("authentication required by registry `{origin}`")
}

/// Stage an `auth-required` registry holding `acme/demo` 0.1.0 plus an
/// `app` package under `dir` that depends on it, and return the
/// registry directory for [`AuthRegistryServer::serve`].  Shared by the
/// `CABIN_REGISTRY_TOKEN` provenance tests below.
fn stage_auth_required_registry(dir: &Path) -> PathBuf {
    let pkg_root = dir.join("pkg");
    write_scoped_publishable_package(&pkg_root);
    let registry = dir.join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();
    let config = fs::read_to_string(registry.join("config.json")).unwrap();
    fs::write(
        registry.join("config.json"),
        config.replace("\"kind\"", "\"auth-required\": true, \"kind\""),
    )
    .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
"acme/demo" = "0.1.0"
"#,
        )
        .unwrap();
    registry
}

/// A loopback index origin the *project* picked never receives the
/// `CABIN_REGISTRY_TOKEN` override.  The variable carries no origin
/// key, so releasing it to any loopback origin would let a hostile
/// checkout - whose `.cabin/config.toml` steers the reads at a
/// listener it started itself - collect the operator's registry
/// token, the very disclosure the `env_remove` at every child-spawn
/// site exists to prevent.  Both index-loading paths are covered:
/// `cabin resolve` and the fetch / build pipeline.
#[test]
fn a_project_config_loopback_index_never_receives_the_env_token() {
    let dir = TempDir::new().unwrap();
    let registry = stage_auth_required_registry(dir.path());
    let server = AuthRegistryServer::serve(registry, TEST_TOKEN);
    let app = dir.path().join("app");
    assert_fs::fixture::ChildPath::new(app.join(".cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{}\"\n", server.url()))
        .unwrap();

    for command in ["resolve", "fetch"] {
        let mut cmd = cabin();
        cmd.args([command, "--manifest-path"])
            .arg(app.join("cabin.toml"));
        if command == "fetch" {
            cmd.arg("--cache-dir").arg(dir.path().join("cache"));
        }
        let assertion = cmd
            .env_remove("CABIN_NO_CONFIG")
            .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            flat_contains(&stderr, &auth_required_for(server.url())),
            "`cabin {command}` must reach the project-picked registry unauthenticated: {stderr}"
        );
        // A silently withheld credential would leave the user with a
        // bare "authentication required" and no way to learn that the
        // token they exported was ignored on purpose.
        assert!(
            flat_contains(
                &stderr,
                &format!(
                    "CABIN_REGISTRY_TOKEN is set but was not used for `{}`",
                    server.url()
                )
            ),
            "`cabin {command}` must say the override was withheld: {stderr}"
        );
    }

    // The same refused origin with no variable set must stay quiet:
    // the explanation exists for the operator who *did* export a
    // token, and would be noise for everyone else.
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(app.join("cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, &auth_required_for(server.url())),
        "expected the tokenless failure in: {stderr}"
    );
    assert!(
        !stderr.contains("CABIN_REGISTRY_TOKEN"),
        "an unset override must not be mentioned: {stderr}"
    );

    // `cabin yank` resolves its origin the same way, from the config
    // of the directory it runs in, and must withhold the token too.
    // The withheld-override warning is the discriminator here: it only
    // prints when the exported token was deliberately not attached
    // (the api-less fixture fails the same way with or without one).
    let assertion = cabin()
        .args(["-Z", "remote-registry", "yank", "acme/demo@0.1.0"])
        .current_dir(&app)
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            &format!(
                "CABIN_REGISTRY_TOKEN is set but was not used for `{}`",
                server.url()
            )
        ),
        "`cabin yank` must say the override was withheld: {stderr}"
    );

    // Control: the same origin, named by the user on the command
    // line, is eligible - so the failures above are the provenance
    // gate and not a broken fixture.
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(app.join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
}

/// A `[source-replacement]` hop disqualifies the override even when
/// the user declared both the replaced source and the replacement in
/// their own config: a resolution records the hops it walked, not
/// which file declared each of them, so the safe answer is the only
/// available one.  The sibling test above is the control - the same
/// user config reaching the same server *without* a hop keeps the
/// token.
#[test]
fn a_user_config_source_replacement_hop_loses_the_env_token() {
    let dir = TempDir::new().unwrap();
    let registry = stage_auth_required_registry(dir.path());
    let server = AuthRegistryServer::serve(registry, TEST_TOKEN);
    let declared = dead_loopback_url();
    let home = dir.path().join("config-home");
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!(
            "[registry]\nindex-url = \"{declared}\"\n\n[source-replacement]\n\"{declared}\" = \
             {{ index-url = \"{}\" }}\n",
            server.url()
        ))
        .unwrap();

    let assertion = cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, &auth_required_for(server.url())),
        "a replaced origin must be reached unauthenticated: {stderr}"
    );
}

/// The user's own config file is not the project speaking: a loopback
/// registry configured in `<CABIN_CONFIG_HOME>/config.toml` keeps the
/// `CABIN_REGISTRY_TOKEN` override, so the local-registry testing
/// workflow does not have to repeat `--index-url` on every command.
#[test]
fn a_user_config_loopback_index_still_receives_the_env_token() {
    let dir = TempDir::new().unwrap();
    let registry = stage_auth_required_registry(dir.path());
    let server = AuthRegistryServer::serve(registry, TEST_TOKEN);
    let home = dir.path().join("config-home");
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{}\"\n", server.url()))
        .unwrap();

    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
}

/// Consuming a hosted-style public registry needs no account, login,
/// or token: with no credentials configured anywhere, a scoped
/// dependency resolves, downloads, and builds - and no request ever
/// carries an `Authorization` header.
#[test]
fn build_scoped_package_from_a_public_registry_with_no_credentials() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_scoped_publishable_package(&pkg_root);
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();
    // The hosted registry's config shape: reads are public.
    let config = fs::read_to_string(registry.join("config.json")).unwrap();
    fs::write(
        registry.join("config.json"),
        config.replace(
            "\"kind\"",
            "\"auth-required\": false, \"api\": \"https://cabinpkg.com\", \"kind\"",
        ),
    )
    .unwrap();

    // A public file server that records whether any request carried a
    // credential.
    let server = std::sync::Arc::new(
        tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
    );
    let addr = server.server_addr().to_ip().expect("loopback addr");
    let url = format!("http://{addr}");
    let server_for_thread = std::sync::Arc::clone(&server);
    let registry_for_thread = registry.clone();
    let thread = std::thread::spawn(move || {
        let mut saw_authorization = false;
        while let Ok(req) = server_for_thread.recv() {
            saw_authorization |= req.headers().iter().any(|h| h.field.equiv("Authorization"));
            let path = req.url().trim_start_matches('/').to_owned();
            if path.contains("..") {
                let _ = req.respond(tiny_http::Response::empty(400));
                continue;
            }
            match fs::read(registry_for_thread.join(&path)) {
                Ok(bytes) => {
                    let _ = req.respond(tiny_http::Response::from_data(bytes));
                }
                Err(_) => {
                    let _ = req.respond(tiny_http::Response::empty(404));
                }
            }
        }
        saw_authorization
    });

    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"

[dependencies]
"acme/demo" = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["acme/demo"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int demo(void);\nint main(void) { return demo(); }\n")
        .unwrap();
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(&url)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    server.unblock();
    assert!(
        !thread.join().unwrap(),
        "a credential-less build must never send Authorization"
    );
}

/// The sparse read client never follows redirects, so a registry
/// cannot bounce an authenticated read toward another server: the
/// command fails on the 3xx and the redirect target receives zero
/// requests - the credential is not forwarded because *nothing* is.
#[test]
fn redirecting_registry_fails_without_contacting_the_target() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let dir = TempDir::new().unwrap();
    write_app_manifest(dir.path());

    // Target: counts every request it receives.
    let target = std::sync::Arc::new(
        tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
    );
    let target_addr = target.server_addr().to_ip().expect("loopback addr");
    let target_hits = std::sync::Arc::new(AtomicUsize::new(0));
    let target_for_thread = std::sync::Arc::clone(&target);
    let hits_for_thread = std::sync::Arc::clone(&target_hits);
    let target_thread = std::thread::spawn(move || {
        while let Ok(req) = target_for_thread.recv() {
            hits_for_thread.fetch_add(1, Ordering::SeqCst);
            let _ = req.respond(tiny_http::Response::empty(200));
        }
    });

    // Redirector: answers every request with a 302 toward the target.
    let redirector = std::sync::Arc::new(
        tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
    );
    let redirector_addr = redirector.server_addr().to_ip().expect("loopback addr");
    let redirector_url = format!("http://{redirector_addr}");
    let redirector_for_thread = std::sync::Arc::clone(&redirector);
    let location = format!("http://{target_addr}/config.json");
    let redirector_thread = std::thread::spawn(move || {
        while let Ok(req) = redirector_for_thread.recv() {
            let header = tiny_http::Header::from_bytes(&b"Location"[..], location.as_bytes())
                .expect("valid Location header");
            let _ = req.respond(tiny_http::Response::empty(302).with_header(header));
        }
    });

    // The credential makes the scenario adversarial: a client that
    // followed the redirect could be steered off the credentialed
    // origin.  (Loopback, so the env override is eligible.)
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(&redirector_url)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "server returned 302"),
        "expected the unfollowed-redirect error in: {stderr}"
    );
    assert_eq!(
        target_hits.load(Ordering::SeqCst),
        0,
        "the redirect target must never be contacted"
    );

    redirector.unblock();
    let _ = redirector_thread.join();
    target.unblock();
    let _ = target_thread.join();
}

/// A token for a plain-http, non-loopback origin would never be
/// attached by the client, so `cabin login` refuses to store it.
/// Loopback http (the local-testing exception) still works - the
/// end-to-end test above logs into `http://127.0.0.1:<port>`.
#[test]
fn login_refuses_plain_http_beyond_loopback() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "login",
            "--index-url",
            "http://registry.example.com",
        ])
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "never sent over plain `http`"),
        "expected the cleartext rejection in: {stderr}"
    );
    assert!(!home.join("credentials.toml").exists());
}

/// The GitHub grant the device flow mints is confined to the hosted
/// registry's API and loopback: `cabin login` refuses any other
/// index up front - before fetching its `config.json`, so this needs
/// no network - because a malicious registry could otherwise relay
/// the grant to the hosted mint and trade it for the user's session.
#[test]
fn login_refuses_a_non_hosted_https_registry() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "login",
            "--index-url",
            "https://third-party.example",
        ])
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            "refusing to log in to `https://third-party.example`: the login's GitHub grant"
        ),
        "expected the grant-confinement rejection in: {stderr}"
    );
    assert!(!home.join("credentials.toml").exists());
}

/// An explicit `--index-url` skips config discovery entirely, so a
/// broken config file (which fails every config-consuming command)
/// cannot fail `cabin login` / `cabin logout`.
#[test]
fn login_with_explicit_index_url_ignores_broken_config() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let github = GithubOauthServer::start();
    let registry = SessionRegistryServer::start();
    let base = registry.url.clone();
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str("this is not toml [")
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["-Z", "remote-registry", "login", "--index-url", &base])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "session for `{base}` saved"
        )));
}

// -----------------------------------------------------------------
// remote publish (`cabin publish --index-url`, -Z remote-registry)
// -----------------------------------------------------------------

/// Minimal publishable C package, so the staged-archive assertions
/// cover a C source tree.
fn write_publishable_package(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
c-standard = "c11"

[target.demo]
type = "library"
sources = ["src/demo.c"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/demo.c"))
        .write_str("int demo(void) { return 0; }\n")
        .unwrap();
}

/// A scoped variant of [`write_publishable_package`]: the shape a
/// real registry package takes, since publish requires a scoped
/// name.  Stages as `acme-demo-0.1.0.*` and publishes on the
/// `/api/v1/packages/acme/demo/<version>` route.
fn write_scoped_publishable_package(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "acme/demo"
version = "0.1.0"
c-standard = "c11"

[target.demo]
type = "library"
sources = ["src/demo.c"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/demo.c"))
        .write_str("int demo(void) { return 0; }\n")
        .unwrap();
}

/// Like [`write_scoped_publishable_package`], but versioned and with
/// a declared C interface standard, for the PL3 (a patch narrows a
/// declared standard) baseline tests.
fn write_scoped_c_interface_package(root: &Path, version: &str, interface_c: &str) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "acme/demo"
version = "{version}"

[target.demo]
type = "library"
sources = ["src/demo.c"]
include-dirs = ["include"]
c-standard = "c17"
interface-c-standard = "{interface_c}"
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("include/demo.h"))
        .write_str("#pragma once\nint demo_value(void);\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/demo.c"))
        .write_str("#include \"demo.h\"\nint demo_value(void) { return 1; }\n")
        .unwrap();
}

/// One captured mutation request against [`RemoteRegistryServer`].
struct CapturedPut {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

/// Mock remote registry: serves `config.json` (optionally declaring
/// this server as its own `api` origin), 404s every package read
/// (nothing is published yet), and answers mutation requests under
/// `/api/v1/packages/` with the configured status sequence (the last
/// entry repeats), capturing each one.  With `require_auth`, every
/// route 401s tokenless requests - the `auth-required` registry
/// shape, where even `config.json` is behind auth.
struct RemoteRegistryServer {
    server: std::sync::Arc<tiny_http::Server>,
    thread: Option<std::thread::JoinHandle<()>>,
    url: String,
    puts: std::sync::Arc<std::sync::Mutex<Vec<CapturedPut>>>,
}

impl RemoteRegistryServer {
    fn start(include_api: bool, require_auth: bool, put_statuses: &'static [u16]) -> Self {
        Self::start_full(include_api, None, require_auth, put_statuses, None)
    }

    /// Like [`Self::start`], but every mutation response carries
    /// `put_body` instead of the per-status default - e.g. a `201`
    /// with the verification lifecycle's `"verification":"pending"`.
    fn start_with_put_body(
        include_api: bool,
        require_auth: bool,
        put_statuses: &'static [u16],
        put_body: Option<&'static str>,
    ) -> Self {
        Self::start_full(include_api, None, require_auth, put_statuses, put_body)
    }

    /// Like [`Self::start`], but `config.json` declares `api_origin` -
    /// a *different* server - as the mutation origin, the shape of the
    /// hostname-role split.
    fn start_with_api_origin(
        api_origin: String,
        require_auth: bool,
        put_statuses: &'static [u16],
    ) -> Self {
        Self::start_full(true, Some(api_origin), require_auth, put_statuses, None)
    }

    fn start_full(
        include_api: bool,
        api_override: Option<String>,
        require_auth: bool,
        put_statuses: &'static [u16],
        put_body: Option<&'static str>,
    ) -> Self {
        let server = std::sync::Arc::new(
            tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
        );
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let url = format!("http://{addr}");
        let auth_field = if require_auth {
            ",\n    \"auth-required\": true"
        } else {
            ""
        };
        let api_value = api_override.unwrap_or_else(|| url.clone());
        let config = if include_api {
            format!(
                r#"{{
    "schema": 1,
    "kind": "file-registry",
    "packages": "packages",
    "artifacts": "artifacts"{auth_field},
    "api": "{api_value}"
}}"#
            )
        } else {
            format!(
                r#"{{
    "schema": 1,
    "kind": "file-registry",
    "packages": "packages",
    "artifacts": "artifacts"{auth_field}
}}"#
            )
        };
        let puts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CapturedPut>::new()));
        let puts_for_thread = std::sync::Arc::clone(&puts);
        let server_for_thread = std::sync::Arc::clone(&server);
        let thread = std::thread::spawn(move || {
            while let Ok(mut req) = server_for_thread.recv() {
                let path = req.url().to_owned();
                if require_auth {
                    let authorized = req.headers().iter().any(|h| {
                        h.field.equiv("Authorization") && h.value == format!("Bearer {TEST_TOKEN}")
                    });
                    if !authorized {
                        let _ = req.respond(
                            tiny_http::Response::from_string(
                                r#"{"errors":[{"detail":"authentication required"}]}"#,
                            )
                            .with_status_code(401),
                        );
                        continue;
                    }
                }
                if path == "/config.json" {
                    let _ = req.respond(tiny_http::Response::from_string(config.clone()));
                } else if path.starts_with("/api/v1/packages/") {
                    let mut body = Vec::new();
                    let _ = req.as_reader().read_to_end(&mut body);
                    let mut puts = puts_for_thread.lock().unwrap();
                    let status = put_statuses[puts.len().min(put_statuses.len().saturating_sub(1))];
                    puts.push(CapturedPut {
                        method: req.method().as_str().to_owned(),
                        path,
                        authorization: req
                            .headers()
                            .iter()
                            .find(|h| h.field.equiv("Authorization"))
                            .map(|h| h.value.to_string()),
                        body,
                    });
                    drop(puts);
                    let body = put_body.unwrap_or(match status {
                        200 => r#"{"ok":true,"no_op":true}"#,
                        201 => r#"{"ok":true}"#,
                        409 => {
                            r#"{"errors":[{"detail":"the version is already published with different bytes; published revisions are immutable - pass `--new-revision` to publish the changed bytes as a new packaging revision of this version, or bump the version"}]}"#
                        }
                        _ => r#"{"errors":[{"detail":"unexpected"}]}"#,
                    });
                    let _ = req
                        .respond(tiny_http::Response::from_string(body).with_status_code(status));
                } else {
                    // Package reads: nothing is published yet.
                    let _ = req.respond(tiny_http::Response::empty(404));
                }
            }
        });
        Self {
            server,
            thread: Some(thread),
            url,
            puts,
        }
    }
}

impl Drop for RemoteRegistryServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Decode the crates.io-style publish frame:
/// `[u32 LE metadata_len][metadata][u32 LE archive_len][archive]`.
fn decode_publish_frame(body: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let metadata_len = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
    let metadata = body[4..4 + metadata_len].to_vec();
    let rest = &body[4 + metadata_len..];
    let archive_len = u32::from_le_bytes(rest[0..4].try_into().unwrap()) as usize;
    let archive = rest[4..4 + archive_len].to_vec();
    assert_eq!(
        body.len(),
        8 + metadata_len + archive_len,
        "the frame must be exactly consumed"
    );
    (metadata, archive)
}

/// Without `-Z remote-registry`, the `--index-url` flag fails with
/// the standard experimental-feature error before any network or
/// staging work - on the real publish path and on the (local)
/// dry-run path alike, so the experimental flag is never silently
/// ignored.
#[test]
fn publish_against_an_http_index_requires_the_feature() {
    let dir = TempDir::new().unwrap();
    write_publishable_package(dir.path());
    for dry_run in [false, true] {
        let mut cmd = cabin();
        cmd.arg("publish");
        if dry_run {
            cmd.arg("--dry-run");
        }
        let assertion = cmd
            .args([
                "--index-url",
                "https://registry.example.com",
                "--manifest-path",
            ])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            flat_contains(
                &stderr,
                "`cabin publish --index-url` requires the experimental remote-registry client; \
                 run with `-Z remote-registry` to enable it"
            ),
            "expected the gated-command error (dry_run={dry_run}) in: {stderr}"
        );
    }
}

/// `--dry-run` stays entirely local: the staging artifacts land in
/// the output directory and no connection is ever opened to the
/// index URL.
#[test]
fn publish_dry_run_with_an_http_index_opens_no_connection() {
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());
    // A bound-but-unaccepting listener: any connection attempt would
    // be observable below.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    cabin()
        .args([
            "-Z",
            "remote-registry",
            "publish",
            "--dry-run",
            "--index-url",
        ])
        .arg(&url)
        .args(["--output-dir"])
        .arg(dir.path().join("staging"))
        .args(["--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "This was a dry run. No registry was modified.",
        ));

    assert!(
        dir.path().join("staging/acme-demo-0.1.0.zip").is_file(),
        "the dry-run must stage locally into --output-dir"
    );
    match listener.accept() {
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("--dry-run must not open a connection to the registry"),
        Err(err) => panic!("unexpected listener state: {err}"),
    }
}

/// Registry packages are always `<scope>/<name>`: a bare name fails
/// the publish gate before credentials or any connection.
#[test]
fn publish_rejects_bare_names_before_any_connection() {
    let dir = TempDir::new().unwrap();
    // A bound-but-unaccepting listener: any connection attempt would
    // be observable below.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    write_publishable_package(dir.path());
    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&url)
        .args(["--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "registry packages must be named `<scope>/<name>`"),
        "expected the bare-name gate diagnostic in: {stderr}"
    );

    match listener.accept() {
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("a refused publish must not open a connection to the registry"),
        Err(err) => panic!("unexpected listener state: {err}"),
    }
}

/// Registry dependency maps key on canonical `<scope>/<name>` names:
/// a bare dependency key fails the same pre-network gate as a bare
/// package name.
#[test]
fn publish_rejects_bare_dependency_names_before_any_connection() {
    let dir = TempDir::new().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    write_scoped_publishable_package(dir.path());
    let manifest = dir.path().join("cabin.toml");
    let mut body = std::fs::read_to_string(&manifest).unwrap();
    body.push_str("\n[dependencies]\nzlib = \"^1.3\"\n");
    std::fs::write(&manifest, body).unwrap();

    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&url)
        .args(["--manifest-path"])
        .arg(&manifest)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            "registry dependencies must use the canonical `<scope>/<name>` grammar"
        ),
        "expected the dependency-key gate diagnostic in: {stderr}"
    );

    match listener.accept() {
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("a refused publish must not open a connection to the registry"),
        Err(err) => panic!("unexpected listener state: {err}"),
    }
}

/// The full upload path: the PUT hits the registry's `api` origin on
/// the scoped route with the bearer token, and the framed metadata +
/// archive bytes are byte-identical to what `cabin package` produces
/// for the same source tree.
#[test]
fn publish_uploads_bytes_identical_to_cabin_package() {
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());

    // What `cabin package` produces for this tree.
    let dist = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success();
    let packaged_archive = fs::read(dist.join("acme-demo-0.1.0.zip")).unwrap();
    let packaged_metadata = fs::read(dist.join("acme-demo-0.1.0.json")).unwrap();

    let server = RemoteRegistryServer::start(true, true, &[201]);
    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&server.url)
        .args(["--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains(&format!("Published acme/demo 0.1.0 to {}", server.url)),
        "expected the created report in: {stdout}"
    );
    assert!(
        stdout.contains("checksum: sha256:"),
        "expected the checksum in: {stdout}"
    );
    // A registry without the verification lifecycle omits the field;
    // the report must not invent a verification line.
    assert!(
        !stdout.contains("verification"),
        "unexpected verification line in: {stdout}"
    );

    let puts = server.puts.lock().unwrap();
    assert_eq!(puts.len(), 1, "exactly one publish request");
    let put = &puts[0];
    assert_eq!(put.method, "PUT");
    assert_eq!(put.path, "/api/v1/packages/acme/demo/0.1.0");
    assert_eq!(
        put.authorization.as_deref(),
        Some(format!("Bearer {TEST_TOKEN}").as_str()),
        "the publish must carry the bearer credential"
    );
    let (metadata, archive) = decode_publish_frame(&put.body);
    assert_eq!(
        metadata, packaged_metadata,
        "uploaded metadata must be the canonical document cabin package writes"
    );
    assert_eq!(
        archive, packaged_archive,
        "uploaded archive must be byte-identical to the cabin package archive"
    );
}

/// The amended credential-destination rule (`docs/remote-registry.md`,
/// "When the token is sent"): a token stored under the index origin is
/// sent to that origin's reads *and* to the `api` origin its
/// authenticated `config.json` declares - here a different server, the
/// hostname-role split's shape - and the mutation reaches only the api
/// origin, never the index origin.
#[test]
fn publish_sends_the_token_to_the_config_declared_api_origin() {
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());

    // The api origin accepts the upload; the index origin serves the
    // auth-required reads and must see no mutation (a PUT reaching it
    // would fail the run with its 500).
    let api_server = RemoteRegistryServer::start(false, false, &[201]);
    let index_server =
        RemoteRegistryServer::start_with_api_origin(api_server.url.clone(), true, &[500]);

    // The credential is stored under the *index* origin, exactly as
    // `cabin login` would leave it.
    let home = dir.path().join("config-home");
    write_session_credentials(&home, &index_server.url, TEST_TOKEN);

    cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&index_server.url)
        .args(["--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .success();

    let api_puts = api_server.puts.lock().unwrap();
    assert_eq!(api_puts.len(), 1, "exactly one publish, on the api origin");
    assert_eq!(api_puts[0].path, "/api/v1/packages/acme/demo/0.1.0");
    assert_eq!(
        api_puts[0].authorization.as_deref(),
        Some(format!("Bearer {TEST_TOKEN}").as_str()),
        "the stored token must reach the config-declared api origin"
    );
    assert!(
        index_server.puts.lock().unwrap().is_empty(),
        "the index origin must never receive the mutation"
    );
}

/// Re-publishing identical bytes is the idempotent `200` no-op, and
/// a `409` explains that published versions are immutable.
#[test]
fn publish_reports_no_op_and_conflict_outcomes() {
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());

    let server = RemoteRegistryServer::start(true, false, &[201, 200]);
    for _ in 0..2 {
        cabin()
            .args(["-Z", "remote-registry", "publish", "--index-url"])
            .arg(&server.url)
            .args(["--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
            .assert()
            .success();
    }
    drop(server);

    let server = RemoteRegistryServer::start(true, false, &[200]);
    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&server.url)
        .args(["--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("acme/demo 0.1.0 is already published to")
            && stdout.contains("identical bytes; nothing to do"),
        "expected the no-op report in: {stdout}"
    );
    drop(server);

    let server = RemoteRegistryServer::start(true, false, &[409]);
    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&server.url)
        .args(["--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "already published with different bytes"),
        "expected the conflict explanation in: {stderr}"
    );
    // The diagnostic explains the packaging-revision mechanism: the
    // opt-in is the intended path for packaging-only corrections.
    assert!(
        flat_contains(&stderr, "published revisions are immutable"),
        "expected the immutability explanation in: {stderr}"
    );
    assert!(
        flat_contains(&stderr, "--new-revision"),
        "expected the opt-in guidance in: {stderr}"
    );
}

/// A registry with the asynchronous verification lifecycle answers
/// the publish with `"verification":"pending"`; the report says the
/// version was accepted and becomes resolvable after verification.
#[test]
fn publish_reports_pending_verification() {
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());
    let server = RemoteRegistryServer::start_with_put_body(
        true,
        false,
        &[201],
        Some(
            r#"{"ok":true,"name":"acme/demo","version":"0.1.0","checksum":"sha256:aa","verification":"pending"}"#,
        ),
    );
    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&server.url)
        .args(["--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains(&format!("Published acme/demo 0.1.0 to {}", server.url)),
        "expected the created report in: {stdout}"
    );
    assert!(
        stdout.contains("verification: pending"),
        "expected the pending verification line in: {stdout}"
    );
    assert!(
        stdout.contains("accepted") && stdout.contains("typically within a few minutes"),
        "expected the resolvable-after-verification wording in: {stdout}"
    );
}

/// A registry whose `config.json` lacks the `api` field cannot be
/// published to; the error names the missing field.
#[test]
fn publish_requires_the_api_url_in_the_registry_config() {
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());
    let server = RemoteRegistryServer::start(false, false, &[201]);
    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&server.url)
        .args(["--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "does not declare an `api` URL in its config.json"),
        "expected the missing-api error in: {stderr}"
    );
    assert!(
        server.puts.lock().unwrap().is_empty(),
        "no mutation request may be sent without an api origin"
    );
}

/// `--output-dir` belongs to the dry-run staging flow, so passing it
/// without `--dry-run` keeps the "requires --registry-dir or
/// --dry-run" error even when the config supplies an `index-url` -
/// an intended local staging run must never fall through into a
/// real remote publish.
#[test]
fn publish_output_dir_without_dry_run_never_publishes_remotely() {
    let dir = TempDir::new().unwrap();
    write_publishable_package(dir.path());
    // A bound-but-unaccepting listener as the configured registry:
    // any connection attempt would be observable below.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let home = dir.path().join("config-home");
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!(
            "[registry]\nindex-url = \"http://{}\"\n",
            listener.local_addr().unwrap()
        ))
        .unwrap();

    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    let assertion = cmd
        .args(["-Z", "remote-registry", "publish", "--output-dir"])
        .arg(dir.path().join("staging"))
        .args(["--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "requires either `--registry-dir <DIR>`"),
        "expected the dry-run-required error in: {stderr}"
    );
    match listener.accept() {
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("--output-dir without --dry-run must not contact the registry"),
        Err(err) => panic!("unexpected listener state: {err}"),
    }
}

// -----------------------------------------------------------------
// cabin yank (`-Z remote-registry`)
// -----------------------------------------------------------------

/// Without `-Z remote-registry`, `cabin yank` fails with the
/// standard experimental-feature wording before parsing the spec or
/// touching config.
#[test]
fn yank_requires_the_feature() {
    let assertion = cabin()
        .args([
            "yank",
            "fmt@10.2.1",
            "--index-url",
            "https://registry.example.com",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            "`cabin yank` requires the experimental remote-registry client; run with \
             `-Z remote-registry` to enable it"
        ),
        "expected the gated-command error in: {stderr}"
    );
}

/// The spec is strict: a missing version, an inexact version, and a
/// range are all rejected with a clear message before any index
/// resolution or network work.
#[test]
fn yank_rejects_malformed_specs() {
    for (spec, expected) in [
        ("fmt", "expected `<name>@<version>`"),
        ("fmt@banana", "is not an exact SemVer version"),
        ("fmt@^10.0.0", "is not an exact SemVer version"),
        ("fmt@10.2", "is not an exact SemVer version"),
    ] {
        let assertion = cabin()
            .args(["-Z", "remote-registry", "yank", spec])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            flat_contains(&stderr, &format!("invalid package spec `{spec}`"))
                && flat_contains(&stderr, expected),
            "{spec}: expected the spec-parse error in: {stderr}"
        );
    }
}

/// A bare name cannot exist on a remote registry, so `cabin yank`
/// refuses it before credentials, config reads, or any connection.
#[test]
fn yank_rejects_bare_names_before_any_connection() {
    // A bound-but-unaccepting listener: any connection attempt would
    // be observable below.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    let assertion = cabin()
        .args(["-Z", "remote-registry", "yank", "fmt@10.2.1", "--index-url"])
        .arg(&url)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "registry packages must be named `<scope>/<name>`"),
        "expected the bare-name rejection in: {stderr}"
    );
    assert!(
        flat_contains(&stderr, "`<scope>/fmt@10.2.1`"),
        "expected the scoped-spec example in: {stderr}"
    );
    match listener.accept() {
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("a refused yank must not open a connection to the registry"),
        Err(err) => panic!("unexpected listener state: {err}"),
    }
}

/// The full yank path against an `auth-required` registry: the PATCH
/// hits the registry's `api` origin with the bearer token and the
/// documented JSON body, and the report states the resulting state.
/// The route only ever answers a successful call with the idempotent
/// `200`, so this also pins the wording a no-op renders.
#[test]
fn yank_and_undo_patch_the_yank_route() {
    let server = RemoteRegistryServer::start(true, true, &[200]);

    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "yank",
            "fmtlib/fmt@10.2.1",
            "--index-url",
        ])
        .arg(&server.url)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("fmtlib/fmt@10.2.1 is now yanked"),
        "expected the resulting-state report in: {stdout}"
    );

    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "yank",
            "--undo",
            "fmtlib/fmt@10.2.1",
            "--index-url",
        ])
        .arg(&server.url)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("fmtlib/fmt@10.2.1 is no longer yanked"),
        "expected the resulting-state report in: {stdout}"
    );

    let requests = server.puts.lock().unwrap();
    assert_eq!(requests.len(), 2, "exactly one request per invocation");
    for (request, expected_body) in requests.iter().zip([
        br#"{"yanked":true}"#.as_slice(),
        br#"{"yanked":false}"#.as_slice(),
    ]) {
        assert_eq!(request.method, "PATCH");
        assert_eq!(request.path, "/api/v1/packages/fmtlib/fmt/10.2.1/yank");
        assert_eq!(request.body, expected_body);
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {TEST_TOKEN}").as_str()),
            "the yank must carry the bearer credential"
        );
    }
}

/// A `404` from the yank route maps to the not-published error.
#[test]
fn yank_maps_404_to_not_published() {
    let server = RemoteRegistryServer::start(true, false, &[404]);
    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "yank",
            "fmtlib/fmt@9.9.9",
            "--index-url",
        ])
        .arg(&server.url)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            "`fmtlib/fmt@9.9.9` is not published on this registry"
        ),
        "expected the not-published error in: {stderr}"
    );
}

/// A config-supplied local `index-path` cannot be yanked against:
/// yanked state lives in the remote registry's index.
#[test]
fn yank_rejects_a_local_index_path() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str("[registry]\nindex-path = \"registry\"\n")
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    let assertion = cmd
        .args(["-Z", "remote-registry", "yank", "fmtlib/fmt@10.2.1"])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "`cabin yank` requires an HTTP registry"),
        "expected the local-path rejection in: {stderr}"
    );
}

/// A registry whose `config.json` lacks the `api` field cannot be
/// yanked against; the error names the missing field and no mutation
/// request is ever sent.
#[test]
fn yank_requires_the_api_url_in_the_registry_config() {
    let server = RemoteRegistryServer::start(false, false, &[200]);
    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "yank",
            "fmtlib/fmt@10.2.1",
            "--index-url",
        ])
        .arg(&server.url)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "does not declare an `api` URL in its config.json"),
        "expected the missing-api error in: {stderr}"
    );
    assert!(
        server.puts.lock().unwrap().is_empty(),
        "no mutation request may be sent without an api origin"
    );
}

/// A config-supplied registry source is subject to
/// `[source-replacement]` on the fetch path, so `cabin login` keys
/// the token on the replaced origin - the one a later fetch will
/// actually contact.
#[test]
fn login_applies_source_replacement_to_the_config_registry() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    // The upstream is never contacted (the replacement wins before
    // any request); the mirror mints the session.
    let github = GithubOauthServer::start();
    let mirror_registry = SessionRegistryServer::start();
    let mirror = mirror_registry.url.clone();
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!(
            "[registry]\nindex-url = \"https://upstream.example.com/index\"\n\n\
             [source-replacement]\n\"https://upstream.example.com/index\" = \
             {{ index-url = \"{mirror}/index\" }}\n",
        ))
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["-Z", "remote-registry", "login"])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_GITHUB_OAUTH_URL", &github.url)
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "session for `{mirror}` saved"
        )));
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert!(body.contains(&mirror), "{body}");
    assert!(!body.contains("upstream.example.com"), "{body}");
}

// -----------------------------------------------------------------
// trusted publishing (GitHub Actions auto-exchange)
// -----------------------------------------------------------------

/// The token the trustpub fake mints, in the registry's real
/// `cabin_tp_<base64url>` shape.
const MINTED_TOKEN: &str = "cabin_tp_bWludGVkLXRva2VuLWJ5dGVz0123456_-A";

/// A stand-in Actions OIDC JWT; the client treats it as an opaque
/// string.
const FAKE_JWT: &str = "eyJhbGciOiJSUzI1NiJ9.eyJhdWQiOiJjYWJpbnBrZy5jb20ifQ.c2ln";

/// One request captured by [`TrustpubRegistryServer`], in arrival
/// order across every route, so tests can assert the
/// exchange -> publish -> revoke sequencing.
struct CapturedTrustpub {
    method: String,
    path: String,
    authorization: Option<String>,
    body: Vec<u8>,
}

/// Mock hosted-registry shape for trusted publishing: a public
/// `config.json` declaring this server as its own `api` origin,
/// `PUT /api/v1/trusted_publishing/tokens` minting [`MINTED_TOKEN`]
/// (no `Authorization` expected - the JWT in the body is the
/// credential), `DELETE` on the same route answering `204`, package
/// mutations under `/api/v1/packages/` answering the configured
/// status sequence (last entry repeats), and package reads 404ing.
struct TrustpubRegistryServer {
    server: std::sync::Arc<tiny_http::Server>,
    thread: Option<std::thread::JoinHandle<()>>,
    url: String,
    captured: std::sync::Arc<std::sync::Mutex<Vec<CapturedTrustpub>>>,
}

impl TrustpubRegistryServer {
    fn start(mutation_statuses: &'static [u16]) -> Self {
        Self::start_with_api(None, mutation_statuses)
    }

    /// Like [`Self::start`], but config.json declares `api_override`
    /// instead of this server itself - the hostile shape a
    /// project-steered loopback index can take.
    fn start_with_api(api_override: Option<&str>, mutation_statuses: &'static [u16]) -> Self {
        let server = std::sync::Arc::new(
            tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
        );
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let url = format!("http://{addr}");
        let api_value = api_override.map_or_else(|| url.clone(), str::to_owned);
        let config = format!(
            r#"{{
    "schema": 1,
    "kind": "file-registry",
    "packages": "packages",
    "artifacts": "artifacts",
    "api": "{api_value}"
}}"#
        );
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CapturedTrustpub>::new()));
        let captured_for_thread = std::sync::Arc::clone(&captured);
        let server_for_thread = std::sync::Arc::clone(&server);
        let thread = std::thread::spawn(move || {
            let mut mutations = 0usize;
            while let Ok(mut req) = server_for_thread.recv() {
                let path = req.url().to_owned();
                let method = req.method().as_str().to_owned();
                let mut body = Vec::new();
                let _ = req.as_reader().read_to_end(&mut body);
                let authorization = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.to_string());
                let record = path.starts_with("/api/v1/");
                if record {
                    captured_for_thread.lock().unwrap().push(CapturedTrustpub {
                        method: method.clone(),
                        path: path.clone(),
                        authorization: authorization.clone(),
                        body,
                    });
                }
                if path == "/config.json" {
                    let _ = req.respond(tiny_http::Response::from_string(config.clone()));
                } else if path == "/api/v1/trusted_publishing/tokens" {
                    let response = match method.as_str() {
                        "PUT" => tiny_http::Response::from_string(format!(
                            r#"{{"token":"{MINTED_TOKEN}","expires_at":"2099-01-01T00:00:00.000Z"}}"#
                        ))
                        .with_status_code(200),
                        "DELETE"
                            if authorization.as_deref()
                                == Some(&format!("Bearer {MINTED_TOKEN}")) =>
                        {
                            tiny_http::Response::from_string("").with_status_code(204)
                        }
                        _ => tiny_http::Response::from_string(
                            r#"{"errors":[{"detail":"unauthorized"}]}"#,
                        )
                        .with_status_code(401),
                    };
                    let _ = req.respond(response);
                } else if path.starts_with("/api/v1/packages/") {
                    let status =
                        mutation_statuses[mutations.min(mutation_statuses.len().saturating_sub(1))];
                    mutations += 1;
                    let body = match status {
                        200 => r#"{"ok":true,"no_op":true}"#,
                        201 => r#"{"ok":true}"#,
                        403 => r#"{"errors":[{"detail":"scope membership required"}]}"#,
                        429 => r#"{"errors":[{"detail":"rate limited"}]}"#,
                        _ => r#"{"errors":[{"detail":"unexpected"}]}"#,
                    };
                    let mut response =
                        tiny_http::Response::from_string(body).with_status_code(status);
                    if status == 429 {
                        // A short advertised delay keeps pacing tests fast.
                        response.add_header(
                            tiny_http::Header::from_bytes(&b"Retry-After"[..], &b"1"[..])
                                .expect("valid test header"),
                        );
                    }
                    let _ = req.respond(response);
                } else {
                    // Package reads: nothing is published yet.
                    let _ = req.respond(tiny_http::Response::empty(404));
                }
            }
        });
        Self {
            server,
            thread: Some(thread),
            url,
            captured,
        }
    }

    fn captured(&self) -> Vec<CapturedTrustpub> {
        std::mem::take(&mut *self.captured.lock().unwrap())
    }
}

impl Drop for TrustpubRegistryServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// One request captured by [`OidcServer`]: the request line and the
/// `Authorization` header.
type CapturedOidc = (String, Option<String>);

/// Mock of the runner's OIDC endpoint: answers `{"value": FAKE_JWT}`
/// and captures the request line + `Authorization` header.
struct OidcServer {
    server: std::sync::Arc<tiny_http::Server>,
    thread: Option<std::thread::JoinHandle<()>>,
    request_url: String,
    captured: std::sync::Arc<std::sync::Mutex<Vec<CapturedOidc>>>,
}

impl OidcServer {
    fn start() -> Self {
        let server = std::sync::Arc::new(
            tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
        );
        let addr = server.server_addr().to_ip().expect("loopback addr");
        // The real variable always carries a query string already;
        // the client appends `&audience=...`.
        let request_url = format!("http://{addr}/token?api-version=2");
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_for_thread = std::sync::Arc::clone(&captured);
        let server_for_thread = std::sync::Arc::clone(&server);
        let thread = std::thread::spawn(move || {
            while let Ok(req) = server_for_thread.recv() {
                let authorization = req
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .map(|h| h.value.to_string());
                captured_for_thread
                    .lock()
                    .unwrap()
                    .push((req.url().to_owned(), authorization));
                let _ = req.respond(tiny_http::Response::from_string(format!(
                    r#"{{"value":"{FAKE_JWT}"}}"#
                )));
            }
        });
        Self {
            server,
            thread: Some(thread),
            request_url,
            captured,
        }
    }

    fn captured(&self) -> Vec<CapturedOidc> {
        std::mem::take(&mut *self.captured.lock().unwrap())
    }
}

impl Drop for OidcServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// A `cabin` invocation dressed as a GitHub Actions run with
/// `id-token: write`: the marker plus the runner's OIDC endpoint
/// pair.
fn cabin_under_actions(oidc: &OidcServer) -> Command {
    let mut cmd = cabin();
    cmd.env("GITHUB_ACTIONS", "true")
        .env("ACTIONS_ID_TOKEN_REQUEST_URL", &oidc.request_url)
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "runner-request-token");
    cmd
}

/// The full happy path: with no explicit token under Actions, publish
/// fetches the run's JWT (bearer + audience), exchanges it, publishes
/// with the minted token, masks both secrets, and revokes on exit.
#[test]
fn publish_auto_exchanges_under_github_actions() {
    let registry = TrustpubRegistryServer::start(&[201]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());

    let assertion = cabin_under_actions(&oidc)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .success()
        .stdout(predicate::str::contains("Published acme/demo 0.1.0"));

    // Both secrets are masked out of the runner log, in mint order,
    // on stderr - the runner processes workflow commands on both
    // streams, and stdout must stay parseable (`--format json`).
    let output = assertion.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let jwt_mask = stderr.find(&format!("::add-mask::{FAKE_JWT}"));
    let token_mask = stderr.find(&format!("::add-mask::{MINTED_TOKEN}"));
    assert!(jwt_mask.is_some() && token_mask.is_some(), "{stderr}");
    assert!(jwt_mask < token_mask, "JWT must be masked first: {stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("::add-mask::"),
        "workflow commands must not corrupt stdout: {stdout}"
    );

    // The runner's endpoint saw the bearer and the audience.
    let oidc_requests = oidc.captured();
    assert_eq!(oidc_requests.len(), 1, "one exchange per process");
    assert_eq!(
        oidc_requests[0].0,
        "/token?api-version=2&audience=cabinpkg.com"
    );
    assert_eq!(
        oidc_requests[0].1.as_deref(),
        Some("Bearer runner-request-token")
    );

    // Exchange -> publish -> revoke, with the right credentials.
    let captured = registry.captured();
    let sequence: Vec<(&str, &str)> = captured
        .iter()
        .map(|c| (c.method.as_str(), c.path.as_str()))
        .collect();
    assert_eq!(
        sequence,
        [
            ("PUT", "/api/v1/trusted_publishing/tokens"),
            ("PUT", "/api/v1/packages/acme/demo/0.1.0"),
            ("DELETE", "/api/v1/trusted_publishing/tokens"),
        ],
        "unexpected request sequence"
    );
    assert_eq!(captured[0].authorization, None, "the JWT is the credential");
    assert_eq!(
        captured[0].body,
        format!(r#"{{"jwt":"{FAKE_JWT}"}}"#).into_bytes()
    );
    assert_eq!(
        captured[1].authorization.as_deref(),
        Some(&*format!("Bearer {MINTED_TOKEN}"))
    );
}

/// Revocation is exit-path behavior, not success-path behavior: a
/// publish the registry refuses still revokes the minted token.
#[test]
fn revoke_runs_when_the_publish_fails() {
    let registry = TrustpubRegistryServer::start(&[403]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());

    cabin_under_actions(&oidc)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .failure();

    let captured = registry.captured();
    let last = captured.last().expect("requests were made");
    assert_eq!(
        (last.method.as_str(), last.path.as_str()),
        ("DELETE", "/api/v1/trusted_publishing/tokens"),
        "the failed publish must still revoke"
    );
}

/// Precedence: the explicit `CABIN_REGISTRY_TOKEN` override outranks
/// the exchange - no OIDC fetch, no mint, no revoke.
#[test]
fn explicit_env_token_wins_over_the_exchange() {
    let registry = TrustpubRegistryServer::start(&[201]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());

    cabin_under_actions(&oidc)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .success();

    assert_eq!(oidc.captured().len(), 0, "no OIDC fetch may happen");
    let captured = registry.captured();
    let sequence: Vec<(&str, &str)> = captured
        .iter()
        .map(|c| (c.method.as_str(), c.path.as_str()))
        .collect();
    assert_eq!(sequence, [("PUT", "/api/v1/packages/acme/demo/0.1.0")]);
    assert_eq!(
        captured[0].authorization.as_deref(),
        Some(&*format!("Bearer {TEST_TOKEN}"))
    );
}

/// Precedence: under Actions the exchange outranks a stored
/// `credentials.toml` entry, matching the documented order.
#[test]
fn the_exchange_outranks_a_stored_credential() {
    let registry = TrustpubRegistryServer::start(&[201]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    write_scoped_publishable_package(dir.path());
    write_session_credentials(&home, &registry.url, TEST_TOKEN);

    cabin_under_actions(&oidc)
        .env("CABIN_CONFIG_HOME", &home)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .success();

    let captured = registry.captured();
    let publish = captured
        .iter()
        .find(|c| c.path.starts_with("/api/v1/packages/"))
        .expect("a publish request");
    assert_eq!(
        publish.authorization.as_deref(),
        Some(&*format!("Bearer {MINTED_TOKEN}")),
        "the exchange outranks the stored credential"
    );
}

/// Actions without the OIDC endpoint is a misconfigured workflow: the
/// error names the missing permission before any network traffic.
#[test]
fn github_actions_without_the_oidc_endpoint_fails_actionably() {
    let registry = TrustpubRegistryServer::start(&[201]);
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());

    cabin()
        .env("GITHUB_ACTIONS", "true")
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .failure()
        .stderr(predicate::str::contains("id-token: write"));

    assert_eq!(
        registry.captured().len(),
        0,
        "the misconfiguration must fail before any exchange or mutation request"
    );
}

/// `cabin yank` never auto-exchanges: the registry mints exchanged
/// tokens with only the `publish` scope, so the yank route would
/// refuse one - and the exchange outranking the store would shadow a
/// working stored yank credential.  Under full Actions ambience the
/// stored token is used and the runner's OIDC endpoint sees nothing.
#[test]
fn yank_never_auto_exchanges_under_github_actions() {
    let registry = TrustpubRegistryServer::start(&[200]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    write_session_credentials(&home, &registry.url, TEST_TOKEN);

    cabin_under_actions(&oidc)
        .env("CABIN_CONFIG_HOME", &home)
        .args([
            "-Z",
            "remote-registry",
            "yank",
            "acme/demo@0.1.0",
            "--index-url",
            &registry.url,
        ])
        .assert()
        .success();

    assert_eq!(
        oidc.captured().len(),
        0,
        "yank must not fetch an OIDC token"
    );
    let captured = registry.captured();
    let sequence: Vec<(&str, &str)> = captured
        .iter()
        .map(|c| (c.method.as_str(), c.path.as_str()))
        .collect();
    assert_eq!(
        sequence,
        [("PATCH", "/api/v1/packages/acme/demo/0.1.0/yank")]
    );
    assert_eq!(
        captured[0].authorization.as_deref(),
        Some(&*format!("Bearer {TEST_TOKEN}")),
        "the stored yank-capable credential must win"
    );
}

/// The run's OIDC JWT is itself a credential: a project-steered
/// loopback index whose config.json names a non-loopback `api` must
/// be refused BEFORE the JWT is even fetched, or the named origin
/// could exchange the unconsumed JWT against the real registry.
#[test]
fn a_loopback_index_cannot_route_the_jwt_off_loopback() {
    let registry = TrustpubRegistryServer::start_with_api(Some("https://evil.example"), &[201]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    write_scoped_publishable_package(dir.path());

    cabin_under_actions(&oidc)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "must not receive the run's OIDC token",
        ));

    assert_eq!(
        oidc.captured().len(),
        0,
        "the refusal must precede the OIDC fetch"
    );
}

/// A loopback index reaches the exchange only through an explicit
/// `--index-url`: when project/user CONFIG selects the same loopback
/// registry, the run's OIDC token is never fetched - a checked-out
/// project's config could otherwise steer the JWT to a daemon its own
/// build code left listening.  The publish itself proceeds tokenless.
#[test]
fn a_config_selected_loopback_index_never_exchanges() {
    let registry = TrustpubRegistryServer::start(&[201]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    write_scoped_publishable_package(dir.path());
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{}\"\n", registry.url))
        .unwrap();

    cabin_under_actions(&oidc)
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();

    assert_eq!(
        oidc.captured().len(),
        0,
        "a config-selected loopback index must not trigger the OIDC fetch"
    );
    let captured = registry.captured();
    let sequence: Vec<(&str, &str)> = captured
        .iter()
        .map(|c| (c.method.as_str(), c.path.as_str()))
        .collect();
    assert_eq!(
        sequence,
        [("PUT", "/api/v1/packages/acme/demo/0.1.0")],
        "no exchange, no revocation - the publish went tokenless"
    );
    assert_eq!(captured[0].authorization, None);
}

/// The point of the batch: however many packages one invocation
/// publishes, the trusted-publishing leg fetches ONE OIDC token and
/// performs ONE exchange - the minted token serves every upload, in
/// the order the manifests were given, and revokes once at the end.
#[test]
fn a_batch_publish_exchanges_exactly_once() {
    let registry = TrustpubRegistryServer::start(&[201]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    assert_fs::fixture::ChildPath::new(zlib.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "acme/zlib"
version = "1.3.1"
c-standard = "c11"

[target.z]
type = "library"
sources = ["src/z.c"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(zlib.join("src/z.c"))
        .write_str("int z(void) { return 0; }\n")
        .unwrap();
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);

    cabin_under_actions(&oidc)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(zlib.join("cabin.toml"))
        .arg("--manifest-path")
        .arg(demo.join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .success();

    assert_eq!(oidc.captured().len(), 1, "one OIDC fetch per batch");
    let captured = registry.captured();
    let sequence: Vec<(&str, &str)> = captured
        .iter()
        .map(|c| (c.method.as_str(), c.path.as_str()))
        .collect();
    assert_eq!(
        sequence,
        [
            ("PUT", "/api/v1/trusted_publishing/tokens"),
            ("PUT", "/api/v1/packages/acme/zlib/1.3.1"),
            ("PUT", "/api/v1/packages/acme/demo/0.1.0"),
            ("DELETE", "/api/v1/trusted_publishing/tokens"),
        ],
        "one exchange, uploads in argv order, one revocation"
    );
    for upload in &captured[1..=2] {
        assert_eq!(
            upload.authorization.as_deref(),
            Some(&*format!("Bearer {MINTED_TOKEN}")),
            "every upload rides the one minted token"
        );
    }
}

/// The ordered batch simulates sequential publishes: a later
/// version's PL3 baseline must include the same-name versions this
/// invocation publishes before it, which the registry cannot know
/// yet (every baseline here is fetched before the first upload).
#[test]
fn an_in_batch_earlier_version_joins_the_lint_baseline() {
    let dir = TempDir::new().unwrap();
    let older = dir.path().join("older");
    write_scoped_c_interface_package(&older, "1.0.0", "c11");
    let newer = dir.path().join("newer");
    write_scoped_c_interface_package(&newer, "1.0.1", "c17");

    let server = RemoteRegistryServer::start(true, false, &[201, 201]);
    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&server.url)
        .args(["--manifest-path"])
        .arg(older.join("cabin.toml"))
        .args(["--manifest-path"])
        .arg(newer.join("cabin.toml"))
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "narrowed from")
            && flat_contains(&stderr, "c11")
            && flat_contains(&stderr, "c17"),
        "expected the in-batch PL3 warning in: {stderr}"
    );
}

/// A lint rejection renders target names, not package names; the
/// batch flow's context must say WHICH member to fix - and the
/// rejection publishes nothing (lints run before the first upload).
#[test]
fn a_batch_lint_rejection_names_the_failing_member() {
    let dir = TempDir::new().unwrap();
    let good = dir.path().join("good");
    write_scoped_c_interface_package(&good, "1.0.0", "c11");
    let bad = dir.path().join("bad");
    // A header-only implementation below its declared interface
    // minimum: the PL1 pair only the publish lints see (the load-time
    // contradiction check covers compiled targets).
    assert_fs::fixture::ChildPath::new(bad.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "acme/demo"
version = "1.0.1"

[target.demo]
type = "header-only"
include-dirs = ["include"]
cxx-standard = "c++17"
interface-cxx-standard = "c++20"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(bad.join("include/demo.h"))
        .write_str("#pragma once\n")
        .unwrap();

    let server = RemoteRegistryServer::start(true, false, &[201, 201]);
    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--index-url"])
        .arg(&server.url)
        .args(["--manifest-path"])
        .arg(good.join("cabin.toml"))
        .args(["--manifest-path"])
        .arg(bad.join("cabin.toml"))
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "linting acme/demo 1.0.1"),
        "expected the failing member's name in: {stderr}"
    );
    assert!(
        server.puts.lock().unwrap().is_empty(),
        "a lint rejection anywhere in the batch publishes nothing"
    );
}

/// A `429` mid-batch is paced, not fatal: the rate-limited upload
/// retries after the advertised delay and the batch completes.  The
/// pacing is batch-only - a single-package publish keeps its
/// historical fail-fast `429`, pinned by the second half.
#[test]
fn a_rate_limited_upload_is_paced_in_a_batch_and_fatal_alone() {
    // First upload (zlib) succeeds, second (demo) is limited once.
    let registry = TrustpubRegistryServer::start(&[201, 429, 201]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    assert_fs::fixture::ChildPath::new(zlib.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "acme/zlib"
version = "1.3.1"
c-standard = "c11"

[target.z]
type = "library"
sources = ["src/z.c"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(zlib.join("src/z.c"))
        .write_str("int z(void) { return 0; }\n")
        .unwrap();
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);

    cabin_under_actions(&oidc)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(zlib.join("cabin.toml"))
        .arg("--manifest-path")
        .arg(demo.join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .success()
        .stderr(predicate::str::contains("rate limited acme/demo 0.1.0"));

    let uploads = registry
        .captured()
        .iter()
        .filter(|c| c.path.starts_with("/api/v1/packages/"))
        .count();
    assert_eq!(uploads, 3, "zlib, demo's 429 attempt, demo's paced retry");

    // Alone, the same 429 fails fast: no pacing, no second attempt.
    let registry = TrustpubRegistryServer::start(&[429]);
    let oidc = OidcServer::start();
    cabin_under_actions(&oidc)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(demo.join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .failure()
        .stderr(predicate::str::contains("rate limited"));
    let uploads = registry
        .captured()
        .iter()
        .filter(|c| c.path.starts_with("/api/v1/packages/"))
        .count();
    assert_eq!(uploads, 1, "a single-package 429 must fail fast");
}

/// An upload failure mid-batch cannot swallow what already landed:
/// the first package's report is emitted before the second package's
/// refusal aborts the run, and the error names the failing member.
#[test]
fn a_mid_batch_upload_failure_keeps_the_earlier_reports() {
    let registry = TrustpubRegistryServer::start(&[201, 403]);
    let oidc = OidcServer::start();
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    assert_fs::fixture::ChildPath::new(zlib.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "acme/zlib"
version = "1.3.1"
c-standard = "c11"

[target.z]
type = "library"
sources = ["src/z.c"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(zlib.join("src/z.c"))
        .write_str("int z(void) { return 0; }\n")
        .unwrap();
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);

    cabin_under_actions(&oidc)
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(zlib.join("cabin.toml"))
        .arg("--manifest-path")
        .arg(demo.join("cabin.toml"))
        .args(["--index-url", &registry.url])
        .assert()
        .failure()
        .stdout(predicate::str::contains("Published acme/zlib 1.3.1"))
        .stderr(predicate::str::contains("publishing acme/demo 0.1.0"));
}

/// Like [`write_scoped_publishable_package`], under another name, so
/// a batch can carry two distinct packages.
fn write_scoped_zlib_package(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "acme/zlib"
version = "1.3.1"
c-standard = "c11"

[target.z]
type = "library"
sources = ["src/z.c"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/z.c"))
        .write_str("int z(void) { return 0; }\n")
        .unwrap();
}

/// A flagless batch under one shared user-level config publishes both
/// members to the configured registry - and a user-chosen origin
/// keeps its `CABIN_REGISTRY_TOKEN` eligibility for the whole batch.
#[test]
fn a_batch_publishes_to_the_shared_user_config_registry() {
    let server = RemoteRegistryServer::start(true, false, &[201, 201]);
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    write_scoped_zlib_package(&zlib);
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);
    let home = dir.path().join("config-home");
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{}\"\n", server.url))
        .unwrap();

    cabin()
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(zlib.join("cabin.toml"))
        .arg("--manifest-path")
        .arg(demo.join("cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();

    let puts = server.puts.lock().unwrap();
    let sequence: Vec<&str> = puts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(
        sequence,
        [
            "/api/v1/packages/acme/zlib/1.3.1",
            "/api/v1/packages/acme/demo/0.1.0"
        ],
        "both members upload to the one configured registry, in argv order"
    );
    for upload in puts.iter() {
        assert_eq!(
            upload.authorization.as_deref(),
            Some(&*format!("Bearer {TEST_TOKEN}")),
            "a user-chosen origin keeps the env token for every member"
        );
    }
}

/// Two different config files resolving to the same URL agree - but
/// consolidating them must not make the origin MORE trusted than it
/// would be for an individual member: one member picking the origin
/// through its project config drops `CABIN_REGISTRY_TOKEN`
/// eligibility for the whole batch, exactly as it would publishing
/// alone.
#[test]
fn an_agreeing_batch_is_only_as_credential_eligible_as_its_least_member() {
    let server = RemoteRegistryServer::start(true, false, &[201, 201]);
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    write_scoped_zlib_package(&zlib);
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);
    // The same origin, chosen two ways: the user's own config file for
    // the batch, and demo's in-tree project config for demo.
    let home = dir.path().join("config-home");
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{}\"\n", server.url))
        .unwrap();
    assert_fs::fixture::ChildPath::new(demo.join(".cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{}\"\n", server.url))
        .unwrap();

    cabin()
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(zlib.join("cabin.toml"))
        .arg("--manifest-path")
        .arg(demo.join("cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();

    let puts = server.puts.lock().unwrap();
    assert_eq!(puts.len(), 2, "both members published");
    for upload in puts.iter() {
        assert_eq!(
            upload.authorization, None,
            "a project-chosen member must strip the env token from the whole batch"
        );
    }
}

/// Members whose effective configs name different registries refuse
/// before staging, credentials, or any connection: both URLs are dead
/// loopback ports, so any network attempt would surface as a
/// connection error instead of the agreement refusal.
#[test]
fn a_batch_disagreeing_on_the_registry_fails_before_any_network() {
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    write_scoped_zlib_package(&zlib);
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);
    let (first, second) = dead_loopback_url_pair();
    assert_fs::fixture::ChildPath::new(zlib.join(".cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{first}\"\n"))
        .unwrap();
    assert_fs::fixture::ChildPath::new(demo.join(".cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{second}\"\n"))
        .unwrap();

    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(zlib.join("cabin.toml"))
        .arg("--manifest-path")
        .arg(demo.join("cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "the batch does not agree on one registry")
            && flat_contains(&stderr, &first)
            && flat_contains(&stderr, &second),
        "expected the agreement refusal naming both registries in: {stderr}"
    );
}

/// Without `-Z remote-registry` a disagreeing batch answers the
/// standard experimental-feature error, not the agreement refusal:
/// config-supplied HTTP indexes without the feature always fail with
/// that diagnostic (`docs/remote-registry.md`, "Publishing from the
/// client").
#[test]
fn a_flagless_disagreeing_batch_answers_the_feature_error() {
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    write_scoped_zlib_package(&zlib);
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);
    let (first, second) = dead_loopback_url_pair();
    assert_fs::fixture::ChildPath::new(zlib.join(".cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{first}\"\n"))
        .unwrap();
    assert_fs::fixture::ChildPath::new(demo.join(".cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{second}\"\n"))
        .unwrap();

    let assertion = cabin()
        .args(["publish", "--manifest-path"])
        .arg(zlib.join("cabin.toml"))
        .arg("--manifest-path")
        .arg(demo.join("cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            "`cabin publish --index-url` requires the experimental remote-registry client"
        ) && !flat_contains(&stderr, "does not agree"),
        "expected the gated-command error without the agreement refusal in: {stderr}"
    );
}

/// A `[source-replacement]` hop is part of a member's effective
/// config: a hop that reroutes only one member splits the batch and
/// is refused, again without touching the network.
#[test]
fn a_source_replacement_hop_splitting_the_batch_is_refused() {
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    write_scoped_zlib_package(&zlib);
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);
    let (declared, replaced) = dead_loopback_url_pair();
    let home = dir.path().join("config-home");
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{declared}\"\n"))
        .unwrap();
    assert_fs::fixture::ChildPath::new(demo.join(".cabin/config.toml"))
        .write_str(&format!(
            "[source-replacement]\n\"{declared}\" = {{ index-url = \"{replaced}\" }}\n"
        ))
        .unwrap();

    let assertion = cabin()
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(zlib.join("cabin.toml"))
        .arg("--manifest-path")
        .arg(demo.join("cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "the batch does not agree on one registry")
            && flat_contains(&stderr, &declared)
            && flat_contains(&stderr, &replaced),
        "expected the post-replacement refusal naming both registries in: {stderr}"
    );
}

/// A batch mixing a remote member with a local-path or
/// registry-source-less member refuses too - never a partial remote
/// publish, and never the single-member DryRunRequired error that
/// would misdescribe the remote member.
#[test]
fn a_remote_and_local_mix_in_a_batch_is_refused() {
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    write_scoped_zlib_package(&zlib);
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);
    assert_fs::fixture::ChildPath::new(zlib.join(".cabin/config.toml"))
        .write_str(&format!(
            "[registry]\nindex-url = \"{}\"\n",
            dead_loopback_url()
        ))
        .unwrap();

    // demo with no registry source at all, then with a local path:
    // both are the same "no remote index" answer.
    for local_config in [None, Some("[registry]\nindex-path = \"registry\"\n")] {
        if let Some(config) = local_config {
            assert_fs::fixture::ChildPath::new(demo.join(".cabin/config.toml"))
                .write_str(config)
                .unwrap();
        }
        let assertion = cabin()
            .args(["-Z", "remote-registry", "publish", "--manifest-path"])
            .arg(zlib.join("cabin.toml"))
            .arg("--manifest-path")
            .arg(demo.join("cabin.toml"))
            .env_remove("CABIN_NO_CONFIG")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            flat_contains(&stderr, "resolves no remote index"),
            "expected the remote/local mix refusal in: {stderr}"
        );
    }
}

/// `--index-url` stays an explicit whole-batch override: conflicting
/// project configs are never consulted, and every member publishes to
/// the named registry.
#[test]
fn an_explicit_index_url_overrides_every_member_config() {
    let server = RemoteRegistryServer::start(true, false, &[201, 201]);
    let dir = TempDir::new().unwrap();
    let zlib = dir.path().join("zlib");
    write_scoped_zlib_package(&zlib);
    let demo = dir.path().join("demo");
    write_scoped_publishable_package(&demo);
    let (zlib_url, demo_url) = dead_loopback_url_pair();
    assert_fs::fixture::ChildPath::new(zlib.join(".cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{zlib_url}\"\n"))
        .unwrap();
    assert_fs::fixture::ChildPath::new(demo.join(".cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{demo_url}\"\n"))
        .unwrap();

    cabin()
        .args(["-Z", "remote-registry", "publish", "--manifest-path"])
        .arg(zlib.join("cabin.toml"))
        .arg("--manifest-path")
        .arg(demo.join("cabin.toml"))
        .args(["--index-url", &server.url])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();

    let puts = server.puts.lock().unwrap();
    assert_eq!(puts.len(), 2, "both members land on the flag's registry");
}
