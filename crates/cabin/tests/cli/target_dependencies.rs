//! End-to-end coverage for `[target.'cfg(...)'.<kind>]`
//! handling.  The tests exercise the full pipeline: parser,
//! workspace loader, resolver, lockfile, package metadata,
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
fn metadata_evaluates_conditions_for_the_target_triple() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[target.'cfg(all(os = "linux", arch = "riscv64"))'.dependencies]
fmt = ">=10"

[target.'cfg(not(arch = "riscv64"))'.dependencies]
spdlog = "^1"
"#,
        )
        .unwrap();
    let value = run_json(
        cabin()
            .args(["metadata", "--target", "riscv64gc-unknown-linux-gnu"])
            .arg("--manifest-path")
            .arg(dir.path().join("cabin.toml")),
    );
    let target_platform = &value["target_platform"];
    assert_eq!(target_platform["target"], "riscv64gc-unknown-linux-gnu");
    assert_eq!(target_platform["arch"], "riscv64");
    let deps = value["packages"][0]["dependencies"].as_array().unwrap();
    let active = |name: &str| deps.iter().find(|d| d["name"] == name).unwrap()["active"].clone();
    assert_eq!(active("fmt"), true);
    assert_eq!(active("spdlog"), false);
}

#[test]
fn metadata_sorts_dependencies_across_conditional_declarations() {
    // Conditional deps are appended after the unconditional ones
    // in the parsed package; the metadata view must still honor
    // its documented `(dependency_kind, name)` ordering.
    let dir = TempDir::new().unwrap();
    let manifest = format!(
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
zlib = "^1"

[dev-dependencies]
catch2 = "^3"

[target.'cfg(os = "{host}")'.dependencies]
abseil = ">=1"
"#,
        host = host_os_value(),
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let deps: Vec<(String, String)> = value["packages"][0]["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["dependency_kind"].as_str().unwrap().to_owned(),
                d["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        deps,
        vec![
            ("normal".to_owned(), "abseil".to_owned()),
            ("normal".to_owned(), "zlib".to_owned()),
            ("dev".to_owned(), "catch2".to_owned()),
        ]
    );
}

/// The lockfile serves every platform: a registry dependency gated to
/// another OS - declared directly or by a path dependency gated the
/// same way - locks exactly as on that OS.
#[test]
fn resolve_locks_target_dependencies_for_every_platform() {
    let dir = TempDir::new().unwrap();
    let manifest = format!(
        r#"[package]
name = "app"
version = "0.1.0"

[target.'cfg(os = "{host}")'.dependencies]
fmt = ">=10"

[target.'cfg(os = "{other}")'.dependencies]
spdlog = "^1"
helper = {{ path = "helper" }}
"#,
        host = host_os_value(),
        other = other_os_value(),
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    dir.child("helper/cabin.toml")
        .write_str(
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n\n[dependencies]\nzlib = \"^1\"\n",
        )
        .unwrap();
    let index = dir.path().join("index");
    write_index_entry_no_source(&index, "fmt", "10.2.1", &"0".repeat(64));
    write_index_entry_no_source(&index, "spdlog", "1.14.1", &"1".repeat(64));
    write_index_entry_no_source(&index, "zlib", "1.3.1", &"2".repeat(64));
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(&index)
        .assert()
        .success();
    let lock = std::fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
    for name in ["fmt", "spdlog", "zlib"] {
        assert!(
            lock.contains(&format!("name = \"{name}\"")),
            "{name} must be locked: {lock}"
        );
    }
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
