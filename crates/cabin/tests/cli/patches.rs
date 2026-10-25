//! End-to-end coverage for the patch / override layer.
//!
//! Each test stages a temp workspace plus a sibling
//! "patched fork" directory that holds a real `cabin.toml`,
//! then drives `cabin metadata` (or `cabin package`) and
//! inspects the resulting JSON / errors.  No tests here
//! perform network access; the patch path is the only source
//! of truth.

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

fn write_root_manifest(root: &Path, body: &str) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(body)
        .unwrap();
}

fn write_patched_fork(parent: &Path, dir_name: &str, body: &str) -> PathBuf {
    let path = parent.join(dir_name);
    assert_fs::fixture::ChildPath::new(path.join("cabin.toml"))
        .write_str(body)
        .unwrap();
    path
}

#[test]
fn metadata_reports_active_manifest_patch() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt" }
"#,
    );
    write_patched_fork(
        parent.path(),
        "fmt",
        r#"[package]
name = "fmt"
version = "10.2.1"
"#,
    );
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let patches = value["patches"].as_array().expect("patches array");
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0]["package"].as_str(), Some("fmt"));
    assert_eq!(patches[0]["version"].as_str(), Some("10.2.1"));
    assert_eq!(patches[0]["kind"].as_str(), Some("path"));
    assert_eq!(patches[0]["provenance"].as_str(), Some("manifest"));
    let pkg_names: Vec<&str> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(pkg_names.contains(&"fmt"));
}

#[test]
fn inspection_commands_load_chained_patched_forks() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = "^10"

[patch]
fmt = { path = "../fmt" }
base = { path = "../base" }
"#,
    );
    write_patched_fork(
        parent.path(),
        "fmt",
        r#"[package]
name = "fmt"
version = "10.2.1"

[dependencies]
base = "^1"
"#,
    );
    write_patched_fork(
        parent.path(),
        "base",
        r#"[package]
name = "base"
version = "1.0.0"
"#,
    );
    let manifest = root.join("cabin.toml");
    cabin()
        .args(["tree", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "base v1.0.0 [normal] (patched via manifest)",
        ));
    cabin()
        .args(["explain", "--manifest-path"])
        .arg(&manifest)
        .args(["package", "base"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "app v0.1.0 -> fmt v10.2.1 [normal] -> base v1.0.0 [normal]",
        ));
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success();
    let value: serde_json::Value = serde_json::from_slice(&assertion.get_output().stdout).unwrap();
    assert!(
        value["packages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "base")
    );
}

#[test]
fn metadata_reports_config_supplied_patch_overriding_manifest() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt-manifest" }
"#,
    );
    write_patched_fork(
        parent.path(),
        "fmt-manifest",
        r#"[package]
name = "fmt"
version = "10.0.0"
"#,
    );
    // Config-supplied patches resolve relative to the
    // *config file's* directory (`<root>/.cabin`), so the
    // fixture lives at `<root>/fmt-config` and the path is
    // written as `../fmt-config`.
    write_patched_fork(
        &root,
        "fmt-config",
        r#"[package]
name = "fmt"
version = "10.5.0"
"#,
    );
    write_workspace_config(
        &root,
        r#"[patch]
fmt = { path = "../fmt-config" }
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let patches = value["patches"].as_array().expect("patches array");
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0]["version"].as_str(), Some("10.5.0"));
    assert_eq!(patches[0]["provenance"].as_str(), Some("package-config"));
}

#[test]
fn no_patches_flag_disables_active_patches() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt" }
"#,
    );
    write_patched_fork(
        parent.path(),
        "fmt",
        r#"[package]
name = "fmt"
version = "10.2.1"
"#,
    );
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .arg("--no-patches")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(value["patches"].as_array().unwrap().is_empty());
}

#[test]
fn missing_patch_path_yields_clear_error() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt" }
"#,
    );
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("does not contain a cabin.toml"),
        "expected missing-manifest error, got: {stderr}"
    );
}

#[test]
fn patch_package_name_mismatch_yields_clear_error() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[patch]
fmt = { path = "../fmt-fork" }
"#,
    );
    write_patched_fork(
        parent.path(),
        "fmt-fork",
        r#"[package]
name = "wrong-name"
version = "10.2.1"
"#,
    );
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("patch package name must match `fmt`"),
        "expected name mismatch, got: {stderr}"
    );
}

#[test]
fn patch_version_mismatch_yields_clear_error() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=11.0.0 <12.0.0"

[patch]
fmt = { path = "../fmt" }
"#,
    );
    write_patched_fork(
        parent.path(),
        "fmt",
        r#"[package]
name = "fmt"
version = "10.0.0"
"#,
    );
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("does not satisfy dependency requirement"),
        "expected version mismatch, got: {stderr}"
    );
}

#[test]
fn package_rejects_manifest_with_patch_table() {
    let parent = TempDir::new().unwrap();
    let dir = parent.path().join("app");
    write_root_manifest(
        &dir,
        r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[target.app]
type = "library"
sources = ["src/lib.cc"]

[patch]
fmt = { path = "../fmt" }
"#,
    );
    assert_fs::fixture::ChildPath::new(dir.join("src/lib.cc"))
        .write_str("int app() { return 0; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.join("cabin.toml"))
        .args(["--output-dir"])
        .arg(dir.join("dist"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("declares a `[patch]` table"),
        "expected patch-rejection error, got: {stderr}"
    );
}

#[test]
fn member_manifest_with_patch_table_is_rejected() {
    let dir = TempDir::new().unwrap();
    write_root_manifest(
        dir.path(),
        r#"[workspace]
members = ["member"]
"#,
    );
    dir.child("member/cabin.toml")
        .write_str(
            r#"[package]
name = "member"
version = "0.1.0"

[patch]
fmt = { path = "../fmt" }
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
        // miette's `GraphicalReportHandler` may hard-wrap the
        // long message at the terminal width, so the literal
        // "workspace root manifest" can be split across lines
        // with a `│` continuation prefix.  Pin the load-bearing
        // phrase up to the wrap point instead.
        stderr.contains("only appear in the workspace root"),
        "expected member-rejection, got: {stderr}"
    );
}

#[test]
fn metadata_reports_active_source_replacement() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"
"#,
    );
    write_workspace_config(
        &root,
        r#"[source-replacement]
"https://example.com/index" = { index-path = "../mirror" }
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let entries = value["source_replacements"]
        .as_array()
        .expect("source_replacements array");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["original"].as_str(),
        Some("https://example.com/index")
    );
    assert_eq!(entries[0]["replacement_kind"].as_str(), Some("index-path"));
}

#[test]
fn explain_source_no_patches_still_reports_configured_source_replacements() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"
"#,
    );
    write_workspace_config(
        &root,
        r#"[source-replacement]
"https://example.com/index" = { index-path = "../mirror" }
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["explain", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .args(["--format", "json", "--no-patches", "source", "app"])
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let entries = value["source_replacements"]
        .as_array()
        .expect("source_replacements array");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].as_str(),
        Some("https://example.com/index -> ../mirror (package-config)")
    );
}

#[test]
fn source_replacement_credentials_in_url_yield_clear_error() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"
"#,
    );
    write_workspace_config(
        &root,
        r#"[source-replacement]
"https://user:pw@example.com/index" = { index-path = "../mirror" }
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("must not contain credentials"),
        "expected credential rejection, got: {stderr}"
    );
}

/// Lock `app` (depending on `fmt`) once with no patches, then add a
/// `[patch]` whose fork supplies `fmt` at the locked version.  The
/// package set is unchanged and nothing is left to resolve; only
/// the recorded patch state differs from the active policy.
/// Returns the root and index directories.
fn lockfile_then_add_covering_patch(parent: &Path) -> (PathBuf, PathBuf) {
    let root = parent.join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
    );
    let index = parent.join("index");
    assert_fs::fixture::ChildPath::new(index.join("fmt.json"))
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();

    // The fork supplies fmt at 10.2.0 - the same version the
    // resolver picked from the index.
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt" }
"#,
    );
    write_patched_fork(
        parent,
        "fmt",
        r#"[package]
name = "fmt"
version = "10.2.0"
"#,
    );
    (root, index)
}

#[test]
fn locked_fails_when_patch_policy_changed_after_lockfile() {
    let parent = TempDir::new().unwrap();
    let (root, index) = lockfile_then_add_covering_patch(parent.path());
    let assertion = cabin()
        .args(["resolve", "--locked", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("patch / source-replacement policy differs"),
        "expected patch staleness error, got: {stderr}"
    );
}

/// The patch-staleness check does not depend on the resolver
/// running: the patch covers the only versioned dependency, so the
/// artifact commands have nothing to resolve or fetch, yet `--locked`
/// must still refuse - and leave the lockfile bytes alone.
fn parse_lock(path: &Path) -> cabin_lockfile::Lockfile {
    cabin_lockfile::read_lockfile(path).unwrap()
}

/// Pins a known modification time so a later check can tell "not
/// rewritten" from "rewritten with the same bytes".
fn pin_mtime(path: &Path) -> std::time::SystemTime {
    let stamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    stamp
}

/// Without `--locked`, a run with nothing to resolve leaves the
/// lockfile alone, and the artifact commands never read it.
#[test]
fn unlocked_commands_leave_the_lockfile_alone_with_nothing_to_resolve() {
    let parent = TempDir::new().unwrap();
    let (root, index) = lockfile_then_add_covering_patch(parent.path());
    let lockfile = root.join("cabin.lock");
    let cache_dir = parent.path().join("cache");
    let stamp = pin_mtime(&lockfile);
    // `build` shares the helper and is pinned by the locked test
    // below; the fixture's `app` has no target for it to build.
    for command in ["resolve", "fetch", "vendor"] {
        let mut cmd = cabin();
        cmd.args([command, "--manifest-path"])
            .arg(root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&index);
        if command != "resolve" {
            cmd.arg("--cache-dir").arg(&cache_dir);
        }
        cmd.assert().success();
        assert_eq!(
            fs::metadata(&lockfile).unwrap().modified().unwrap(),
            stamp,
            "{command} rewrote the lockfile"
        );
    }
    fs::write(&lockfile, "not a lockfile").unwrap();
    for command in ["fetch", "vendor"] {
        cabin()
            .args([command, "--manifest-path"])
            .arg(root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&index)
            .arg("--cache-dir")
            .arg(&cache_dir)
            .assert()
            .success();
    }
}

/// The remedy the `--locked` error names must work: `cabin update`
/// with nothing to resolve reconciles the stale patch record and
/// keeps every `[[package]]` entry it cannot judge; a matching
/// record is left untouched, as is a missing lockfile.
#[test]
fn update_refreshes_a_stale_patch_record_with_nothing_to_resolve() {
    let parent = TempDir::new().unwrap();
    let (root, index) = lockfile_then_add_covering_patch(parent.path());
    let lockfile = root.join("cabin.lock");
    let update = || {
        cabin()
            .args(["update", "--manifest-path"])
            .arg(root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&index)
            .assert()
            .success();
    };
    let before = parse_lock(&lockfile);
    update();
    let refreshed = parse_lock(&lockfile);
    assert_eq!(
        refreshed.packages, before.packages,
        "entries must be kept verbatim"
    );
    assert_eq!(
        refreshed.patches.len(),
        1,
        "expected the patch record:\n{refreshed:?}"
    );

    let stamp = pin_mtime(&lockfile);
    update();
    for command in ["resolve", "fetch"] {
        let mut cmd = cabin();
        cmd.args([command, "--locked", "--manifest-path"])
            .arg(root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&index);
        if command != "resolve" {
            cmd.arg("--cache-dir").arg(parent.path().join("cache"));
        }
        cmd.assert().success();
    }
    assert_eq!(
        fs::metadata(&lockfile).unwrap().modified().unwrap(),
        stamp,
        "a matching record must not be rewritten"
    );

    fs::remove_file(&lockfile).unwrap();
    update();
    assert!(!lockfile.exists(), "update must not create a lockfile here");
}

const FMT_INDEX_WITH_HELPER: &str = r#"{
  "schema": 1,
  "name": "fmt",
  "versions": {
    "10.2.0": { "dependencies": { "helper": "^1" }, "yanked": false, "revision": "bbbbbbbbbbbbbbbb", "revisions": { "bbbbbbbbbbbbbbbb": { "checksum": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "published-at": "2026-01-01T00:00:00Z", "source": { "type": "archive", "path": "unfetched.zip", "format": "zip" } } } }
  }
}"#;

fn locked_names(lockfile: &Path) -> Vec<String> {
    parse_lock(lockfile)
        .packages
        .iter()
        .map(|locked| locked.name.to_string())
        .collect()
}

/// The lockfile is workspace-wide.  A member-scoped `cabin update`
/// with nothing to resolve reconciles the patch record only: every
/// entry survives, the other member's pins included, and nothing
/// promises a canonical workspace-wide lockfile - the selected
/// member passes `--locked`, a whole-workspace `resolve --locked`
/// still refuses.  `cabin update --workspace` resolves for real and
/// writes the canonical lockfile; once patches cover every
/// dependency of the workspace it, too, only reconciles the record.
#[test]
fn workspace_update_after_a_patch_covers_a_members_dependencies() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("ws");
    let workspace = "[workspace]\nmembers = [\"app\", \"other\"]\ndefault-members = [\"app\"]\n";
    let write_workspace_manifest = |patches: &str| {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(&format!("{workspace}{patches}"))
            .unwrap();
    };
    write_workspace_manifest("");
    write_root_manifest(
        &root.join("app"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfmt = \">=10.0.0 <11.0.0\"\n",
    );
    write_root_manifest(
        &root.join("other"),
        "[package]\nname = \"other\"\nversion = \"0.1.0\"\n\n[dependencies]\nspdlog = \"^1\"\n",
    );
    let index = parent.path().join("index");
    assert_fs::fixture::ChildPath::new(index.join("fmt.json"))
        .write_str(FMT_INDEX_WITH_HELPER)
        .unwrap();
    write_index_entry_no_source(&index, "helper", "1.0.0", &"d".repeat(64));
    write_index_entry_no_source(&index, "spdlog", "1.13.0", &"1".repeat(64));
    let run = |args: &[&str]| {
        let mut cmd = cabin();
        cmd.args(args)
            .arg("--manifest-path")
            .arg(root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&index);
        cmd.assert()
    };
    let lockfile = root.join("cabin.lock");

    run(&["resolve", "--workspace"]).success();
    let original = parse_lock(&lockfile);
    assert_eq!(locked_names(&lockfile), ["fmt", "helper", "spdlog"]);

    // The fork drops fmt's registry dependency, so helper is now
    // needed by nothing.
    write_workspace_manifest("\n[patch]\nfmt = { path = \"../fmt\" }\n");
    write_patched_fork(
        parent.path(),
        "fmt",
        "[package]\nname = \"fmt\"\nversion = \"10.2.0\"\n",
    );
    run(&["update", "--default-members"]).success();
    let scoped = parse_lock(&lockfile);
    assert_eq!(
        scoped.packages, original.packages,
        "scoped update must keep every entry"
    );
    assert_eq!(scoped.patches.len(), 1, "{scoped:?}");
    run(&["resolve", "--locked", "--default-members"]).success();
    let stale = run(&["resolve", "--locked", "--workspace"]).failure();
    assert!(
        String::from_utf8_lossy(&stale.get_output().stderr).contains("is stale"),
        "a whole-workspace --locked resolve must refuse the non-canonical lockfile"
    );
    assert_eq!(
        scoped,
        parse_lock(&lockfile),
        "--locked must not modify the lockfile"
    );

    run(&["update", "--workspace"]).success();
    assert_eq!(
        locked_names(&lockfile),
        ["spdlog"],
        "obsolete entries must go"
    );
    assert_eq!(
        parse_lock(&lockfile).packages[0],
        original.packages[2],
        "spdlog must stay pinned as before"
    );
    run(&["resolve", "--locked", "--workspace"]).success();

    // Patches now cover every dependency of the workspace: a
    // whole-workspace update has nothing to resolve either and
    // reconciles the record only.
    write_workspace_manifest(
        "\n[patch]\nfmt = { path = \"../fmt\" }\nspdlog = { path = \"../spdlog\" }\n",
    );
    write_patched_fork(
        parent.path(),
        "spdlog",
        "[package]\nname = \"spdlog\"\nversion = \"1.13.0\"\n",
    );
    run(&["update", "--workspace"]).success();
    let all_patched = parse_lock(&lockfile);
    assert_eq!(
        locked_names(&lockfile),
        ["spdlog"],
        "nothing resolved, so nothing pruned"
    );
    assert_eq!(all_patched.patches.len(), 2, "{all_patched:?}");
    run(&["resolve", "--locked", "--workspace"]).success();
    assert_eq!(
        all_patched,
        parse_lock(&lockfile),
        "--locked must not modify the lockfile"
    );
}

#[test]
fn locked_artifact_commands_refuse_a_patch_policy_change_with_nothing_to_resolve() {
    let parent = TempDir::new().unwrap();
    let (root, index) = lockfile_then_add_covering_patch(parent.path());
    let lockfile = root.join("cabin.lock");
    let before = fs::read(&lockfile).unwrap();
    for command in ["fetch", "build", "vendor"] {
        let assertion = cabin()
            .args([command, "--locked", "--manifest-path"])
            .arg(root.join("cabin.toml"))
            .arg("--index-path")
            .arg(&index)
            .arg("--cache-dir")
            .arg(parent.path().join("cache"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("patch / source-replacement policy differs")
                && stderr.contains("run cabin update"),
            "{command} --locked: expected patch staleness error, got: {stderr}"
        );
        assert_eq!(
            fs::read(&lockfile).unwrap(),
            before,
            "{command} --locked must not rewrite the lockfile"
        );
    }
}

#[test]
fn source_replacement_self_loop_yields_clear_cycle_error() {
    // A workspace config whose source-replacement entry
    // points back at its own original triggers cycle detection
    // the moment the CLI tries to resolve the index source for
    // a fetch / resolve invocation that needs versioned deps.
    // `cabin metadata` is intentionally lazy here - it never
    // walks the replacement chain - so we drive `resolve`,
    // which always applies replacement before any fetch.
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0"
"#,
    );
    write_workspace_config(
        &root,
        r#"[registry]
index-url = "https://example.com/index"

[source-replacement]
"https://example.com/index" = { index-url = "https://example.com/index" }
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["resolve", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("source replacement cycle detected"),
        "expected source-replacement cycle error, got: {stderr}"
    );
}

#[test]
fn offline_rejects_index_path_redirected_to_url_via_source_replacement() {
    // `--offline` paired with `--index-path` is allowed up
    // front, but a `[source-replacement]` entry can rewrite that
    // path into a URL before the artifact pipeline opens the
    // index.  The post-replacement check must catch the
    // bypass and the error must blame the source-replacement
    // entry so the user knows which knob to turn.
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = "10.2.1"
"#,
    );
    write_workspace_config(
        &root,
        r#"[source-replacement]
"./mirror" = { index-url = "https://example.com/index" }
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["build", "--offline", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .args(["--index-path", "./mirror"])
        .arg("--build-dir")
        .arg(root.join("build"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("source-replacement"),
        "expected source-replacement blame, got: {stderr}"
    );
    assert!(
        stderr.contains("https://example.com/index"),
        "diagnostic should name the offending URL, got: {stderr}"
    );
}

#[test]
fn vendor_rejects_index_path_redirected_to_url_via_source_replacement() {
    // `cabin vendor` requires a local index source, but a
    // `[source-replacement]` path → URL rewrite would bypass
    // the pre-check the same way it bypassed `--offline`.
    // The post-replacement vendor check must refuse the URL
    // terminal and blame source-replacement.
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = "10.2.1"
"#,
    );
    write_workspace_config(
        &root,
        r#"[source-replacement]
"./mirror" = { index-url = "https://example.com/index" }
"#,
    );
    let user_home = TempDir::new().unwrap();
    let assertion = cabin_with_config()
        .args(["vendor", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .args(["--index-path", "./mirror"])
        .arg("--vendor-dir")
        .arg(root.join("vendor"))
        .env("CABIN_CONFIG_HOME", user_home.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("source-replacement"),
        "expected source-replacement blame, got: {stderr}"
    );
    assert!(
        stderr.contains("cabin vendor"),
        "diagnostic should mention `cabin vendor`, got: {stderr}"
    );
}

#[test]
fn metadata_succeeds_when_only_inactive_dep_mismatches_patch_version() {
    // The patched fmt is at 0.1.0; the manifest's only
    // mention of fmt is a *dev* dep with `>= 99` - clearly
    // unsatisfiable, but dev deps are inactive for the
    // default invocation, so patch validation must skip the
    // edge and metadata succeeds.  This is the end-to-end
    // counterpart to the cabin-workspace patch-gating tests.
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dev-dependencies]
fmt = ">=99"

[patch]
fmt = { path = "../fmt" }
"#,
    );
    write_patched_fork(
        parent.path(),
        "fmt",
        r#"[package]
name = "fmt"
version = "0.1.0"
"#,
    );
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let patches = value["patches"].as_array().expect("patches array");
    assert_eq!(patches.len(), 1);
    assert_eq!(patches[0]["package"].as_str(), Some("fmt"));
    assert_eq!(patches[0]["version"].as_str(), Some("0.1.0"));
}

#[test]
fn resolve_includes_versioned_deps_introduced_by_patched_manifest() {
    // Regression for the "patched manifest's own
    // [dependencies] are dropped from the resolver input"
    // bug: the workspace declares only a patched dep, but
    // the patched fork itself depends on a registry-only
    // package.  After the fix, `cabin resolve` must include
    // the transitive registry edge in its output.
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt-fork" }
"#,
    );
    // The patched fmt fork carries its own registry-bound
    // dep on `spdlog`.  Without the patched-deps fix, this
    // edge never reaches the resolver and the build later
    // surfaces a missing-include failure.
    write_patched_fork(
        parent.path(),
        "fmt-fork",
        r#"[package]
name = "fmt"
version = "10.2.1"

[dependencies]
spdlog = ">=1.13.0 <2.0.0"
"#,
    );
    parent
        .child("index/spdlog.json")
        .write_str(SPDLOG_INDEX)
        .unwrap();
    parent.child("index/fmt.json").write_str(FMT_INDEX).unwrap();
    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(root.join("cabin.toml"))
            .arg("--index-path")
            .arg(parent.path().join("index"))
            .args(["--format", "json"]),
    );
    let names: Vec<&str> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"spdlog"),
        "spdlog must enter resolution through the patched fmt manifest, got: {names:?}"
    );
}

#[test]
fn vendor_requires_index_for_versioned_deps_introduced_by_patched_manifest() {
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt-fork" }
"#,
    );
    write_patched_fork(
        parent.path(),
        "fmt-fork",
        r#"[package]
name = "fmt"
version = "10.2.1"

[dependencies]
spdlog = ">=1.13.0 <2.0.0"
"#,
    );
    let assertion = cabin()
        .args(["vendor", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .arg("--vendor-dir")
        .arg(parent.path().join("vendor"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("versioned dependencies require --index-path"),
        "patched manifest's registry deps should require an index: {stderr}"
    );
}

#[test]
fn source_replacement_does_not_leak_into_package_metadata() {
    let dir = TempDir::new().unwrap();
    write_root_manifest(
        dir.path(),
        r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[target.demo]
type = "library"
sources = ["src/lib.cc"]
"#,
    );
    dir.child("src/lib.cc")
        .write_str("int demo() { return 0; }\n")
        .unwrap();
    write_workspace_config(
        dir.path(),
        r#"[source-replacement]
"https://example.com/index" = { index-path = "../mirror" }
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
    assert!(!body.contains("source-replacement"), "{body}");
    assert!(!body.contains("https://example.com/index"), "{body}");
}

/// A dormant patch - nothing in the invocation depends on its name -
/// must not fail the build over its fork's own versioned deps: they
/// are deliberately never resolved, so the strict workspace reload
/// cannot require them.
#[test]
fn build_tolerates_dormant_patch_with_versioned_deps() {
    require_cxx_build_tools();
    let parent = TempDir::new().unwrap();
    let root = parent.path().join("app");
    write_root_manifest(
        &root,
        r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[patch]
ghost = { path = "../ghost-fork" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
"#,
    );
    assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    write_patched_fork(
        parent.path(),
        "ghost-fork",
        r#"[package]
name = "ghost"
version = "1.0.0"
cxx-standard = "c++17"

[dependencies]
absent = ">=1"

[target.ghost]
type = "library"
sources = ["src/ghost.cc"]
"#,
    );
    assert_fs::fixture::ChildPath::new(parent.path().join("ghost-fork/src/ghost.cc"))
        .write_str("void g() {}\n")
        .unwrap();
    cabin()
        .args(["build", "--manifest-path"])
        .arg(root.join("cabin.toml"))
        .arg("--build-dir")
        .arg(root.join("build"))
        .assert()
        .success();
}

/// An index-discovered fork's *path dependency* declaring registry
/// dependencies is invisible to both the fork-manifest fold and the
/// pre-resolve closure pass (the fork was not reachable yet), so
/// those dependencies must be folded from the fork's island - and a
/// path dependency naming another patched package chains that patch,
/// whose own island then contributes deps of its own (fixed point).
/// Both registry deps must land in the resolution: dropping them was
/// silent (nothing errored; the edges just vanished).
#[test]
fn activated_fork_path_deps_fold_registry_and_chained_patch_deps() {
    require_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    for (dir_name, manifest) in [
        (
            "aaa",
            r#"[package]
name = "acme/aaa"
version = "1.0.0"
cxx-standard = "c++17"

[dependencies]
"acme/bbb" = ">=1"

[target.aaa]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
        (
            "bbb",
            r#"[package]
name = "acme/bbb"
version = "1.0.0"
cxx-standard = "c++17"

[target.bbb]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
        (
            "ccc",
            r#"[package]
name = "acme/ccc"
version = "1.0.0"
cxx-standard = "c++17"

[target.ccc]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
        (
            "ddd",
            r#"[package]
name = "acme/ddd"
version = "1.0.0"
cxx-standard = "c++17"

[target.ddd]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
    ] {
        let pkg_root = tmp.path().join(format!("src-{dir_name}"));
        assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
            .write_str(manifest)
            .unwrap();
        assert_fs::fixture::ChildPath::new(pkg_root.join("src/lib.cc"))
            .write_str("int x() { return 0; }\n")
            .unwrap();
        cabin()
            .args(["publish", "--manifest-path"])
            .arg(pkg_root.join("cabin.toml"))
            .arg("--registry-dir")
            .arg(tmp.path().join("registry"))
            .assert()
            .success();
    }

    // The fork of bbb pulls in local xxx, which declares a registry
    // dep (ccc) and a dep on the patched ppp; ppp's fork declares a
    // registry dep of its own (ddd).
    for (dir, manifest) in [
        (
            "bbb-fork",
            r#"[package]
name = "acme/bbb"
version = "1.0.0"
cxx-standard = "c++17"

[dependencies]
xxx = { path = "../xxx" }

[target.bbb]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
        (
            "xxx",
            r#"[package]
name = "xxx"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
"acme/ccc" = ">=1"
"acme/ppp" = ">=1"

[target.xxx]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
        (
            "ppp-fork",
            r#"[package]
name = "acme/ppp"
version = "1.0.0"
cxx-standard = "c++17"

[dependencies]
"acme/ddd" = ">=1"

[target.ppp]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
    ] {
        assert_fs::fixture::ChildPath::new(tmp.path().join(dir).join("cabin.toml"))
            .write_str(manifest)
            .unwrap();
        assert_fs::fixture::ChildPath::new(tmp.path().join(dir).join("src/lib.cc"))
            .write_str("int y() { return 0; }\n")
            .unwrap();
    }
    assert_fs::fixture::ChildPath::new(tmp.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
"acme/aaa" = ">=1"

[patch]
"acme/bbb" = { path = "../bbb-fork" }
"acme/ppp" = { path = "../ppp-fork" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(tmp.path().join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();

    cabin()
        .args(["build", "--offline", "--manifest-path"])
        .arg(tmp.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(tmp.path().join("registry"))
        .arg("--cache-dir")
        .arg(tmp.path().join("cache"))
        .arg("--build-dir")
        .arg(tmp.path().join("build"))
        .assert()
        .success();

    let lockfile = std::fs::read_to_string(tmp.path().join("app/cabin.lock")).unwrap();
    for name in ["acme/ccc", "acme/ddd"] {
        assert!(
            lockfile.contains(&format!("name = \"{name}\"")),
            "expected {name} in the lockfile; a dropped fork-island dep is silent: {lockfile}"
        );
    }
}

/// A patched name selected only through a replaced upstream's dead
/// index edges stays dormant: `app -> aaa -> ppp` activates ppp's
/// fork through a live edge, but upstream ppp's `bbb` edge dies with
/// the replacement (the fork drops it), so bbb's patch must not
/// activate.  Its fork declares a dependency that exists nowhere -
/// if activation leaks through the dead edge, resolution fails on
/// that name and this build breaks.
#[test]
fn dead_upstream_edges_of_activated_forks_do_not_activate_patches() {
    require_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    for (dir_name, manifest) in [
        (
            "aaa",
            r#"[package]
name = "acme/aaa"
version = "1.0.0"
cxx-standard = "c++17"

[dependencies]
"acme/ppp" = ">=1"

[target.aaa]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
        (
            "ppp",
            r#"[package]
name = "acme/ppp"
version = "1.0.0"
cxx-standard = "c++17"

[dependencies]
"acme/bbb" = ">=1"

[target.ppp]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
        (
            "bbb",
            r#"[package]
name = "acme/bbb"
version = "1.0.0"
cxx-standard = "c++17"

[target.bbb]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
    ] {
        let pkg_root = tmp.path().join(format!("src-{dir_name}"));
        assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
            .write_str(manifest)
            .unwrap();
        assert_fs::fixture::ChildPath::new(pkg_root.join("src/lib.cc"))
            .write_str("int x() { return 0; }\n")
            .unwrap();
        cabin()
            .args(["publish", "--manifest-path"])
            .arg(pkg_root.join("cabin.toml"))
            .arg("--registry-dir")
            .arg(tmp.path().join("registry"))
            .assert()
            .success();
    }
    for (dir, manifest) in [
        (
            "ppp-fork",
            r#"[package]
name = "acme/ppp"
version = "1.0.0"
cxx-standard = "c++17"

[target.ppp]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
        (
            "bbb-fork",
            r#"[package]
name = "acme/bbb"
version = "1.0.0"
cxx-standard = "c++17"

[dependencies]
"acme/never-published" = ">=1"

[target.bbb]
type = "library"
sources = ["src/lib.cc"]
"#,
        ),
    ] {
        assert_fs::fixture::ChildPath::new(tmp.path().join(dir).join("cabin.toml"))
            .write_str(manifest)
            .unwrap();
        assert_fs::fixture::ChildPath::new(tmp.path().join(dir).join("src/lib.cc"))
            .write_str("int y() { return 0; }\n")
            .unwrap();
    }
    assert_fs::fixture::ChildPath::new(tmp.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
"acme/aaa" = ">=1"

[patch]
"acme/ppp" = { path = "../ppp-fork" }
"acme/bbb" = { path = "../bbb-fork" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(tmp.path().join("app/src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();

    cabin()
        .args(["build", "--offline", "--manifest-path"])
        .arg(tmp.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(tmp.path().join("registry"))
        .arg("--cache-dir")
        .arg(tmp.path().join("cache"))
        .arg("--build-dir")
        .arg(tmp.path().join("build"))
        .assert()
        .success();
}

/// A patch discovered only through an index edge is validated
/// against that edge's requirement exactly like a locally-visible
/// one: `resolve_active_patches` never saw `aaa`'s `bbb >= 2`
/// (aaa is index-only), and the final reload substitutes the fork
/// without re-solving, so the post-solve check is the only place
/// the mismatch can surface before an incompatible fork links.
#[test]
fn transitive_activation_validates_fork_version() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index.join("aaa.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "aaa",
  "versions": {
    "1.0.0": { "dependencies": { "bbb": ">=2" }, "yanked": false }
  }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(index.join("bbb.json"))
        .write_str(
            r#"{
  "schema": 1,
  "name": "bbb",
  "versions": { "2.0.0": { "dependencies": {}, "yanked": false } }
}"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/cabin.toml"))
        .write_str(
            r#"[package]
name = "bbb"
version = "1.0.0"

[target.bbb]
type = "library"
sources = ["src/bbb.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("bbb-fork/src/bbb.c"))
        .write_str("void b(void) {}\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
aaa = ">=1"

[patch]
bbb = { path = "../bbb-fork" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    for needle in [
        "patch package `bbb` has version `1.0.0`",
        "does not satisfy dependency requirement `>=2`",
    ] {
        assert!(stderr.contains(needle), "expected {needle:?} in: {stderr}");
    }
}
