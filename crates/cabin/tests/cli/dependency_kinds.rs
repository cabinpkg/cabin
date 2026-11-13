//! End-to-end coverage for the dependency-kind feature: every
//! kind shows up in `cabin metadata`, the resolver only sees
//! resolvable kinds, dev deps stay declaration-only outside
//! `cabin test`, and unsupported syntax is rejected with clear
//! errors.

use super::*;

/// Single-package manifest declaring a dependency of every kind.
const MIXED_KINDS_MANIFEST: &str = r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = ">=10"

[dev-dependencies]
gtest = "^1.14"
"#;

/// Find a dep entry on a `cabin metadata` package view by
/// `(name, dependency_kind)`.
fn dep_entry<'a>(package: &'a serde_json::Value, name: &str, kind: &str) -> &'a serde_json::Value {
    package["dependencies"]
        .as_array()
        .expect("dependencies array")
        .iter()
        .find(|d| d["name"] == name && d["dependency_kind"] == kind)
        .unwrap_or_else(|| panic!("dep {name:?} of kind {kind:?} not found in {package}"))
}

#[test]
fn metadata_lists_every_dependency_kind_with_explicit_kind_field() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(MIXED_KINDS_MANIFEST)
        .unwrap();
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let demo = package_in(&value, "demo");
    for (name, kind) in [("fmt", "normal"), ("gtest", "dev")] {
        let dep = dep_entry(demo, name, kind);
        assert_eq!(dep["kind"], "version", "{name} should be a version source");
    }
}

#[test]
fn metadata_dependency_listing_is_sorted_by_kind_then_name() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(MIXED_KINDS_MANIFEST)
        .unwrap();
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let demo = package_in(&value, "demo");
    let listed: Vec<(String, String)> = demo["dependencies"]
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
    // Canonical kind order: normal, dev.  Within each kind,
    // names are sorted ascending.
    assert_eq!(
        listed,
        vec![
            ("normal".into(), "fmt".into()),
            ("dev".into(), "gtest".into()),
        ]
    );
}

#[test]
fn metadata_keeps_existing_shape_for_dependencies_only_manifest() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"
"#,
        )
        .unwrap();
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let app = package_in(&value, "app");
    let deps = app["dependencies"].as_array().unwrap();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0]["name"], "fmt");
    assert_eq!(deps[0]["dependency_kind"], "normal");
    assert_eq!(deps[0]["kind"], "version");
}

#[test]
fn resolve_excludes_dev_dependencies() {
    // A manifest with a normal dep plus a dev-only dep that
    // the index does *not* declare.  With dev correctly
    // excluded, resolution succeeds; if the walker leaked the
    // dev requirement, resolution would fail with `package
    // "gtest" not found`.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10 <11"

[dev-dependencies]
gtest = "^1.14"
"#,
        )
        .unwrap();
    write_index_entry_no_source(&dir.path().join("index"), "fmt", "10.2.1", &"0".repeat(64));
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
        !stdout.contains("gtest"),
        "dev dep gtest must not enter ordinary resolution: {stdout}"
    );
}

/// A version published while Cabin read `system = true` lists the
/// libraries it took from pkg-config, in an index that carries them as
/// the hosted registry's does.  Nothing supplies them now, so a build
/// that selects the version stops, and the package's other versions
/// resolve as before.
#[test]
fn a_version_published_with_system_dependencies_is_refused() {
    let dir = TempDir::new().unwrap();
    let version = |system_dependencies: &str| {
        format!(
            r#"{{
      "dependencies": {{}},{system_dependencies}
      "yanked": false,
      "revision": "0000000000000000",
      "revisions": {{
        "0000000000000000": {{
          "checksum": "sha256:{}",
          "published-at": "2026-01-01T00:00:00Z",
          "source": {{ "type": "archive", "path": "unfetched.zip", "format": "zip" }}
        }}
      }}
    }}"#,
            "0".repeat(64)
        )
    };
    dir.child("index/fmt.json")
        .write_str(&format!(
            r#"{{
  "schema": 1,
  "name": "fmt",
  "versions": {{
    "10.1.0": {},
    "10.2.0": {}
  }}
}}"#,
            version(""),
            version(
                r#"
      "system-dependencies": { "zlib": { "version": ">=1.2", "dependency_kind": "normal" } },"#
            ),
        ))
        .unwrap();
    let resolve = |requirement: &str| {
        dir.child("cabin.toml")
            .write_str(&format!(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = "{requirement}"
"#
            ))
            .unwrap();
        let mut cmd = cabin();
        cmd.args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"));
        cmd.assert()
    };

    let refused = stderr_with_wrapping_joined(&resolve(">=10").failure());
    assert!(
        refused.contains("`fmt10.2.0`waspublishedwithsystemdependencies(`zlib`)"),
        "{refused}"
    );
    resolve("=10.1.0")
        .success()
        .stdout(predicate::str::contains("10.1.0"));
}

#[test]
fn workspace_inheritance_per_kind_is_validated_kind_specifically() {
    // `[dev-dependencies] foo = { workspace = true }` must
    // *not* fall back to `[workspace.dependencies]`.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dev-dependencies]
fmt = { workspace = true }
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
        stderr.contains("[workspace.dev-dependencies]"),
        "error should name the missing workspace table: {stderr}"
    );
    assert!(
        stderr.contains("[dev-dependencies]"),
        "error should name the declaring section: {stderr}"
    );
}

#[test]
fn package_metadata_round_trips_every_dependency_kind() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(MIXED_KINDS_MANIFEST)
        .unwrap();
    let out = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--output-dir"])
        .arg(&out)
        .assert()
        .success();
    let metadata_path = out.join("demo-0.1.0.json");
    let metadata: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&metadata_path).unwrap()).unwrap();
    assert_eq!(metadata["dependencies"]["fmt"].as_str().unwrap(), ">=10");
    assert_eq!(
        metadata["dev-dependencies"]["gtest"].as_str().unwrap(),
        "^1.14"
    );
}
