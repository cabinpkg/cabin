//! End-to-end coverage for standard-aware version preference
//! (`[resolver] incompatible-standards`).
//!
//! A local index carries two versions of `dep`: `2.0.0` declares
//! `interface-cxx-standard = "c++20"` and `1.0.0` declares `"c++17"`.
//! The consumer compiles C++17.  Under `fallback` (the default) the
//! resolver prefers the older compatible `1.0.0` and reports the
//! hold-back naming the newer version and its requirement; under
//! `allow` (config or env) it takes the newest `2.0.0` and reports
//! nothing.  Resolving never builds, so these tests need no toolchain.

use super::*;

/// Write a two-version local index entry for `dep`, each version's
/// `standards` table declaring a single library target's
/// `interface-cxx-standard`.  No `source` block: `cabin update` only
/// resolves and writes the lockfile, it never fetches.
fn write_dep_index(index_dir: &Path, newer_cxx: &str, older_cxx: &str) {
    let body = format!(
        r#"{{
  "schema": 1,
  "name": "dep",
  "versions": {{
    "2.0.0": {{
      "dependencies": {{}},
      "yanked": false,
      "standards": {{ "targets": {{ "dep": {{ "interface": {{ "c++": {{ "min": "{newer_cxx}" }} }} }} }} }}
    }},
    "1.0.0": {{
      "dependencies": {{}},
      "yanked": false,
      "standards": {{ "targets": {{ "dep": {{ "interface": {{ "c++": {{ "min": "{older_cxx}" }} }} }} }} }}
    }}
  }}
}}"#
    );
    assert_fs::fixture::ChildPath::new(index_dir.join("dep.json"))
        .write_str(&body)
        .unwrap();
}

/// Write a consumer `app` compiling C++17 that depends on `dep`.
/// `config` (when non-empty) is written to `app/.cabin/config.toml`.
fn write_cxx17_app(dir: &Path, config: &str) {
    assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
dep = ">=1.0.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    if !config.is_empty() {
        assert_fs::fixture::ChildPath::new(dir.join("app/.cabin/config.toml"))
            .write_str(config)
            .unwrap();
    }
}

fn update_json(app_toml: &Path, index: &Path) -> serde_json::Value {
    run_json(
        cabin()
            .args(["update", "--manifest-path"])
            .arg(app_toml)
            .arg("--index-path")
            .arg(index)
            .args(["--format", "json"]),
    )
}

fn dep_version(value: &serde_json::Value) -> String {
    value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "dep")
        .unwrap()["version"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The default (`fallback`) prefers the older compatible version and
/// reports the hold-back naming the selected version, the newest
/// available, and the requirement that held it back.
#[test]
fn fallback_default_holds_back_incompatible_newer_version() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    write_dep_index(&index, "c++20", "c++17");
    write_cxx17_app(dir.path(), "");

    let value = update_json(&dir.path().join("app/cabin.toml"), &index);
    assert_eq!(dep_version(&value), "1.0.0");
    assert_eq!(
        value["held_back"][0]["message"],
        "dep v1.0.0 (available: v2.0.0, requires interface c++20)"
    );
}

/// The human format renders the hold-back under a dedicated heading.
#[test]
fn fallback_human_output_renders_held_back_section() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    write_dep_index(&index, "c++20", "c++17");
    write_cxx17_app(dir.path(), "");

    let output = cabin()
        .args(["update", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Held back for standard compatibility:"),
        "missing held-back heading in: {stdout}"
    );
    assert!(
        stdout.contains("dep v1.0.0 (available: v2.0.0, requires interface c++20)"),
        "missing held-back line in: {stdout}"
    );
}

/// `[resolver] incompatible-standards = "allow"` disables the
/// preference: the newest version is selected and nothing is held back.
#[test]
fn allow_via_config_selects_newest() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    write_dep_index(&index, "c++20", "c++17");
    write_cxx17_app(
        dir.path(),
        "[resolver]\nincompatible-standards = \"allow\"\n",
    );

    // `cabin_with_config()` re-enables config discovery (the default
    // test harness sets `CABIN_NO_CONFIG=1`).
    let value = run_json(
        cabin_with_config()
            .args(["update", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(&index)
            .args(["--format", "json"]),
    );
    assert_eq!(dep_version(&value), "2.0.0");
    assert!(value["held_back"].as_array().unwrap().is_empty());
}

/// `CABIN_RESOLVER_INCOMPATIBLE_STANDARDS=allow` overrides the config
/// default the same way, and its vocabulary is validated.
#[test]
fn allow_via_env_selects_newest_and_rejects_bad_value() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    write_dep_index(&index, "c++20", "c++17");
    write_cxx17_app(dir.path(), "");
    let app_toml = dir.path().join("app/cabin.toml");

    let value = run_json(
        cabin()
            .env("CABIN_RESOLVER_INCOMPATIBLE_STANDARDS", "allow")
            .args(["update", "--manifest-path"])
            .arg(&app_toml)
            .arg("--index-path")
            .arg(&index)
            .args(["--format", "json"]),
    );
    assert_eq!(dep_version(&value), "2.0.0");

    cabin()
        .env("CABIN_RESOLVER_INCOMPATIBLE_STANDARDS", "warn")
        .args(["update", "--manifest-path"])
        .arg(&app_toml)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .stderr(predicate::str::contains("expected one of: allow, fallback"));
}
