//! End-to-end coverage for the compiler-cache wrapper feature
//! (`ccache` / `sccache`).  Each test stages a fake wrapper +
//! compiler / archiver, points the CLI at them, and inspects
//! either the metadata JSON or a stub `cabin build` invocation.

// This module's tests drive Unix-only shell-script fakes.
#[cfg(unix)]
use super::*;
#[cfg(unix)]
use std::path::PathBuf;

/// Re-implementation of `compiler_detection::fake_tool_with_output`.
/// The detection module is private to its `mod`, so the helper
/// is duplicated here rather than reaching across module
/// boundaries.
#[cfg(unix)]
fn fake_tool_with_output(
    dir: &Path,
    name: &str,
    stdout: &str,
    stderr: &str,
    status: i32,
) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let escaped_stdout = stdout.replace('\'', "'\\''");
    let escaped_stderr = stderr.replace('\'', "'\\''");
    let script = format!(
        "#!/bin/sh\nprintf '%s' '{escaped_stdout}'\nprintf '%s' '{escaped_stderr}' >&2\nexit {status}\n"
    );
    assert_fs::fixture::ChildPath::new(&path)
        .write_str(&script)
        .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn metadata_reports_no_wrapper_by_default() {
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
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .env_remove("CABIN_COMPILER_WRAPPER")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        value["toolchain"]["compiler_wrapper"].is_null(),
        "expected null compiler_wrapper, got: {}",
        value["toolchain"]["compiler_wrapper"],
    );
}

#[cfg(unix)]
#[test]
fn metadata_reports_cli_selected_ccache() {
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
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let _ccache = fake_tool_with_output(
        bin.path(),
        "ccache",
        "ccache version 4.10.2\nFeatures: file-storage http-storage\n",
        "",
        0,
    );
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--compiler-wrapper", "ccache"])
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .env_remove("CABIN_COMPILER_WRAPPER")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let wrapper = &value["toolchain"]["compiler_wrapper"];
    assert_eq!(wrapper["kind"].as_str(), Some("ccache"));
    assert_eq!(wrapper["source"].as_str(), Some("cli"));
    assert_eq!(wrapper["version"].as_str(), Some("4.10.2"));
}

#[cfg(unix)]
#[test]
fn no_compiler_wrapper_overrides_manifest_selection() {
    let dir = TempDir::new().unwrap();
    // Manifest selects ccache, but `--no-compiler-wrapper`
    // wins.  The wrapper executable is intentionally absent so
    // a regression that ignored the override would surface as
    // a NotFound error instead of a silent pass.
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[profile.cache]
compiler-wrapper = "ccache"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--no-compiler-wrapper"])
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .env_remove("CABIN_COMPILER_WRAPPER")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(value["toolchain"]["compiler_wrapper"].is_null());
}

#[cfg(unix)]
#[test]
fn manifest_build_cache_selects_wrapper_when_no_override() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[profile.cache]
compiler-wrapper = "sccache"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let _sccache = fake_tool_with_output(bin.path(), "sccache", "sccache 0.7.7\n", "", 0);
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .env_remove("CABIN_COMPILER_WRAPPER")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let wrapper = &value["toolchain"]["compiler_wrapper"];
    assert_eq!(wrapper["kind"].as_str(), Some("sccache"));
    assert_eq!(wrapper["source"].as_str(), Some("manifest"));
    assert_eq!(wrapper["version"].as_str(), Some("0.7.7"));
}

#[cfg(unix)]
#[test]
fn env_overrides_manifest_compiler_wrapper() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[profile.cache]
compiler-wrapper = "sccache"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let _ccache = fake_tool_with_output(bin.path(), "ccache", "ccache version 4.10.2\n", "", 0);
    let _sccache = fake_tool_with_output(bin.path(), "sccache", "sccache 0.7.7\n", "", 0);
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", bin.path())
        .env("CABIN_COMPILER_WRAPPER", "ccache")
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let wrapper = &value["toolchain"]["compiler_wrapper"];
    assert_eq!(wrapper["kind"].as_str(), Some("ccache"));
    assert_eq!(wrapper["source"].as_str(), Some("env"));
}

#[cfg(unix)]
#[test]
fn missing_wrapper_executable_yields_clear_build_error() {
    // CLI requests ccache, but PATH has no `ccache` binary -
    // the build orchestration must surface a typed
    // "not found" error rather than silently dropping the
    // wrapper.  A `ninja` stub is staged so the build path
    // reaches the wrapper-resolution step before bailing on
    // missing tools.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "library"
sources = ["src/lib.cc"]
"#,
        )
        .unwrap();
    dir.child("src/lib.cc")
        .write_str("int demo() { return 0; }\n")
        .unwrap();
    let bin = TempDir::new().unwrap();
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let _ninja = fake_tool_with_output(bin.path(), "ninja", "1.11.1\n", "", 0);
    // No ccache staged.
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--compiler-wrapper", "ccache"])
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .env_remove("CABIN_COMPILER_WRAPPER")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("ccache") && stderr.contains("could not be found"),
        "expected NotFound message naming ccache, got: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn unsupported_cli_value_is_rejected() {
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
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--compiler-wrapper", "fastcache"])
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .env_remove("CABIN_COMPILER_WRAPPER")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("fastcache") && stderr.contains("not supported"),
        "expected unsupported-wrapper error, got: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn cli_flags_are_mutually_exclusive() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    // Clap rejects the combination before any orchestration
    // runs, which makes the test fully hermetic.
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--compiler-wrapper", "ccache", "--no-compiler-wrapper"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("--no-compiler-wrapper")
            || stderr.contains("--compiler-wrapper")
            || stderr.contains("cannot be used"),
        "expected mutually-exclusive error, got: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn build_fingerprint_changes_when_wrapper_changes() {
    // Two `cabin metadata` runs differing only in the wrapper
    // selection must produce different `fingerprint` values
    // so cache layers can distinguish them.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[features]
default = []
fast = []
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let _ccache = fake_tool_with_output(bin.path(), "ccache", "ccache version 4.10.2\n", "", 0);
    let common = |extra: &[&str]| {
        let mut cmd = cabin();
        cmd.args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(extra)
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .env_remove("CABIN_COMPILER_WRAPPER");
        cmd
    };

    let baseline = common(&[]).assert().success();
    let baseline_value: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&baseline.get_output().stdout)).unwrap();
    let baseline_fp = baseline_value["packages"][0]["configuration"]["fingerprint"]
        .as_str()
        .expect("baseline fingerprint")
        .to_owned();

    let with_wrapper = common(&["--compiler-wrapper", "ccache"]).assert().success();
    let with_wrapper_value: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&with_wrapper.get_output().stdout)).unwrap();
    let with_wrapper_fp = with_wrapper_value["packages"][0]["configuration"]["fingerprint"]
        .as_str()
        .expect("wrapper fingerprint");

    assert_ne!(
        baseline_fp, with_wrapper_fp,
        "fingerprint must differ when a wrapper is selected"
    );
}

#[cfg(unix)]
#[test]
fn member_manifest_with_build_cache_is_rejected() {
    // Wrapper settings must only appear at the workspace
    // root.  A member declaring `[profile.cache]` should surface
    // a clear `MemberDeclaresCompilerWrapper`-shaped error.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["member"]

[package]
name = "root"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("member/cabin.toml")
        .write_str(
            r#"[package]
name = "member"
version = "0.1.0"

[profile.cache]
compiler-wrapper = "ccache"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("PATH", bin.path())
        .env_remove("CABIN_COMPILER_WRAPPER")
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("compiler-cache wrapper")
            || stderr.contains("[profile.cache]")
            || stderr.contains("workspace root"),
        "expected member-rejection error, got: {stderr}"
    );
}
