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
//! `examples/`.  Each test copies one example into a temp dir and
//! drives `cabin build` / `cabin run` / `cabin test` against it through
//! the compiled `cabin` binary, so CI checks the examples against the
//! Cabin of the same commit.

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

/// Copy `examples/<name>/` into a fresh temp dir so builds never
/// accumulate `build/` directories in the source tree.
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
fn assembly_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("assembly");
    // GNU `.S` on Linux and macOS, MASM through `ml64.exe` on
    // Windows; the program exits non-zero unless the assembly
    // returned the target's `ANSWER_VALUE` define.
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
        stdout.contains("Assembled answer: 42"),
        "assembly run: stdout = {stdout}"
    );
}

#[test]
fn simd_dispatch_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("simd-dispatch");
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
    // GCC and Clang reject the SSE4.1 source without `-msse4.1`; `cl.exe`
    // takes it without a flag.  An x86 host with SSE4.1 runs that
    // variant, and other hosts never compile it.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    let variant = if std::arch::is_x86_feature_detected!("sse4.1") {
        "sse4.1"
    } else {
        "generic"
    };
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    let variant = "generic";
    assert!(
        stdout.contains(&format!("max = 42 ({variant})")),
        "simd-dispatch run: stdout = {stdout}"
    );
}

#[test]
fn platform_cfg_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("platform-cfg");
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
    // The `[target.'cfg(...)']` conditions resolve against the host
    // platform, so each OS compiles its own define and its own C
    // source and prints both - exercising the per-platform define
    // path (MSVC `/D` on Windows, GCC/Clang `-D` elsewhere) and source
    // selection end to end.  The two C sources define one symbol, so
    // compiling the other platform's too would fail the link.
    let expected = if cfg!(windows) {
        [
            "Hello from Cabin on Windows",
            "Compiled src/platform_windows.c",
        ]
    } else {
        ["Hello from Cabin on Unix", "Compiled src/platform_unix.c"]
    };
    for expected in expected {
        assert!(
            stdout.contains(expected),
            "platform-cfg run: stdout = {stdout}, expected to contain {expected:?}"
        );
    }
}

#[test]
fn library_and_app_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("library-and-app");
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
fn header_only_lib_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("header-only-lib");
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
    // Both lines require code compiled against the `header-only`
    // target's include dir; the target itself contributes no archive
    // to the link, so a passing run proves the graph-only edge.
    for expected in [
        "circle area (r = 2): 12.57",
        "rectangle area (3 x 4): 12.00",
    ] {
        assert!(
            stdout.contains(expected),
            "header-only-lib run: missing `{expected}`; stdout = {stdout}"
        );
    }
}

/// `cabin run` on the copy `dir` of an example.
fn run_example(dir: &TempDir) -> assert_cmd::Command {
    let mut cmd = cabin();
    cmd.args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"));
    cmd
}

#[test]
fn build_script_header_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("build-script-header");
    run_example(&dir)
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "built with the dev profile for {}",
            std::env::consts::OS
        )));
}

/// The example is C only, its build script included: it builds on a
/// `PATH` that holds no C++ compiler.
#[cfg(unix)]
#[test]
fn build_script_header_builds_without_a_cxx_compiler() {
    require_c_and_cxx_build_tools();
    let bin = TempDir::new().unwrap();
    let cc = ["cc", "clang", "gcc"]
        .into_iter()
        .find_map(|name| which::which(name).ok())
        .unwrap();
    std::os::unix::fs::symlink(cc, bin.path().join("cc")).unwrap();
    // `as`/`ld`: GNU drivers look them up on PATH; `xcrun`: Apple SDK lookup.
    for tool in ["ar", "ninja", "as", "ld", "xcrun"] {
        if let Ok(path) = which::which(tool) {
            std::os::unix::fs::symlink(path, bin.path().join(tool)).unwrap();
        }
    }
    let dir = copy_example("build-script-header");
    run_example(&dir)
        .env("PATH", bin.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("built with the dev profile"));
}

#[test]
fn build_script_helper_lib_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("build-script-helper-lib");
    run_example(&dir)
        .assert()
        .success()
        .stdout(predicates::str::contains("crc32(\"cabin\") = ce4ce919"));
}

/// The build script runs the `embed` Cabin built, by the path Cabin
/// gave it: a failing program of that name first on `PATH` changes
/// nothing.
#[test]
fn build_script_generator_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("build-script-generator");
    let decoys = TempDir::new().unwrap();
    let decoy = decoys.child(if cfg!(windows) { "embed.bat" } else { "embed" });
    decoy
        .write_str(if cfg!(windows) {
            "@exit /b 1\r\n"
        } else {
            "#!/bin/sh\nexit 1\n"
        })
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(decoy.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = std::env::join_paths(
        std::iter::once(decoys.path().to_owned())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    run_example(&dir)
        .env("PATH", path)
        .assert()
        .success()
        .stdout(predicates::str::contains("Hello from an embedded file"));
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

// The website demo (`demo.tape`) shows this tree.
#[test]
fn workspace_basic_tree_shows_the_path_dependency() {
    let dir = copy_example("workspace-basic");
    cabin()
        .args(["tree", "-p", "cli", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "└── util v0.1.0 [normal] (workspace)",
        ));
}

// Real-upstream registry examples are external-network smoke tests:
// they consume the `cabin-ports/*` packages that the publisher's
// staging step stages from the pinned catalog (`FOUNDATION_PORTS_DIR`)
// into a local file registry (once per test process, downloading the
// pinned upstream archives).  They do not run in default PR CI; the required CI
// exercises the same consumption path hermetically via
// `cli/registry_ports.rs`.
#[test]
#[ignore = "requires external network"]
fn zlib_usage_builds_and_runs() {
    // The zlib package compiles `.c` sources, so this gate
    // includes the C compiler and the C++ one used to build
    // `src/main.cc`.
    require_c_and_cxx_build_tools();
    let dir = copy_example("zlib-usage");
    run_port_build_then_run(&PortBuildRun {
        label: "zlib-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["zlib version: 1.3"],
    });
}

#[test]
#[ignore = "requires external network"]
fn cjson_usage_builds_and_runs() {
    // The cJSON package compiles a `.c` source and the
    // consumer is also C, so this gate needs the C compiler.
    require_c_and_cxx_build_tools();
    let dir = copy_example("cjson-usage");
    run_port_build_then_run(&PortBuildRun {
        label: "cjson-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["cJSON parsed name: Cabin", "cJSON version: 1.7"],
    });
}

#[test]
#[ignore = "requires external network"]
fn xxhash_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("xxhash-usage");
    // `XXH64("Cabin", seed=0)` is a stable, well-defined digest, so
    // pinning it proves the linked library computed the
    // canonical xxHash result rather than linking an arbitrary symbol.
    run_port_build_then_run(&PortBuildRun {
        label: "xxhash-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
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
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["tinyxml2 parsed to: Cabin", "tinyxml2 version: 11.0.0"],
    });
}

#[test]
#[ignore = "requires external network"]
fn sqlite3_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("sqlite3-usage");
    // Both sqlite tests fetch the *same* package; give each its own
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
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &[
            "sqlite version: 3.53",
            "sqlite threadsafe: 1",
            "sqlite query result: 42",
        ],
    });
}

/// End-to-end proof that the `single-threaded` feature flows all the
/// way to the compiled object: enabling it on the registry dependency
/// must compile SQLite with `SQLITE_THREADSAFE=0`, which
/// `sqlite3_threadsafe()` reports as `0` at run time.
#[test]
#[ignore = "requires external network"]
fn sqlite3_single_threaded_feature_disables_threadsafety() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("sqlite3-usage");
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "sqlite3-usage"
version = "0.1.0"
c-standard = "c11"

[dependencies]
"cabin-ports/sqlite3" = { version = "=3.53.2", features = ["single-threaded"] }

[target.sqlite3-usage]
type = "executable"
sources = ["src/main.c"]
deps = ["cabin-ports/sqlite3"]
"#,
        )
        .unwrap();
    run_port_build_then_run(&PortBuildRun {
        label: "sqlite3-usage single-threaded",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["sqlite threadsafe: 0"],
    });
}

/// libpng depends on zlib, so this example exercises a transitive
/// registry edge end to end: the libpng archive needs zlib's on the
/// final link line.  libpng's edge onto zlib is private, so the example
/// declares zlib itself for its own `zlibVersion()` call; both edges
/// resolve to one zlib version.  The hermetic counterpart is
/// `transitive_scoped_dep_builds_and_links_both_archives` in
/// `cli/registry_ports.rs`.
#[test]
#[ignore = "requires external network"]
fn libpng_usage_builds_and_runs() {
    // libpng and zlib are both C; the consumer is C too.
    require_c_and_cxx_build_tools();
    let dir = copy_example("libpng-usage");
    run_port_build_then_run(&PortBuildRun {
        label: "libpng-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["libpng version: 1.6.59", "zlib version: 1.3"],
    });
}

#[test]
#[ignore = "requires external network"]
fn fmt_usage_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("fmt-usage");
    // `FMT_VERSION` is a compile-time constant of the pinned release,
    // and the formatted greeting proves `fmt::format` linked from the
    // compiled library rather than an arbitrary symbol.
    run_port_build_then_run(&PortBuildRun {
        label: "fmt-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["fmt version: 120200", "Hello, Cabin!"],
    });
}

#[test]
#[ignore = "requires external network"]
fn spdlog_usage_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("spdlog-usage");
    // The `[info]` log line proves the header-only sink machinery
    // works (its timestamp prefix stays unasserted); the version line
    // is a compile-time constant of the pinned release.
    run_port_build_then_run(&PortBuildRun {
        label: "spdlog-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["[info] Hello from spdlog!", "spdlog version: 1.17.0"],
    });
}

#[test]
#[ignore = "requires external network"]
fn googletest_usage_runs_tests() {
    require_cxx_build_tools();
    let dir = copy_example("googletest-usage");
    // `cabin test` fetches the package, builds the test target
    // against it, and runs the produced binary; the package ships no
    // gtest_main, so the passing run also proves the example's own
    // `main` linked against the published archive.
    let output = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--index-path")
        .arg(pinned_ports_registry())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    for expected in [
        "test googletest-usage:calc_gtest ... ok",
        "test result: ok. 1 passed; 0 failed;",
    ] {
        assert!(
            stdout.contains(expected),
            "googletest-usage test: missing `{expected}`; stdout = {stdout}"
        );
    }
}

#[test]
#[ignore = "requires external network"]
fn catch2_usage_runs_tests() {
    require_cxx_build_tools();
    let dir = copy_example("catch2-usage");
    // `cabin test` fetches the package, builds the test target, and
    // runs it; the passing run proves the amalgamated TU's default
    // main() drove the TEST_CASEs (the test source defines no main).
    let output = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--index-path")
        .arg(pinned_ports_registry())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    for expected in [
        "test catch2-usage:calc_catch2 ... ok",
        "test result: ok. 1 passed; 0 failed;",
    ] {
        assert!(
            stdout.contains(expected),
            "catch2-usage test: missing `{expected}`; stdout = {stdout}"
        );
    }
}

#[test]
#[ignore = "requires external network"]
fn nlohmann_json_usage_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("nlohmann-json-usage");
    run_port_build_then_run(&PortBuildRun {
        label: "nlohmann-json-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &[
            "json parsed name: Cabin",
            "json parsed answer: 42",
            "nlohmann_json version: 3.12.0",
        ],
    });
}

#[test]
#[ignore = "requires external network"]
fn cli11_usage_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("cli11-usage");
    // `cabin run` passes no arguments, so the printed count is the
    // declared default - proving CLI11's parser ran and left it be.
    run_port_build_then_run(&PortBuildRun {
        label: "cli11-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["CLI11 parsed count: 3", "CLI11 version: 2.6.2"],
    });
}

#[test]
#[ignore = "requires external network"]
fn miniz_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("miniz-usage");
    // The compress/uncompress roundtrip proves real DEFLATE code
    // linked from the zip-sourced amalgamation, not just a symbol.
    // (`mz_version()` reports miniz's internal zlib-style version,
    // which intentionally differs from the 3.1.2 release number.)
    run_port_build_then_run(&PortBuildRun {
        label: "miniz-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &[
            "miniz roundtrip: Cabin compresses with miniz",
            "miniz version: 11.3.2",
        ],
    });
}

#[test]
#[ignore = "requires external network"]
fn stb_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("stb-usage");
    // The formatted string proves the STB_SPRINTF_IMPLEMENTATION
    // pattern compiled the function bodies into the consumer TU.
    run_port_build_then_run(&PortBuildRun {
        label: "stb-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["stb_sprintf formatted: Cabin scores 42"],
    });
}

#[test]
#[ignore = "requires external network"]
fn uthash_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("uthash-usage");
    // The passing run proves the real uthash tarball - whose root
    // carries an `include -> src` symlink entry - staged cleanly
    // under the skip-symlinks port extraction policy before
    // publishing, and the published package builds.
    run_port_build_then_run(&PortBuildRun {
        label: "uthash-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["uthash lookup: cabin = 42", "uthash version: 2.4.0"],
    });
}

#[test]
#[ignore = "requires external network"]
fn inih_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("inih-usage");
    // The two parsed values prove ini.c's handler dispatch linked
    // and ran from the published archive.
    run_port_build_then_run(&PortBuildRun {
        label: "inih-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &["inih parsed name: Cabin", "inih parsed port: 8080"],
    });
}

#[test]
#[ignore = "requires external network"]
fn picohttpparser_usage_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("picohttpparser-usage");
    // The parsed method/path/header-count triple proves
    // phr_parse_request linked from the published archive and ran.
    run_port_build_then_run(&PortBuildRun {
        label: "picohttpparser-usage",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &[
            "picohttpparser method: GET",
            "picohttpparser path: /hello",
            "picohttpparser headers: 1",
        ],
    });
}

/// End-to-end proof that the `custom-main` feature flows to the
/// package's translation unit: with CATCH_AMALGAMATED_CUSTOM_MAIN the
/// amalgamation compiles out its default main(), so the consumer's
/// own entry point links without a duplicate-symbol error and drives
/// the TEST_CASEs through Catch::Session.
#[test]
#[ignore = "requires external network"]
fn catch2_custom_main_feature_links_consumer_main() {
    require_cxx_build_tools();
    let dir = copy_example("catch2-usage");
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "catch2-usage"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
"cabin-ports/catch2" = { version = "=3.15.1", features = ["custom-main"] }

[target.calc]
type = "library"
sources = ["src/calc.cc"]
include-dirs = ["include"]

[target.calc_catch2]
type = "test"
sources = ["tests/calc_catch2.cc"]
deps = ["calc", "cabin-ports/catch2"]
"#,
        )
        .unwrap();
    dir.child("tests/calc_catch2.cc")
        .write_str(
            r#"#include <catch_amalgamated.hpp>

#include "calc.h"

TEST_CASE("triple scales integers") { REQUIRE(triple(2) == 6); }

int main(int argc, char* argv[]) {
    return Catch::Session().run(argc, argv);
}
"#,
        )
        .unwrap();
    let output = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--index-path")
        .arg(pinned_ports_registry())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("test result: ok. 1 passed; 0 failed;"),
        "catch2-usage custom-main test: stdout = {stdout}"
    );
}

#[test]
#[ignore = "requires external network"]
fn cli_with_spdlog_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("cli-with-spdlog");
    // `cabin run` passes no flags, so the printed lines are the CLI11
    // defaults; the `[info]` line proves spdlog's sink ran, and the
    // external-fmt version line proves SPDLOG_FMT_EXTERNAL compiled
    // spdlog against the external fmt package instead of its
    // bundled copy.
    run_port_build_then_run(&PortBuildRun {
        label: "cli-with-spdlog",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &[
            "[info] preparing 2 greeting(s) for Cabin",
            "1/2: Hello, Cabin!",
            "2/2: Hello, Cabin!",
            "spdlog version: 1.17.0",
            "fmt version (external): 120200",
        ],
    });
}

#[test]
#[ignore = "requires external network"]
fn unit_test_gtest_runs_tests() {
    require_cxx_build_tools();
    let dir = copy_example("unit-test-gtest");
    // `cabin test` fetches the package, builds the `stats` library
    // and the gtest target against it, and runs the produced binary; the
    // TEST_F/TEST cases (fixture, value, and exception assertions)
    // all pass inside the single target run.
    let output = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--index-path")
        .arg(pinned_ports_registry())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    for expected in [
        "test unit-test-gtest:stats_gtest ... ok",
        "test result: ok. 1 passed; 0 failed;",
    ] {
        assert!(
            stdout.contains(expected),
            "unit-test-gtest test: missing `{expected}`; stdout = {stdout}"
        );
    }
}

#[test]
#[ignore = "requires external network"]
fn json_cli_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("json-cli");
    // The summary line proves the full round trip: parse, typed
    // reads, and re-serialization (nlohmann::json keeps object keys
    // sorted, so the dump is deterministic).
    run_port_build_then_run(&PortBuildRun {
        label: "json-cli",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &[
            "package: json-cli v0.1.0",
            "dependency count: 3",
            r#"summary: {"deps":["fmt","spdlog","sqlite3"],"name":"json-cli"}"#,
        ],
    });
}

#[test]
#[ignore = "requires external network"]
fn sqlite_todo_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("sqlite-todo");
    // The listing proves the whole in-memory session ran: DDL and
    // DML through sqlite3_exec (including the UPDATE that checks off
    // todo #1), then a prepare/step/finalize SELECT loop.
    run_port_build_then_run(&PortBuildRun {
        label: "sqlite-todo",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &[
            "[x] #1 write the manifest",
            "[ ] #2 add a lockfile",
            "[ ] #3 ship v0.1.0",
            "open todos: 2",
        ],
    });
}

#[test]
#[ignore = "requires external network"]
fn png_info_builds_and_runs() {
    require_c_and_cxx_build_tools();
    let dir = copy_example("png-info");
    // The matching roundtrip proves real DEFLATE data flowed through
    // the transitive libpng -> zlib registry edge in both directions;
    // the encoded byte count varies with zlib, so it stays
    // unasserted.
    run_port_build_then_run(&PortBuildRun {
        label: "png-info",
        manifest: dir.path().join("cabin.toml"),
        build_dir: dir.path().join("build"),
        cache_dir: dir.path().join("cache"),
        index_path: Some(pinned_ports_registry().to_path_buf()),
        expected_stdout: &[
            "png-info: 2x2, 4 channel(s)",
            "roundtrip pixels match: yes",
            "libpng version: 1.6.59",
        ],
    });
}

#[test]
#[ignore = "requires external network"]
fn workspace_app_and_lib_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("workspace-app-and-lib");
    cabin()
        .args(["build", "--workspace", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--index-path")
        .arg(pinned_ports_registry())
        .assert()
        .success();
    // `default-members = ["packages/app"]` selects the app without
    // `-p`. The fmt-formatted greeting proves the fmt archive reached
    // `app`'s link transitively through the internal `greeter` path
    // dependency.
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--index-path")
        .arg(pinned_ports_registry())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(
        stdout.contains("Hello, Cabin! (formatted by fmt 120200)"),
        "workspace-app-and-lib run: stdout = {stdout}"
    );
}

#[test]
fn feature_gated_targets_builds_and_runs() {
    require_cxx_build_tools();
    let dir = copy_example("feature-gated-targets");
    // `--workspace` builds netlib too; the `tls` feature enabled on
    // app's dependency edge makes the gated `netlib:tls` target
    // buildable, and app's explicit `deps` entries link it.
    cabin()
        .args(["build", "--workspace", "--manifest-path"])
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
        stdout.contains("TLS GET example.org"),
        "feature-gated-targets run: stdout = {stdout}"
    );
    // With only netlib's default (empty) feature set, the gated
    // target is skipped rather than built.
    cabin()
        .args(["build", "-p", "netlib", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build2"))
        .assert()
        .success();
    // Archive naming is dialect-specific (`libtls.a` vs `tls.lib`);
    // assert on both spellings so the check holds on MSVC too.
    for name in ["libtls.a", "tls.lib"] {
        let gated_lib = dir.path().join("build2/dev/packages/netlib").join(name);
        assert!(
            !gated_lib.exists(),
            "gated target must not build without its required feature: {}",
            gated_lib.display()
        );
    }
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

// ---------------------------------------------------------------
// PROBE ONLY, never merged: `/bigobj` on this runner's toolchain.
// `CABIN_PROBE_NO_BIGOBJ` (probe-only switch in cabin-driver) drops
// the flag so one binary builds both ways.
// ---------------------------------------------------------------

fn probe_walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            probe_walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Total bytes of the files under `dir` with extension `ext`.
fn probe_bytes(dir: &Path, ext: &str) -> (usize, u64) {
    let mut files = Vec::new();
    probe_walk(dir, &mut files);
    let matching: Vec<_> = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == ext))
        .collect();
    let bytes = matching
        .iter()
        .map(|p| std::fs::metadata(p).map_or(0, |m| m.len()))
        .sum();
    (matching.len(), bytes)
}

struct ProbeRun {
    ok: bool,
    secs: f64,
    text: String,
}

fn probe_build(root: &Path, release: bool, bigobj: bool) -> ProbeRun {
    let mut cmd = cabin();
    cmd.args(["-v", "build", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .arg("--build-dir")
        .arg(root.join("build"));
    if release {
        cmd.arg("--release");
    }
    if !bigobj {
        cmd.env("CABIN_PROBE_NO_BIGOBJ", "1");
    }
    let start = std::time::Instant::now();
    let output = cmd.output().expect("spawn cabin");
    let secs = start.elapsed().as_secs_f64();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    ProbeRun {
        ok: output.status.success(),
        secs,
        text,
    }
}

fn probe_write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A package with one translation unit of `count` functions, each in
/// its own COMDAT where the compiler makes one (`inline` in C++;
/// function-level linking in an optimized C build).
fn probe_many_functions(cxx: bool, count: usize) -> TempDir {
    let dir = TempDir::new().unwrap();
    let (ext, inline) = if cxx { ("cc", "inline ") } else { ("c", "") };
    probe_write(
        dir.path(),
        "cabin.toml",
        &format!(
            r#"[package]
name = "many"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.many]
type = "executable"
sources = ["src/main.{ext}"]
"#
        ),
    );
    let mut source = String::from("#include <stdio.h>\n");
    for i in 0..count {
        source.push_str(&format!("{inline}int f{i}(int x) {{ return x + {i}; }}\n"));
    }
    // External linkage and a run-time index keep an optimizing
    // compiler from dropping the functions.
    source.push_str("extern int (*const table[])(int);\nint (*const table[])(int) = {\n");
    for i in 0..count {
        source.push_str(&format!("f{i},\n"));
    }
    source.push_str("};\n");
    source.push_str(&format!(
        "int main(int argc, char **argv) {{ (void)argv; printf(\"last=%d\\n\", table[argc + {}](1)); return 0; }}\n",
        count - 2
    ));
    probe_write(dir.path(), &format!("src/main.{ext}"), &source);
    dir
}

/// A translation unit that instantiates a fair amount of the standard
/// library, for a visible compile and link cost.
fn probe_stl() -> TempDir {
    let dir = TempDir::new().unwrap();
    probe_write(
        dir.path(),
        "cabin.toml",
        r#"[package]
name = "stl"
version = "0.1.0"
cxx-standard = "c++17"

[target.text]
type = "library"
sources = ["src/text.cc"]
include-dirs = ["include"]

[target.stl]
type = "executable"
sources = ["src/main.cc"]
deps = ["text"]
"#,
    );
    probe_write(
        dir.path(),
        "include/text.h",
        "#include <map>\n#include <string>\n#include <vector>\nstd::map<std::string, int> count_words(const std::vector<std::string>& lines);\n",
    );
    probe_write(
        dir.path(),
        "src/text.cc",
        r#"#include "text.h"
#include <algorithm>
#include <regex>
#include <sstream>
std::map<std::string, int> count_words(const std::vector<std::string>& lines) {
    std::map<std::string, int> counts;
    const std::regex word("[A-Za-z]+");
    for (const auto& line : lines) {
        for (std::sregex_iterator it(line.begin(), line.end(), word), end; it != end; ++it) {
            std::string w = it->str();
            std::transform(w.begin(), w.end(), w.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
            ++counts[w];
        }
    }
    return counts;
}
"#,
    );
    probe_write(
        dir.path(),
        "src/main.cc",
        r#"#include "text.h"
#include <iostream>
#include <memory>
#include <unordered_map>
int main() {
    auto lines = std::make_unique<std::vector<std::string>>();
    lines->push_back("The quick brown fox");
    lines->push_back("jumps over the lazy dog");
    std::unordered_map<std::string, int> seen;
    for (const auto& [word, n] : count_words(*lines)) {
        seen[word] += n;
    }
    std::cout << "the=" << seen["the"] << " words=" << seen.size() << "\n";
}
"#,
    );
    dir
}

fn probe_remove(root: &Path, ext: Option<&str>) {
    let build = root.join("build");
    match ext {
        None => {
            let _ = std::fs::remove_dir_all(&build);
        }
        Some(ext) => {
            let mut files = Vec::new();
            probe_walk(&build, &mut files);
            for file in files {
                if file.extension().is_some_and(|e| e == ext) {
                    std::fs::remove_file(file).unwrap();
                }
            }
        }
    }
}

#[test]
fn probe_bigobj() {
    if !cfg!(windows) && std::env::var_os("PROBE_LOCAL").is_none() {
        return;
    }
    require_c_and_cxx_build_tools();
    let exe_ext = if cfg!(windows) { "exe" } else { "" };
    let obj_ext = host_obj_ext();
    let lib_ext = if cfg!(windows) { "lib" } else { "a" };
    println!(
        "PROBE arch={} os={}",
        std::env::consts::ARCH,
        std::env::consts::OS
    );

    // 1. The section limit: a 70,000-function translation unit.
    for cxx in [true, false] {
        let lang = if cxx { "c++" } else { "c" };
        for release in [false, true] {
            let profile = if release { "release" } else { "dev" };
            let dir = probe_many_functions(cxx, 70_000);
            let without = probe_build(dir.path(), release, false);
            let c1128 = without.text.contains("C1128");
            println!(
                "PROBE limit lang={lang} profile={profile} flag=off ok={} c1128={c1128} secs={:.1}",
                without.ok, without.secs
            );
            if !without.ok && !c1128 {
                println!(
                    "PROBE unexpected failure without the flag:\n{}",
                    without.text
                );
            }
            // Same build directory: nothing from the failed or
            // flagless build may satisfy this one.
            let with = probe_build(dir.path(), release, true);
            let compile_line = with
                .text
                .lines()
                .find(|l| l.contains("main.") && (l.contains("/c ") || l.contains(" -c ")))
                .unwrap_or("<no compile line>");
            println!(
                "PROBE limit lang={lang} profile={profile} flag=on ok={} secs={:.1} no_work={}",
                with.ok,
                with.secs,
                with.text.contains("ninja: no work to do.")
            );
            println!(
                "PROBE limit compile line: {}",
                &compile_line[..compile_line.len().min(400)]
            );
            assert!(with.ok, "build with /bigobj failed:\n{}", with.text);
            let mut files = Vec::new();
            probe_walk(&dir.path().join("build"), &mut files);
            let exe = files
                .iter()
                .find(|p| {
                    p.file_stem().is_some_and(|s| s == "many")
                        && p.extension().map_or(exe_ext.is_empty(), |e| e == exe_ext)
                })
                .expect("built executable");
            let stdout = run_artifact(exe, "many");
            println!(
                "PROBE limit lang={lang} profile={profile} run: {}",
                stdout.trim()
            );
            assert_eq!(stdout.trim(), "last=70000");
            let (objs, obj_bytes) = probe_bytes(&dir.path().join("build"), obj_ext);
            println!("PROBE limit lang={lang} profile={profile} objects={objs} bytes={obj_bytes}");
        }
    }

    // 2. Stale objects: a flag change must recompile.
    let dir = copy_example("library-and-app");
    let header = |root: &Path| {
        let mut files = Vec::new();
        probe_walk(&root.join("build"), &mut files);
        let mut heads: Vec<String> = files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == obj_ext))
            .map(|p| {
                let bytes = std::fs::read(p).unwrap();
                format!(
                    "{}:{:02x?}",
                    p.file_name().unwrap().to_string_lossy(),
                    &bytes[..bytes.len().min(8)]
                )
            })
            .collect();
        heads.sort();
        heads.join(" ")
    };
    let off = probe_build(dir.path(), false, false);
    assert!(off.ok, "{}", off.text);
    println!("PROBE stale flag=off headers: {}", header(dir.path()));
    let off_again = probe_build(dir.path(), false, false);
    println!(
        "PROBE stale flag=off again no_work={}",
        off_again.text.contains("ninja: no work to do.")
    );
    let on = probe_build(dir.path(), false, true);
    assert!(on.ok, "{}", on.text);
    println!(
        "PROBE stale flag=on no_work={} headers: {}",
        on.text.contains("ninja: no work to do."),
        header(dir.path())
    );
    let on_again = probe_build(dir.path(), false, true);
    println!(
        "PROBE stale flag=on again no_work={}",
        on_again.text.contains("ninja: no work to do.")
    );

    // 2b. A user's own `/bigobj` next to Cabin's: does the compiler object?
    if cfg!(windows) {
        let dir = copy_example("hello-cpp");
        let output = cabin()
            .args(["-v", "build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .env("CXXFLAGS", "/bigobj")
            .output()
            .expect("spawn cabin");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let line = text.lines().find(|l| l.contains("/bigobj")).unwrap_or("");
        let diagnostics: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("warning") || l.contains("D90") || l.contains("error"))
            .collect();
        println!(
            "PROBE repeat ok={} count={} diagnostics={diagnostics:?}",
            output.status.success(),
            line.matches("/bigobj").count()
        );
    }

    // 3. Time and size, with and without the flag.
    let stl = probe_stl();
    let hello_c = copy_example("hello-c");
    let lib_app = copy_example("library-and-app");
    for (name, root) in [
        ("stl", stl.path()),
        ("hello-c", hello_c.path()),
        ("library-and-app", lib_app.path()),
    ] {
        for release in [false, true] {
            let profile = if release { "release" } else { "dev" };
            for bigobj in [false, true] {
                let flag = if bigobj { "on" } else { "off" };
                let (mut clean, mut relink, mut noop) = (Vec::new(), Vec::new(), Vec::new());
                for _ in 0..3 {
                    probe_remove(root, None);
                    let run = probe_build(root, release, bigobj);
                    assert!(run.ok, "{}", run.text);
                    clean.push(run.secs);
                    noop.push(probe_build(root, release, bigobj).secs);
                    if !exe_ext.is_empty() {
                        probe_remove(root, Some(exe_ext));
                        let run = probe_build(root, release, bigobj);
                        assert!(run.ok, "{}", run.text);
                        relink.push(run.secs);
                    }
                }
                let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
                let build = root.join("build");
                let (objs, obj_bytes) = probe_bytes(&build, obj_ext);
                let (_, lib_bytes) = probe_bytes(&build, lib_ext);
                let (_, exe_bytes) = probe_bytes(&build, exe_ext);
                println!(
                    "PROBE cost target={name} profile={profile} flag={flag} clean_min={:.2} clean_all={clean:.2?} relink_min={:.2} noop_min={:.2} objects={objs} obj_bytes={obj_bytes} lib_bytes={lib_bytes} exe_bytes={exe_bytes}",
                    min(&clean),
                    min(&relink),
                    min(&noop),
                );
            }
        }
    }

    // 4. Compilers Cabin did not drive above, compile only: the
    // ARM64-targeting `cl` and `clang-cl`.
    if cfg!(windows) {
        let dir = probe_many_functions(true, 70_000);
        let src = dir.path().join("src").join("main.cc");
        let where_cl = std::process::Command::new("where")
            .arg("cl")
            .output()
            .expect("where cl");
        let cl = PathBuf::from(
            String::from_utf8_lossy(&where_cl.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim(),
        );
        // `...\bin\Host<host>\<target>\cl.exe`
        let host_dir = cl.parent().and_then(Path::parent).unwrap_or(Path::new(""));
        let bin_dir = host_dir.parent().unwrap_or(Path::new(""));
        let compilers = [
            ("cl-samehost-arm64", host_dir.join("arm64").join("cl.exe")),
            (
                "cl-hostarm64-arm64",
                bin_dir.join("HostARM64").join("arm64").join("cl.exe"),
            ),
            ("clang-cl-path", PathBuf::from("clang-cl")),
            (
                "clang-cl-llvm",
                PathBuf::from(r"C:\Program Files\LLVM\bin\clang-cl.exe"),
            ),
        ];
        for (name, exe) in compilers {
            if name.starts_with("clang") {
                if let Ok(v) = std::process::Command::new(&exe).arg("--version").output() {
                    let text = String::from_utf8_lossy(&v.stdout);
                    let lines: Vec<&str> = text.lines().take(2).collect();
                    println!("PROBE direct compiler={name} version={lines:?}");
                }
            }
            for flag in [false, true] {
                let obj = dir.path().join(format!("{name}-{flag}.obj"));
                let mut cmd = std::process::Command::new(&exe);
                cmd.args(["/nologo", "/c", "/std:c++17", "/EHsc"]);
                if flag {
                    cmd.arg("/bigobj");
                }
                cmd.arg(format!("/Tp{}", src.display()))
                    .arg(format!("/Fo{}", obj.display()));
                match cmd.output() {
                    Err(e) => println!("PROBE direct compiler={name} flag={flag} spawn-error={e}"),
                    Ok(o) => {
                        let text = format!(
                            "{}{}",
                            String::from_utf8_lossy(&o.stdout),
                            String::from_utf8_lossy(&o.stderr)
                        );
                        let head = std::fs::read(&obj).map_or_else(
                            |_| "none".to_owned(),
                            |b| format!("{:02x?}", &b[..b.len().min(8)]),
                        );
                        let notes: Vec<&str> = text
                            .lines()
                            .filter(|l| l.contains("warning") || l.contains("error"))
                            .take(3)
                            .collect();
                        println!(
                            "PROBE direct compiler={name} exe={} flag={flag} ok={} header={head} notes={notes:?}",
                            exe.display(),
                            o.status.success()
                        );
                    }
                }
            }
        }
    }
}
