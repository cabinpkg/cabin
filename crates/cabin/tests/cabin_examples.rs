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

use assert_cmd::Command;
use assert_fs::TempDir;
use assert_fs::prelude::*;

/// `Command` builder pointed at the test-built `cabin` binary, with
/// the same environment scrub `crates/cabin/tests/cli.rs` uses so an
/// integration test only sees the env it sets explicitly.
fn cabin() -> Command {
    let mut cmd = Command::cargo_bin("cabin").expect("the `cabin` binary should be built by cargo");
    cmd.env("CABIN_NO_CONFIG", "1")
        .env_remove("CABIN_CONFIG")
        .env_remove("CABIN_CONFIG_HOME");
    for key in [
        "CC",
        "CXX",
        "AR",
        "NINJA",
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "CABIN_NET_OFFLINE",
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
        cmd.env_remove(key);
    }
    cmd.env("CABIN_TERM_COLOR", "never");
    cmd.env(
        "CABIN_CACHE_HOME",
        std::env::temp_dir().join("cabin-tests-cache-home"),
    );
    cmd
}

fn command_exists(name: &str) -> bool {
    StdCommand::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn ninja_available() -> bool {
    command_exists("ninja")
}

fn c_compiler_available() -> bool {
    ["cc", "clang", "gcc"]
        .iter()
        .any(|name| command_exists(name))
}

fn cxx_compiler_available() -> bool {
    ["c++", "clang++", "g++"]
        .iter()
        .any(|name| command_exists(name))
}

/// Whether integration tests that build both C and C++ targets via
/// real Ninja can run. Cabin still requires a C++ compiler at
/// toolchain resolution time even when only C sources are built, so
/// pure-C tests gate on this helper too.
fn c_and_cxx_build_tools_available() -> bool {
    ninja_available() && c_compiler_available() && cxx_compiler_available()
}

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
        eprintln!("test skipped: requires ninja + C and C++ compilers");
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
    // `cabin run` is reserved for `cpp_executable` targets, so the
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
