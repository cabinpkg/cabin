use super::*;
use std::path::PathBuf;

/// `main.cc` that prints argv (one arg per line) plus the
/// injected `CABIN_PACKAGE_NAME` / `CABIN_PROFILE` env vars
/// so a single run can confirm `--` arg forwarding and the
/// `CABIN_*` overlay together.
const ARGV_AND_ENV_MAIN_CC: &str = r#"
#include <cstdio>
#include <cstdlib>
int main(int argc, char** argv) {
    for (int i = 1; i < argc; ++i) {
        std::printf("ARG %d %s\n", i, argv[i]);
    }
    if (const char* p = std::getenv("CABIN_PACKAGE_NAME")) {
        std::printf("PKG %s\n", p);
    }
    if (const char* p = std::getenv("CABIN_PROFILE")) {
        std::printf("PROFILE %s\n", p);
    }
    return 0;
}
"#;

fn write_run_fixture(root: &Path, package: &str, target: &str) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "{package}"
version = "0.1.0"

[target.{target}]
type = "executable"
sources = ["src/main.cc"]
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
        .write_str(ARGV_AND_ENV_MAIN_CC)
        .unwrap();
}

#[test]
fn run_executes_default_binary_and_forwards_trailing_args() {
    if !ninja_available() || !cxx_compiler_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    write_run_fixture(dir.path(), "demo", "demo_app");
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .args(["--", "first", "second"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ARG 1 first"), "got: {stdout}");
    assert!(stdout.contains("ARG 2 second"), "got: {stdout}");
    assert!(stdout.contains("PKG demo"), "got: {stdout}");
    assert!(stdout.contains("PROFILE dev"), "got: {stdout}");
}

#[test]
fn run_does_not_materialize_dev_dependencies() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[dev-dependencies]
test_only = "1.0.0"

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc")
        .write_str(ARGV_AND_ENV_MAIN_CC)
        .unwrap();

    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("PKG demo"), "got: {stdout}");
}

#[test]
fn run_with_bin_flag_picks_named_target() {
    if !ninja_available() || !cxx_compiler_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    // Two executable targets with distinct sources;
    // --bin selects which one runs.
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "two-bins"
version = "0.1.0"

[target.alpha]
type = "executable"
sources = ["src/alpha.cc"]

[target.beta]
type = "executable"
sources = ["src/beta.cc"]
"#,
        )
        .unwrap();
    dir.child("src/alpha.cc")
        .write_str("#include <cstdio>\nint main() { std::printf(\"WHICH alpha\\n\"); return 0; }\n")
        .unwrap();
    dir.child("src/beta.cc")
        .write_str("#include <cstdio>\nint main() { std::printf(\"WHICH beta\\n\"); return 0; }\n")
        .unwrap();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .args(["--bin", "beta"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("WHICH beta"), "got: {stdout}");
}

#[test]
fn run_in_pure_workspace_omits_fingerprint_env() {
    if !ninja_available() || !cxx_compiler_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[features]
default = []
fast = []

[target.app]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("packages/app/src/main.cc")
        .write_str(ARGV_AND_ENV_MAIN_CC)
        .unwrap();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .args(["--package", "app", "--features", "fast"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("PKG app"),
        "the selected package's binary should have run: {stdout}"
    );
    assert!(
        !stdout.contains("FINGERPRINT"),
        "CABIN_BUILD_CONFIGURATION_FINGERPRINT must not be injected: {stdout}"
    );
}

#[test]
fn run_without_bin_when_multiple_executables_errors() {
    if !ninja_available() || !cxx_compiler_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "ambiguous"
version = "0.1.0"

[target.alpha]
type = "executable"
sources = ["src/main.cc"]

[target.beta]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
    let assertion = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("multiple `executable` targets"),
        "expected ambiguous-executable error, got: {stderr}"
    );
}

#[test]
fn run_unknown_bin_returns_actionable_error() {
    if !ninja_available() || !cxx_compiler_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    write_run_fixture(dir.path(), "demo", "demo_app");
    let assertion = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .args(["--bin", "missing"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("`missing`") && stderr.contains("not found"),
        "expected --bin missing error, got: {stderr}"
    );
}

#[test]
fn run_for_c_executable_target() {
    if !ninja_available() || !c_compiler_available() {
        eprintln!("test skipped: requires ninja + a C compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "c-app"
version = "0.1.0"

[target.c_app]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    dir.child("src/main.c")

            .write_str("#include <stdio.h>\nint main(int argc, char** argv) { (void)argc; (void)argv; puts(\"c-ok\"); return 0; }\n")

            .unwrap();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("c-ok"), "got: {stdout}");
}

#[test]
fn run_for_mixed_c_and_cpp_executable_target() {
    if !ninja_available() || !cxx_compiler_available() || !c_compiler_available() {
        eprintln!("test skipped: requires ninja + C/C++ compilers");
        return;
    }
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "mixed-app"
version = "0.1.0"

[target.mixed_app]
type = "executable"
sources = ["src/main.cc", "src/util.c"]
"#,
        )
        .unwrap();
    dir.child("src/util.c")
        .write_str("int util_value(void) { return 42; }\n")
        .unwrap();
    dir.child("src/main.cc")
        .write_str(
            r#"#include <cstdio>
extern "C" int util_value();
int main() { std::printf("util=%d\n", util_value()); return 0; }
"#,
        )
        .unwrap();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("util=42"), "got: {stdout}");
}

#[test]
fn cabin_build_dir_env_var_overrides_default_directory() {
    if !ninja_available() || !cxx_compiler_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    write_run_fixture(dir.path(), "envbin", "envbin");
    let custom_dir: PathBuf = dir.path().join("custom-build");
    cabin()
        .env("CABIN_BUILD_DIR", &custom_dir)
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    assert!(
        custom_dir.join("dev/build.ninja").is_file(),
        "expected build.ninja under {}",
        custom_dir.display()
    );
    assert!(
        !dir.path().join("build/dev/build.ninja").is_file(),
        "default `build/` should not have been created"
    );
}

#[test]
fn cli_build_dir_flag_wins_over_cabin_build_dir_env() {
    if !ninja_available() || !cxx_compiler_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    write_run_fixture(dir.path(), "winflag", "winflag");
    let env_dir = dir.path().join("env-build");
    let cli_dir = dir.path().join("cli-build");
    cabin()
        .env("CABIN_BUILD_DIR", &env_dir)
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(&cli_dir)
        .assert()
        .success();
    assert!(
        cli_dir.join("dev/build.ninja").is_file(),
        "CLI flag must win — expected build.ninja under {}",
        cli_dir.display()
    );
    assert!(
        !env_dir.exists(),
        "env-supplied build dir should not have been used"
    );
}

/// An explicit `--build-dir build` that happens to spell the
/// built-in default literal must still beat `CABIN_BUILD_DIR`.
/// Without value-source-aware detection the precedence check
/// can't tell the user-supplied value from the clap default
/// and silently routes outputs to the env-supplied directory.
#[test]
fn cli_build_dir_default_literal_wins_over_cabin_build_dir_env() {
    if !ninja_available() || !cxx_compiler_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    write_run_fixture(dir.path(), "explicit_default", "explicit_default");
    let env_dir = dir.path().join("env-build");
    cabin()
        .current_dir(dir.path())
        .env("CABIN_BUILD_DIR", &env_dir)
        .args(["build", "--build-dir", "build"])
        .assert()
        .success();
    assert!(
        dir.path().join("build/dev/build.ninja").is_file(),
        "explicit `--build-dir build` must win — expected build.ninja under build/dev/"
    );
    assert!(
        !env_dir.exists(),
        "env-supplied build dir should not have been used"
    );
}

#[test]
fn cabin_net_offline_env_var_blocks_url_index() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "needs-fmt"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .env("CABIN_NET_OFFLINE", "1")
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg("https://example.com/index")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("--offline forbids network access"),
        "expected offline diagnostic, got: {stderr}"
    );
}

#[test]
fn invalid_cabin_net_offline_env_value_is_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "needs-fmt"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    dir.child("index/fmt.json").write_str(FMT_INDEX).unwrap();
    let assertion = cabin()
        .env("CABIN_NET_OFFLINE", "nope")
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("CABIN_NET_OFFLINE") && stderr.contains("nope"),
        "invalid offline env diagnostic should name the variable and value: {stderr}"
    );
}

#[test]
fn cabin_run_exits_with_program_status_code() {
    if !ninja_available() || !cxx_compiler_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "exit-code"
version = "0.1.0"

[target.exit_code]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc")
        .write_str("int main() { return 42; }\n")
        .unwrap();
    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure()
        .get_output()
        .clone();
    assert_eq!(output.status.code(), Some(42));
}
