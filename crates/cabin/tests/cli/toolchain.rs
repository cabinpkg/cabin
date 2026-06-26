//! End-to-end coverage for explicit toolchain selection and
//! conditional build flags.

use super::*;
// The shell-script fakes below are Unix-only and are the only use of
// `PathBuf` in this module.
#[cfg(unix)]
use std::path::PathBuf;

/// Helper: write a fake compiler/archiver `name` into `dir`
/// and return its absolute path.  The fake binary is a
/// minimal POSIX shell script so `--cxx /path/to/it` can be
/// resolved; the tests never invoke it.
#[cfg(unix)]
fn fake_tool(dir: &Path, name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    assert_fs::fixture::ChildPath::new(&path)
        .write_str("#!/bin/sh\nexit 0\n")
        .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn metadata_reports_default_toolchain_source() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let cxx = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(cxx["kind"].as_str(), Some("cxx"));
    assert_eq!(cxx["source"].as_str(), Some("default"));
}

#[test]
fn metadata_requires_resolvable_cxx_before_detection() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let empty_path = TempDir::new().unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", empty_path.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("no usable C++ compiler found on PATH"),
        "expected missing-CXX diagnostic, got: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn cli_cxx_flag_overrides_default() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let cxx = fake_tool(bin.path(), "my-cxx");
    let _ar = fake_tool(bin.path(), "ar");
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--cxx"])
        .arg(&cxx)
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let entry = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(entry["source"].as_str(), Some("cli"));
    assert_eq!(entry["spec"].as_str().unwrap(), cxx.to_str().unwrap());
}

#[cfg(unix)]
#[test]
fn cxx_env_var_is_respected_when_no_cli_flag() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let cxx = fake_tool(bin.path(), "env-cxx");
    let _ar = fake_tool(bin.path(), "ar");
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", bin.path())
        .env("CXX", &cxx)
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let entry = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(entry["source"].as_str(), Some("env"));
}

#[cfg(unix)]
#[test]
fn missing_explicit_cxx_errors_clearly() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let _ar = fake_tool(bin.path(), "ar");
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--cxx", "definitely-not-a-real-compiler-99"])
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("definitely-not-a-real-compiler-99"),
        "expected toolchain error mentioning the spec, got: {stderr}",
    );
    assert!(
        stderr.contains("could not be found"),
        "expected `could not be found` wording, got: {stderr}",
    );
}

#[cfg(unix)]
#[test]
fn manifest_toolchain_table_is_honored_when_no_cli_or_env() {
    let dir = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    let _g = fake_tool(bin.path(), "g++");
    let _c = fake_tool(bin.path(), "clang++");
    let _ar = fake_tool(bin.path(), "ar");
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[toolchain]
cxx = "clang++"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let entry = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(entry["source"].as_str(), Some("manifest"));
    assert_eq!(entry["spec"].as_str(), Some("clang++"));
}

#[test]
fn unsupported_toolchain_field_is_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[toolchain]
compiler-family = "clang"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("compiler-family"),
        "expected unknown-field error mentioning compiler-family, got: {stderr}",
    );
}

#[test]
fn invalid_include_path_with_parent_traversal_is_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[profile]
include-dirs = ["../sneaky"]
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("..") || stderr.contains("sneaky"),
        "expected include-dir traversal rejection, got: {stderr}",
    );
}

#[cfg(unix)]
#[test]
fn target_conditioned_build_flags_apply_to_compile_commands() {
    require_cxx_build_tools();
    let host_os = std::env::consts::OS;
    let other_os = if host_os == "linux" { "macos" } else { "linux" };
    let dir = TempDir::new().unwrap();
    let manifest = format!(
        r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "executable"
sources = ["src/main.cc"]

[target.'cfg(os = "{host_os}")'.profile]
defines = ["CABIN_HOST_MATCHED"]

[target.'cfg(os = "{other_os}")'.profile]
defines = ["CABIN_HOST_NOT_MATCHED"]
"#
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    assert!(
        cc.contains("-DCABIN_HOST_MATCHED"),
        "expected matching cfg define present: {cc}"
    );
    assert!(
        !cc.contains("CABIN_HOST_NOT_MATCHED"),
        "expected non-matching cfg define absent: {cc}"
    );
}

#[cfg(unix)]
#[test]
fn build_includes_dirs_from_build_table() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "executable"
sources = ["src/main.cc"]

[profile]
defines = ["CABIN_BUILD_DEFINE"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
    dir.child("include/.gitkeep").write_str("").unwrap();

    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let cc = std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    assert!(
        cc.contains("-DCABIN_BUILD_DEFINE"),
        "expected build define in compile_commands: {cc}"
    );
    assert!(
        cc.contains("/include"),
        "expected include dir in compile_commands: {cc}"
    );
}

#[test]
fn member_manifest_with_toolchain_table_is_rejected() {
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

[toolchain]
cxx = "clang++"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("toolchain"),
        "expected member-toolchain rejection, got: {stderr}",
    );
}
