//! End-to-end coverage for `required-features` on targets: default
//! enumeration skips gated targets, explicit requests (a `deps`
//! entry, `cabin test --test`) hard-error, and feature selection
//! (CLI flags or dependency-edge requests) makes gated targets
//! buildable.

use super::*;

/// Single package with an ungated `core` library and a `tls`
/// library gated on the `ssl` feature.
fn write_gated_package(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[features]
default = []
ssl = []

[target.core]
type = "library"
sources = ["src/core.cc"]

[target.tls]
type = "library"
sources = ["src/tls.cc"]
required-features = ["ssl"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/core.cc"))
        .write_str("int core_value() { return 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/tls.cc"))
        .write_str("int tls_value() { return 2; }\n")
        .unwrap();
}

/// The static-archive spellings of `name` across both dialects
/// (`lib<name>.a` for GNU/Clang, `<name>.lib` for MSVC).
fn archive_spellings(name: &str) -> [String; 2] {
    [format!("lib{name}.a"), format!("{name}.lib")]
}

fn archive_exists(build_dir: &Path, package: &str, name: &str) -> bool {
    archive_spellings(name).iter().any(|spelling| {
        build_dir
            .join("dev/packages")
            .join(package)
            .join(spelling)
            .exists()
    })
}

#[test]
fn default_build_skips_gated_target() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_gated_package(dir.path());
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    assert!(archive_exists(&build_dir, "demo", "core"));
    assert!(
        !archive_exists(&build_dir, "demo", "tls"),
        "feature-gated target must be skipped by the default build"
    );
}

#[test]
fn build_with_feature_builds_gated_target() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_gated_package(dir.path());
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--features", "ssl", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    assert!(archive_exists(&build_dir, "demo", "tls"));
}

#[test]
fn build_all_features_builds_gated_c_target() {
    // C parity: a gated C library follows the same rules.
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(
            r#"[package]
name = "cdemo"
version = "0.1.0"
c-standard = "c11"

[features]
compress = []

[target.cz]
type = "library"
sources = ["src/cz.c"]
required-features = ["compress"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/cz.c"))
        .write_str("int cz_value(void) { return 3; }\n")
        .unwrap();
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--all-features", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    assert!(archive_exists(&build_dir, "cdemo", "cz"));
}

#[test]
fn all_gated_default_build_reports_actionable_error() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[features]
ssl = []

[target.tls]
type = "library"
sources = ["src/tls.cc"]
required-features = ["ssl"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/tls.cc"))
        .write_str("int tls_value() { return 2; }\n")
        .unwrap();
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("demo:tls"))
        .stderr(predicate::str::contains("--features"));
}

/// Workspace fixture: `app` links `netlib:tls`, which requires
/// netlib's `ssl` feature.  `edge_features` controls whether app's
/// dependency edge requests it.
fn write_gated_dep_workspace(root: &Path, edge_features: &str) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["packages/*"]
default-members = ["packages/app"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/netlib/cabin.toml"))
        .write_str(
            r#"[package]
name = "netlib"
version = "0.1.0"
cxx-standard = "c++17"

[features]
ssl = []

[target.tls]
type = "library"
sources = ["src/tls.cc"]
include-dirs = ["include"]
required-features = ["ssl"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/netlib/include/tls.h"))
        .write_str("#pragma once\nint tls_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/netlib/src/tls.cc"))
        .write_str("#include \"tls.h\"\nint tls_value() { return 2; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
netlib = {{ path = "../netlib"{edge_features} }}

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["netlib:tls"]
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/src/main.cc"))
        .write_str("#include \"tls.h\"\nint main() { return tls_value() == 2 ? 0 : 1; }\n")
        .unwrap();
}

#[test]
fn dep_on_gated_target_without_feature_fails_with_edge_help() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_gated_dep_workspace(dir.path(), "");
    cabin()
        .args(["build", "-p", "app", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("netlib:tls"))
        .stderr(predicate::str::contains("features = [\"ssl\"]"));
}

#[test]
fn dep_on_gated_target_with_edge_feature_builds() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_gated_dep_workspace(dir.path(), ", features = [\"ssl\"]");
    cabin()
        .args(["build", "-p", "app", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
}

/// Package with one ungated and one feature-gated test target.
fn write_gated_test_package(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[features]
slow = []

[target.demo]
type = "library"
sources = ["src/lib.cc"]

[target.fast_test]
type = "test"
sources = ["tests/fast.cc"]
deps = ["demo"]

[target.slow_test]
type = "test"
sources = ["tests/slow.cc"]
deps = ["demo"]
required-features = ["slow"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/lib.cc"))
        .write_str("int lib_value() { return 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("tests/fast.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("tests/slow.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
}

#[test]
fn test_enumeration_skips_gated_test_and_counts_it_filtered() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_gated_test_package(dir.path());
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("running 1 test"),
        "gated test must be skipped: {stdout}"
    );
    assert!(
        stdout.contains("1 filtered out"),
        "the skip must be visible in the summary: {stdout}"
    );
}

#[test]
fn test_with_feature_runs_gated_test() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_gated_test_package(dir.path());
    let assertion = cabin()
        .args(["test", "--features", "slow", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("running 2 tests"),
        "enabled feature must un-gate the test: {stdout}"
    );
}

#[test]
fn named_gated_test_without_feature_is_a_hard_error() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_gated_test_package(dir.path());
    cabin()
        .args(["test", "--test", "slow_test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("demo:slow_test"))
        .stderr(predicate::str::contains("--features slow"));
}

#[test]
fn all_gated_test_enumeration_names_the_gate() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[features]
slow = []

[target.demo]
type = "library"
sources = ["src/lib.cc"]

[target.slow_test]
type = "test"
sources = ["tests/slow.cc"]
deps = ["demo"]
required-features = ["slow"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/lib.cc"))
        .write_str("int lib_value() { return 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("tests/slow.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    // Without `--allow-no-tests`, the failure must name the gated
    // targets rather than claim no test target exists.
    cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("demo:slow_test"))
        .stderr(predicate::str::contains("--features"))
        .stderr(predicate::str::contains("no test targets found").not());
    // With it, the outcome reports the filtered count instead of
    // pretending the package declares no tests.
    let assertion = cabin()
        .args(["test", "--allow-no-tests", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("1 filtered out by required-features"),
        "the all-gated skip must be visible: {stdout}"
    );
}

/// Package with one ungated and one feature-gated executable.
fn write_gated_run_package(root: &Path, gated_only: bool) {
    let main_target = if gated_only {
        ""
    } else {
        r#"
[target.main]
type = "executable"
sources = ["src/main.cc"]
"#
    };
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[features]
extra = []
{main_target}
[target.extra]
type = "executable"
sources = ["src/extra.cc"]
required-features = ["extra"]
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
        .write_str("#include <cstdio>\nint main() { std::puts(\"main ran\"); return 0; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/extra.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
}

#[test]
fn run_enumeration_skips_gated_executable() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_gated_run_package(dir.path(), false);
    // The gated `extra` executable must not make the default run
    // ambiguous; `main` is the only buildable candidate.
    let assertion = cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(stdout.contains("main ran"), "stdout = {stdout}");
}

#[test]
fn run_with_only_gated_executables_reports_missing_features() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_gated_run_package(dir.path(), true);
    cabin()
        .args(["run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("demo:extra"))
        .stderr(predicate::str::contains("--features extra"));
}

#[test]
fn metadata_reports_required_features() {
    let dir = TempDir::new().unwrap();
    write_gated_package(dir.path());
    let assertion = cabin()
        .args(["metadata", "--format", "json", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("metadata is valid JSON");
    let packages = json["packages"].as_array().expect("packages array");
    let demo = packages
        .iter()
        .find(|p| p["name"] == "demo")
        .expect("demo package present");
    let targets = demo["targets"].as_array().expect("targets array");
    let tls = targets
        .iter()
        .find(|t| t["name"] == "tls")
        .expect("tls target present");
    assert_eq!(tls["required_features"][0], "ssl");
    let core = targets
        .iter()
        .find(|t| t["name"] == "core")
        .expect("core target present");
    assert!(
        core.get("required_features").is_none(),
        "ungated targets keep their previous JSON shape"
    );
}
