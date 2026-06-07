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
    if !c_and_cxx_build_tools_available() {
        eprintln!("test skipped: requires ninja + C/C++ compilers");
        return;
    }
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
    if !c_and_cxx_build_tools_available() {
        eprintln!("test skipped: requires ninja + C/C++ compilers");
        return;
    }
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
    if !c_and_cxx_build_tools_available() {
        eprintln!("test skipped: requires ninja + C/C++ compilers");
        return;
    }
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
