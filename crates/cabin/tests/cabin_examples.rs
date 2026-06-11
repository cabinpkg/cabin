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
    require_cxx_build_tools();
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
    require_cxx_build_tools();
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
    require_cxx_build_tools();
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
    require_cxx_build_tools();
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
    require_cxx_build_tools();
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
    require_cxx_build_tools();
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

// Real-upstream bundled-port examples are external-network smoke tests.
// They intentionally do not run in default PR/push CI; the required CI
// exercises the same Cabin port machinery hermetically via the
// loopback tests under `cli/foundation_port_*`, including the
// transitive libpng -> zlib + `[[copy]]` lifecycle in
// `foundation_port_libpng::fake_libpng_cache_lifecycle`.
#[test]
#[ignore = "requires external network"]
fn zlib_usage_builds_and_runs() {
    // The bundled zlib port compiles `.c` sources, so this gate
    // includes the C compiler — not only the C++ one used to build
    // `src/main.cc`.
    require_c_and_cxx_build_tools();
    let dir = copy_example("zlib-usage");
    run_port_build_then_run(&PortBuildRun {
        label: "zlib-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        expected_stdout: &["zlib version: 1.3"],
    });
}

#[test]
#[ignore = "requires external network"]
fn cjson_usage_builds_and_runs() {
    // The bundled cJSON port compiles a `.c` source and the
    // consumer is also C, so this gate needs the C compiler.
    require_c_and_cxx_build_tools();
    let dir = copy_example("cjson-usage");
    run_port_build_then_run(&PortBuildRun {
        label: "cjson-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        expected_stdout: &["cJSON parsed name: Cabin", "cJSON version: 1.7"],
    });
}

#[test]
#[ignore = "requires external network"]
fn xxhash_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("xxhash-usage");
    // `XXH64("Cabin", seed=0)` is a stable, well-defined digest, so
    // pinning it proves the linked library actually computed the
    // canonical xxHash result rather than just linking some symbol.
    run_port_build_then_run(&PortBuildRun {
        label: "xxhash-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        expected_stdout: &["xxHash version: 803", "XXH64(\"Cabin\") = 002d85a6f376e171"],
    });
}

#[test]
#[ignore = "requires external network"]
fn tinyxml2_usage_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("tinyxml2-usage");
    run_port_build_then_run(&PortBuildRun {
        label: "tinyxml2-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        expected_stdout: &["tinyxml2 parsed to: Cabin", "tinyxml2 version: 11.0.0"],
    });
}

#[test]
#[ignore = "requires external network"]
fn sqlite3_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("sqlite3-usage");
    // Both sqlite tests prepare the *same* port; give each its own
    // cache dir so concurrent test runs do not race on one shared
    // content-addressed source tree.
    // The default build is threadsafe; the in-memory query proves the
    // amalgamation linked (incl. the propagated -lpthread/-ldl/-lm on
    // Unix) and runs.
    run_port_build_then_run(&PortBuildRun {
        label: "sqlite3-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        expected_stdout: &[
            "sqlite version: 3.53",
            "sqlite threadsafe: 1",
            "sqlite query result: 42",
        ],
    });
}

/// End-to-end proof that the `single-threaded` feature flows all the
/// way to the compiled object: enabling it on the port dependency
/// must compile SQLite with `SQLITE_THREADSAFE=0`, which
/// `sqlite3_threadsafe()` reports as `0` at run time.
#[test]
#[ignore = "requires external network"]
fn sqlite3_single_threaded_feature_disables_threadsafety() {
    require_c_and_cxx_build_tools();
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
    run_port_build_then_run(&PortBuildRun {
        label: "sqlite3-usage single-threaded",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        expected_stdout: &["sqlite threadsafe: 0"],
    });
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
#[ignore = "requires external network"]
fn libpng_usage_cache_lifecycle_builds_and_runs() {
    // libpng and zlib are both C; the consumer is C too.
    require_c_and_cxx_build_tools();
    // The cold-cache run also fetches the transitive zlib port, whose
    // archive is pinned to GitHub — so this test needs GitHub reachable
    // too, not just SourceForge; on an unreachable host it fails rather
    // than fetching.
    let dir = copy_example("libpng-usage");
    let manifest = dir.path().join("cabin.toml");
    // A warm cache shared across the cold/warm/offline phases, plus a
    // pristine cache for the frozen-cold phase. Per-test cache dirs
    // keep concurrent runs from racing on one content-addressed tree.
    let warm_cache = dir.path().join("cache");
    let frozen_cache = dir.path().join("cache-frozen");

    run_port_cache_lifecycle(&PortCacheLifecycle {
        label: "libpng-usage",
        manifest,
        build_root: dir.path().join("build"),
        warm_cache,
        pristine_cache: frozen_cache,
        expected_stdout: &[
            "libpng version: 1.6.50",
            "zlib version (via libpng port edge): 1.3",
        ],
        expected_downloads: &["libpng", "zlib"],
        frozen_port: "libpng",
    });
}

#[test]
fn library_with_tests_runs_tests() {
    require_cxx_build_tools();
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
        "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in",
    ] {
        assert!(
            stdout.contains(expected),
            "library-with-tests test: missing `{expected}`; stdout = {stdout}"
        );
    }
}
