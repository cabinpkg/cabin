use super::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

/// Tiny static HTTP server backed by `tiny_http`.  Serves files
/// from a directory; missing files yield 404.
pub(crate) struct TestServer {
    server: Arc<tiny_http::Server>,
    thread: Option<JoinHandle<()>>,
    url: String,
}

impl TestServer {
    pub(crate) fn serve(root: PathBuf) -> Self {
        let server =
            Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"));
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let url = format!("http://{addr}");
        let server_for_thread = Arc::clone(&server);
        let thread = std::thread::spawn(move || {
            loop {
                let Ok(req) = server_for_thread.recv() else {
                    break;
                };
                let raw_url = req.url().to_string();
                let path = raw_url
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .trim_start_matches('/')
                    .to_owned();
                if path.contains("..") {
                    let _ = req.respond(tiny_http::Response::empty(400));
                    continue;
                }
                let file_path = root.join(&path);
                if file_path.is_file() {
                    match fs::read(&file_path) {
                        Ok(bytes) => {
                            let _ = req.respond(tiny_http::Response::from_data(bytes));
                        }
                        Err(_) => {
                            let _ = req.respond(tiny_http::Response::empty(500));
                        }
                    }
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

    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Publish the scoped package `fmtlib/fmt` into a fresh file
/// registry through the real `cabin publish --registry-dir` flow, so
/// the fixture served over HTTP has exactly the scoped layout the
/// hosted registry speaks: `packages/fmtlib/fmt.json` and
/// `artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1.zip`, linked by the
/// canonical `../../artifacts/...` source path.
fn publish_scoped_fmt_to_registry(dir: &Path) -> PathBuf {
    let pkg_root = dir.join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "fmtlib/fmt"
version = "10.2.1"
cxx-standard = "c++17"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("include/fmt.h"))
        .write_str("#pragma once\nvoid say_hello();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("src/fmt.cc"))
            .write_str("#include <iostream>\n#include \"fmt.h\"\nvoid say_hello() { std::cout << \"hello from fmt\\n\"; }\n")
            .unwrap();
    let registry = dir.join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();
    registry
}

/// Hand-assemble a *bare*-name registry (`packages/fmt.json`,
/// `artifacts/fmt/fmt-10.2.1.zip`) from ungated `cabin package`
/// staging output.  Bare names stay legal in locally-produced file
/// registries, and serving one over HTTP must keep working; `cabin
/// publish` requires scoped names, hence the by-hand assembly.
fn assemble_bare_fmt_registry(dir: &Path) -> PathBuf {
    let pkg_root = dir.join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"
cxx-standard = "c++17"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("include/fmt.h"))
        .write_str("#pragma once\nvoid say_hello();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("src/fmt.cc"))
            .write_str("#include <iostream>\n#include \"fmt.h\"\nvoid say_hello() { std::cout << \"hello from fmt\\n\"; }\n")
            .unwrap();
    let dist = dir.join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success();
    let staged: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dist.join("fmt-10.2.1.json")).unwrap()).unwrap();
    let revision = staged["checksum"]
        .as_str()
        .unwrap()
        .strip_prefix("sha256:")
        .unwrap()[..16]
        .to_owned();
    let mut revisions = serde_json::Map::new();
    revisions.insert(
        revision.clone(),
        serde_json::json!({
            "checksum": staged["checksum"],
            "published-at": "2026-01-01T00:00:00Z",
            "source": {
                "type": "archive",
                "path": "../artifacts/fmt/fmt-10.2.1.zip",
                "format": "zip"
            }
        }),
    );
    let mut version_entry = serde_json::json!({
        "dependencies": {},
        "yanked": false,
        "revision": revision,
        "revisions": revisions
    });
    if let Some(standards) = staged.get("standards") {
        version_entry["standards"] = standards.clone();
    }
    if let Some(links) = staged.get("links") {
        version_entry["links"] = links.clone();
    }
    let index = serde_json::json!({
        "schema": 1,
        "name": "fmt",
        "versions": { "10.2.1": version_entry }
    });
    let registry = dir.join("registry");
    assert_fs::fixture::ChildPath::new(registry.join("config.json"))
        .write_str(
            r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts"}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(registry.join("packages/fmt.json"))
        .write_str(&index.to_string())
        .unwrap();
    fs::create_dir_all(registry.join("artifacts/fmt")).unwrap();
    fs::copy(
        dist.join("fmt-10.2.1.zip"),
        registry.join("artifacts/fmt/fmt-10.2.1.zip"),
    )
    .unwrap();
    registry
}

#[test]
fn resolve_via_index_url_finds_published_package() {
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);

    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .args(["--format", "json"]),
    );
    let names: Vec<&str> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"fmtlib/fmt"),
        "fmtlib/fmt missing from resolve: {names:?}"
    );
}

/// A lockfile written by one resolve run is reused by the next:
/// `--locked` succeeds against the same registry and the lockfile
/// bytes do not change.
#[test]
fn resolve_reuses_the_lockfile_under_locked() {
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);

    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .success();
    let lock_path = dir.path().join("app/cabin.lock");
    let first = std::fs::read_to_string(&lock_path).unwrap();
    assert!(first.contains("fmtlib/fmt"), "{first}");

    cabin()
        .args(["resolve", "--locked", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .success();
    assert_eq!(
        first,
        std::fs::read_to_string(&lock_path).unwrap(),
        "--locked must not rewrite the lockfile"
    );
}

/// Bare names stay legal in locally-produced file registries, and
/// the sparse-HTTP client keeps reading their flat layout
/// (`packages/<name>.json`, `../artifacts/<name>/...`) when one is
/// served over HTTP.
#[test]
fn resolve_via_index_url_reads_bare_name_layouts() {
    let dir = TempDir::new().unwrap();
    let registry = assemble_bare_fmt_registry(dir.path());
    write_app_using_fmt(dir.path(), None);
    let server = TestServer::serve(registry);

    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .args(["--format", "json"]),
    );
    let names: Vec<&str> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"fmt"),
        "fmt missing from resolve: {names:?}"
    );
}

#[test]
fn fetch_via_index_url_extracts_archive_into_cache() {
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);

    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .success();
    let sources = cache.join("sources/sha256");
    assert!(sources.is_dir());
    let mut found_cabin_toml = false;
    for entry in fs::read_dir(&sources).unwrap() {
        let entry = entry.unwrap();
        if entry.path().join("cabin.toml").is_file() {
            found_cabin_toml = true;
            break;
        }
    }
    assert!(
        found_cabin_toml,
        "expected an extracted cabin.toml in cache"
    );
}

#[test]
fn build_via_index_url_builds_executable() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    let app_main = "#include \"fmt.h\"\nint main() { say_hello(); return 0; }\n";
    write_app_using_scoped_fmt(dir.path(), Some(app_main));
    let server = TestServer::serve(registry);

    let cache = dir.path().join("cache");
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    let exe = build_dir.join("dev/packages/app").join(host_exe("app"));
    assert!(exe.is_file());
    let output = std::process::Command::new(&exe).output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello from fmt"));
}

#[test]
fn index_path_and_index_url_together_fail() {
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry.clone());
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&registry)
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--index-path"))
        .stderr(predicate::str::contains("--index-url"));
}

#[test]
fn http_package_not_found_surfaces_clear_error() {
    let dir = TempDir::new().unwrap();
    let empty_registry = dir.path().join("registry");
    assert_fs::fixture::ChildPath::new(empty_registry.join("packages"))
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(empty_registry.join("artifacts"))
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(empty_registry.join("config.json"))
        .write_str(
            r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts"}"#,
        )
        .unwrap();
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(empty_registry);
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found in HTTP index"));
}

#[test]
fn http_invalid_metadata_surfaces_clear_error() {
    let dir = TempDir::new().unwrap();
    let registry = dir.path().join("registry");
    assert_fs::fixture::ChildPath::new(registry.join("packages"))
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(registry.join("artifacts"))
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(registry.join("config.json"))
        .write_str(
            r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts"}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(registry.join("packages/fmtlib/fmt.json"))
        .write_binary(b"{ not really json")
        .unwrap();
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid package metadata"));
}

/// A remote registry picks the JSON keys of the metadata it serves, and
/// the parse error quotes the rejected key back.
#[test]
fn http_metadata_cannot_smuggle_terminal_escapes_into_diagnostics() {
    let dir = TempDir::new().unwrap();
    let registry = dir.path().join("registry");
    assert_fs::fixture::ChildPath::new(registry.join("packages"))
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(registry.join("artifacts"))
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(registry.join("config.json"))
        .write_str(
            r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts"}"#,
        )
        .unwrap();
    // Valid JSON whose sole key is unknown, so `deny_unknown_fields`
    // quotes the key back; `\u001b` decodes to ESC, and `ESC [ 2 K`
    // erases the line the terminal is on.
    assert_fs::fixture::ChildPath::new(registry.join("packages/fmtlib/fmt.json"))
        .write_str(r#"{"\u001b[2Khidden":1}"#)
        .unwrap();
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);
    let assertion = cabin()
        .args(["resolve", "--color", "never", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .failure()
        .stderr(predicate::str::contains("\u{1b}").not());
    assert!(
        stderr_with_wrapping_joined(&assertion).contains(r"\u{1b}[2Khidden"),
        "expected the escaped key in stderr: {}",
        String::from_utf8_lossy(&assertion.get_output().stderr)
    );
}

#[test]
fn cross_origin_http_artifact_url_is_rejected() {
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    let pkg_index = registry.join("packages/fmtlib/fmt.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pkg_index).unwrap()).unwrap();
    let revision = value["versions"]["10.2.1"]["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    value["versions"]["10.2.1"]["revisions"][&revision]["source"]["path"] =
        serde_json::Value::String("http://127.0.0.1/artifacts/fmt.zip".into());
    assert_fs::fixture::ChildPath::new(&pkg_index)
        .write_str(&(serde_json::to_string_pretty(&value).unwrap() + "\n"))
        .unwrap();
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .failure()
        .stderr(predicate::str::contains("same origin"));
}

#[test]
fn http_artifact_checksum_mismatch_fails() {
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    // Tamper with the published `fmt.json` to advertise a wrong
    // checksum so the artifact bytes the server returns will
    // mismatch what the index claims.
    let pkg_index = registry.join("packages/fmtlib/fmt.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pkg_index).unwrap()).unwrap();
    let revision = value["versions"]["10.2.1"]["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    // Tamper only the digest's tail: the revision id stays the
    // checksum's prefix (the loader validates that pairing), so the
    // mismatch surfaces at fetch time, not at load time.
    value["versions"]["10.2.1"]["revisions"][&revision]["checksum"] =
        serde_json::Value::String(format!("sha256:{revision}{}", "0".repeat(48)));
    assert_fs::fixture::ChildPath::new(&pkg_index)
        .write_str(&(serde_json::to_string_pretty(&value).unwrap() + "\n"))
        .unwrap();
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .failure()
        .stderr(predicate::str::contains("checksum mismatch"));
}

#[test]
fn relative_artifact_path_resolves_correctly() {
    // A successful resolve confirms the HTTP loader resolves the
    // scoped `../../artifacts/<scope>/<name>/<scope>-<name>-<version>-<revision>.zip`
    // source path against the nested package metadata URL.
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .success();
}

#[test]
fn frozen_with_index_url_fails_clearly() {
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);
    // Pre-populate a lockfile so `--frozen` reaches the
    // documented HTTP-metadata-cache check rather than the
    // "missing lockfile" path.
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .assert()
        .success();
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--frozen", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-url")
        .arg(server.url())
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--index-url"))
        .stderr(predicate::str::contains("--frozen"));
}

#[test]
fn resolve_frozen_rejects_config_index_url() {
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    write_app_using_scoped_fmt(dir.path(), None);
    let server = TestServer::serve(registry);
    assert_fs::fixture::ChildPath::new(dir.path().join("app/.cabin/config.toml"))
        .write_str(&format!("[registry]\nindex-url = \"{}\"\n", server.url()))
        .unwrap();
    // `CABIN_CONFIG_HOME` points at an empty fixture home rather
    // than being unset: unset, the credential store would fall back
    // to the *platform* config home - the developer's real
    // `credentials.toml` - which `pin_test_user_config_home_to_empty`
    // (HOME / XDG only) does not cover on Windows.
    let empty_home = dir.path().join("empty-config-home");
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .env_remove("CABIN_CONFIG")
        .env("CABIN_CONFIG_HOME", &empty_home)
        .assert()
        .success();

    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["resolve", "--frozen", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .env_remove("CABIN_CONFIG")
        .env("CABIN_CONFIG_HOME", &empty_home)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--index-url"))
        .stderr(predicate::str::contains("--frozen"));
}

/// A fork dependency of a transitively-activated patch can name a
/// package the pre-activation sparse crawl never fetched (nothing in
/// the upstream metadata closure references it).  The activation
/// loop extends the index instead of failing the re-solve with an
/// unknown package.
#[test]
fn index_url_extends_crawl_for_transitively_activated_patch_deps() {
    let dir = TempDir::new().unwrap();
    let registry = dir.path().join("registry");
    assert_fs::fixture::ChildPath::new(registry.join("config.json"))
        .write_str(
            r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts"}"#,
        )
        .unwrap();
    for (name, deps) in [
        ("aaa", r#"{ "bbb": ">=1" }"#),
        ("bbb", r#"{}"#),
        // Unreachable from any upstream metadata: only the bbb
        // fork's own [dependencies] names it.
        ("ccc", r#"{}"#),
    ] {
        assert_fs::fixture::ChildPath::new(registry.join(format!("packages/{name}.json")))
            .write_str(&format!(
                r#"{{
  "schema": 1,
  "name": "{name}",
  "versions": {{
    "1.0.0": {{
      "dependencies": {deps},
      "yanked": false
    }}
  }}
}}"#
            ))
            .unwrap();
    }
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "bbb"
version = "1.0.0"

[dependencies]
ccc = ">=1"

[target.bbb]
type = "library"
sources = ["src/bbb.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/src/bbb.c"))
        .write_str("void b(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
aaa = ">=1"

[patch]
bbb = { path = "../bbb-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let server = TestServer::serve(registry);

    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .args(["--format", "json"]),
    );
    let names: Vec<&str> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"ccc"),
        "the fork's crawl-invisible dependency must resolve: {names:?}"
    );
}
