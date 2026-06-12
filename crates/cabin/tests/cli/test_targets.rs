//! End-to-end coverage for `test` / `example` target kinds and
//! the `cabin test` command.

use super::*;

/// Single-package fixture with one library plus one passing test
/// target. Returns the temp dir guard so the caller can drive
/// commands against it.
fn passing_test_project() -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "library"
sources = ["src/lib.cc"]

[target.demo_test]
type = "test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]
"#,
        )
        .unwrap();
    dir.child("src/lib.cc")
        .write_str("int demo() { return 42; }\n")
        .unwrap();
    dir.child("tests/lib_test.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    dir
}

fn project_with_dev_kinds() -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "library"
sources = ["src/lib.cc"]

[target.demo_test]
type = "test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]

[target.hello_example]
type = "example"
sources = ["examples/hello.cc"]
deps = ["demo"]
"#,
        )
        .unwrap();
    dir.child("src/lib.cc")
        .write_str("int demo() { return 1; }\n")
        .unwrap();
    dir.child("tests/lib_test.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    dir.child("examples/hello.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    dir
}

#[test]
fn metadata_lists_test_and_example_target_kinds() {
    let dir = project_with_dev_kinds();
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let demo = package_in(&value, "demo");
    let kinds: std::collections::BTreeMap<String, String> = demo["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap().to_owned(),
                t["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(kinds.get("demo").map(String::as_str), Some("library"));
    assert_eq!(kinds.get("demo_test").map(String::as_str), Some("test"));
    assert_eq!(
        kinds.get("hello_example").map(String::as_str),
        Some("example")
    );
}

#[test]
fn invalid_target_kind_is_rejected_with_helpful_message() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[target.broken]
type = "cpp_tests"
sources = ["src/x.cc"]
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    // Wording is stable: enumerate the supported kinds so the
    // user can correct the typo without reading docs.
    assert!(
        stderr.contains("\"test\"")
            && stderr.contains("\"library\"")
            && stderr.contains("\"executable\"")
            && stderr.contains("\"header-only\"")
            && stderr.contains("\"example\""),
        "expected target-type error mentioning the supported kinds, got: {stderr}"
    );
}

#[test]
fn build_default_does_not_build_dev_only_targets() {
    require_cxx_build_tools();
    let dir = project_with_dev_kinds();
    // `-v` keeps Ninja's `[N/M] AR / CXX / LINK …` progress
    // lines on stdout so the assertion below can pin the
    // archive action.  At the default verbosity the lines
    // are filtered to match cargo's terser banner shape.
    let assertion = cabin()
        .args(["build", "-v", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    // The library object/archive must build; the dev-only
    // targets must NOT appear in the ninja output.
    assert!(
        stdout.contains("AR"),
        "library archive should build: {stdout}"
    );
    for forbidden in ["demo_test", "hello_example"] {
        assert!(
            !stdout.contains(forbidden),
            "default build must not produce {forbidden}: {stdout}"
        );
    }
}

#[test]
fn cabin_test_builds_and_runs_passing_test() {
    require_cxx_build_tools();
    let dir = passing_test_project();
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("test demo:demo_test ... ok"),
        "expected per-test result line, got: {stdout}"
    );
    // The summary line carries the full `cargo test` field
    // shape; only the trailing wall-clock time is variable.
    assert!(
        stdout.contains(
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in"
        ),
        "expected passing summary, got: {stdout}"
    );
}

/// Single-package fixture with one library and three passing
/// test targets, for `--test <NAME>` selection coverage.
fn three_test_project() -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "library"
sources = ["src/lib.cc"]

[target.alpha_test]
type = "test"
sources = ["tests/alpha_test.cc"]
deps = ["demo"]

[target.beta_test]
type = "test"
sources = ["tests/beta_test.cc"]
deps = ["demo"]

[target.gamma_test]
type = "test"
sources = ["tests/gamma_test.cc"]
deps = ["demo"]
"#,
        )
        .unwrap();
    dir.child("src/lib.cc")
        .write_str("int demo() { return 42; }\n")
        .unwrap();
    for name in ["alpha_test", "beta_test", "gamma_test"] {
        dir.child(format!("tests/{name}.cc"))
            .write_str("int main() { return 0; }\n")
            .unwrap();
    }
    dir
}

#[test]
fn cabin_test_runs_only_the_named_test_target() {
    require_cxx_build_tools();
    let dir = three_test_project();
    let assertion = cabin()
        .args(["test", "--test", "beta_test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("running 1 test"),
        "expected single-test header, got: {stdout}"
    );
    assert!(
        stdout.contains("test demo:beta_test ... ok"),
        "expected the named test to run, got: {stdout}"
    );
    for absent in ["test demo:alpha_test", "test demo:gamma_test"] {
        assert!(
            !stdout.contains(absent),
            "deselected test must not run ({absent}): {stdout}"
        );
    }
    // The two deselected test targets surface in the summary's
    // `filtered out` field.
    assert!(
        stdout
            .contains("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out"),
        "expected filtered-out count in summary, got: {stdout}"
    );
}

#[test]
fn cabin_test_repeated_test_flags_run_each_named_target() {
    require_cxx_build_tools();
    let dir = three_test_project();
    let assertion = cabin()
        .args([
            "test",
            "--test",
            "alpha_test",
            "--test",
            "gamma_test",
            "--manifest-path",
        ])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    for expected in ["test demo:alpha_test ... ok", "test demo:gamma_test ... ok"] {
        assert!(
            stdout.contains(expected),
            "expected `{expected}`, got: {stdout}"
        );
    }
    assert!(
        !stdout.contains("test demo:beta_test"),
        "deselected test must not run: {stdout}"
    );
    assert!(
        stdout
            .contains("test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out"),
        "expected filtered-out count in summary, got: {stdout}"
    );
}

#[test]
fn cabin_test_unknown_named_test_errors_even_with_allow_no_tests() {
    let dir = passing_test_project();
    // `--allow-no-tests` must not soften an explicitly named
    // test that does not exist.
    let assertion = cabin()
        .args([
            "test",
            "--test",
            "nope_test",
            "--allow-no-tests",
            "--manifest-path",
        ])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("--test `nope_test` was not found in the selected packages"),
        "expected unknown-test error, got: {stderr}"
    );
}

#[test]
fn cabin_test_named_target_of_other_kind_errors() {
    let dir = passing_test_project();
    // `demo` exists, but as the library target.
    let assertion = cabin()
        .args(["test", "--test", "demo", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("--test `demo` matched a target of kind `library`; expected `test`"),
        "expected kind-mismatch error, got: {stderr}"
    );
}

#[test]
fn cabin_test_named_target_runs_in_every_matching_package() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // Two members declare a same-named test target; `--test`
    // keeps every match across the selected packages.
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/a", "packages/b"]
"#,
        )
        .unwrap();
    for member in ["a", "b"] {
        dir.child(format!("packages/{member}/cabin.toml"))
            .write_str(&format!(
                r#"[package]
name = "{member}"
version = "0.1.0"

[target.{member}]
type = "library"
sources = ["src/lib.cc"]

[target.common_test]
type = "test"
sources = ["tests/lib_test.cc"]
deps = ["{member}"]
"#
            ))
            .unwrap();
        dir.child(format!("packages/{member}/src/lib.cc"))
            .write_str("int x() { return 0; }\n")
            .unwrap();
        dir.child(format!("packages/{member}/tests/lib_test.cc"))
            .write_str("int main() { return 0; }\n")
            .unwrap();
    }
    let assertion = cabin()
        .args([
            "test",
            "--workspace",
            "--test",
            "common_test",
            "--manifest-path",
        ])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    for expected in ["test a:common_test ... ok", "test b:common_test ... ok"] {
        assert!(
            stdout.contains(expected),
            "expected `{expected}`, got: {stdout}"
        );
    }
    assert!(
        stdout
            .contains("test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"),
        "expected both matches to run, got: {stdout}"
    );
}

#[test]
fn cabin_test_sets_per_test_cabin_env_overlay() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "env_demo"
version = "0.1.0"

[target.env_test]
type = "test"
sources = ["tests/env_test.cc"]
"#,
        )
        .unwrap();
    dir.child("tests/env_test.cc")
        .write_str(
            r#"#include <cstdio>
#include <cstdlib>
#include <cstring>

static int status = 0;

void keep(const char* name, const char* expected) {
    const char* v = std::getenv(name);
    if (v == nullptr) {
        std::printf("MISSING %s\n", name);
        status |= 1;
        return;
    }
    std::printf("KEEP %s=%s\n", name, v);
    if (expected != nullptr && std::strcmp(v, expected) != 0) {
        status |= 2;
    }
}

void keep_present(const char* name) {
    const char* v = std::getenv(name);
    if (v == nullptr || v[0] == '\0') {
        std::printf("MISSING %s\n", name);
        status |= 1;
        return;
    }
    std::printf("KEEP %s\n", name);
}

void must_be_absent(const char* name) {
    if (std::getenv(name) != nullptr) {
        std::printf("LEAK %s\n", name);
        status |= 4;
    } else {
        std::printf("ABSENT %s\n", name);
    }
}

int main() {
    keep("CABIN_PACKAGE_NAME", "env_demo");
    keep("CABIN_PACKAGE_VERSION", "0.1.0");
    keep("CABIN_PROFILE", "dev");
    keep_present("CABIN_MANIFEST_DIR");
    keep_present("CABIN_MANIFEST_PATH");
    keep_present("CABIN_BUILD_DIR");
    must_be_absent("CABIN");
    must_be_absent("CABIN_PACKAGE_NAME_CANONICAL");
    must_be_absent("CABIN_BIN_NAME");
    must_be_absent("CABIN_BIN_NAME_CANONICAL");
    must_be_absent("CABIN_TEST_NAME");
    must_be_absent("CABIN_TEST_NAME_CANONICAL");
    must_be_absent("CABIN_TARGET_KIND");
    must_be_absent("CABIN_TARGET_TRIPLE");
    must_be_absent("CABIN_HOST_TRIPLE");
    must_be_absent("CABIN_BUILD_CONFIGURATION_FINGERPRINT");
    return status;
}
"#,
        )
        .unwrap();

    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    for expected in [
        "KEEP CABIN_PACKAGE_NAME=env_demo",
        "KEEP CABIN_PACKAGE_VERSION=0.1.0",
        "KEEP CABIN_PROFILE=dev",
        "KEEP CABIN_MANIFEST_DIR",
        "KEEP CABIN_MANIFEST_PATH",
        "KEEP CABIN_BUILD_DIR",
        "ABSENT CABIN",
        "ABSENT CABIN_PACKAGE_NAME_CANONICAL",
        "ABSENT CABIN_BIN_NAME",
        "ABSENT CABIN_BIN_NAME_CANONICAL",
        "ABSENT CABIN_TEST_NAME",
        "ABSENT CABIN_TEST_NAME_CANONICAL",
        "ABSENT CABIN_TARGET_KIND",
        "ABSENT CABIN_TARGET_TRIPLE",
        "ABSENT CABIN_HOST_TRIPLE",
        "ABSENT CABIN_BUILD_CONFIGURATION_FINGERPRINT",
        "test env_demo:env_test ... ok",
    ] {
        assert!(
            stdout.contains(expected),
            "expected `{expected}` in test output, got: {stdout}"
        );
    }
    assert!(
        !stdout.contains("LEAK "),
        "no removed CABIN_* variable may be injected, got: {stdout}"
    );
}

#[test]
fn cabin_test_exits_non_zero_on_failure() {
    require_cxx_build_tools();
    let dir = passing_test_project();
    dir.child("tests/lib_test.cc")
        .write_str("int main() { return 17; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stdout.contains("test demo:demo_test ... FAILED (exit 17)"),
        "expected per-test failure line, got stdout: {stdout}"
    );
    // The cargo-style epilogue recaps the failed test names
    // before the summary line.
    assert!(
        stdout.contains("failures:\n    demo:demo_test"),
        "expected failures recap, got stdout: {stdout}"
    );
    assert!(
        stdout.contains("test result: FAILED. 0 passed; 1 failed"),
        "expected failing summary line, got stdout: {stdout}"
    );
    assert!(
        stderr.contains("test failures: 1 of 1"),
        "expected failure summary in stderr, got: {stderr}"
    );
}

#[test]
fn cabin_test_no_targets_errors_by_default() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "lib_only"
version = "0.1.0"

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
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("no test targets found"),
        "expected no-test-targets error, got: {stderr}"
    );
}

#[test]
fn cabin_test_no_targets_succeeds_with_allow_no_tests() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "lib_only"
version = "0.1.0"

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
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--allow-no-tests")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("no test targets found"),
        "expected explanatory line, got: {stdout}"
    );
}

#[test]
fn cabin_test_runs_in_deterministic_package_then_target_order() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // Workspace with two members; member `b` declares its
    // tests *before* member `a` in TOML order, but the runner
    // must sort by package then target.
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/b", "packages/a"]
"#,
        )
        .unwrap();
    for (member, deps_table) in [("a", "[target.a_z_test]"), ("b", "[target.b_a_test]")] {
        assert_fs::fixture::ChildPath::new(
            dir.path().join(format!("packages/{member}/cabin.toml")),
        )
        .write_str(&format!(
            r#"[package]
name = "{member}"
version = "0.1.0"

[target.{member}]
type = "library"
sources = ["src/lib.cc"]

{deps_table}
type = "test"
sources = ["tests/lib_test.cc"]
deps = ["{member}"]
"#
        ))
        .unwrap();
        dir.child(format!("packages/{member}/src/lib.cc"))
            .write_str("int x() { return 0; }\n")
            .unwrap();
        dir.child(format!("packages/{member}/tests/lib_test.cc"))
            .write_str("int main() { return 0; }\n")
            .unwrap();
    }
    let assertion = cabin()
        .args(["test", "--workspace", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    // Both tests must appear, with `a:a_z_test` before
    // `b:b_a_test` regardless of TOML declaration order.
    let a_pos = stdout
        .find("test a:a_z_test ... ok")
        .unwrap_or_else(|| panic!("missing a:a_z_test in: {stdout}"));
    let b_pos = stdout
        .find("test b:b_a_test ... ok")
        .unwrap_or_else(|| panic!("missing b:b_a_test in: {stdout}"));
    assert!(
        a_pos < b_pos,
        "tests must run in (package, target) ascending order; got: {stdout}"
    );
}

#[test]
fn cabin_test_rejects_target_flag_as_unknown_argument() {
    // `cabin test` mirrors `cabin build`: the historic
    // `--target` manifest-target selector is gone, with the
    // flag name reserved for a future platform/toolchain
    // target. clap must reject the flag at parse time so the
    // overload cannot creep back in.
    cabin()
        .args(["test", "--target", "foo"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "unexpected argument '--target' found",
        ));
}

#[test]
fn package_archive_includes_test_and_example_sources() {
    let dir = project_with_dev_kinds();
    let out = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--output-dir"])
        .arg(&out)
        .assert()
        .success();
    // The archive must carry every declared source — including
    // dev-only target sources — so the package round-trips.
    let archive = out.join("demo-0.1.0.tar.gz");
    let bytes = fs::read(&archive).expect("archive readable");
    let listing = list_tar_gz_paths(&bytes);
    for expected in ["src/lib.cc", "tests/lib_test.cc", "examples/hello.cc"] {
        assert!(
            listing.iter().any(|p| p.ends_with(expected)),
            "archive missing {expected}; got: {listing:?}"
        );
    }
}

fn list_tar_gz_paths(bytes: &[u8]) -> Vec<String> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    archive
        .entries()
        .expect("entries iterator")
        .map(|e| {
            e.expect("entry")
                .path()
                .expect("path")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}
