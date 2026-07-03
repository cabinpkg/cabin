use super::*;
use cabin_core::{DependencySource, PortDepSource};
use cabin_manifest::load_manifest;
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;

/// Minimal `zlib.h` and `zlib.c` placed under the
/// `zlib-1.3.1/` prefix.  The C source exports a
/// `zlibVersion()` function with the canonical signature so
/// the downstream consumer can link against it.
const FAKE_ZLIB_HEADER: &str = r#"#ifndef ZLIB_H
#define ZLIB_H
#ifdef __cplusplus
extern "C" {
#endif
const char *zlibVersion(void);
#ifdef __cplusplus
}
#endif
#endif
"#;

const FAKE_ZLIB_SOURCE: &str = r#"#include "zlib.h"
const char *zlibVersion(void) { return "1.3.1"; }
"#;

/// Build a `.tar.gz` archive containing the given entries
/// and return `(path, hex_sha256, request_counter)`.  The
/// counter is unused; the test server tracks its own count.
fn make_archive(dir: &Path, name: &str, entries: &[(&str, &str)]) -> (PathBuf, String) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("archive parent dir");
    }
    let f = fs::File::create(&path).expect("create archive");
    let enc = GzEncoder::new(f, Compression::default());
    let mut builder = tar::Builder::new(enc);
    for (rel, body) in entries {
        let bytes = body.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(&mut header, rel, &mut std::io::Cursor::new(bytes))
            .expect("append entry");
    }
    let enc = builder.into_inner().expect("finalize tar");
    enc.finish().expect("finalize gzip").flush().expect("flush");
    let bytes = fs::read(&path).expect("hash archive");
    let mut h = Sha256::new();
    h.update(&bytes);
    (path, cabin_core::hash::hex_digest(&h.finalize()))
}

/// Loopback HTTP server that serves a single archive file
/// and counts the number of GET requests it handles.
struct ArchiveServer {
    server: Arc<tiny_http::Server>,
    thread: Option<JoinHandle<()>>,
    url: String,
    request_count: Arc<AtomicUsize>,
}

impl ArchiveServer {
    fn start(archive_bytes: Vec<u8>) -> Self {
        let server =
            Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"));
        let addr = server.server_addr().to_ip().expect("loopback addr");
        let url = format!("http://{addr}");
        let request_count = Arc::new(AtomicUsize::new(0));
        let count_for_thread = Arc::clone(&request_count);
        let server_for_thread = Arc::clone(&server);
        let bytes = Arc::new(archive_bytes);
        let bytes_for_thread = Arc::clone(&bytes);
        let thread = std::thread::spawn(move || {
            while let Ok(req) = server_for_thread.recv() {
                let path = req.url().to_string();
                if path.ends_with("/zlib-1.3.1.tar.gz") {
                    count_for_thread.fetch_add(1, Ordering::SeqCst);
                    let body = (*bytes_for_thread).clone();
                    let _ = req.respond(tiny_http::Response::from_data(body));
                } else {
                    let _ = req.respond(tiny_http::Response::empty(404));
                }
            }
        });
        Self {
            server,
            thread: Some(thread),
            url,
            request_count,
        }
    }

    fn url(&self) -> &str {
        &self.url
    }

    fn request_count(&self) -> usize {
        self.request_count.load(Ordering::SeqCst)
    }
}

impl Drop for ArchiveServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Lay out a fake-zlib port + consumer fixture and return the
/// consumer manifest path.
fn lay_fixture(
    tmp: &Path,
    archive_url: &str,
    sha256_hex: &str,
    strip_prefix: Option<&str>,
    port_type: &str,
) -> PathBuf {
    use std::fmt::Write as _;
    let mut port_toml = String::new();
    port_toml.push_str("[port]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[source]\n");
    writeln!(port_toml, "type = \"{port_type}\"").unwrap();
    writeln!(port_toml, "url = \"{archive_url}\"").unwrap();
    writeln!(port_toml, "sha256 = \"{sha256_hex}\"").unwrap();
    if let Some(prefix) = strip_prefix {
        writeln!(port_toml, "strip_prefix = \"{prefix}\"").unwrap();
    }
    port_toml.push_str("\n[overlay]\nmanifest = \"cabin.toml\"\n");
    assert_fs::fixture::ChildPath::new(tmp.join("ports/zlib/1.3.1/port.toml"))
        .write_str(&port_toml)
        .unwrap();

    assert_fs::fixture::ChildPath::new(tmp.join("ports/zlib/1.3.1/cabin.toml"))
        .write_str(
            r#"[package]
name = "zlib"
version = "1.3.1"

[target.zlib]
type = "library"
sources = ["zlib.c"]
include-dirs = ["."]
"#,
        )
        .unwrap();

    let consumer_manifest = tmp.join("consumer/cabin.toml");
    assert_fs::fixture::ChildPath::new(&consumer_manifest)
        .write_str(
            r#"[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.consumer]
type = "executable"
sources = ["src/main.c"]
deps = ["zlib"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(tmp.join("consumer/src/main.c"))
        .write_str(
            r#"#include <zlib.h>
#include <stdio.h>

int main(void) {
    const char *v = zlibVersion();
    if (!v || !*v) return 1;
    puts(v);
    return 0;
}
"#,
        )
        .unwrap();
    consumer_manifest
}

#[test]
fn builds_and_runs_downstream_consumer() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let (archive_path, hex) = make_archive(
        &tmp.path().join("downloads"),
        "zlib-1.3.1.tar.gz",
        &[
            ("zlib-1.3.1/zlib.h", FAKE_ZLIB_HEADER),
            ("zlib-1.3.1/zlib.c", FAKE_ZLIB_SOURCE),
        ],
    );
    let bytes = fs::read(&archive_path).unwrap();
    let server = ArchiveServer::start(bytes);
    let archive_url = format!("{}/zlib-1.3.1.tar.gz", server.url());
    let consumer_manifest = lay_fixture(
        tmp.path(),
        &archive_url,
        &hex,
        Some("zlib-1.3.1"),
        "archive",
    );
    let build_dir = tmp.path().join("build");
    let cache_dir = tmp.path().join("cache");

    cabin()
        .args([
            "build",
            "--manifest-path",
            consumer_manifest.to_str().unwrap(),
            "--build-dir",
            build_dir.to_str().unwrap(),
            "--cache-dir",
            cache_dir.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Locate and execute the built binary.  The planner
    // places executables under
    // `<build_dir>/<profile>/packages/<package>/<target>`.
    let exe_name = format!("consumer{}", std::env::consts::EXE_SUFFIX);
    let candidate_dev = build_dir.join("dev/packages/consumer").join(&exe_name);
    let candidate_release = build_dir.join("release/packages/consumer").join(&exe_name);
    let exe = if candidate_dev.is_file() {
        candidate_dev
    } else if candidate_release.is_file() {
        candidate_release
    } else {
        panic!(
            "could not find consumer executable under {}; expected `{}` in `dev/packages/consumer/` or `release/packages/consumer/`",
            build_dir.display(),
            exe_name
        );
    };
    let output = std::process::Command::new(&exe)
        .output()
        .expect("run consumer");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "consumer exited non-zero: {stdout}"
    );
    assert!(
        stdout.contains("1.3.1"),
        "expected zlib version output, got {stdout:?}"
    );

    // A port's sources are third-party upstream code: the consumer
    // compiles with the prepared include dir marked as a system
    // include (`-isystem`, or `/external:I` in the MSVC dialect the
    // Windows runner builds with), while the port's own translation
    // unit keeps a plain user include.
    let ccdb: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(build_dir.join("dev/compile_commands.json")).unwrap(),
    )
    .unwrap();
    let command_for = |suffix: &str| -> String {
        ccdb.as_array()
            .unwrap()
            .iter()
            .find(|e| {
                e["file"]
                    .as_str()
                    .is_some_and(|f| f.replace('\\', "/").ends_with(suffix))
            })
            .unwrap_or_else(|| panic!("compile entry for {suffix} present"))["command"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let system_flag = if cfg!(windows) {
        "/external:I"
    } else {
        "-isystem"
    };
    assert!(
        command_for("src/main.c").contains(system_flag),
        "consumer compile must mark the port include dir as a system include",
    );
    assert!(
        !command_for("zlib.c").contains(system_flag),
        "the port's own compile keeps plain user includes",
    );

    let first_count = server.request_count();
    assert!(first_count >= 1, "expected at least one archive download");

    // Re-run: the cache should satisfy preparation so the
    // HTTP server sees no additional requests.
    cabin()
        .args([
            "build",
            "--manifest-path",
            consumer_manifest.to_str().unwrap(),
            "--build-dir",
            build_dir.to_str().unwrap(),
            "--cache-dir",
            cache_dir.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert_eq!(
        server.request_count(),
        first_count,
        "second cabin build should reuse the cached archive (no new HTTP requests)"
    );
}

#[test]
fn checksum_mismatch_surfaces_clear_diagnostic() {
    let tmp = TempDir::new().unwrap();
    let (archive_path, _real_hex) = make_archive(
        &tmp.path().join("downloads"),
        "zlib-1.3.1.tar.gz",
        &[
            ("zlib-1.3.1/zlib.h", FAKE_ZLIB_HEADER),
            ("zlib-1.3.1/zlib.c", FAKE_ZLIB_SOURCE),
        ],
    );
    let bytes = fs::read(&archive_path).unwrap();
    let server = ArchiveServer::start(bytes);
    let archive_url = format!("{}/zlib-1.3.1.tar.gz", server.url());
    let bogus = "0".repeat(64);
    let consumer_manifest = lay_fixture(
        tmp.path(),
        &archive_url,
        &bogus,
        Some("zlib-1.3.1"),
        "archive",
    );

    let assertion = cabin()
        .args([
            "build",
            "--manifest-path",
            consumer_manifest.to_str().unwrap(),
            "--build-dir",
            tmp.path().join("build").to_str().unwrap(),
            "--cache-dir",
            tmp.path().join("cache").to_str().unwrap(),
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("checksum mismatch") && stderr.contains("zlib"),
        "expected a checksum-mismatch diagnostic mentioning zlib, got: {stderr}"
    );
}

#[test]
fn missing_strip_prefix_surfaces_clear_diagnostic() {
    let tmp = TempDir::new().unwrap();
    // Archive's top-level directory does not match the
    // declared `strip_prefix`.
    let (archive_path, hex) = make_archive(
        &tmp.path().join("downloads"),
        "zlib-1.3.1.tar.gz",
        &[
            ("other-1.0/zlib.h", FAKE_ZLIB_HEADER),
            ("other-1.0/zlib.c", FAKE_ZLIB_SOURCE),
        ],
    );
    let bytes = fs::read(&archive_path).unwrap();
    let server = ArchiveServer::start(bytes);
    let archive_url = format!("{}/zlib-1.3.1.tar.gz", server.url());
    let consumer_manifest = lay_fixture(
        tmp.path(),
        &archive_url,
        &hex,
        Some("zlib-1.3.1"),
        "archive",
    );

    let assertion = cabin()
        .args([
            "build",
            "--manifest-path",
            consumer_manifest.to_str().unwrap(),
            "--build-dir",
            tmp.path().join("build").to_str().unwrap(),
            "--cache-dir",
            tmp.path().join("cache").to_str().unwrap(),
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("strip_prefix") && stderr.contains("zlib-1.3.1"),
        "expected a missing-strip_prefix diagnostic, got: {stderr}"
    );
}

#[test]
fn unsupported_source_type_is_rejected_before_network() {
    let tmp = TempDir::new().unwrap();
    // Use a clearly-bogus URL so a network attempt would
    // fail loudly.  The parser should refuse the `git` source
    // type before any download happens.
    let consumer_manifest = lay_fixture(
        tmp.path(),
        "https://example.invalid/zlib.tar.gz",
        &"a".repeat(64),
        Some("zlib-1.3.1"),
        "git",
    );
    let assertion = cabin()
        .args([
            "build",
            "--manifest-path",
            consumer_manifest.to_str().unwrap(),
            "--build-dir",
            tmp.path().join("build").to_str().unwrap(),
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("unsupported source type") || stderr.contains("`git`"),
        "expected an unsupported-source-type diagnostic, got: {stderr}"
    );
}

/// `cabin metadata` must be network-free: a fresh checkout
/// that declares an HTTP-backed port whose archive has never
/// been cached must still render metadata successfully.
/// Provenance for the unprepared port is gracefully omitted
/// rather than the command erroring on a download attempt.
#[test]
fn cabin_metadata_succeeds_against_unfetched_http_port() {
    let tmp = TempDir::new().unwrap();
    let consumer_manifest = lay_fixture(
        tmp.path(),
        "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
        &"a".repeat(64),
        Some("zlib-1.3.1"),
        "archive",
    );
    cabin()
        .env("CABIN_CACHE_DIR", tmp.path().join("cache"))
        .args([
            "metadata",
            "--manifest-path",
            consumer_manifest.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success();
}

#[test]
fn cabin_metadata_surfaces_prepared_port_provenance() {
    let tmp = TempDir::new().unwrap();
    let (archive_path, hex) = make_archive(
        &tmp.path().join("downloads"),
        "zlib-1.3.1.tar.gz",
        &[
            ("zlib-1.3.1/zlib.h", FAKE_ZLIB_HEADER),
            ("zlib-1.3.1/zlib.c", FAKE_ZLIB_SOURCE),
        ],
    );
    // `cabin metadata` forces `offline = true` (it is a
    // local-introspection command), so the fixture uses a
    // `file://` URL the resolver always satisfies without
    // touching the network.  The metadata view should still
    // surface the prepared port's full provenance.
    let archive_url = url::Url::from_file_path(&archive_path).unwrap().to_string();
    let consumer_manifest = lay_fixture(
        tmp.path(),
        &archive_url,
        &hex,
        Some("zlib-1.3.1"),
        "archive",
    );

    // `cabin metadata` does not expose `--cache-dir`; the
    // env var is the equivalent knob for per-test cache
    // isolation now that the default lives at
    // `$HOME/.cache/cabin`.
    let assertion = cabin()
        .env("CABIN_CACHE_DIR", tmp.path().join("cache"))
        .args([
            "metadata",
            "--manifest-path",
            consumer_manifest.to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("metadata JSON should parse");
    let ports = value
        .get("ports")
        .and_then(serde_json::Value::as_array)
        .expect("metadata view should expose a `ports` array");
    assert_eq!(
        ports.len(),
        1,
        "expected exactly one prepared port, got {ports:?}"
    );
    let port = &ports[0];
    assert!(
        port.get("port_dir").is_none(),
        "top-level port_dir should be replaced by the origin block; got: {port:?}"
    );
    assert_eq!(port["name"].as_str(), Some("zlib"));
    assert_eq!(port["version"].as_str(), Some("1.3.1"));
    let origin = port.get("origin").expect("origin block");
    assert_eq!(origin["kind"].as_str(), Some("path"));
    let port_dir = origin["port_dir"].as_str().expect("port_dir is a string");
    assert!(
        std::path::Path::new(port_dir).is_absolute(),
        "port_dir should be absolute, got {port_dir}"
    );
    assert!(
        port_dir.ends_with(&host_path("ports/zlib/1.3.1")),
        "port_dir should point at the recipe directory, got {port_dir}"
    );
    let source = port.get("source").expect("source block");
    assert_eq!(source["kind"].as_str(), Some("archive"));
    assert_eq!(source["url"].as_str(), Some(archive_url.as_str()));
    assert_eq!(
        source["sha256"].as_str(),
        Some(format!("sha256:{hex}").as_str())
    );
    assert_eq!(source["strip_prefix"].as_str(), Some("zlib-1.3.1"));
    let overlay = port["overlay_manifest"]
        .as_str()
        .expect("overlay_manifest should be a string");
    assert!(
        std::path::Path::new(overlay).is_absolute(),
        "overlay_manifest should be absolute, got {overlay}"
    );
    assert!(
        overlay.ends_with(&host_path("ports/zlib/1.3.1/cabin.toml")),
        "overlay_manifest should point at the port's overlay file, got {overlay}"
    );
}

/// Regression for #26: port discovery must run *after* patch
/// resolution.  The root manifest declares a versioned dep on
/// `foo`; the patched fork pulls in zlib via a `port-path`.
/// Without the patches-before-discovery ordering, the walker
/// never sees the patched fork's port edge and `cabin
/// metadata` emits an empty `ports` array.
#[test]
fn metadata_discovers_port_introduced_by_patched_manifest() {
    let tmp = TempDir::new().unwrap();
    let (archive_path, hex) = make_archive(
        &tmp.path().join("downloads"),
        "zlib-1.3.1.tar.gz",
        &[
            ("zlib-1.3.1/zlib.h", FAKE_ZLIB_HEADER),
            ("zlib-1.3.1/zlib.c", FAKE_ZLIB_SOURCE),
        ],
    );
    let archive_url = url::Url::from_file_path(&archive_path).unwrap().to_string();
    tmp.child("ports/zlib/1.3.1/port.toml")

            .write_str(&format!(
                "[port]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[source]\ntype = \"archive\"\nurl = \"{archive_url}\"\nsha256 = \"{hex}\"\nstrip_prefix = \"zlib-1.3.1\"\n\n[overlay]\nmanifest = \"cabin.toml\"\n"
            ))

            .unwrap();
    tmp.child("ports/zlib/1.3.1/cabin.toml")
        .write_str(
            r#"[package]
name = "zlib"
version = "1.3.1"

[target.zlib]
type = "library"
sources = ["zlib.c"]
include-dirs = ["."]
"#,
        )
        .unwrap();

    let root = tmp.path().join("app");
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
foo = ">=0.1.0 <1.0.0"

[patch]
foo = { path = "../foo-fork" }
"#,
        )
        .unwrap();
    // The patched fork is what introduces the port edge.
    // `cabin metadata` only sees it if discovery runs against
    // the post-patch skeleton.
    tmp.child("foo-fork/cabin.toml")
        .write_str(
            r#"[package]
name = "foo"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }
"#,
        )
        .unwrap();

    let assertion = cabin()
        .env("CABIN_CACHE_DIR", tmp.path().join("cache"))
        .args([
            "metadata",
            "--manifest-path",
            root.join("cabin.toml").to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let port_names: Vec<&str> = value["ports"]
        .as_array()
        .expect("metadata view should expose a `ports` array")
        .iter()
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(
        port_names.contains(&"zlib"),
        "zlib must enter port discovery through the patched foo manifest, got: {port_names:?}"
    );
}

#[test]
fn port_toml_schema_for_real_ports_zlib_matches_published_values() {
    // Regression test that locks the on-disk port.toml in
    // crates/cabin-port/ports/zlib/1.3.1/ against the typed parser.
    // Catches accidental edits without requiring any network.
    let descriptor =
        load_real_port_and_assert_schema("zlib", &semver::Version::new(1, 3, 1), "Zlib");
    assert_tar_gz_source(&descriptor, "zlib-1.3.1");
}

#[test]
fn port_true_resolves_against_bundled_zlib() {
    let tmp = TempDir::new().unwrap();
    let consumer = tmp.path().join("consumer");
    assert_fs::fixture::ChildPath::new(&consumer)
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(consumer.join("cabin.toml"))
        .write_str(
            r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^1.3" }
"#,
        )
        .unwrap();

    let manifest = load_manifest(consumer.join("cabin.toml")).expect("manifest parses");
    let pkg = manifest.package.expect("[package]");
    let dep = pkg
        .dependencies
        .iter()
        .find(|d| d.name.as_str() == "zlib")
        .unwrap();
    match &dep.source {
        DependencySource::Port(PortDepSource::Builtin { name, version_req }) => {
            assert_eq!(name.as_str(), "zlib");
            assert_eq!(version_req.to_string(), "^1.3");
        }
        other => panic!("expected Builtin, got {other:?}"),
    }

    // The bundled recipe is what discovery would resolve this to.
    let entry = cabin_port::builtin::lookup("zlib", &semver::VersionReq::parse("^1.3").unwrap())
        .expect("bundled zlib");
    let descriptor = cabin_port::parse_port_str(
        entry.port_toml,
        std::path::Path::new("<builtin:zlib>/port.toml"),
    )
    .unwrap();
    assert_eq!(descriptor.name.as_str(), "zlib");
    assert_eq!(descriptor.version.to_string(), "1.3.1");
}

#[test]
fn port_true_with_unsatisfiable_version_surfaces_clear_diagnostic() {
    let tmp = TempDir::new().unwrap();
    let consumer = tmp.path().join("consumer");
    assert_fs::fixture::ChildPath::new(&consumer)
        .create_dir_all()
        .unwrap();
    assert_fs::fixture::ChildPath::new(consumer.join("cabin.toml"))
        .write_str(
            r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^2" }
"#,
        )
        .unwrap();

    let assertion = cabin()
        .args([
            "metadata",
            "--manifest-path",
            consumer.join("cabin.toml").to_str().unwrap(),
            "--format",
            "json",
            "--offline",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("no bundled foundation port `zlib` satisfies `^2`")
            && stderr.contains("1.3.1"),
        "expected version-not-found diagnostic, got: {stderr}"
    );
}

/// `cabin build` does not activate `[dev-dependencies]`, so a
/// port reachable only through a member's dev-deps must not
/// force a download - even when its URL is unreachable.  The
/// build target itself has no port edges, so the build
/// pipeline runs cleanly.
#[test]
fn build_skips_dev_only_port_preparation() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    // Lay a port + dev-only consumer; sibling `app` is what
    // we build.
    let _ = lay_fixture(
        tmp.path(),
        "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
        &"a".repeat(64),
        Some("zlib-1.3.1"),
        "archive",
    );
    // Rewrite consumer to reference zlib only as a dev-dep.
    tmp.child("consumer/cabin.toml")
        .write_str(
            r#"[package]
name = "consumer"
version = "0.1.0"

[dev-dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.consumer]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    tmp.child("consumer/src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    cabin()
        .args([
            "build",
            "--manifest-path",
            tmp.path().join("consumer/cabin.toml").to_str().unwrap(),
            "--build-dir",
            tmp.path().join("build").to_str().unwrap(),
        ])
        .assert()
        .success();
}

/// Port discovery must not propagate `[dev-dependencies]`
/// through path-dep recursion: the loader's dev policy
/// activates dev edges only on the selected test runners
/// themselves, so a transitive path-dep's dev-only port
/// would never become an active graph edge for this run.
/// `cabin test` must therefore skip preparing such ports -
/// even when the unreachable URL would otherwise stall the
/// command on a fresh checkout.
#[test]
fn test_skips_transitive_path_dep_dev_only_port_preparation() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    // A port whose URL would fail every download attempt; if
    // the walker ever decided to prep it, `cabin test` would
    // fail rather than skip.
    tmp.child("ports/zlib/1.3.1/port.toml")

            .write_str("[port]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[source]\ntype = \"archive\"\nurl = \"http://127.0.0.1:1/zlib-1.3.1.tar.gz\"\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n\n[overlay]\nmanifest = \"cabin.toml\"\n")

            .unwrap();
    tmp.child("ports/zlib/1.3.1/cabin.toml")
        .write_str("[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n")
        .unwrap();

    // The transitive path-dep `lib` is what declares the
    // dev-only port. `app`'s own dev-deps are empty.
    tmp.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
lib = { path = "../lib" }

[target.app_test]
type = "test"
sources = ["src/test.c"]
"#,
        )
        .unwrap();
    tmp.child("app/src/test.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    tmp.child("lib/cabin.toml")
        .write_str(
            r#"[package]
name = "lib"
version = "0.1.0"

[dev-dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.lib]
type = "library"
sources = ["src/lib.c"]
"#,
        )
        .unwrap();
    tmp.child("lib/src/lib.c")
        .write_str("int lib_dummy(void) { return 0; }\n")
        .unwrap();

    cabin()
        .args([
            "test",
            "--manifest-path",
            tmp.path().join("app/cabin.toml").to_str().unwrap(),
            "--build-dir",
            tmp.path().join("build").to_str().unwrap(),
        ])
        .assert()
        .success();
}

/// `cabin build --package <name>` must scope port
/// preparation to `<name>`'s closure.  A workspace sibling
/// that declares an uncached HTTP-backed port must therefore
/// not block the build of an unrelated package - the
/// reviewer's P1 concern around selection isolation.
#[test]
fn build_scoped_to_package_ignores_sibling_port() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    // Lay the standard zlib consumer fixture and wrap a
    // sibling `app` (no port deps) into a workspace.
    let _ = lay_fixture(
        tmp.path(),
        "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
        &"a".repeat(64),
        Some("zlib-1.3.1"),
        "archive",
    );
    tmp.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    tmp.child("app/src/main.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    tmp.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["consumer", "app"]
"#,
        )
        .unwrap();
    // Building only `app` must not fail on `consumer`'s
    // uncached HTTP-backed port.  The sibling is outside the
    // selected closure, so port discovery never walks it.
    cabin()
        .args([
            "build",
            "--manifest-path",
            tmp.path().join("cabin.toml").to_str().unwrap(),
            "--package",
            "app",
            "--build-dir",
            tmp.path().join("build").to_str().unwrap(),
        ])
        .assert()
        .success();
}

/// The flip side of `build_scoped_to_package_ignores_sibling_port`:
/// when the SELECTED package itself has a typoed `port-path`
/// (or a port-prep miss), the loader must still surface the
/// typed `PortDirectoryMissing` / `PortDependencyNotPrepared`
/// diagnostic instead of silently dropping the edge under
/// the tolerate-missing-ports policy.  Selection isolation
/// must only relax unselected siblings.
#[test]
fn build_scoped_port_miss_on_selected_package_still_errors() {
    let tmp = TempDir::new().unwrap();
    // Consumer references a non-existent port-path directory.
    tmp.child("consumer/cabin.toml")
        .write_str(
            r#"[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.consumer]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    tmp.child("consumer/src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    // No ports/ directory anywhere on disk and no workspace
    // wrapper - the consumer with a broken port-path.
    let assertion = cabin()
        .args([
            "build",
            "--manifest-path",
            tmp.path().join("consumer/cabin.toml").to_str().unwrap(),
            "--build-dir",
            tmp.path().join("build").to_str().unwrap(),
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("port") && stderr.contains("zlib"),
        "expected a port-related diagnostic naming `zlib`, got: {stderr}"
    );
}

/// Two consumers declaring conflicting bundled-port version
/// requirements must surface a clear diagnostic instead of
/// silently resolving against the first dependent's request.
#[test]
fn conflicting_builtin_version_requirements_surface_clear_diagnostic() {
    let tmp = TempDir::new().unwrap();
    // Workspace layout: root has two members; one accepts the
    // bundled 1.3.x recipe, the other demands ^2 which no
    // bundled recipe satisfies.  The 1.3 request is declared
    // first lexicographically (`alpha` < `beta`).
    tmp.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["alpha", "beta"]
"#,
        )
        .unwrap();
    tmp.child("alpha/cabin.toml")
        .write_str(
            r#"[package]
name = "alpha"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^1.3" }
"#,
        )
        .unwrap();
    tmp.child("beta/cabin.toml")
        .write_str(
            r#"[package]
name = "beta"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^2" }
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args([
            "metadata",
            "--manifest-path",
            tmp.path().join("cabin.toml").to_str().unwrap(),
            "--format",
            "json",
            "--offline",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("zlib") && stderr.contains("^2"),
        "expected a version-not-found diagnostic naming the unsatisfied requirement, got: {stderr}"
    );
}

/// `cabin fmt` rewrites local source files only; it must
/// succeed on a fresh checkout even when the workspace
/// declares an HTTP-backed port whose archive has never been
/// cached, because formatting needs no port content.
#[test]
fn fmt_succeeds_against_workspace_with_unfetched_http_port() {
    let tmp = TempDir::new().unwrap();
    let consumer_manifest = lay_fixture(
        tmp.path(),
        "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
        &"a".repeat(64),
        Some("zlib-1.3.1"),
        "archive",
    );
    let mut cmd = cabin();
    require_external_tool("clang-format");
    // We do not run `--check`: clang-format would reject the
    // fixture sources because they are not LLVM-style.  What we
    // care about is that `cabin fmt` reaches the formatter at
    // all - i.e. the port-preparation step does *not* block
    // formatting on an uncached HTTP-backed port.
    cmd.args([
        "fmt",
        "--manifest-path",
        consumer_manifest.to_str().unwrap(),
    ])
    .assert()
    .success();
}

/// `cabin clean` only touches local build outputs, so it must
/// succeed on a fresh checkout even when the workspace declares
/// an HTTP-backed port whose archive has never been cached.
/// The bogus URL would fail any actual download.
#[test]
fn clean_succeeds_against_workspace_with_unfetched_http_port() {
    let tmp = TempDir::new().unwrap();
    let consumer_manifest = lay_fixture(
        tmp.path(),
        "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
        &"a".repeat(64),
        Some("zlib-1.3.1"),
        "archive",
    );
    cabin()
        .args([
            "clean",
            "--manifest-path",
            consumer_manifest.to_str().unwrap(),
            "--build-dir",
            tmp.path().join("build").to_str().unwrap(),
        ])
        .assert()
        .success();
}

/// `cabin publish --dry-run --package <other>` selects a
/// single workspace member; foundation-port edges from any
/// member should not force a download in the selection step,
/// so a workspace with an uncached HTTP-backed port still
/// reaches `cabin package`'s own validation (which rejects
/// the dry-run on `cabin publish` only after the selection
/// has succeeded).
#[test]
fn package_selection_does_not_force_http_port_fetch() {
    let tmp = TempDir::new().unwrap();
    // Lay out the same fixture as the build tests but wrap
    // both the consumer and a sibling, port-free `app` package
    // in a workspace root.
    let _ = lay_fixture(
        tmp.path(),
        "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
        &"a".repeat(64),
        Some("zlib-1.3.1"),
        "archive",
    );
    tmp.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    tmp.child("app/src/main.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    tmp.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["consumer", "app"]
"#,
        )
        .unwrap();
    // `cabin package --package app` must not need the port
    // archive because `app` has no port deps.  With selection
    // forced to fetch ports, this would fail with a network
    // error on the bogus URL.
    cabin()
        .args([
            "package",
            "--manifest-path",
            tmp.path().join("cabin.toml").to_str().unwrap(),
            "--package",
            "app",
            "--output-dir",
            tmp.path().join("dist").to_str().unwrap(),
        ])
        .assert()
        .success();
}
