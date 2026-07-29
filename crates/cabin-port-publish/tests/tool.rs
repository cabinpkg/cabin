//! End-to-end tests for the `cabin-port-publish` binary.
//!
//! Offline by default: every recipe pins an `https://` URL (the
//! provenance rules require it) whose archive is pre-seeded into the
//! port cache, so the cache-first fetch never touches the network.
//! The `--publish` test serves a fake registry on a loopback
//! `tiny_http` server.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use assert_fs::TempDir;
use assert_fs::prelude::*;
use sha2::{Digest, Sha256};

const SCOPE_ZLIB_INDEX: &str = "packages/cabin-ports/zlib.json";

/// The tool's own binary (built by cargo for this crate's tests).
fn tool() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cabin-port-publish"))
}

/// The `cabin` binary from the same target directory, built on
/// demand: a package-scoped `cargo test -p cabinpkg-port-publish`
/// does not build the sibling `cabinpkg` executable, so relying on
/// the full-workspace run having built it would make these tests
/// order-dependent.  Cargo releases the target-directory lock before
/// running tests, so the nested build is safe; when the binary is
/// already present (the CI gate builds `--all-targets` first) the
/// nested invocation is a fast no-op.
fn cabin_binary() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_cabin-port-publish"))
        .parent()
        .expect("tool binary has a parent directory")
        .join(format!("cabin{}", std::env::consts::EXE_SUFFIX));
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "cabinpkg", "--bin", "cabin"])
        .status()
        .expect("cargo is runnable from a test");
    assert!(status.success(), "building the cabin binary failed");
    assert!(
        path.is_file(),
        "cabin binary not found at {} after `cargo build -p cabinpkg`",
        path.display()
    );
    path
}

/// Preflight builds compile real C/C++ sources through `cabin`,
/// which needs Ninja plus C and C++ toolchains.  Fail (not skip)
/// when the host lacks them, mirroring `crates/cabin`'s require
/// helpers.  On Windows, MSVC is discovered by cabin's own
/// toolchain logic (`cl` is normally not on PATH), so only Ninja is
/// checked there.
fn require_build_tools() {
    assert!(which("ninja"), "preflight tests need `ninja` on PATH");
    if cfg!(windows) {
        return;
    }
    let cc = ["cc", "clang", "gcc"].iter().any(|tool| which(tool));
    assert!(
        cc,
        "preflight tests need a C compiler (cc/clang/gcc) on PATH"
    );
    let cxx = ["c++", "clang++", "g++"].iter().any(|tool| which(tool));
    assert!(
        cxx,
        "preflight tests need a C++ compiler (c++/clang++/g++) on PATH"
    );
}

fn which(tool: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(format!("{tool}{}", std::env::consts::EXE_SUFFIX));
        candidate.is_file()
    })
}

/// Build a gzipped tarball from `(path, contents)` entries and
/// return `(bytes, sha256_hex)`.
fn make_tar_gz(entries: &[(&str, &str)]) -> (Vec<u8>, String) {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (rel, body) in entries {
        let bytes = body.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(&mut header, rel, std::io::Cursor::new(bytes))
            .unwrap();
    }
    let bytes = builder.into_inner().unwrap().finish().unwrap();
    let hex = cabin_core::hash::hex_digest(&Sha256::digest(&bytes));
    (bytes, hex)
}

/// Seed an archive into the port cache's content-addressed slot so
/// the tool's cache-first fetch finds it without network access.
fn seed_port_cache(cache_dir: &Path, bytes: &[u8], hex: &str) {
    let slot = cache_dir
        .join("ports")
        .join("archives")
        .join("sha256")
        .join(format!("{hex}.tar.gz"));
    fs::create_dir_all(slot.parent().unwrap()).unwrap();
    let mut file = fs::File::create(&slot).unwrap();
    file.write_all(bytes).unwrap();
}

fn write_recipe(
    ports: &assert_fs::fixture::ChildPath,
    name: &str,
    version: &str,
    sha256: &str,
    overlay: &str,
) {
    let dir = ports.child(format!("{name}/{version}"));
    dir.child("port.toml")
        .write_str(&format!(
            "[port]\nname = \"{name}\"\nversion = \"{version}\"\n\n[source]\ntype = \
             \"archive\"\nurl = \"https://ports.invalid/{name}-{version}.tar.gz\"\nsha256 = \
             \"{sha256}\"\nstrip_prefix = \"{name}-{version}\"\n\n[overlay]\nmanifest = \
             \"cabin.toml\"\n"
        ))
        .unwrap();
    dir.child("cabin.toml").write_str(overlay).unwrap();
}

/// Two fake ports: `zlib` (exercises the `z` native target key) and
/// `libpng` depending on it (exercises the scoped dependency
/// rewrite and publication ordering).  Returns the ports dir and the
/// seeded cache dir.
fn fake_ports(dir: &TempDir) -> (PathBuf, PathBuf) {
    let ports = dir.child("ports");
    let cache = dir.child("cache");

    let (zlib_bytes, zlib_hex) = make_tar_gz(&[
        (
            "zlib-1.3.1/zlib.h",
            "#ifndef ZLIB_H\n#define ZLIB_H\nint zlib_answer(void);\n#endif\n",
        ),
        (
            "zlib-1.3.1/zlib.c",
            "#include \"zlib.h\"\nint zlib_answer(void) { return 42; }\n",
        ),
    ]);
    seed_port_cache(cache.path(), &zlib_bytes, &zlib_hex);
    write_recipe(
        &ports,
        "zlib",
        "1.3.1",
        &zlib_hex,
        "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[target.zlib]\ntype = \
         \"library\"\nsources = [\"zlib.c\"]\ninclude-dirs = [\".\"]\nc-standard = \"c11\"\n",
    );
    let (libpng_bytes, libpng_hex) = make_tar_gz(&[
        (
            "libpng-1.6.50/png.h",
            "#ifndef PNG_H\n#define PNG_H\nint png_answer(void);\n#endif\n",
        ),
        (
            "libpng-1.6.50/png.c",
            "#include \"png.h\"\n#include \"zlib.h\"\nint png_answer(void) { return \
             zlib_answer(); }\n",
        ),
    ]);
    seed_port_cache(cache.path(), &libpng_bytes, &libpng_hex);
    write_recipe(
        &ports,
        "libpng",
        "1.6.50",
        &libpng_hex,
        "[package]\nname = \"libpng\"\nversion = \"1.6.50\"\n\n[dependencies]\nzlib = { port = \
         true, version = \"^1.3\" }\n\n[target.libpng]\ntype = \"library\"\nsources = \
         [\"png.c\"]\ninclude-dirs = [\".\"]\nc-standard = \"c11\"\ndeps = [\"zlib\"]\n",
    );

    (ports.to_path_buf(), cache.to_path_buf())
}

/// Scrub the environment with the same variable set as
/// `crates/cabin/tests/common/mod.rs`'s `cabin()` helper (that
/// helper is a private test module of the `cabinpkg` crate, so it
/// cannot be imported here), so a developer's config, toolchain, or
/// wrapper overrides cannot leak into the spawned `cabin` builds.
fn scrubbed(mut command: Command) -> Command {
    command.env("CABIN_NO_CONFIG", "1");
    command.env("CABIN_TERM_COLOR", "never");
    for key in [
        "CABIN_CONFIG",
        "CABIN_CONFIG_HOME",
        "CC",
        "CXX",
        "AR",
        "NINJA",
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "CABIN_NET_OFFLINE",
        "CABIN_RESOLVER_INCOMPATIBLE_STANDARDS",
        "CABIN_REGISTRY_TOKEN",
        "CABIN_COMPILER_WRAPPER",
        "CABIN_CACHE_DIR",
        "CABIN_CACHE_HOME",
        "CABIN_FMT",
        "CABIN_TIDY",
        "CABIN_PKG_CONFIG",
        "PKG_CONFIG_PATH",
        "PKG_CONFIG_LIBDIR",
        "PKG_CONFIG_SYSROOT_DIR",
        "NO_COLOR",
        "CLICOLOR",
        "CLICOLOR_FORCE",
    ] {
        command.env_remove(key);
    }
    command
}

/// `true` when `name` exists anywhere under `dir`.
fn tree_contains(dir: &Path, name: &str) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if tree_contains(&path, name) {
                return true;
            }
        } else if entry.file_name().to_string_lossy() == name {
            return true;
        }
    }
    false
}

#[test]
fn dry_run_preflights_against_a_temporary_file_registry() {
    require_build_tools();
    let dir = TempDir::new().unwrap();
    let (ports, cache) = fake_ports(&dir);
    let work = dir.child("work").to_path_buf();

    let output = scrubbed(tool())
        .arg("--dry-run")
        .arg("--ports-dir")
        .arg(&ports)
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--work-dir")
        .arg(&work)
        .arg("--cabin")
        .arg(cabin_binary())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // Publication order: the dependency before the dependent.
    assert!(
        stdout.contains("publication order: cabin-ports/zlib 1.3.1, cabin-ports/libpng 1.6.50"),
        "{stdout}"
    );

    // The temporary file registry holds both packages, scoped.
    let registry = work.join("run").join("registry");
    assert!(registry.join("config.json").is_file());
    let zlib_index: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(registry.join(SCOPE_ZLIB_INDEX)).unwrap())
            .unwrap();
    assert_eq!(zlib_index["name"], "cabin-ports/zlib");
    let version = &zlib_index["versions"]["1.3.1"];
    assert_eq!(
        version["upstream"]["url"],
        "https://ports.invalid/zlib-1.3.1.tar.gz"
    );
    assert_eq!(version["upstream"]["format"], "tar.gz");
    assert_eq!(version["upstream"]["strip-prefix"], "zlib-1.3.1");
    let libpng_index: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(registry.join("packages/cabin-ports/libpng.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        libpng_index["versions"]["1.6.50"]["dependencies"]["cabin-ports/zlib"],
        "^1.3"
    );
    // The artifact filename embeds the packaging revision the index
    // points at (the archive checksum's leading hex prefix).
    let revision = version["revision"].as_str().expect("revision recorded");
    assert_eq!(
        version["revisions"][revision]["checksum"]
            .as_str()
            .unwrap()
            .strip_prefix("sha256:")
            .unwrap()
            .get(..16),
        Some(revision)
    );
    assert!(
        registry
            .join(format!(
                "artifacts/cabin-ports/zlib/cabin-ports-zlib-1.3.1-{revision}.zip"
            ))
            .is_file()
    );

    // Each port was built *from the registry* through its probe; the
    // target key `z` decides the artifact stem (libz.a / z.lib).
    let zlib_probe = work.join("run").join("probes").join("zlib");
    assert!(
        tree_contains(&zlib_probe, "libz.a") || tree_contains(&zlib_probe, "z.lib"),
        "no z artifact under {}",
        zlib_probe.display()
    );
    // libpng's probe resolved cabin-ports/libpng (and transitively
    // cabin-ports/zlib) from the generated registry.
    let libpng_probe = work.join("run").join("probes").join("libpng");
    assert!(
        tree_contains(&libpng_probe, "libpng.a") || tree_contains(&libpng_probe, "png.lib"),
        "no png artifact under {}",
        libpng_probe.display()
    );
    assert!(
        tree_contains(&libpng_probe, "libz.a") || tree_contains(&libpng_probe, "z.lib"),
        "no transitive z artifact under {}",
        libpng_probe.display()
    );

    // Dry run performs no remote mutation and reports completion.
    assert!(stdout.contains("dry run complete"), "{stdout}");

    // A second run over the same work dir succeeds (the scratch
    // registry is rebuilt, not appended to).
    let rerun = scrubbed(tool())
        .arg("--dry-run")
        .arg("--ports-dir")
        .arg(&ports)
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--work-dir")
        .arg(&work)
        .arg("--cabin")
        .arg(cabin_binary())
        .output()
        .unwrap();
    assert!(
        rerun.status.success(),
        "rerun stderr:\n{}",
        String::from_utf8_lossy(&rerun.stderr)
    );
}

/// Fake remote registry: `config.json` + empty package reads on the
/// sparse index, and a publish API recording every PUT.
struct FakeRegistry {
    url: String,
    requests: Arc<Mutex<Vec<(String, String, usize)>>>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FakeRegistry {
    fn start() -> Self {
        Self::start_rate_limiting_puts(0)
    }

    /// Like [`FakeRegistry::start`], but the first `limited_puts`
    /// uploads answer `429` with a one-second `Retry-After` before
    /// the registry starts accepting - the shape of a drained publish
    /// token bucket.
    fn start_rate_limiting_puts(limited_puts: u32) -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr());
        let requests: Arc<Mutex<Vec<(String, String, usize)>>> = Arc::default();
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let remaining_429s = Arc::new(std::sync::atomic::AtomicU32::new(limited_puts));
        let handle = {
            let base = url.clone();
            let requests = Arc::clone(&requests);
            let shutdown = Arc::clone(&shutdown);
            let remaining_429s = Arc::clone(&remaining_429s);
            std::thread::spawn(move || {
                while !shutdown.load(std::sync::atomic::Ordering::SeqCst) {
                    let Ok(Some(mut request)) =
                        server.recv_timeout(std::time::Duration::from_millis(50))
                    else {
                        continue;
                    };
                    let method = request.method().to_string();
                    let path = request.url().to_owned();
                    let mut body = Vec::new();
                    request.as_reader().read_to_end(&mut body).unwrap();
                    let authorized = request
                        .headers()
                        .iter()
                        .any(|h| h.field.equiv("Authorization"));
                    requests
                        .lock()
                        .unwrap()
                        .push((method.clone(), path.clone(), body.len()));
                    let respond = |request: tiny_http::Request, status: u16, body: &str| {
                        let response = tiny_http::Response::from_string(body)
                            .with_status_code(status)
                            .with_header(
                                tiny_http::Header::from_bytes(
                                    &b"Content-Type"[..],
                                    &b"application/json"[..],
                                )
                                .unwrap(),
                            );
                        let _ = request.respond(response);
                    };
                    if method == "GET" && path == "/config.json" {
                        respond(
                            request,
                            200,
                            &format!(
                                "{{\"schema\":1,\"kind\":\"file-registry\",\"packages\":\
                                 \"packages\",\"artifacts\":\"artifacts\",\"auth-required\":\
                                 true,\"api\":\"{base}\"}}"
                            ),
                        );
                    } else if method == "GET" && path == "/packages/cabin-ports/zlib.json" {
                        // The version the tool is about to publish is
                        // already visible in the public index.  The
                        // upload must still happen: pending versions
                        // are invisible here, so the index can never
                        // justify skipping a PUT.
                        respond(
                            request,
                            200,
                            "{\"schema\":1,\"name\":\"cabin-ports/zlib\",\"versions\":\
                             {\"1.3.1\":{\"yanked\":false}}}",
                        );
                    } else if method == "PUT" && path.starts_with("/api/v1/packages/") {
                        if !authorized {
                            respond(request, 401, "{\"error\":\"authentication required\"}");
                            continue;
                        }
                        let limited = remaining_429s
                            .fetch_update(
                                std::sync::atomic::Ordering::SeqCst,
                                std::sync::atomic::Ordering::SeqCst,
                                |n| n.checked_sub(1),
                            )
                            .is_ok();
                        if limited {
                            respond_rate_limited(request);
                            continue;
                        }
                        let version = path.rsplit('/').next().unwrap_or_default().to_owned();
                        respond(
                            request,
                            201,
                            &format!(
                                "{{\"ok\":true,\"version\":\"{version}\",\"verification\":\
                                 \"pending\"}}"
                            ),
                        );
                    } else {
                        respond(request, 404, "{\"error\":\"not found\"}");
                    }
                }
            })
        };
        Self {
            url,
            requests,
            shutdown,
            handle: Some(handle),
        }
    }

    fn requests(&self) -> Vec<(String, String, usize)> {
        self.requests.lock().unwrap().clone()
    }
}

/// The drained-bucket refusal: `429` with a one-second `Retry-After`,
/// the smallest delay the retry loop honors, keeping the test fast.
fn respond_rate_limited(request: tiny_http::Request) {
    let response = tiny_http::Response::from_string(
        "{\"error\":{\"code\":\"rate_limited\",\"detail\":\"publish rate limit exceeded\"}}",
    )
    .with_status_code(429)
    .with_header(tiny_http::Header::from_bytes(&b"Retry-After"[..], &b"1"[..]).unwrap());
    let _ = request.respond(response);
}

impl Drop for FakeRegistry {
    fn drop(&mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn publish_uploads_every_package_in_dependency_order() {
    require_build_tools();
    let dir = TempDir::new().unwrap();
    let (ports, cache) = fake_ports(&dir);
    let work = dir.child("work").to_path_buf();
    let registry = FakeRegistry::start();

    let output = scrubbed(tool())
        .arg("--publish")
        .arg("--index-url")
        .arg(&registry.url)
        .arg("--ports-dir")
        .arg(&ports)
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--work-dir")
        .arg(&work)
        .arg("--cabin")
        .arg(cabin_binary())
        .env("CABIN_REGISTRY_TOKEN", "cabin_testtoken1234")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let puts: Vec<(String, usize)> = registry
        .requests()
        .into_iter()
        .filter(|(method, _, _)| method == "PUT")
        .map(|(_, path, len)| (path, len))
        .collect();
    // Dependency order: zlib uploads before libpng, both framed
    // bodies non-trivial (metadata JSON + zip archive).  Every upload
    // opts into new revisions: a changed committed recipe IS the
    // deliberate intent to respin its published version.
    assert_eq!(
        puts.iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "/api/v1/packages/cabin-ports/zlib/1.3.1?new-revision=true",
            "/api/v1/packages/cabin-ports/libpng/1.6.50?new-revision=true",
        ]
    );
    assert!(puts.iter().all(|(_, len)| *len > 500), "{puts:?}");
    // The upload runs through `cabin publish` itself.
    assert!(
        stdout.contains("Published cabin-ports/zlib 1.3.1"),
        "{stdout}"
    );
    assert!(stdout.contains("verification: pending"), "{stdout}");
}

/// A drained publish token bucket answers `429` with `Retry-After`;
/// the tool must wait it out and retry the same package instead of
/// failing the run (every attempt charges the bucket, so a rerun
/// could otherwise never make progress past the burst).
#[test]
fn publish_retries_a_rate_limited_package() {
    require_build_tools();
    let dir = TempDir::new().unwrap();
    let (ports, cache) = fake_ports(&dir);
    let work = dir.child("work").to_path_buf();
    let registry = FakeRegistry::start_rate_limiting_puts(1);

    let output = scrubbed(tool())
        .arg("--publish")
        .arg("--index-url")
        .arg(&registry.url)
        .arg("--ports-dir")
        .arg(&ports)
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--work-dir")
        .arg(&work)
        .arg("--cabin")
        .arg(cabin_binary())
        .env("CABIN_REGISTRY_TOKEN", "cabin_testtoken1234")
        // Inherited color-forcing must not defeat the retry parser:
        // the tool passes --color never to the captured subprocess.
        .env("CABIN_TERM_COLOR", "always")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("note: the registry rate limited cabin-ports/zlib"),
        "{stderr}"
    );

    // The limited PUT is retried: zlib uploads twice (429 then 201),
    // libpng once, and order is preserved.
    let puts: Vec<String> = registry
        .requests()
        .into_iter()
        .filter(|(method, _, _)| method == "PUT")
        .map(|(_, path, _)| path)
        .collect();
    assert_eq!(
        puts,
        [
            "/api/v1/packages/cabin-ports/zlib/1.3.1?new-revision=true",
            "/api/v1/packages/cabin-ports/zlib/1.3.1?new-revision=true",
            "/api/v1/packages/cabin-ports/libpng/1.6.50?new-revision=true",
        ]
    );
}

/// A failing preflight must leave the remote registry untouched:
/// zlib preflights clean, libpng's sources do not compile, so the
/// run aborts before any upload - including zlib's.
#[test]
fn a_failed_preflight_makes_no_remote_mutation() {
    require_build_tools();
    let dir = TempDir::new().unwrap();
    let ports = dir.child("ports");
    let cache = dir.child("cache");

    let (zlib_bytes, zlib_hex) = make_tar_gz(&[
        ("zlib-1.3.1/zlib.h", "int zlib_answer(void);\n"),
        (
            "zlib-1.3.1/zlib.c",
            "#include \"zlib.h\"\nint zlib_answer(void) { return 42; }\n",
        ),
    ]);
    seed_port_cache(cache.path(), &zlib_bytes, &zlib_hex);
    write_recipe(
        &ports,
        "zlib",
        "1.3.1",
        &zlib_hex,
        "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[target.zlib]\ntype = \
         \"library\"\nsources = [\"zlib.c\"]\ninclude-dirs = [\".\"]\nc-standard = \"c11\"\n",
    );
    let (libpng_bytes, libpng_hex) = make_tar_gz(&[(
        "libpng-1.6.50/png.c",
        "#error this port must fail its preflight build\n",
    )]);
    seed_port_cache(cache.path(), &libpng_bytes, &libpng_hex);
    write_recipe(
        &ports,
        "libpng",
        "1.6.50",
        &libpng_hex,
        "[package]\nname = \"libpng\"\nversion = \"1.6.50\"\n\n[dependencies]\nzlib = { port = \
         true, version = \"^1.3\" }\n\n[target.libpng]\ntype = \"library\"\nsources = \
         [\"png.c\"]\ninclude-dirs = [\".\"]\nc-standard = \"c11\"\ndeps = [\"zlib\"]\n",
    );

    let registry = FakeRegistry::start();
    let output = scrubbed(tool())
        .arg("--publish")
        .arg("--index-url")
        .arg(&registry.url)
        .arg("--ports-dir")
        .arg(ports.path())
        .arg("--cache-dir")
        .arg(cache.path())
        .arg("--work-dir")
        .arg(dir.child("work").path())
        .arg("--cabin")
        .arg(cabin_binary())
        .env("CABIN_REGISTRY_TOKEN", "cabin_testtoken1234")
        .output()
        .unwrap();
    assert!(!output.status.success(), "the preflight failure must abort");
    assert!(
        registry.requests().is_empty(),
        "no request may reach the registry after a failed preflight: {:?}",
        registry.requests()
    );
}

#[test]
fn dry_run_rejects_an_index_url() {
    let output = tool()
        .arg("--dry-run")
        .arg("--index-url")
        .arg("https://registry.invalid")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--index-url only applies to --publish")
    );
}

#[test]
fn requires_exactly_one_mode() {
    let output = tool().output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("pass exactly one of --dry-run or --publish")
    );
}
