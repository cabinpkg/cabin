//! End-to-end coverage for `[target.'cfg(...)'.<kind>]`
//! handling.  The tests exercise the full pipeline: parser,
//! workspace loader, resolver, fetch, package metadata,
//! and the CLI metadata JSON view.

use super::*;

fn host_os_value() -> &'static str {
    std::env::consts::OS
}

fn other_os_value() -> &'static str {
    // Pick a value the host is guaranteed not to be so the
    // negative branch exercises predicate failure
    // deterministically on every supported runner.
    if std::env::consts::OS == "linux" {
        "macos"
    } else {
        "linux"
    }
}

#[test]
fn metadata_reports_target_platform_and_active_flag() {
    let dir = TempDir::new().unwrap();
    let manifest = format!(
        r#"[package]
name = "app"
version = "0.1.0"

[target.'cfg(os = "{host}")'.dependencies]
fmt = ">=10"

[target.'cfg(os = "{other}")'.dependencies]
spdlog = "^1"
"#,
        host = host_os_value(),
        other = other_os_value(),
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // The view always reports the resolved host platform.
    let target_platform = &value["target_platform"];
    assert_eq!(target_platform["os"].as_str().unwrap(), host_os_value());
    // Two deps are listed; the host-matching one is active,
    // the other is inactive.
    let deps = value["packages"][0]["dependencies"].as_array().unwrap();
    let fmt = deps.iter().find(|d| d["name"] == "fmt").unwrap();
    assert_eq!(fmt["active"].as_bool(), Some(true));
    assert!(fmt["target"].as_str().unwrap().contains("os ="));
    let spdlog = deps.iter().find(|d| d["name"] == "spdlog").unwrap();
    assert_eq!(spdlog["active"].as_bool(), Some(false));
}

#[test]
fn resolve_filters_inactive_target_dependency() {
    // Even though the manifest declares `spdlog`, only the
    // `fmt` constraint reaches the resolver because the
    // `spdlog` declaration is gated by a non-matching `cfg`.
    // This proves the index does not need to know about
    // `spdlog` for `cabin resolve` to succeed.
    let dir = TempDir::new().unwrap();
    let manifest = format!(
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"

[target.'cfg(os = "{other}")'.dependencies]
spdlog = "^1"
"#,
        other = other_os_value(),
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    write_index_entry_no_source(&dir.path().join("index"), "fmt", "10.2.1", &"0".repeat(64));
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(stdout.contains("fmt"), "fmt should resolve: {stdout}");
    assert!(
        !stdout.contains("spdlog"),
        "inactive spdlog must not enter resolution: {stdout}",
    );
}

#[test]
fn package_metadata_round_trips_target_field() {
    // `cabin package` writes canonical metadata with the
    // condition preserved as `target` on the rich entry.
    let dir = TempDir::new().unwrap();
    let manifest = format!(
        r#"[package]
name = "demo"
version = "0.1.0"

[target.'cfg(os = "{host}")'.dependencies]
fmt = ">=10"
"#,
        host = host_os_value(),
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    let out = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--output-dir"])
        .arg(&out)
        .assert()
        .success();
    let metadata: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("demo-0.1.0.json")).unwrap()).unwrap();
    let fmt = &metadata["dependencies"]["fmt"];
    // A target-conditional dep is always serialized in the
    // rich (table) form because the bare form has nowhere
    // to put the predicate.
    assert!(fmt.is_object(), "expected rich table: {fmt}");
    assert_eq!(fmt["version"].as_str().unwrap(), ">=10");
    assert!(fmt["target"].as_str().unwrap().contains("os ="));
}

#[test]
fn workspace_inheritance_inside_target_cfg_is_rejected() {
    let dir = TempDir::new().unwrap();
    let manifest = format!(
        r#"[package]
name = "app"
version = "0.1.0"

[target.'cfg(os = "{host}")'.dependencies]
fmt = {{ workspace = true }}
"#,
        host = host_os_value(),
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("workspace") && stderr.contains("cfg"),
        "expected workspace-inside-cfg rejection, got: {stderr}",
    );
}

#[test]
fn invalid_cfg_predicate_is_rejected_with_clear_error() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[target.'cfg(host_endian = "little")'.dependencies]
fmt = ">=10"
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
        stderr.contains("host_endian") || stderr.contains("cfg"),
        "expected cfg parse error, got: {stderr}",
    );
}
