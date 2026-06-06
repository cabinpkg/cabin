use super::*;

fn write_simple_package(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include_dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("include/fmt.h"))
        .write_str("#pragma once\nvoid say_hello();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/fmt.cc"))
            .write_str("#include <iostream>\n#include \"fmt.h\"\nvoid say_hello() { std::cout << \"hello from fmt\\n\"; }\n")
            .unwrap();
}

#[test]
fn publish_creates_registry_layout() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_simple_package(&pkg_root);
    let registry = dir.path().join("registry");

    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    assert!(registry.join("config.json").is_file());
    assert!(registry.join("packages/fmt.json").is_file());
    assert!(registry.join("artifacts/fmt/fmt-10.2.1.tar.gz").is_file());
}

#[test]
fn published_package_index_is_well_formed() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_simple_package(&pkg_root);
    let registry = dir.path().join("registry");

    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    let body = fs::read_to_string(registry.join("packages/fmt.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["name"], "fmt");
    let entry = &value["versions"]["10.2.1"];
    assert_eq!(entry["yanked"], false);
    assert!(entry["checksum"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(entry["source"]["type"], "archive");
    assert_eq!(entry["source"]["format"], "tar.gz");
    assert_eq!(
        entry["source"]["path"],
        "../artifacts/fmt/fmt-10.2.1.tar.gz"
    );
}

#[test]
fn published_index_preserves_manifest_compiler_cache_settings() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_simple_package(&pkg_root);
    let manifest_path = pkg_root.join("cabin.toml");
    let mut manifest = fs::read_to_string(&manifest_path).unwrap();
    manifest.push_str(
        r#"
[profile.cache]
compiler-wrapper = "sccache"
"#,
    );
    assert_fs::fixture::ChildPath::new(&manifest_path)
        .write_str(&manifest)
        .unwrap();
    let registry = dir.path().join("registry");

    cabin()
        .args(["publish", "--manifest-path"])
        .arg(&manifest_path)
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    let body = fs::read_to_string(registry.join("packages/fmt.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let entry = &value["versions"]["10.2.1"];
    assert_eq!(
        entry["compiler_wrapper"]["general"],
        serde_json::json!({"kind": "use", "wrapper": "sccache"})
    );
}

#[test]
fn duplicate_publish_fails_clearly() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_simple_package(&pkg_root);
    let registry = dir.path().join("registry");

    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn publish_json_format_emits_machine_readable_summary() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_simple_package(&pkg_root);
    let registry = dir.path().join("registry");

    let value = run_json(
        cabin()
            .args(["publish", "--manifest-path"])
            .arg(pkg_root.join("cabin.toml"))
            .arg("--registry-dir")
            .arg(&registry)
            .args(["--format", "json"]),
    );
    assert_eq!(value["published"], true);
    assert_eq!(value["dry_run"], false);
    assert_eq!(value["registry_modified"], true);
    assert_eq!(value["name"], "fmt");
    assert_eq!(value["version"], "10.2.1");
    assert!(
        value["artifact_path"]
            .as_str()
            .unwrap()
            .ends_with("fmt-10.2.1.tar.gz")
    );
    assert!(
        value["package_index_path"]
            .as_str()
            .unwrap()
            .ends_with("fmt.json")
    );
    assert!(value["checksum"].as_str().unwrap().starts_with("sha256:"));
}

#[test]
fn dry_run_against_registry_does_not_mutate() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_simple_package(&pkg_root);
    let registry = dir.path().join("registry");

    let output = cabin()
        .args(["publish", "--dry-run", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("dry-run") || stdout.contains("dry run"));
    assert!(stdout.contains("No registry was modified"));
    // Registry must NOT have been initialized.
    assert!(!registry.join("config.json").exists());
    assert!(!registry.join("packages").exists());
    assert!(!registry.join("artifacts").exists());
}

#[test]
fn dry_run_against_registry_json_reports_no_mutation() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_simple_package(&pkg_root);
    let registry = dir.path().join("registry");

    let value = run_json(
        cabin()
            .args(["publish", "--dry-run", "--manifest-path"])
            .arg(pkg_root.join("cabin.toml"))
            .arg("--registry-dir")
            .arg(&registry)
            .args(["--format", "json"]),
    );
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["registry_modified"], false);
    assert_eq!(value["published"], false);
}

#[test]
fn publish_without_dry_run_or_registry_dir_fails_clearly() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_simple_package(&pkg_root);
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("--registry-dir"))
        .stderr(predicate::str::contains("--dry-run"));
}

#[test]
fn publish_rejects_output_dir_with_registry_dir() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_simple_package(&pkg_root);
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .arg("--output-dir")
        .arg(dir.path().join("dist"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("--output-dir"))
        .stderr(predicate::str::contains("--registry-dir"));
}

#[test]
fn path_dependency_publish_fails_clearly() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
local = { path = "../local" }
"#,
        )
        .unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .failure()
        .stderr(predicate::str::contains("path dependencies"));
}

#[test]
fn workspace_root_publish_fails_clearly() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    dir.child("packages/a/cabin.toml")
        .write_str(
            r#"[package]
name = "a"
version = "0.1.0"
"#,
        )
        .unwrap();
    let registry = dir.path().join("registry");
    // `cabin publish` against a workspace root must refuse
    // without a single `--package <name>` selection.
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--package <name>"));
}

fn publish_simple_package(dir: &Path) -> std::path::PathBuf {
    let pkg_root = dir.join("pkg");
    write_simple_package(&pkg_root);
    let registry = dir.join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();
    registry
}

fn write_app_using_fmt(dir: &Path, app_main: Option<&str>) {
    let manifest = if app_main.is_some() {
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["fmt"]
"#
    } else {
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#
    };
    assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
        .write_str(manifest)
        .unwrap();
    if let Some(body) = app_main {
        assert_fs::fixture::ChildPath::new(dir.join("app/src/main.cc"))
            .write_str(body)
            .unwrap();
    }
}

#[test]
fn published_registry_can_be_resolved() {
    let dir = TempDir::new().unwrap();
    let registry = publish_simple_package(dir.path());
    write_app_using_fmt(dir.path(), None);

    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(&registry)
            .args(["--format", "json"]),
    );
    let names: Vec<&str> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"fmt"),
        "fmt missing from resolve: {names:?}"
    );
}

#[test]
fn published_registry_can_be_fetched() {
    let dir = TempDir::new().unwrap();
    let registry = publish_simple_package(dir.path());
    write_app_using_fmt(dir.path(), None);

    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(&cache)
        .assert()
        .success();
    // Source extracted into cache.
    let sources = cache.join("sources/sha256");
    let mut found_cabin_toml = false;
    for entry in fs::read_dir(&sources).unwrap() {
        let entry = entry.unwrap();
        if entry.path().join("cabin.toml").is_file() {
            found_cabin_toml = true;
            break;
        }
    }
    assert!(
        found_cabin_toml,
        "expected an extracted cabin.toml in cache"
    );
}

#[test]
fn published_registry_can_be_built() {
    skip_if!(
        !build_tools_available(),
        "published_registry_can_be_built",
        "ninja or a C++ compiler is not available"
    );
    let dir = TempDir::new().unwrap();
    let registry = publish_simple_package(dir.path());
    let app_main = "#include \"fmt.h\"\nint main() { say_hello(); return 0; }\n";
    write_app_using_fmt(dir.path(), Some(app_main));

    let cache = dir.path().join("cache");
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(&cache)
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    let exe = build_dir.join("dev/packages/app").join(host_exe("app"));
    assert!(exe.is_file());
    let output = std::process::Command::new(&exe).output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello from fmt"));
}
