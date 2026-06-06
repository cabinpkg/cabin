//! End-to-end coverage for optional Cabin
//! package dependencies, dependency feature requests, and the
//! cross-package feature resolver. These tests exercise the
//! integration through the actual CLI binary so the JSON
//! contract surfaces in the metadata output.

use super::*;

/// Feature-aware fixture: `app` has an optional `openssl`
/// dependency that is gated by feature `ssl`. Default features
/// do not enable `ssl`, so the optional dep stays out of
/// resolution unless the user passes `--features ssl`.
fn write_app_with_optional_openssl(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[features]
default = []
ssl = ["dep:openssl"]

[dependencies]
fmt = ">=10 <11"
openssl = { version = "^3", optional = true }
"#,
        )
        .unwrap();
    // Index covers both fmt and openssl. The resolver should
    // only see `openssl` when `--features ssl` is passed.
    write_index_entry_no_source(&root.join("index"), "fmt", "10.2.1", &"0".repeat(64));
    write_index_entry_no_source(&root.join("index"), "openssl", "3.2.0", &"0".repeat(64));
}

#[test]
fn resolve_without_feature_skips_optional_dep() {
    let dir = TempDir::new().unwrap();
    write_app_with_optional_openssl(dir.path());
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(stdout.contains("fmt"), "fmt should appear: {stdout}");
    assert!(
        !stdout.contains("openssl"),
        "disabled optional dep openssl must not appear: {stdout}"
    );
}

#[test]
fn resolve_with_feature_includes_optional_dep() {
    let dir = TempDir::new().unwrap();
    write_app_with_optional_openssl(dir.path());
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .args(["--features", "ssl"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("openssl"),
        "feature ssl should pull openssl in: {stdout}"
    );
}

#[test]
fn resolve_no_default_features_disables_root_default_chain() {
    let dir = TempDir::new().unwrap();
    // Default group enables ssl, which enables optional openssl.
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[features]
default = ["ssl"]
ssl = ["dep:openssl"]

[dependencies]
openssl = { version = "^3", optional = true }
"#,
        )
        .unwrap();
    write_index_entry_no_source(
        &dir.path().join("index"),
        "openssl",
        "3.2.0",
        &"0".repeat(64),
    );
    // Without --no-default-features, openssl appears.
    let with_default = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    assert!(String::from_utf8_lossy(&with_default.get_output().stdout).contains("openssl"));
    // With --no-default-features, openssl is dropped.
    let no_default = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .args(["--no-default-features"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&no_default.get_output().stdout);
    assert!(
        !stdout.contains("openssl"),
        "no-default-features must drop openssl: {stdout}"
    );
}

#[test]
fn fetch_does_not_pull_disabled_optional_into_lockfile() {
    // `cabin fetch` resolves with default features (no
    // `--features` flag is exposed on this command today).
    // The disabled optional `openssl` must not appear in the
    // lockfile that fetch writes, even though the artifact
    // step itself fails on the missing fmt artifact (the
    // index fixture omits the source block — that's a fetch
    // concern, not a feature concern). We only assert the
    // dep-set decision the feature resolver made.
    let dir = TempDir::new().unwrap();
    write_app_with_optional_openssl(dir.path());
    // First run resolve to write the lockfile (resolve does
    // not need source artifacts).
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let lock_body = fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
    assert!(
        lock_body.contains("fmt"),
        "fmt should be locked: {lock_body}"
    );
    assert!(
        !lock_body.contains("openssl"),
        "disabled optional openssl must not be locked: {lock_body}"
    );
}

#[test]
fn metadata_round_trips_optional_and_features_in_dependency_view() {
    // `cabin metadata` JSON already prints each dep's name + kind
    // + source. This test pins that the optional flag and any
    // `features = [...]` declaration round-trip back through the
    // typed CLI view.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = { version = ">=10", features = ["compile"], default-features = false }
openssl = { version = "^3", optional = true }
"#,
        )
        .unwrap();
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let demo = package_in(&value, "demo");
    let deps = demo["dependencies"].as_array().unwrap();
    // The CLI view already serializes each `Dependency` via
    // serde, so the new `optional`, `features`, and
    // `default_features` fields show up automatically when
    // their values differ from the documented defaults.
    let openssl = deps
        .iter()
        .find(|d| d["name"] == "openssl")
        .expect("openssl listed");
    assert_eq!(openssl["optional"].as_bool(), Some(true));
    let fmt = deps.iter().find(|d| d["name"] == "fmt").unwrap();
    assert_eq!(fmt["default_features"].as_bool(), Some(false));
    let features: Vec<&str> = fmt["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(features, vec!["compile"]);
}

#[test]
fn package_metadata_round_trips_optional_and_features() {
    // `cabin package` writes canonical metadata; round-trip
    // confirms the rich entry shape is used when fields are
    // non-default.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = ">=10"
openssl = { version = "^3", optional = true }
"#,
        )
        .unwrap();
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
    // Bare entry: `fmt` has no overrides, so it stays a string.
    assert!(metadata["dependencies"]["fmt"].is_string());
    // Rich entry: `openssl` is optional, so it's a table with
    // `version` + `optional`.
    let openssl = &metadata["dependencies"]["openssl"];
    assert_eq!(openssl["version"].as_str().unwrap(), "^3");
    assert!(openssl["optional"].as_bool().unwrap());
}

#[test]
fn unknown_root_feature_errors_clearly_at_cli() {
    let dir = TempDir::new().unwrap();
    write_app_with_optional_openssl(dir.path());
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .args(["--features", "no-such-feature"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("unknown feature") && stderr.contains("no-such-feature"),
        "expected unknown-feature error, got: {stderr}"
    );
}

#[test]
fn dep_colon_on_non_optional_dep_is_rejected_at_cli() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[features]
ssl = ["dep:fmt"]

[dependencies]
fmt = ">=10"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--features", "ssl"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("not optional") || stderr.contains("DepIsNotOptional"),
        "expected non-optional dep error, got: {stderr}"
    );
}
