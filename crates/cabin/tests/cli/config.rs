//! End-to-end coverage for the typed config layer:
//! discovery, parsing, merging, precedence, and metadata
//! reporting.  Tests stage temp directories for the user
//! config home (via `CABIN_CONFIG_HOME`) and the workspace
//! root so they never read or write a developer's real
//! `~/.config/cabin/config.toml`.

use super::*;
use std::path::PathBuf;

fn write_workspace_config(workspace_root: &Path, body: &str) -> PathBuf {
    let dir = workspace_root.join(".cabin");
    assert_fs::fixture::ChildPath::new(&dir)
        .create_dir_all()
        .unwrap();
    let path = dir.join("config.toml");
    assert_fs::fixture::ChildPath::new(&path)
        .write_str(body)
        .unwrap();
    path
}

fn write_user_config(home: &Path, body: &str) -> PathBuf {
    assert_fs::fixture::ChildPath::new(home)
        .create_dir_all()
        .unwrap();
    let path = home.join("config.toml");
    assert_fs::fixture::ChildPath::new(&path)
        .write_str(body)
        .unwrap();
    path
}

fn project_dir(template: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(template).unwrap();
    dir
}

const MINIMAL_PROJECT: &str = r#"[package]
name = "demo"
version = "0.1.0"
"#;

#[test]
fn metadata_without_config_emits_empty_loaded_files_block() {
    let dir = project_dir(MINIMAL_PROJECT);
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let config = &value["config"];
    assert_eq!(config["loaded_files"], serde_json::json!([]));
    assert_eq!(config["registry"], serde_json::Value::Null);
    assert_eq!(config["build"]["profile"], serde_json::Value::Null);
    assert_eq!(config["compiler_wrapper"], serde_json::Value::Null);
    assert_eq!(config["paths"]["cache_dir"], serde_json::Value::Null);
}

#[test]
fn metadata_reports_loaded_workspace_config_file() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[build]
profile = "release"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let loaded = value["config"]["loaded_files"]
        .as_array()
        .expect("loaded_files array");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0]["source"].as_str(), Some("package"));
    // The synthetic single-package package's `.cabin` dir is
    // labeled `package` rather than `workspace` because the
    // root manifest does not declare `[workspace]`.
    let profile = &value["config"]["build"]["profile"];
    assert_eq!(profile["name"].as_str(), Some("release"));
    assert_eq!(profile["value_source"].as_str(), Some("package-config"));
}

#[test]
fn metadata_workspace_root_label_is_workspace_when_root_declares_workspace() {
    // Pure-workspace root (no `[package]` table) carries
    // `[workspace]` so its `.cabin/config.toml` is labeled
    // `workspace`.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["pkg"]
"#,
        )
        .unwrap();
    dir.child("pkg/cabin.toml")
        .write_str(
            r#"[package]
name = "pkg"
version = "0.1.0"
"#,
        )
        .unwrap();
    write_workspace_config(
        dir.path(),
        r#"[build]
profile = "release"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let loaded = value["config"]["loaded_files"]
        .as_array()
        .expect("loaded_files array");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0]["source"].as_str(), Some("workspace"));
}

#[test]
fn workspace_config_overrides_user_config_for_overlapping_profile_setting() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[build]
profile = "release"
"#,
    );
    let user_home = TempDir::new().unwrap();
    write_user_config(
        user_home.path(),
        r#"[build]
profile = "dev"
"#,
    );
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let profile = &value["config"]["build"]["profile"];
    assert_eq!(profile["name"].as_str(), Some("release"));
    assert_eq!(profile["value_source"].as_str(), Some("package-config"));
    // `profiles.selected.name` reflects the resolved selection.
    assert_eq!(
        value["profiles"]["selected"]["name"].as_str(),
        Some("release")
    );
}

#[test]
fn cli_profile_overrides_config_default() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[build]
profile = "release"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--profile", "dev"])
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // The CLI choice wins.
    assert_eq!(value["profiles"]["selected"]["name"].as_str(), Some("dev"));
    // The config-recorded default still appears in the
    // `config.build.profile` block (reporting layer remains
    // unchanged).
    assert_eq!(
        value["config"]["build"]["profile"]["name"].as_str(),
        Some("release")
    );
}

#[test]
fn cabin_no_config_disables_discovery() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[build]
profile = "release"
"#,
    );
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let loaded = value["config"]["loaded_files"]
        .as_array()
        .expect("loaded_files array");
    assert!(loaded.is_empty());
    assert_eq!(value["config"]["build"]["profile"], serde_json::Value::Null);
}

#[test]
fn explicit_config_path_loads_a_specific_file() {
    let dir = project_dir(MINIMAL_PROJECT);
    let explicit = TempDir::new().unwrap();
    let explicit_path = explicit.path().join("explicit.toml");
    assert_fs::fixture::ChildPath::new(&explicit_path)
        .write_str(
            r#"[build]
profile = "release"
"#,
        )
        .unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG", &explicit_path)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let loaded = value["config"]["loaded_files"]
        .as_array()
        .expect("loaded_files array");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0]["source"].as_str(), Some("explicit"));
    assert_eq!(loaded[0]["path"].as_str(), explicit_path.to_str());
    assert_eq!(
        value["profiles"]["selected"]["name"].as_str(),
        Some("release")
    );
}

#[test]
fn explicit_config_path_missing_yields_clear_error() {
    let dir = project_dir(MINIMAL_PROJECT);
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env(
            "CABIN_CONFIG",
            "/definitely/not/a/real/path/cabin/config.toml",
        )
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("requested explicitly"),
        "expected explicit-config rejection, got: {stderr}"
    );
}

#[test]
fn invalid_top_level_table_in_config_yields_clear_error() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[networking]
mode = "offline"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("unknown top-level config table"),
        "expected unknown-table error, got: {stderr}"
    );
}

#[test]
fn auth_token_keys_in_config_are_rejected() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[auth]
token = "secret"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("does not handle credentials"),
        "expected auth rejection, got: {stderr}"
    );
}

#[test]
fn target_conditioned_config_table_yields_clear_error() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[target.'cfg(os = "linux")'.toolchain]
cxx = "clang++"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("target-conditioned config tables are not supported"),
        "expected target-conditioned rejection, got: {stderr}"
    );
}

#[test]
fn registry_path_url_conflict_yields_clear_error() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[registry]
index-path = "registry"
index-url = "https://example.com/index"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("conflicts with"),
        "expected registry conflict error, got: {stderr}"
    );
}

#[test]
fn whitespace_compiler_wrapper_in_config_yields_clear_error() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[build]
compiler-wrapper = "   "
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("compiler-wrapper") && stderr.contains("must not be empty"),
        "expected wrapper-value error, got: {stderr}"
    );
}

#[test]
fn registry_index_path_default_resolves_relative_to_workspace_config() {
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[registry]
index-path = "registry"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let registry = &value["config"]["registry"];
    assert_eq!(registry["kind"].as_str(), Some("path"));
    let resolved = registry["value"]
        .as_str()
        .expect("registry path is reported as a string");
    assert!(
        resolved.ends_with(&host_path("/.cabin/registry")),
        "expected the relative `registry` path to resolve against the config directory, got: {resolved}",
    );
    assert_eq!(registry["value_source"].as_str(), Some("package-config"));
}

#[test]
fn config_does_not_appear_in_published_package_metadata() {
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
    write_workspace_config(
        dir.path(),
        r#"[build]
profile = "release"
compiler-wrapper = "ccache"
"#,
    );
    let out = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--output-dir"])
        .arg(&out)
        .assert()
        .success();
    let body = fs::read_to_string(out.join("demo-0.1.0.json")).unwrap();
    // None of the config keys should appear in published
    // metadata - `cabin package` is supposed to drop local
    // policy entirely.
    assert!(!body.contains("compiler-wrapper"), "{body}");
    assert!(!body.contains("\"build\""), "{body}");
    assert!(!body.contains("\"config\""), "{body}");
    // The archive itself should not include `.cabin/config.toml`.
    let archive = out.join("demo-0.1.0.tar.gz");
    let archive_bytes = fs::read(&archive).unwrap();
    let decoder = flate2::read::GzDecoder::new(archive_bytes.as_slice());
    let mut tar = tar::Archive::new(decoder);
    for entry in tar.entries().unwrap() {
        let entry = entry.unwrap();
        let path = entry.path().unwrap().display().to_string();
        assert!(
            !path.contains(".cabin"),
            "archive must not contain .cabin entries, found: {path}",
        );
    }
}

#[test]
fn cli_index_path_overrides_config_registry() {
    // `cabin resolve` succeeds when there are no versioned
    // dependencies regardless of index settings; this test
    // verifies that *when both are present* the CLI flag is
    // honored.  We point the CLI at a temp index and the
    // config at a non-existent path; if the config layer were
    // ever consulted we would see a different error.
    let dir = project_dir(MINIMAL_PROJECT);
    write_workspace_config(
        dir.path(),
        r#"[registry]
index-path = "/definitely/not/a/real/path"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let cli_index = TempDir::new().unwrap();
    // No versioned deps means resolve() short-circuits before
    // touching the index, so success here confirms the
    // CLI value is plumbed through.
    cabin_with_config()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(cli_index.path())
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
}

#[test]
fn no_index_anywhere_for_a_versioned_dep_mentions_config() {
    // When versioned deps require an index source and neither
    // CLI nor config supplies one, the error wording should
    // mention all three escapes (CLI flag, env, config) so
    // the user knows the config layer is an option.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("--index-path") && stderr.contains("[registry]"),
        "expected index-source error to mention CLI flag and config, got: {stderr}"
    );
}

/// Stage a fake tool that prints fixed `--version` output -
/// duplicated from `compiler_cache::fake_tool_with_output`
/// because cross-module visibility would force a much larger
/// refactor than this helper warrants.
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
fn metadata_reports_config_supplied_toolchain_cxx() {
    let dir = project_dir(MINIMAL_PROJECT);
    let bin = TempDir::new().unwrap();
    let cxx = fake_tool_with_output(bin.path(), "fake-clang++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    write_workspace_config(
        dir.path(),
        &format!(
            r#"[toolchain]
cxx = "{cxx}"
"#,
            cxx = cxx.display()
        ),
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // The toolchain block reports the resolved spec with its
    // source label.
    let cxx_view = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(cxx_view["spec"].as_str(), Some(cxx.to_str().unwrap()));
    assert_eq!(cxx_view["source"].as_str(), Some("package-config"));
    // The config block records the same value with its
    // dedicated provenance label.
    assert_eq!(
        value["config"]["toolchain"]["cxx"]["value_source"].as_str(),
        Some("package-config")
    );
}

#[cfg(unix)]
#[test]
fn cxx_env_overrides_config_toolchain_cxx() {
    let dir = project_dir(MINIMAL_PROJECT);
    let bin = TempDir::new().unwrap();
    let env_cxx = fake_tool_with_output(bin.path(), "env-clang++", "clang version 17.0.6\n", "", 0);
    let config_cxx = fake_tool_with_output(
        bin.path(),
        "config-clang++",
        "clang version 17.0.6\n",
        "",
        0,
    );
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    write_workspace_config(
        dir.path(),
        &format!(
            r#"[toolchain]
cxx = "{cxx}"
"#,
            cxx = config_cxx.display()
        ),
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .env("PATH", bin.path())
        .env("CXX", &env_cxx)
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let cxx_view = &value["toolchain"]["tools"]["cxx"];
    assert_eq!(cxx_view["spec"].as_str(), Some(env_cxx.to_str().unwrap()));
    assert_eq!(cxx_view["source"].as_str(), Some("env"));
}

#[cfg(unix)]
#[test]
fn config_supplies_compiler_wrapper_default() {
    let dir = project_dir(MINIMAL_PROJECT);
    let bin = TempDir::new().unwrap();
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    let _ccache = fake_tool_with_output(bin.path(), "ccache", "ccache version 4.10.2\n", "", 0);
    write_workspace_config(
        dir.path(),
        r#"[build]
compiler-wrapper = "ccache"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
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
    assert_eq!(wrapper["source"].as_str(), Some("package-config"));
    assert_eq!(
        value["config"]["compiler_wrapper"]["request"].as_str(),
        Some("ccache")
    );
}

#[cfg(unix)]
#[test]
fn no_compiler_wrapper_flag_overrides_config_default() {
    let dir = project_dir(MINIMAL_PROJECT);
    let bin = TempDir::new().unwrap();
    let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
    write_workspace_config(
        dir.path(),
        r#"[build]
compiler-wrapper = "ccache"
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--no-compiler-wrapper"])
        .env("CABIN_CONFIG_HOME", user_home.path())
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .env_remove("CABIN_COMPILER_WRAPPER")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // CLI wins → no wrapper applies even though config asked
    // for ccache.
    assert!(value["toolchain"]["compiler_wrapper"].is_null());
    // The config block still records the default for
    // visibility.
    assert_eq!(
        value["config"]["compiler_wrapper"]["request"].as_str(),
        Some("ccache")
    );
}

#[test]
fn config_does_not_change_lockfile_layout() {
    // A `[registry]` config setting must not bleed into the
    // lockfile shape: existing lockfiles continue to work and
    // no config-derived fields appear in the produced
    // `cabin.lock`.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    write_workspace_config(
        dir.path(),
        r#"[registry]
index-path = "registry"
"#,
    );
    let user_home = TempDir::new().unwrap();
    cabin_with_config()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    // No versioned deps → no lockfile is written.
    assert!(!dir.path().join("cabin.lock").exists());
}
