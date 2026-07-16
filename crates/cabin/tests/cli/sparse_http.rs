use super::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

/// Tiny static HTTP server backed by `tiny_http`.  Serves files
/// from a directory; missing files yield 404.
struct TestServer {
    server: Arc<tiny_http::Server>,
    thread: Option<JoinHandle<()>>,
    url: String,
}

impl TestServer {
    fn serve(root: PathBuf) -> Self {
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

    fn url(&self) -> &str {
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
/// `artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1.tar.gz`, linked by the
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
/// `artifacts/fmt/fmt-10.2.1.tar.gz`) from ungated `cabin package`
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
    let mut version_entry = serde_json::json!({
        "dependencies": {},
        "yanked": false,
        "checksum": staged["checksum"],
        "source": {
            "type": "archive",
            "path": "../artifacts/fmt/fmt-10.2.1.tar.gz",
            "format": "tar.gz"
        }
    });
    if let Some(standards) = staged.get("standards") {
        version_entry["standards"] = standards.clone();
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
        dist.join("fmt-10.2.1.tar.gz"),
        registry.join("artifacts/fmt/fmt-10.2.1.tar.gz"),
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

#[test]
fn cross_origin_http_artifact_url_is_rejected() {
    let dir = TempDir::new().unwrap();
    let registry = publish_scoped_fmt_to_registry(dir.path());
    let pkg_index = registry.join("packages/fmtlib/fmt.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&pkg_index).unwrap()).unwrap();
    value["versions"]["10.2.1"]["source"]["path"] =
        serde_json::Value::String("http://127.0.0.1/artifacts/fmt.tar.gz".into());
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
    value["versions"]["10.2.1"]["checksum"] =
        serde_json::Value::String(format!("sha256:{}", "0".repeat(64)));
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
    // scoped `../../artifacts/<scope>/<name>/<scope>-<name>-<version>.tar.gz`
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
    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .env_remove("CABIN_CONFIG")
        .env_remove("CABIN_CONFIG_HOME")
        .assert()
        .success();

    let mut cmd = cabin();
    super::pin_test_user_config_home_to_empty(&mut cmd);
    cmd.args(["resolve", "--frozen", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .env_remove("CABIN_NO_CONFIG")
        .env_remove("CABIN_CONFIG")
        .env_remove("CABIN_CONFIG_HOME")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--index-url"))
        .stderr(predicate::str::contains("--frozen"));
}
