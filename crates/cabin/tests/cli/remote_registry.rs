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

/// End-to-end gating through the CLI: a registry `config.json` that
/// carries the remote-registry fields fails to load without the
/// flag - naming the field and instructing `-Z remote-registry` -
/// and resolves normally with it.
#[test]
fn remote_registry_config_fields_gate_on_the_flag() {
    let dir = TempDir::new().unwrap();
    write_app_manifest(dir.path());
    let registry = dir.path().join("registry");
    write_registry(
        &registry,
        r#",
    "auth-required": true,
    "api": "https://registry.cabinpkg.com""#,
    );

    let denied = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-path")
        .arg(&registry)
        .assert()
        .failure();
    // miette wraps long messages at a renderer-chosen width, so the
    // assertion must be wrap-tolerant.
    let stderr = String::from_utf8_lossy(&denied.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            "`auth-required` requires the experimental remote-registry client; run with \
             `-Z remote-registry` to enable it"
        ),
        "expected the gated-field error in: {stderr}"
    );

    let allowed = cabin()
        .args(["-Z", "remote-registry", "resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-path")
        .arg(&registry)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&allowed.get_output().stdout).to_string();
    assert!(
        stdout.contains("fmt"),
        "expected fmt in the resolution output: {stdout}"
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
/// <token>` - the shape of an `auth-required` registry, where even
/// `config.json` is behind auth.
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
                if !authorized {
                    let _ = req.respond(
                        tiny_http::Response::from_string(
                            r#"{"errors":[{"detail":"authentication required"}]}"#,
                        )
                        .with_status_code(401),
                    );
                    continue;
                }
                let path = req.url().trim_start_matches('/').to_owned();
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

/// Both commands are gated: without `-Z remote-registry` they fail
/// with the standard experimental-feature wording.
#[test]
fn login_and_logout_require_the_feature() {
    for sub in ["login", "logout"] {
        let assertion = cabin()
            .args([sub, "--index-url", "https://registry.example.com"])
            .write_stdin(format!("{TEST_TOKEN}\n"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            flat_contains(
                &stderr,
                &format!(
                    "`cabin {sub}` requires the experimental remote-registry client; run with \
                     `-Z remote-registry` to enable it"
                )
            ),
            "expected the gated-command error for {sub} in: {stderr}"
        );
    }
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

/// `cabin login` reads the token from (piped) stdin and stores it
/// keyed by the normalized origin - path and trailing slash
/// stripped.  With the login-URL probe failing (nothing listens on
/// the port), the hint degrades to the generic wording and login
/// still succeeds: the probe never blocks it.  The token itself
/// never appears on stdout or stderr.
#[test]
fn login_stores_the_token_keyed_by_normalized_origin() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let base = dead_loopback_url();
    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "login",
            "--index-url",
            &format!("{base}/some/path/"),
        ])
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success();
    let output = assertion.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stdout.contains("create a token in the registry's web interface"),
        "expected the offline fallback hint in: {stdout}"
    );
    assert!(
        stdout.contains(&format!("token for `{base}` saved")),
        "expected the origin-only confirmation in: {stdout}"
    );
    assert!(
        !stdout.contains(TEST_TOKEN) && !stderr.contains(TEST_TOKEN),
        "the token must never be echoed; stdout: {stdout}; stderr: {stderr}"
    );

    let credentials_path = home.join("credentials.toml");
    let body = fs::read_to_string(&credentials_path).unwrap();
    assert_eq!(
        body,
        format!("[registries.\"{base}\"]\ntoken = \"{TEST_TOKEN}\"\n")
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

/// Against a live `auth-required` registry, `cabin login` probes
/// `config.json` unauthenticated, parses the `WWW-Authenticate`
/// challenge, and prints the server-declared login URL; a 401
/// without the challenge degrades to the generic wording.  Either
/// way the pasted token is stored.
#[test]
fn login_discovers_the_login_url_from_the_challenge() {
    let dir = TempDir::new().unwrap();

    // A 401 carrying the challenge: the login URL is printed verbatim.
    let server = ChallengeRegistryServer::serve(Some(
        r#"Cabin login_url="https://cabinpkg.com/settings/tokens""#,
    ));
    let home = dir.path().join("home-a");
    cabin()
        .args(["-Z", "remote-registry", "login", "--index-url"])
        .arg(server.url())
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "visit https://cabinpkg.com/settings/tokens to create a token",
        ));
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert!(body.contains(TEST_TOKEN), "token must be stored: {body}");
    drop(server);

    // A 401 without the challenge: the generic wording, still stored.
    let server = ChallengeRegistryServer::serve(None);
    let home = dir.path().join("home-b");
    cabin()
        .args(["-Z", "remote-registry", "login", "--index-url"])
        .arg(server.url())
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "create a token in the registry's web interface",
        ));
    assert!(home.join("credentials.toml").exists());
}

/// Registry answering every request 401, optionally with the
/// `WWW-Authenticate` challenge - the shape `cabin login`'s probe
/// sees on an `auth-required` registry.
struct ChallengeRegistryServer {
    server: std::sync::Arc<tiny_http::Server>,
    thread: Option<std::thread::JoinHandle<()>>,
    url: String,
}

impl ChallengeRegistryServer {
    fn serve(challenge: Option<&'static str>) -> Self {
        let server = std::sync::Arc::new(
            tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
        );
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let url = format!("http://{addr}");
        let server_for_thread = std::sync::Arc::clone(&server);
        let thread = std::thread::spawn(move || {
            while let Ok(req) = server_for_thread.recv() {
                let mut response = tiny_http::Response::from_string(
                    r#"{"errors":[{"detail":"authentication required"}]}"#,
                )
                .with_status_code(401);
                if let Some(challenge) = challenge {
                    response.add_header(
                        tiny_http::Header::from_bytes(&b"WWW-Authenticate"[..], challenge)
                            .expect("valid test header"),
                    );
                }
                let _ = req.respond(response);
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

impl Drop for ChallengeRegistryServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// A pasted value that is not a Cabin token is rejected before
/// anything is written, and the error never echoes the value.
#[test]
fn login_rejects_invalid_tokens_without_writing() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "login",
            "--index-url",
            &dead_loopback_url(),
        ])
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin("ghp_notACabinToken12345\n")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "expected the `cabin_` prefix"),
        "expected the token-shape error in: {stderr}"
    );
    assert!(
        !stderr.contains("notACabinToken"),
        "the pasted value must not be echoed: {stderr}"
    );
    assert!(!home.join("credentials.toml").exists());
}

/// Without `--index-url` the `[registry] index-url` config default
/// applies; a config-supplied local `index-path` (or no index at
/// all) is rejected because a token has no local-path counterpart.
#[test]
fn login_resolves_the_registry_from_config_and_rejects_local_paths() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let base = dead_loopback_url();
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{base}/index/\"\n"))
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["-Z", "remote-registry", "login"])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "token for `{base}` saved"
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
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "requires an HTTP registry"),
        "expected the local-path rejection in: {stderr}"
    );

    // No index source anywhere: a clear requirement error.
    let assertion = cabin()
        .args(["-Z", "remote-registry", "login"])
        .env("CABIN_CONFIG_HOME", dir.path().join("empty-home"))
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(
            &stderr,
            "requires --index-url or a `[registry] index-url` config setting"
        ),
        "expected the missing-index error in: {stderr}"
    );
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
             [registries.\"https://registry.example.com\"]\ntoken = \"{TEST_TOKEN}\"\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&credentials_path, fs::Permissions::from_mode(0o600)).unwrap();
    }

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
            "token for `https://registry.example.com` removed",
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
            "no token was stored for `https://registry.example.com`",
        ));
}

/// End-to-end authenticated read path: an `auth-required` registry
/// resolves only when a credential is available - via
/// `CABIN_REGISTRY_TOKEN` or a prior `cabin login` - and the
/// tokenless failure advises `cabin login` for the origin.
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
        .args(["-Z", "remote-registry", "resolve", "--manifest-path"])
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

    // The env override authenticates every request this invocation
    // makes.
    let assertion = cabin()
        .args(["-Z", "remote-registry", "resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .env("CABIN_REGISTRY_TOKEN", TEST_TOKEN)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(stdout.contains("fmt"), "{stdout}");

    // A stored credential (via `cabin login`) works the same way.
    let home = dir.path().join("config-home");
    cabin()
        .args([
            "-Z",
            "remote-registry",
            "login",
            "--index-url",
            server.url(),
        ])
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success();
    cabin()
        .args(["-Z", "remote-registry", "resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .env("CABIN_CONFIG_HOME", &home)
        .assert()
        .success()
        .stdout(predicate::str::contains("fmt"));

    // A wrong stored token surfaces the revoked/expired wording.
    let assertion = cabin()
        .args(["-Z", "remote-registry", "resolve", "--manifest-path"])
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
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        flat_contains(&stderr, "never sent over plain `http`"),
        "expected the cleartext rejection in: {stderr}"
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
    let base = dead_loopback_url();
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str("this is not toml [")
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["-Z", "remote-registry", "login", "--index-url", &base])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "token for `{base}` saved"
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
                        409 => r#"{"errors":[{"detail":"version exists with different bytes"}]}"#,
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
        dir.path().join("staging/acme-demo-0.1.0.tar.gz").is_file(),
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
    let packaged_archive = fs::read(dist.join("acme-demo-0.1.0.tar.gz")).unwrap();
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
    fs::create_dir_all(&home).unwrap();
    let credentials_path = home.join("credentials.toml");
    fs::write(
        &credentials_path,
        format!(
            "[registries.\"{}\"]\ntoken = \"{TEST_TOKEN}\"\n",
            index_server.url
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&credentials_path, fs::Permissions::from_mode(0o600)).unwrap();
    }

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
    assert!(
        flat_contains(&stderr, "published versions are immutable"),
        "expected the immutability explanation in: {stderr}"
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
    // The upstream is never contacted (the replacement wins before the
    // login-URL probe); the mirror is a dead loopback port, so the
    // probe degrades to the generic wording.
    let mirror = dead_loopback_url();
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
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success()
        .stdout(predicate::str::contains(format!(
            "token for `{mirror}` saved"
        )));
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert!(body.contains(&mirror), "{body}");
    assert!(!body.contains("upstream.example.com"), "{body}");
}
