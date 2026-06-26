//! End-to-end coverage for the dependency-kind feature: every
//! kind shows up in `cabin metadata`, the resolver only sees
//! resolvable kinds, dev deps stay declaration-only, system
//! deps never reach the registry, and unsupported syntax is
//! rejected with clear errors.

use super::*;

/// Single-package manifest declaring one dep of every kind.
/// Used by the `cabin metadata` shape tests below.
const MIXED_KINDS_MANIFEST: &str = r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = ">=10"
zlib = { version = ">=1.2", system = true }
openssl = { version = ">=3", system = true }

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

/// Run `cabin metadata` on the mixed-kinds manifest with the bundled
/// fake pkg-config resolving its `zlib` / `openssl` system deps, so
/// the command succeeds on hosts without a real pkg-config or those
/// libraries (e.g.  Windows).  The resolved values are irrelevant to
/// the assertions below, which only inspect *declared* metadata.
fn run_mixed_kinds_metadata(manifest_path: &Path) -> serde_json::Value {
    let fixtures = TempDir::new().expect("fixtures tempdir");
    for (name, version) in [("zlib", "1.3"), ("openssl", "3.2")] {
        assert_fs::fixture::ChildPath::new(fixtures.path().join(format!("{name}.json")))
            .write_str(&format!(
                r#"{{ "version": "{version}", "cflags": "", "libs": "" }}"#
            ))
            .unwrap();
    }
    let output = cabin()
        .env(
            "CABIN_PKG_CONFIG",
            workspace_test_bin("cabin-system-deps-fake-pkg-config"),
        )
        .env("CABIN_FAKE_PKG_CONFIG_FIXTURES", fixtures.path())
        .args(["metadata", "--manifest-path"])
        .arg(manifest_path)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("expected valid JSON, got error {err} for: {stdout}"))
}

#[test]
fn metadata_lists_every_dependency_kind_with_explicit_kind_field() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(MIXED_KINDS_MANIFEST)
        .unwrap();
    let value = run_mixed_kinds_metadata(&dir.path().join("cabin.toml"));
    let demo = package_in(&value, "demo");
    // Each Cabin package dep is listed once with an explicit
    // `dependency_kind` field.
    for (name, kind) in [("fmt", "normal"), ("gtest", "dev")] {
        let dep = dep_entry(demo, name, kind);
        assert_eq!(dep["kind"], "version", "{name} should be a version source");
    }
    // System deps are reported separately, not under `dependencies`.
    let system = demo["system_dependencies"]
        .as_array()
        .expect("system_dependencies array");
    assert_eq!(system.len(), 2);
    let by_name: std::collections::BTreeMap<&str, &serde_json::Value> = system
        .iter()
        .map(|sd| (sd["name"].as_str().expect("system dep name"), sd))
        .collect();
    assert_eq!(by_name["zlib"]["version"], ">=1.2");
    assert!(
        by_name["zlib"].get("required").is_none(),
        "system dep metadata must not expose a `required` field: {:?}",
        by_name["zlib"],
    );
    assert_eq!(by_name["zlib"]["dependency_kind"], "normal");
    assert_eq!(by_name["openssl"]["version"], ">=3");
    assert!(by_name["openssl"].get("required").is_none());
    assert_eq!(by_name["openssl"]["dependency_kind"], "normal");
}

#[test]
fn metadata_dependency_listing_is_sorted_by_kind_then_name() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(MIXED_KINDS_MANIFEST)
        .unwrap();
    let value = run_mixed_kinds_metadata(&dir.path().join("cabin.toml"));
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
    // names are sorted ascending (BTreeMap iteration).
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
    // A manifest that only uses `[dependencies]` should still
    // surface its single dep through the metadata view, with
    // the same source/kind layout existing tooling already
    // expects.
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
    assert!(
        app["system_dependencies"].is_null(),
        "system_dependencies must be omitted when empty: got {app}"
    );
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
    // Index covers fmt but *not* gtest.  If dev deps were
    // resolved, `gtest` would be missing.
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

#[test]
fn resolve_does_not_send_system_dependencies_to_resolver() {
    // System dependencies must never reach the resolver, so
    // declaring an unrelated system dep cannot break a resolve
    // run that is otherwise only about Cabin packages.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"
zlib = { version = ">=1.2", system = true }
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
    // The lockfile / report mentions fmt but not zlib (system
    // deps never enter the resolver).
    assert!(stdout.contains("fmt"));
    assert!(
        !stdout.contains("zlib"),
        "system dep zlib must not appear in resolver output: {stdout}"
    );
}

#[test]
fn optional_dependency_in_system_section_is_rejected_at_cli() {
    // Optional Cabin package dependencies are supported for
    // normal kind.  System dependencies (`system = true`)
    // remain declaration-only and may *not* carry `optional =
    // true`.  Mixing the flags surfaces an explicit `system =
    // true is incompatible with optional` error.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
zlib = { version = ">=1.2", system = true, optional = true }
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
        stderr.contains("optional"),
        "expected an error mentioning the unsupported optional system dep, got: {stderr}"
    );
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
    // `cabin package` writes canonical metadata; we read it
    // back as JSON and confirm each kind survives the
    // round-trip.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(MIXED_KINDS_MANIFEST)
        .unwrap();
    // `cabin package` rejects path / workspace deps and
    // requires a writable output dir.
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
    let zlib = &metadata["system-dependencies"]["zlib"];
    assert_eq!(zlib["version"].as_str().unwrap(), ">=1.2");
    assert!(
        zlib.get("required").is_none(),
        "canonical metadata must not carry `required`: {zlib:?}",
    );
    assert_eq!(zlib["dependency_kind"].as_str().unwrap(), "normal");
    let openssl = &metadata["system-dependencies"]["openssl"];
    assert_eq!(openssl["version"].as_str().unwrap(), ">=3");
    assert!(openssl.get("required").is_none());
    assert_eq!(openssl["dependency_kind"].as_str().unwrap(), "normal");
}
