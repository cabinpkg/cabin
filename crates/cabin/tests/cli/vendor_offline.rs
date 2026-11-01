//! End-to-end coverage for `cabin vendor` and `--offline`
//! mode.  Tests that vendor a dependency stage a real `.zip`
//! archive plus the file-registry index that publishes its
//! checksum.

use super::*;
use std::path::PathBuf;

/// Stage a one-package file-registry index at `<root>/index`
/// containing a single `fmt 10.2.1` entry.  Returns the
/// directory the index lives in.
fn stage_fmt_index(root: &Path) -> PathBuf {
    let index = root.join("index");
    assert_fs::fixture::ChildPath::new(index.join("config.json"))

            .write_str("{\"schema\":1,\"kind\":\"file-registry\",\"packages\":\"packages\",\"artifacts\":\"artifacts\"}\n")

            .unwrap();
    let archive = index.join("artifacts/fmt/fmt-10.2.1.zip");
    let manifest = "[package]\nname = \"fmt\"\nversion = \"10.2.1\"\ncxx-standard = \"c++17\"\n\n[target.fmt]\ntype = \"library\"\nsources = [\"src/fmt.cc\"]\ninclude-dirs = [\"include\"]\n";
    let header = "#pragma once\nint fmt_value();\n";
    let body = "#include \"fmt.h\"\nint fmt_value() { return 42; }\n";
    let checksum = make_archive(
        &archive,
        &[
            ("cabin.toml", manifest),
            ("include/fmt.h", header),
            ("src/fmt.cc", body),
        ],
    );
    let revision = &checksum[..16];
    let entry = format!(
        "{{\n  \"schema\": 1,\n  \"name\": \"fmt\",\n  \"versions\": {{\n    \"10.2.1\": {{\n      \"dependencies\": {{}},\n      \"yanked\": false,\n      \"revision\": \"{revision}\",\n      \"revisions\": {{\n        \"{revision}\": {{\n          \"checksum\": \"sha256:{checksum}\",\n          \"published-at\": \"2026-01-01T00:00:00Z\",\n          \"source\": {{\"type\": \"archive\", \"path\": \"../artifacts/fmt/fmt-10.2.1.zip\", \"format\": \"zip\"}}\n        }}\n      }}\n    }}\n  }}\n}}\n",
    );
    assert_fs::fixture::ChildPath::new(index.join("packages/fmt.json"))
        .write_str(&entry)
        .unwrap();
    index
}

/// Stage a small consuming package that depends on `fmt
/// 10.2.1`.  Includes a working `main.cc` so a follow-up
/// `cabin build` can succeed.
fn stage_consumer_project(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
fmt = "10.2.1"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["fmt"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
        .write_str("extern int fmt_value();\nint main() { return fmt_value() == 42 ? 0 : 1; }\n")
        .unwrap();
}

#[test]
fn vendor_writes_deterministic_file_registry() {
    let dir = TempDir::new().unwrap();
    let index = stage_fmt_index(dir.path());
    // A second, superseded revision the vendored index must drop.
    let source_index = index.join("packages/fmt.json");
    let mut source: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&source_index).unwrap()).unwrap();
    let stale = "f".repeat(16);
    source["versions"]["10.2.1"]["revisions"][&stale] = serde_json::json!({
        "checksum": format!("sha256:{}", "f".repeat(64)),
        "published-at": "2025-01-01T00:00:00Z",
        "source": { "type": "archive", "path": "../artifacts/fmt/stale.zip", "format": "zip" },
    });
    fs::write(&source_index, source.to_string()).unwrap();
    stage_consumer_project(&dir.path().join("proj"));
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(dir.path().join("proj/vendor"))
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let vendor = dir.path().join("proj/vendor");
    assert!(vendor.join("config.json").is_file());
    assert!(vendor.join("packages/fmt.json").is_file());
    assert!(vendor.join("cabin-vendor.json").is_file());

    // The vendored per-package index keeps exactly the fetched
    // revision, pointing at the *vendor's* relative archive path
    // (revision-qualified), not at the source index's.  This is what
    // makes the directory portable.
    let body = fs::read_to_string(vendor.join("packages/fmt.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    let revision = parsed["versions"]["10.2.1"]["revision"].as_str().unwrap();
    let revisions = parsed["versions"]["10.2.1"]["revisions"]
        .as_object()
        .unwrap();
    assert_eq!(
        revisions.keys().collect::<Vec<_>>(),
        [revision],
        "only the fetched revision is vendored: {body}"
    );
    let path = parsed["versions"]["10.2.1"]["revisions"][revision]["source"]["path"]
        .as_str()
        .unwrap();
    assert_eq!(path, format!("../artifacts/fmt/fmt-10.2.1-{revision}.zip"));
    assert!(
        vendor
            .join(format!("artifacts/fmt/fmt-10.2.1-{revision}.zip"))
            .is_file()
    );

    // Re-running with the same inputs must be byte-identical.
    let summary = fs::read(vendor.join("cabin-vendor.json")).unwrap();
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(&vendor)
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let summary_again = fs::read(vendor.join("cabin-vendor.json")).unwrap();
    assert_eq!(summary, summary_again);
}

/// A local mirror of a *hosted* registry carries `auth-required` /
/// `api` in its `config.json`.  Vendoring from such a mirror works
/// without any experimental flag, and the vendored output stays
/// consumable offline.
#[test]
fn vendor_from_a_hosted_registry_mirror_needs_no_flag() {
    let dir = TempDir::new().unwrap();
    let index = stage_fmt_index(dir.path());
    assert_fs::fixture::ChildPath::new(index.join("config.json"))
        .write_str(
            "{\"schema\":1,\"kind\":\"file-registry\",\"packages\":\"packages\",\
             \"artifacts\":\"artifacts\",\"auth-required\":true,\
             \"api\":\"https://cabinpkg.com\"}\n",
        )
        .unwrap();
    stage_consumer_project(&dir.path().join("proj"));
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(dir.path().join("proj/vendor"))
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    // The vendored registry resolves offline like any other.
    cabin()
        .args(["resolve", "--offline", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("proj/vendor"))
        .assert()
        .success()
        .stdout(predicate::str::contains("fmt 10.2.1"));
}

#[test]
fn vendor_then_offline_build_links_against_the_vendored_dependency() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let index = stage_fmt_index(dir.path());
    stage_consumer_project(&dir.path().join("proj"));

    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(dir.path().join("proj/vendor"))
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();

    // With the source index deleted, the offline build can read only
    // the vendored directory and a fresh cache.
    fs::remove_dir_all(&index).unwrap();
    cabin()
        .args(["build", "--offline", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("proj/build"))
        .arg("--index-path")
        .arg(dir.path().join("proj/vendor"))
        .arg("--cache-dir")
        .arg(dir.path().join("vendor-cache"))
        .assert()
        .success();
    let exe = dir
        .path()
        .join("proj/build/dev/packages/app")
        .join(host_exe("app"));
    assert!(exe.is_file(), "offline build must link the executable");
}

#[test]
fn offline_rejects_index_url() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "lone"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["build", "--offline", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg("https://example.com/index")
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("--offline forbids network access"),
        "expected offline rejection, got: {stderr}"
    );
    assert!(
        stderr.contains("https://example.com/index"),
        "diagnostic should name the rejected URL, got: {stderr}"
    );
}

#[test]
fn vendor_with_no_versioned_deps_writes_skeleton_only() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "lone"
version = "0.1.0"
cxx-standard = "c++17"

[target.lone]
type = "library"
sources = ["src/lib.cc"]
"#,
        )
        .unwrap();
    dir.child("src/lib.cc")
        .write_str("int lone_value() { return 0; }\n")
        .unwrap();
    let vendor = dir.path().join("vendor");
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--vendor-dir")
        .arg(&vendor)
        .assert()
        .success();
    // Empty plan still writes the file-registry skeleton so a
    // follow-up `cabin build --offline --index-path ./vendor`
    // can be a no-op rather than an error.
    assert!(vendor.join("config.json").is_file());
    assert!(vendor.join("cabin-vendor.json").is_file());
    assert!(vendor.join("packages").is_dir());
    assert!(vendor.join("artifacts").is_dir());
}

#[test]
fn vendor_locked_succeeds_when_lockfile_is_current() {
    let dir = TempDir::new().unwrap();
    let index = stage_fmt_index(dir.path());
    stage_consumer_project(&dir.path().join("proj"));
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(dir.path().join("proj/vendor"))
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let lock_before = fs::read_to_string(dir.path().join("proj/cabin.lock")).unwrap();
    cabin()
        .args(["vendor", "--locked", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(dir.path().join("proj/vendor"))
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let lock_after = fs::read_to_string(dir.path().join("proj/cabin.lock")).unwrap();
    assert_eq!(lock_before, lock_after);
}

#[test]
fn vendor_with_nothing_to_vendor_still_validates_offline_env() {
    // The empty-plan fast path writes the registry skeleton and
    // returns before any index access; a malformed CABIN_NET_OFFLINE
    // must still fail the run rather than be read only on the path
    // that happens to need the network.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "lib_only"
version = "0.1.0"
cxx-standard = "c++17"

[target.lib_only]
type = "library"
sources = ["src/lib.cc"]
"#,
        )
        .unwrap();
    dir.child("src/lib.cc")
        .write_str("int x() { return 1; }\n")
        .unwrap();
    let assertion = cabin()
        .env("CABIN_NET_OFFLINE", "nope")
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("CABIN_NET_OFFLINE") && stderr.contains("nope"),
        "invalid offline env diagnostic should name the variable and value: {stderr}"
    );
}

/// Vendor's local-index rule applies to the `[source-replacement]`
/// chain's terminal source: a configured `index-url` replaced by a
/// local index vendors, one left alone or replaced by another URL is
/// refused, and `--offline` still refuses the configured URL before
/// replacement - all without a request to either URL.
#[test]
fn vendor_requires_a_local_terminal_after_source_replacement() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let server = std::sync::Arc::new(
        tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
    );
    let url = format!(
        "http://{}",
        server.server_addr().to_ip().expect("loopback addr")
    );
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let server_for_thread = std::sync::Arc::clone(&server);
    let hits_for_thread = std::sync::Arc::clone(&hits);
    let thread = std::thread::spawn(move || {
        while let Ok(req) = server_for_thread.recv() {
            hits_for_thread.fetch_add(1, Ordering::SeqCst);
            let _ = req.respond(tiny_http::Response::empty(404));
        }
    });

    let dir = TempDir::new().unwrap();
    stage_fmt_index(dir.path());
    stage_consumer_project(&dir.path().join("proj"));
    // `../index` resolves against the config file's directory.
    let home = dir.path().join("home");
    let vendor = |config: &str, offline: bool| {
        assert_fs::fixture::ChildPath::new(home.join("config.toml"))
            .write_str(&format!(
                "[registry]\nindex-url = \"{url}/index\"\n\n[source-replacement]\n{config}\n"
            ))
            .unwrap();
        let mut cmd = cabin_with_config();
        cmd.arg("vendor");
        if offline {
            cmd.arg("--offline");
        }
        cmd.arg("--manifest-path")
            .arg(dir.path().join("proj/cabin.toml"))
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .env("CABIN_CONFIG_HOME", &home)
            .assert()
    };
    let to_local = format!("\"{url}/index\" = {{ index-path = \"../index\" }}");
    vendor("", false)
        .failure()
        .stderr(predicate::str::contains(format!(
            "the resolved index source is the URL `{url}/index`"
        )));
    vendor(&to_local, false).success();
    assert!(dir.path().join("proj/vendor/packages/fmt.json").is_file());
    vendor(&to_local, true)
        .failure()
        .stderr(predicate::str::contains("--offline forbids network access"));
    vendor(
        &format!("\"{url}/index\" = {{ index-url = \"{url}/mirror\" }}"),
        false,
    )
    .failure()
    .stderr(predicate::str::contains(format!(
        "`[source-replacement]` redirected the index to the URL `{url}/mirror`"
    )));
    assert_eq!(hits.load(Ordering::SeqCst), 0, "no request to either URL");
    server.unblock();
    let _ = thread.join();
}
