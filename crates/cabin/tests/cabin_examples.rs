#![allow(
    clippy::needless_raw_string_hashes,
    clippy::uninlined_format_args,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::items_after_statements
)]

//! End-to-end tests for the user-facing example projects under
//! `examples/`. Each test copies one example into a temp dir and
//! drives `cabin build` / `cabin run` against it through the
//! compiled `cabin` binary, so the examples ship with a CI-enforced
//! guarantee that they build and run with the version of Cabin in
//! the same commit.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_fs::TempDir;
use assert_fs::prelude::*;

mod common;
use common::*;

/// Whether the host environment has opted out of network-dependent
/// example tests via Cabin's own offline-mode env var.
fn host_offline() -> bool {
    std::env::var_os("CABIN_NET_OFFLINE").is_some()
}

/// Whether the host can open a TCP connection to GitHub (where the
/// zlib foundation port pins its source archive). Used to skip
/// network-dependent example tests when outbound network is blocked
/// but `CABIN_NET_OFFLINE` is not set — without this probe, those
/// environments would fail the test on `cabin build` rather than
/// skip cleanly.
fn network_reachable() -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let Ok(mut addrs) = "github.com:443".to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok()
}

/// Whether the host can open a TCP connection to `www.sqlite.org`,
/// where the sqlite3 foundation port pins its amalgamation archive.
/// The sqlite examples fetch from sqlite.org rather than GitHub, so
/// they need their own reachability probe — `network_reachable()`
/// only checks `github.com:443`.
fn sqlite_org_reachable() -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let Ok(mut addrs) = "www.sqlite.org:443".to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok()
}

/// Whether the host can open a TCP connection to
/// `downloads.sourceforge.net`, where the libpng foundation port pins
/// its source archive. libpng fetches from SourceForge rather than
/// GitHub or sqlite.org, so it needs its own reachability probe.
fn sourceforge_reachable() -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let Ok(mut addrs) = "downloads.sourceforge.net:443".to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_secs(3)).is_ok()
}

/// Root of the user-facing `examples/` directory, computed from the
/// `cabin` crate's `CARGO_MANIFEST_DIR` (which points at
/// `crates/cabin/`) by walking up to the workspace root.
fn examples_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root should be two levels above crates/cabin")
        .join("examples")
}

/// Copy `examples/<name>/` into a fresh `assert_fs::TempDir` and
/// return the temp dir. Builds run against the copy so the source
/// tree never accumulates `build/` directories.
fn copy_example(name: &str) -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    dir.copy_from(examples_root().join(name), &["**"])
        .unwrap_or_else(|err| panic!("failed to copy example `{name}`: {err}"));
    dir
}

/// Run an already-built executable artifact and return its stdout.
fn run_artifact(path: &Path, label: &str) -> String {
    let output = StdCommand::new(path)
        .output()
        .unwrap_or_else(|err| panic!("{label}: failed to spawn `{}`: {err}", path.display()));
    assert!(
        output.status.success(),
        "{label}: `{}` exited with {:?}; stderr = {}",
        path.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap_or_else(|err| panic!("{label}: artifact stdout is not utf-8: {err}"))
}

#[test]
fn hello_c_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("hello-c");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    // `cabin run` is reserved for `executable` targets, so the
    // example is exercised by running its produced binary directly.
    let artifact = dir.path().join(format!(
        "build/dev/packages/hello-c/hello-c{}",
        std::env::consts::EXE_SUFFIX
    ));
    let stdout = run_artifact(&artifact, "hello-c");
    assert!(
        stdout.contains("Hello from Cabin (C)"),
        "hello-c artifact: stdout = {stdout}"
    );
}

#[test]
fn hello_cpp_builds_and_runs() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = copy_example("hello-cpp");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("Hello from Cabin (C++)"),
        "hello-cpp run: stdout = {stdout}"
    );
}

#[test]
fn platform_cfg_builds_and_runs() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = copy_example("platform-cfg");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    // The `[target.'cfg(...)']` condition resolves against the host
    // platform, so each OS compiles its own define and prints it —
    // exercising the per-platform define path end to end (MSVC `/D`
    // on Windows, GCC/Clang `-D` elsewhere).
    let expected = if cfg!(windows) {
        "Hello from Cabin on Windows"
    } else {
        "Hello from Cabin on Unix"
    };
    assert!(
        stdout.contains(expected),
        "platform-cfg run: stdout = {stdout}, expected to contain {expected:?}"
    );
}

#[test]
fn library_and_app_builds_and_runs() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = copy_example("library-and-app");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("Hello, Cabin!"),
        "library-and-app run: stdout = {stdout}"
    );
}

#[test]
fn workspace_basic_builds_workspace() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = copy_example("workspace-basic");
    cabin()
        .args(["build", "--workspace", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
}

#[test]
fn workspace_basic_builds_single_package() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = copy_example("workspace-basic");
    cabin()
        .args(["build", "-p", "cli", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
}

#[test]
fn workspace_basic_runs_selected_package() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = copy_example("workspace-basic");
    let output = cabin()
        .args(["run", "-p", "cli", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("doubled(21) = 42"),
        "workspace-basic run -p cli: stdout = {stdout}"
    );
}

#[test]
fn zlib_usage_builds_and_runs() {
    // The bundled zlib port compiles `.c` sources, so this gate
    // includes the C compiler — not only the C++ one used to build
    // `src/main.cc`.
    require_c_and_cxx_build_tools();
    if host_offline() {
        eprintln!(
            "test skipped: CABIN_NET_OFFLINE is set; zlib-usage needs to fetch the port archive"
        );
        return;
    }
    if !network_reachable() {
        eprintln!(
            "test skipped: cannot reach github.com:443 to fetch the zlib port archive (set CABIN_NET_OFFLINE=1 to silence the probe)"
        );
        return;
    }
    let dir = copy_example("zlib-usage");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("zlib version: 1.3"),
        "zlib-usage run: stdout = {stdout}"
    );
}

#[test]
fn cjson_usage_builds_and_runs() {
    // The bundled cJSON port compiles a `.c` source and the
    // consumer is also C, so this gate needs the C compiler.
    require_c_and_cxx_build_tools();
    if host_offline() {
        eprintln!(
            "test skipped: CABIN_NET_OFFLINE is set; cjson-usage needs to fetch the port archive"
        );
        return;
    }
    if !network_reachable() {
        eprintln!(
            "test skipped: cannot reach github.com:443 to fetch the cJSON port archive (set CABIN_NET_OFFLINE=1 to silence the probe)"
        );
        return;
    }
    let dir = copy_example("cjson-usage");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("cJSON parsed name: Cabin") && stdout.contains("cJSON version: 1.7"),
        "cjson-usage run: stdout = {stdout}"
    );
}

#[test]
fn xxhash_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    if host_offline() {
        eprintln!(
            "test skipped: CABIN_NET_OFFLINE is set; xxhash-usage needs to fetch the port archive"
        );
        return;
    }
    if !network_reachable() {
        eprintln!(
            "test skipped: cannot reach github.com:443 to fetch the xxHash port archive (set CABIN_NET_OFFLINE=1 to silence the probe)"
        );
        return;
    }
    let dir = copy_example("xxhash-usage");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    // `XXH64("Cabin", seed=0)` is a stable, well-defined digest, so
    // pinning it proves the linked library actually computed the
    // canonical xxHash result rather than just linking some symbol.
    assert!(
        stdout.contains("xxHash version: 803")
            && stdout.contains("XXH64(\"Cabin\") = 002d85a6f376e171"),
        "xxhash-usage run: stdout = {stdout}"
    );
}

#[test]
fn tinyxml2_usage_builds_and_runs() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    if host_offline() {
        eprintln!(
            "test skipped: CABIN_NET_OFFLINE is set; tinyxml2-usage needs to fetch the port archive"
        );
        return;
    }
    if !network_reachable() {
        eprintln!(
            "test skipped: cannot reach github.com:443 to fetch the tinyxml2 port archive (set CABIN_NET_OFFLINE=1 to silence the probe)"
        );
        return;
    }
    let dir = copy_example("tinyxml2-usage");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("tinyxml2 parsed to: Cabin") && stdout.contains("tinyxml2 version: 11.0.0"),
        "tinyxml2-usage run: stdout = {stdout}"
    );
}

#[test]
fn sqlite3_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    if host_offline() {
        eprintln!(
            "test skipped: CABIN_NET_OFFLINE is set; sqlite3-usage needs to fetch the port archive"
        );
        return;
    }
    if !sqlite_org_reachable() {
        eprintln!(
            "test skipped: cannot reach www.sqlite.org:443 to fetch the sqlite3 port archive (set CABIN_NET_OFFLINE=1 to silence the probe)"
        );
        return;
    }
    let dir = copy_example("sqlite3-usage");
    // Both sqlite tests prepare the *same* port; give each its own
    // cache dir so concurrent test runs do not race on one shared
    // content-addressed source tree.
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    // The default build is threadsafe; the in-memory query proves the
    // amalgamation linked (incl. the propagated -lpthread/-ldl/-lm on
    // Unix) and runs.
    assert!(
        stdout.contains("sqlite version: 3.53")
            && stdout.contains("sqlite threadsafe: 1")
            && stdout.contains("sqlite query result: 42"),
        "sqlite3-usage run: stdout = {stdout}"
    );
}

/// End-to-end proof that the `single-threaded` feature flows all the
/// way to the compiled object: enabling it on the port dependency
/// must compile SQLite with `SQLITE_THREADSAFE=0`, which
/// `sqlite3_threadsafe()` reports as `0` at run time.
#[test]
fn sqlite3_single_threaded_feature_disables_threadsafety() {
    require_c_and_cxx_build_tools();
    if host_offline() {
        eprintln!("test skipped: CABIN_NET_OFFLINE is set; needs the sqlite3 port archive");
        return;
    }
    if !sqlite_org_reachable() {
        eprintln!(
            "test skipped: cannot reach www.sqlite.org:443 (set CABIN_NET_OFFLINE=1 to silence the probe)"
        );
        return;
    }
    // Start from the example, then enable the feature on the port dep.
    let dir = copy_example("sqlite3-usage");
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "sqlite3-usage"
version = "0.1.0"

[dependencies]
sqlite3 = { port = true, version = "^3", features = ["single-threaded"] }

[target.sqlite3-usage]
type = "executable"
sources = ["src/main.c"]
deps = ["sqlite3"]
"#,
        )
        .unwrap();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("sqlite threadsafe: 0"),
        "single-threaded feature should compile SQLITE_THREADSAFE=0; stdout = {stdout}"
    );
}

/// libpng depends on the bundled zlib port, so this example
/// exercises a transitive port edge end to end. The program forces a
/// real zlib symbol (`zlibVersion()`) reached only through the
/// `libpng -> zlib` edge, proving both the transitive include
/// propagation (zlib.h is visible while compiling) and the transitive
/// link (the zlib archive is on the final link line).
///
/// The single test also walks the full cache lifecycle the way a user
/// would: a cold cache downloads both ports, a warm cache reuses them,
/// an offline build against the warm cache succeeds (which is the proof
/// the warm path needed no network), and a `--frozen` build against a
/// pristine cache fails with a clear, port-named diagnostic.
#[test]
fn libpng_usage_cache_lifecycle_builds_and_runs() {
    // libpng and zlib are both C; the consumer is C too.
    require_c_and_cxx_build_tools();
    if host_offline() {
        eprintln!(
            "test skipped: CABIN_NET_OFFLINE is set; libpng-usage needs to fetch the port archives"
        );
        return;
    }
    if !sourceforge_reachable() {
        eprintln!(
            "test skipped: cannot reach downloads.sourceforge.net:443 to fetch the libpng port archive (set CABIN_NET_OFFLINE=1 to silence the probe)"
        );
        return;
    }
    // The cold-cache run also fetches the transitive zlib port, whose
    // archive is pinned to GitHub — so this test needs GitHub reachable
    // too, not just SourceForge. Without this guard a host that can
    // reach SourceForge but not GitHub would fail mid-build instead of
    // skipping cleanly (as `zlib_usage_builds_and_runs` already does).
    if !network_reachable() {
        eprintln!(
            "test skipped: cannot reach github.com:443 to fetch the transitive zlib port archive (set CABIN_NET_OFFLINE=1 to silence the probe)"
        );
        return;
    }
    let dir = copy_example("libpng-usage");
    let manifest = dir.path().join("cabin.toml");
    let build_dir = dir.path().join("build");
    // A warm cache shared across the cold/warm/offline phases, plus a
    // pristine cache for the frozen-cold phase. Per-test cache dirs
    // keep concurrent runs from racing on one content-addressed tree.
    let warm_cache = dir.path().join("cache");
    let frozen_cache = dir.path().join("cache-frozen");

    // --- cold cache: both libpng and zlib are downloaded/prepared,
    // then the consumer builds and runs. The output proves the
    // transitive zlib edge linked. ---
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(&build_dir)
        .arg("--cache-dir")
        .arg(&warm_cache)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("libpng version: 1.6.50")
            && stdout.contains("zlib version (via libpng port edge): 1.3"),
        "libpng-usage cold run: stdout = {stdout}"
    );

    // --- warm cache: the prepared sources are reused; the build
    // still succeeds. ---
    cabin()
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(&build_dir)
        .arg("--cache-dir")
        .arg(&warm_cache)
        .assert()
        .success();

    // --- offline warm cache: --offline forbids any download, so a
    // success here proves the warm cache was reused rather than
    // re-fetched. ---
    cabin()
        .args(["build", "--offline", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(&build_dir)
        .arg("--cache-dir")
        .arg(&warm_cache)
        .assert()
        .success();

    // --- frozen cold cache: --frozen against a pristine cache must
    // fail clearly, naming the port it could not prepare. ---
    let assertion = cabin()
        .args(["build", "--frozen", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(dir.path().join("build-frozen"))
        .arg("--cache-dir")
        .arg(&frozen_cache)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("libpng") && (stderr.contains("frozen") || stderr.contains("not cached")),
        "frozen-cold build should fail with a clear port-named diagnostic; stderr = {stderr}"
    );
}

#[test]
fn library_with_tests_runs_tests() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = copy_example("library-with-tests");
    // `cabin test` builds every `type = "test"` target and runs each,
    // so this single command exercises the whole example.
    let output = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    for expected in [
        "test library-with-tests:calc_test ... ok",
        "test library-with-tests:parity_test ... ok",
        "test result: ok. 2 passed; 0 failed (of 2)",
    ] {
        assert!(
            stdout.contains(expected),
            "library-with-tests test: missing `{expected}`; stdout = {stdout}"
        );
    }
}
