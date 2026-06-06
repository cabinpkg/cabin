use super::*;

fn write_demo_with_features(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[features]
default = ["simd"]
simd = []
ssl = []

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
        .write_str(HELLO_MAIN_CC)
        .unwrap();
}

#[test]
fn unknown_feature_fails_clearly() {
    let dir = TempDir::new().unwrap();
    write_demo_with_features(dir.path());
    cabin()
        .current_dir(dir.path())
        .args(["build", "--features", "missing", "--build-dir"])
        .arg(dir.path().join("build"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown feature"));
}

#[test]
fn cabin_metadata_reports_declarations_and_selections() {
    let dir = TempDir::new().unwrap();
    write_demo_with_features(dir.path());
    let json = run_json(
        cabin()
            .current_dir(dir.path())
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml")),
    );
    let pkg = &json["packages"][0];
    assert_eq!(pkg["features"]["default"][0], "simd");
    let cfg = &pkg["configuration"];
    assert_eq!(cfg["features"][0], "simd");
    assert_eq!(cfg["fingerprint"].as_str().unwrap().len(), 64);
}

#[test]
fn cabin_metadata_all_features_applies_to_configuration_block() {
    let dir = TempDir::new().unwrap();
    write_demo_with_features(dir.path());
    let json = run_json(
        cabin()
            .current_dir(dir.path())
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--all-features"]),
    );
    let cfg = &json["packages"][0]["configuration"];
    assert_eq!(cfg["features"], serde_json::json!(["simd", "ssl"]));
}

#[test]
fn cabin_package_metadata_includes_declarations() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[features]
default = ["simd"]
simd = []

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
    let dist = dir.path().join("dist");
    cabin()
        .current_dir(dir.path())
        .args(["package", "--output-dir"])
        .arg(&dist)
        .assert()
        .success();
    let meta_path = dist.join("demo-0.1.0.json");
    let body = fs::read_to_string(&meta_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["features"]["default"][0], "simd");
}

#[test]
fn cabin_publish_registry_dir_preserves_declarations() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[features]
default = []
simd = []

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .current_dir(dir.path())
        .args(["publish", "--registry-dir"])
        .arg(&registry)
        .assert()
        .success();
    let entry_path = registry.join("packages/demo.json");
    let body = fs::read_to_string(&entry_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let v = &json["versions"]["0.1.0"];
    assert_eq!(v["features"]["features"]["simd"], serde_json::json!([]));
}
