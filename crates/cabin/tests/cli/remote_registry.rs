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
    "api": "https://dev-registry.cabinpkg.com""#,
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

/// `cabin login` prints the token-creation page for the origin,
/// reads the token from (piped) stdin, and stores it keyed by the
/// normalized origin - path, trailing slash, and default port
/// stripped.  The token itself never appears on stdout or stderr.
#[test]
fn login_stores_the_token_keyed_by_normalized_origin() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    let assertion = cabin()
        .args([
            "-Z",
            "remote-registry",
            "login",
            "--index-url",
            "https://registry.example.com:443/some/path/",
        ])
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success();
    let output = assertion.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stdout.contains("visit https://registry.example.com/me to create a token"),
        "expected the token-creation hint in: {stdout}"
    );
    assert!(
        stdout.contains("token for `https://registry.example.com` saved"),
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
        format!("[registries.\"https://registry.example.com\"]\ntoken = \"{TEST_TOKEN}\"\n")
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
            "https://registry.example.com",
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
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str("[registry]\nindex-url = \"https://config-registry.example.com/index/\"\n")
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["-Z", "remote-registry", "login"])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "token for `https://config-registry.example.com` saved",
        ));

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
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str("this is not toml [")
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args([
        "-Z",
        "remote-registry",
        "login",
        "--index-url",
        "https://registry.example.com",
    ])
    .env_remove("CABIN_NO_CONFIG")
    .env("CABIN_CONFIG_HOME", &home)
    .write_stdin(format!("{TEST_TOKEN}\n"))
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "token for `https://registry.example.com` saved",
    ));
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
        let config = if include_api {
            format!(
                r#"{{
    "schema": 1,
    "kind": "file-registry",
    "packages": "packages",
    "artifacts": "artifacts"{auth_field},
    "api": "{url}"
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
                    let body = match status {
                        200 => r#"{"ok":true,"no_op":true}"#,
                        201 => r#"{"ok":true}"#,
                        409 => r#"{"errors":[{"detail":"version exists with different bytes"}]}"#,
                        _ => r#"{"errors":[{"detail":"unexpected"}]}"#,
                    };
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
    write_publishable_package(dir.path());
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
        dir.path().join("staging/demo-0.1.0.tar.gz").is_file(),
        "the dry-run must stage locally into --output-dir"
    );
    match listener.accept() {
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("--dry-run must not open a connection to the registry"),
        Err(err) => panic!("unexpected listener state: {err}"),
    }
}

/// The full upload path: the PUT hits the registry's `api` origin
/// with the bearer token, and the framed metadata + archive bytes
/// are byte-identical to what `cabin package` produces for the same
/// source tree.
#[test]
fn publish_uploads_bytes_identical_to_cabin_package() {
    let dir = TempDir::new().unwrap();
    write_publishable_package(dir.path());

    // What `cabin package` produces for this tree.
    let dist = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success();
    let packaged_archive = fs::read(dist.join("demo-0.1.0.tar.gz")).unwrap();
    let packaged_metadata = fs::read(dist.join("demo-0.1.0.json")).unwrap();

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
        stdout.contains(&format!("Published demo 0.1.0 to {}", server.url)),
        "expected the created report in: {stdout}"
    );
    assert!(
        stdout.contains("checksum: sha256:"),
        "expected the checksum in: {stdout}"
    );

    let puts = server.puts.lock().unwrap();
    assert_eq!(puts.len(), 1, "exactly one publish request");
    let put = &puts[0];
    assert_eq!(put.method, "PUT");
    assert_eq!(put.path, "/api/v1/packages/demo/0.1.0");
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

/// Re-publishing identical bytes is the idempotent `200` no-op, and
/// a `409` explains that published versions are immutable.
#[test]
fn publish_reports_no_op_and_conflict_outcomes() {
    let dir = TempDir::new().unwrap();
    write_publishable_package(dir.path());

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
        stdout.contains("demo 0.1.0 is already published to")
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

/// A registry whose `config.json` lacks the `api` field cannot be
/// published to; the error names the missing field.
#[test]
fn publish_requires_the_api_url_in_the_registry_config() {
    let dir = TempDir::new().unwrap();
    write_publishable_package(dir.path());
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

/// A config-supplied registry source is subject to
/// `[source-replacement]` on the fetch path, so `cabin login` keys
/// the token on the replaced origin - the one a later fetch will
/// actually contact.
#[test]
fn login_applies_source_replacement_to_the_config_registry() {
    let dir = TempDir::new().unwrap();
    let home = dir.path().join("config-home");
    assert_fs::fixture::ChildPath::new(home.join("config.toml"))
        .write_str(
            "[registry]\nindex-url = \"https://upstream.example.com/index\"\n\n\
             [source-replacement]\n\"https://upstream.example.com/index\" = \
             { index-url = \"https://mirror.example.com/index\" }\n",
        )
        .unwrap();
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["-Z", "remote-registry", "login"])
        .env_remove("CABIN_NO_CONFIG")
        .env("CABIN_CONFIG_HOME", &home)
        .write_stdin(format!("{TEST_TOKEN}\n"))
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "token for `https://mirror.example.com` saved",
        ));
    let body = fs::read_to_string(home.join("credentials.toml")).unwrap();
    assert!(body.contains("https://mirror.example.com"), "{body}");
    assert!(!body.contains("upstream.example.com"), "{body}");
}
