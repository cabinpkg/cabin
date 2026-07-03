#![allow(
    clippy::needless_raw_string_hashes,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::stable_sort_primitive
)]

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_fs::TempDir;
use assert_fs::prelude::*;
use cabin::Cli;
use clap::CommandFactory;
use predicates::prelude::*;

mod common;
use common::*;

/// The Cabin version string `cabin --version` / `cabin version`
/// print, sourced from the same `CARGO_PKG_VERSION` the binary
/// reads (the test crate inherits `version.workspace = true`), so a
/// workspace version bump never requires editing version assertions.
const CABIN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// All top-level subcommand names registered with clap,
/// derived from `Cli::command()` so tests never hard-code the
/// list.  The `help` pseudo-subcommand that clap auto-injects
/// is filtered because Cabin never advertises it as a public
/// command. (Internal plumbing like the `cabin stamp` witness
/// writer is dispatched before clap, outside the clap tree
/// entirely, so it never appears here.)
fn all_subcommand_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .filter(|sub| sub.get_name() != "help")
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

/// Subset of [`all_subcommand_names`] that `cabin --help`
/// advertises - the visible, day-to-day surface.
fn visible_subcommand_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .filter(|sub| sub.get_name() != "help" && !sub.is_hide_set())
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

/// Names of subcommands hidden from `cabin --help` but still
/// reachable through `cabin --list`, shell completions, and
/// per-subcommand man pages. (The `cabin stamp` witness writer
/// is dispatched before clap, outside the clap tree, so it is
/// never part of this curated hidden surface.)
fn hidden_subcommand_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .filter(|sub| sub.is_hide_set())
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

/// Extract the command-row names from a clap-rendered
/// `--help` payload.  Each row in the `Commands:` block has
/// the shape ` <name><spaces><description>`; this helper
/// returns the `<name>` tokens alone so callers do not have to
/// substring-match against description text.
fn parse_help_commands_block(help: &str) -> Vec<String> {
    let mut in_block = false;
    let mut names = Vec::new();
    for line in help.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("Commands:") {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        // The block ends at the first blank line - clap then
        // renders `Options:` (or another section heading).
        if trimmed.is_empty() {
            break;
        }
        // Rows are indented; section headings are not.  Bail
        // defensively if a non-indented line appears before
        // the blank line.
        let Some(content) = line.strip_prefix("  ") else {
            break;
        };
        if let Some(name) = content.split_whitespace().next() {
            names.push(name.to_owned());
        }
    }
    names
}

const VALID_MANIFEST: &str = r#"[package]
name = "hello"
version = "0.1.0"
cxx-standard = "c++17"

[target.hello]
type = "executable"
sources = ["src/main.cc"]
"#;

const HELLO_MAIN_CC: &str = "#include <iostream>\n\nint main() {\n    std::cout << \"Hello from Cabin\\n\";\n    return 0;\n}\n";

/// Write the canonical single-package hello fixture at `root`:
/// `VALID_MANIFEST` plus the greeting `src/main.cc`.  The shared
/// starting point for tests that need a loadable (and buildable)
/// package on disk.
fn write_hello_project(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(VALID_MANIFEST)
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
        .write_str(HELLO_MAIN_CC)
        .unwrap();
}

/// Like [`write_hello_project`] but with a trivial `src/main.cc`
/// body.  Used by the fmt / tidy tests, which assert on the exact
/// on-disk source bytes rather than the program's behavior.
fn write_minimal_project(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(VALID_MANIFEST)
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
}

/// Pin `HOME` and `XDG_CONFIG_HOME` to deterministic temp paths
/// that contain no Cabin config.  The user config home resolver
/// falls back to `getpwuid_r` when `HOME` is unset, so
/// removing `HOME` from the subprocess environment would leave a
/// developer's real `~/.config/cabin/config.toml` reachable.
/// Pointing both variables at empty temp paths is the robust
/// equivalent of "no user config home" for tests that exercise
/// config discovery.  Tests that exercise specific config-home arms
/// override these with later `.env(...)` calls (`assert_cmd`
/// applies env mutations in declaration order).
///
/// The path is wiped once per `cargo test` invocation so a
/// stale `config.toml` written by a previous run can never leak
/// in.  Cabin does not write into the user config home itself, so
/// a single cleanup at the first call is sufficient - later
/// calls see the same already-empty directory.
fn pin_test_user_config_home_to_empty(cmd: &mut Command) {
    static CLEAR_STALE: std::sync::Once = std::sync::Once::new();
    let base = std::env::temp_dir().join("cabin-tests-empty-home");
    CLEAR_STALE.call_once(|| {
        let _ = std::fs::remove_dir_all(&base);
    });
    cmd.env("HOME", &base);
    cmd.env("XDG_CONFIG_HOME", base.join("xdg-config"));
}

/// Build a `cabin` command that re-enables config discovery
/// for a single test.  Mirrors the default test-harness
/// helper but drops the `CABIN_NO_CONFIG=1` opt-out applied
/// to every other integration test.  Shared by the config and
/// patch/override test modules.
fn cabin_with_config() -> Command {
    let mut cmd = Command::cargo_bin("cabin").expect("the `cabin` binary should be built by cargo");
    cmd.env_remove("CABIN_NO_CONFIG")
        .env_remove("CABIN_CONFIG")
        .env_remove("CABIN_CONFIG_HOME");
    pin_test_user_config_home_to_empty(&mut cmd);
    pin_test_cache_home(&mut cmd);
    cmd
}

fn require_external_tool(name: &str) {
    assert!(
        command_exists(name),
        "external tool `{name}` is required for this test; install it"
    );
}

fn workspace_test_bin(name: &str) -> PathBuf {
    let test_exe = std::env::current_exe().expect("current_exe");
    let mut dir = test_exe
        .parent()
        .expect("test exe should live in a directory")
        .to_path_buf();
    if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
        dir.pop();
    }
    let candidate = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        candidate.is_file(),
        "expected test helper binary at {}; build the workspace tests with the matching fake-tool feature enabled",
        candidate.display()
    );
    candidate
}

/// Run `cmd`, assert it exits successfully, and parse its stdout as
/// JSON.  For the common case of a test that only needs the parsed
/// value; tests that also inspect the raw stdout (for example to embed
/// it in a failure message) keep the explicit capture-and-parse form.
fn run_json(cmd: &mut Command) -> serde_json::Value {
    let out = cmd.assert().success().get_output().clone();
    serde_json::from_slice(&out.stdout).expect("command stdout should be valid JSON")
}

/// Build a gzip-compressed tar archive at `path` from `entries` (each a
/// `(relative-path, file-body)` pair) and return its lower-case SHA-256
/// hex digest.  Shared by the registry / vendor / artifact-fetch tests
/// that need a real downloadable archive whose checksum they can assert.
fn make_archive(path: &std::path::Path, entries: &[(&str, &str)]) -> String {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        assert_fs::fixture::ChildPath::new(parent)
            .create_dir_all()
            .unwrap();
    }
    let f = std::fs::File::create(path).unwrap();
    let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut builder = tar::Builder::new(enc);
    for (rel, body) in entries {
        let bytes = body.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(&mut header, rel, &mut std::io::Cursor::new(bytes))
            .unwrap();
    }
    let enc = builder.into_inner().unwrap();
    enc.finish().unwrap().flush().unwrap();
    cabin_core::hash::hash_reader(std::fs::File::open(path).unwrap()).unwrap()
}

/// Write a local index entry at `index_dir/{package}.json` for
/// `package`@`version`, with the given dependencies JSON, checksum (a
/// bare hex digest; the `sha256:` prefix is added here), and an archive
/// `source.path`.  Centralizes the index schema so the registry /
/// resolver / vendor tests share one definition.
fn write_index_entry(
    index_dir: &std::path::Path,
    package: &str,
    version: &str,
    deps_json: &str,
    checksum: &str,
    source_path: &str,
) {
    let body = format!(
        r#"{{
  "schema": 1,
  "name": "{package}",
  "versions": {{
    "{version}": {{
      "dependencies": {deps_json},
      "yanked": false,
      "checksum": "sha256:{checksum}",
      "source": {{ "type": "archive", "path": "{source_path}", "format": "tar.gz" }}
    }}
  }}
}}"#
    );
    assert_fs::fixture::ChildPath::new(index_dir.join(format!("{package}.json")))
        .write_str(&body)
        .unwrap();
}

/// Write an `app/` package whose root manifest depends on
/// `fmt = ">=10 <11"`.  With `app_main` set, the manifest also
/// declares a `[target.app]` executable linking against `fmt` and
/// `app/src/main.cc` is written with the given body; with `None`
/// the package has no targets (resolver / registry-only tests).
/// Shared by the artifact-fetch, file-registry, and sparse-HTTP
/// test modules.
fn write_app_using_fmt(dir: &Path, app_main: Option<&str>) {
    let manifest = if app_main.is_some() {
        r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

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

/// Like [`write_index_entry`] but without a `source` block, for index
/// entries whose archive is never fetched (resolver-only tests).
fn write_index_entry_no_source(
    index_dir: &std::path::Path,
    package: &str,
    version: &str,
    checksum: &str,
) {
    let body = format!(
        r#"{{
  "schema": 1,
  "name": "{package}",
  "versions": {{
    "{version}": {{
      "dependencies": {{}},
      "yanked": false,
      "checksum": "sha256:{checksum}"
    }}
  }}
}}"#
    );
    assert_fs::fixture::ChildPath::new(index_dir.join(format!("{package}.json")))
        .write_str(&body)
        .unwrap();
}

#[path = "cli/external_tool_smoke.rs"]
mod external_tool_smoke;

/// Read `metadata` JSON for a manifest path; returns the parsed JSON.
fn run_metadata(manifest_path: &Path) -> serde_json::Value {
    let output = cabin()
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

/// Find a package by name in a metadata JSON document.
fn package_in<'a>(meta: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    meta["packages"]
        .as_array()
        .expect("packages must be array")
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("package {name:?} not found in metadata: {meta}"))
}

// ---------------------------------------------------------------------------
// init / metadata / invalid manifest
// ---------------------------------------------------------------------------

#[test]
fn init_creates_manifest_and_main_cc() {
    let dir = TempDir::new().expect("tempdir should be created");
    cabin()
        .current_dir(dir.path())
        .args(["init", "--name", "hello"])
        .assert()
        .success();

    let manifest_path = dir.path().join("cabin.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("cabin.toml should be readable");
    assert!(manifest.contains("[package]"));
    assert!(manifest.contains(r#"name = "hello""#));
    assert!(manifest.contains(r#"cxx-standard = "c++17""#));
    assert!(manifest.contains("[target.hello]"));

    let main_cc = dir.path().join("src").join("main.cc");
    assert!(main_cc.is_file(), "src/main.cc should exist");
    let main_contents = fs::read_to_string(&main_cc).unwrap();
    assert!(main_contents.contains("int main"));
}

#[test]
fn stamp_writes_witness_only_on_command_success() {
    // `cabin stamp` (used by the `cabin check` Ninja rule) runs the given
    // command and creates the witness file only when it exits zero - no
    // shell, so build paths with `&` / `|` / `()` never need escaping.
    // The `cabin` binary itself is a portable stand-in: `--version` exits
    // 0, a bogus flag exits non-zero.
    let dir = TempDir::new().expect("tempdir should be created");
    let inner = assert_cmd::cargo::cargo_bin("cabin");

    let ok_stamp = dir.path().join("ok.stamp");
    cabin()
        .arg("stamp")
        .arg(&ok_stamp)
        .arg("--")
        .arg(&inner)
        .arg("--version")
        .assert()
        .success();
    assert!(
        ok_stamp.is_file(),
        "witness must be created when the command succeeds"
    );

    let fail_stamp = dir.path().join("fail.stamp");
    cabin()
        .arg("stamp")
        .arg(&fail_stamp)
        .arg("--")
        .arg(&inner)
        .arg("--definitely-not-a-real-flag")
        .assert()
        .failure();
    assert!(
        !fail_stamp.exists(),
        "witness must not be created when the command fails"
    );
}

#[test]
fn stamp_command_is_absent_from_the_clap_tree_and_completions() {
    // `cabin stamp` is dispatched before clap, so it must never leak into
    // the user-facing surface: not as a subcommand, not in `--list`, not
    // in generated shell completions (Codex flagged the old `__check-stamp`
    // subcommand leaking into `clap_complete` output).  Guard all three.
    for name in all_subcommand_names() {
        assert!(
            name != "stamp" && !name.starts_with("__"),
            "internal command `{name}` must not be a clap subcommand"
        );
    }

    let bash = cabin()
        .args(["compgen", "bash"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let bash = String::from_utf8_lossy(&bash);
    // The old leak was a literal `__check-stamp` subcommand registered on
    // the clap tree; assert the regression is gone. (The `cmd[stamp]=` /
    // `'stamp')` shapes a bash completion uses for a registered
    // subcommand cannot appear because `stamp` is not in the tree.)
    assert!(
        !bash.contains("__check-stamp"),
        "generated completions must not register the internal stamp command"
    );
}

#[test]
fn init_fails_when_manifest_already_exists() {
    let dir = TempDir::new().expect("tempdir should be created");
    let manifest_path = dir.path().join("cabin.toml");
    assert_fs::fixture::ChildPath::new(&manifest_path)
        .write_str("# preexisting cabin.toml\n")
        .unwrap();

    cabin()
        .current_dir(dir.path())
        .args(["init", "--name", "hello"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cabin.toml already exists"));

    let after = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(after, "# preexisting cabin.toml\n");
}

#[test]
fn metadata_prints_valid_json_for_single_package() {
    let dir = TempDir::new().expect("tempdir should be created");
    let manifest_path = dir.path().join("cabin.toml");
    assert_fs::fixture::ChildPath::new(&manifest_path)
        .write_str(VALID_MANIFEST)
        .unwrap();

    let value = run_metadata(&manifest_path);
    // The metadata view wraps the package list.  For a single-package
    // manifest there is no [workspace], so `workspace` is null.
    assert!(value["workspace"].is_null());
    let pkg = package_in(&value, "hello");
    assert_eq!(pkg["version"], "0.1.0");
    assert!(
        pkg.get("language").is_none(),
        "metadata must not surface a package-level language field; got {pkg}"
    );
    assert_eq!(pkg["is_root"], true);
    assert_eq!(pkg["is_primary"], true);
    let targets = pkg["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["name"], "hello");
    assert_eq!(targets[0]["kind"], "executable");
}

#[test]
fn invalid_manifest_fails_with_useful_error() {
    let dir = TempDir::new().expect("tempdir should be created");
    let manifest_path = dir.path().join("cabin.toml");
    assert_fs::fixture::ChildPath::new(&manifest_path)
        .write_str("[package]\nname = \"x\"\nversion = [\n")
        .unwrap();

    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(&manifest_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("cabin::manifest::parse_error"))
        .stderr(predicate::str::contains("could not parse Cabin manifest"));
}

#[test]
fn metadata_round_trips_default_init_template() {
    let dir = TempDir::new().expect("tempdir should be created");
    cabin()
        .current_dir(dir.path())
        .args(["init", "--name", "round-trip"])
        .assert()
        .success();

    let value = run_metadata(&dir.path().join("cabin.toml"));
    let pkg = package_in(&value, "round-trip");
    assert_eq!(pkg["targets"][0]["kind"], "executable");
}

#[test]
fn new_creates_directory_with_manifest_and_main_cc() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("hello");
    cabin()
        .current_dir(parent.path())
        .args(["new", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Created binary (application) `hello` package",
        ));

    assert!(target.is_dir(), "target directory should exist");
    let manifest = fs::read_to_string(target.join("cabin.toml")).unwrap();
    assert!(manifest.contains("[package]"));
    assert!(manifest.contains(r#"name = "hello""#));
    assert!(manifest.contains(r#"cxx-standard = "c++17""#));
    assert!(manifest.contains("[target.hello]"));

    let main_cc = target.join("src").join("main.cc");
    assert!(main_cc.is_file(), "src/main.cc should exist");
    let main_contents = fs::read_to_string(&main_cc).unwrap();
    assert!(main_contents.contains("int main"));
}

#[test]
fn new_uses_path_component_as_default_name() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("derived-name");
    cabin()
        .current_dir(parent.path())
        .args(["new", "derived-name"])
        .assert()
        .success();

    let manifest = fs::read_to_string(target.join("cabin.toml")).unwrap();
    assert!(manifest.contains(r#"name = "derived-name""#));
    assert!(manifest.contains("[target.derived-name]"));
}

#[test]
fn new_supports_explicit_name_override() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("dir-name");
    cabin()
        .current_dir(parent.path())
        .args(["new", "dir-name", "--name", "override"])
        .assert()
        .success();

    let manifest = fs::read_to_string(target.join("cabin.toml")).unwrap();
    assert!(manifest.contains(r#"name = "override""#));
    assert!(manifest.contains("[target.override]"));
}

#[test]
fn new_fails_when_destination_already_exists() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("preexisting");
    fs::create_dir(&target).unwrap();
    let preexisting_file = target.join("user-file.txt");
    assert_fs::fixture::ChildPath::new(&preexisting_file)
        .write_str("user content\n")
        .unwrap();

    cabin()
        .current_dir(parent.path())
        .args(["new", "preexisting"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"))
        .stderr(predicate::str::contains("cabin init"));

    assert!(target.is_dir(), "destination should be left intact");
    let after = fs::read_to_string(&preexisting_file).unwrap();
    assert_eq!(after, "user content\n");
    assert!(
        !target.join("cabin.toml").exists(),
        "no manifest should have been written"
    );
}

#[test]
fn new_rejects_name_with_whitespace() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("bad");
    cabin()
        .current_dir(parent.path())
        .args(["new", "bad", "--name", "foo bar"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid package name"));
    assert!(
        !target.exists(),
        "directory should be cleaned up on validation failure"
    );
}

#[test]
fn new_rejects_name_with_unsupported_characters() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("dot-name");
    cabin()
        .current_dir(parent.path())
        .args(["new", "dot-name", "--name", "foo.bar"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not supported"));
    assert!(
        !target.exists(),
        "directory should be cleaned up on validation failure"
    );
}

#[test]
fn new_fails_when_parent_does_not_exist() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("missing").join("hello");
    cabin()
        .current_dir(parent.path())
        .args(["new"])
        .arg(&target)
        .assert()
        .failure()
        .stderr(predicate::str::contains("parent directory"))
        .stderr(predicate::str::contains("does not exist"));
    assert!(!target.exists());
}

#[test]
fn new_does_not_overwrite_existing_user_files_on_failure() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("invalid");
    cabin()
        .current_dir(parent.path())
        .args(["new", "invalid", "--name", "foo bar"])
        .assert()
        .failure();
    assert!(
        !target.exists(),
        "directory created by new must not survive a failed scaffold"
    );
}

#[test]
fn new_and_init_produce_identical_files() {
    let init_dir = TempDir::new().expect("tempdir should be created");
    cabin()
        .current_dir(init_dir.path())
        .args(["init", "--name", "twin"])
        .assert()
        .success();

    let new_parent = TempDir::new().expect("tempdir should be created");
    let new_target = new_parent.path().join("twin");
    cabin()
        .current_dir(new_parent.path())
        .args(["new", "twin"])
        .assert()
        .success();

    for relative in ["cabin.toml", "src/main.cc"] {
        let init_bytes = fs::read(init_dir.path().join(relative)).unwrap();
        let new_bytes = fs::read(new_target.join(relative)).unwrap();
        assert_eq!(
            init_bytes, new_bytes,
            "byte mismatch for {relative} between init and new"
        );
    }
}

#[test]
fn new_generated_manifest_does_not_contain_absolute_paths() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("portable");
    cabin()
        .current_dir(parent.path())
        .args(["new", "portable"])
        .assert()
        .success();

    let manifest = fs::read_to_string(target.join("cabin.toml")).unwrap();
    let parent_str = parent.path().to_string_lossy().to_string();
    assert!(
        !manifest.contains(&*parent_str),
        "manifest leaks absolute path: {manifest}"
    );
    let target_str = target.to_string_lossy().to_string();
    assert!(
        !manifest.contains(&*target_str),
        "manifest leaks absolute path: {manifest}"
    );

    let main_cc = fs::read_to_string(target.join("src").join("main.cc")).unwrap();
    assert!(
        !main_cc.contains(&*parent_str),
        "main.cc leaks absolute path: {main_cc}"
    );
}

#[test]
fn new_help_describes_path_argument() {
    let stdout = cabin()
        .args(["new", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let body = String::from_utf8(stdout.stdout).unwrap();
    assert!(body.contains("PATH"), "new --help missing PATH:\n{body}");
    assert!(
        body.contains("--name"),
        "new --help missing --name:\n{body}"
    );
    assert!(body.contains("--bin"), "new --help missing --bin:\n{body}");
    assert!(body.contains("--lib"), "new --help missing --lib:\n{body}");
}

#[test]
fn init_help_describes_bin_and_lib_flags() {
    let stdout = cabin()
        .args(["init", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let body = String::from_utf8(stdout.stdout).unwrap();
    assert!(body.contains("--bin"), "init --help missing --bin:\n{body}");
    assert!(body.contains("--lib"), "init --help missing --lib:\n{body}");
}

#[test]
fn new_with_explicit_bin_matches_default() {
    let parent = TempDir::new().expect("tempdir should be created");
    cabin()
        .current_dir(parent.path())
        .args(["new", "default-bin"])
        .assert()
        .success();
    cabin()
        .current_dir(parent.path())
        .args(["new", "explicit-bin", "--bin"])
        .assert()
        .success();

    let default = fs::read(parent.path().join("default-bin/cabin.toml")).unwrap();
    let explicit = fs::read(parent.path().join("explicit-bin/cabin.toml")).unwrap();
    // The package name appears in the manifest, so byte-equality
    // is only meaningful after stripping the name.  We compare
    // structure instead by checking the explicit-bin manifest
    // declares `executable` like the default.
    let _ = default;
    let body = String::from_utf8(explicit).unwrap();
    assert!(body.contains(r#"type = "executable""#), "{body}");
    assert!(
        !body.contains(r#"type = "library""#),
        "--bin must not produce a library manifest:\n{body}"
    );
}

#[test]
fn new_with_lib_generates_library_layout() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("greeter");
    cabin()
        .current_dir(parent.path())
        .args(["new", "greeter", "--lib"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Created library `greeter` package",
        ));

    let manifest = fs::read_to_string(target.join("cabin.toml")).unwrap();
    assert!(manifest.contains(r#"type = "library""#));
    assert!(manifest.contains(r#"sources = ["src/greeter.cc"]"#));
    assert!(manifest.contains(r#"include-dirs = ["include"]"#));

    let header = fs::read_to_string(target.join("include/greeter/greeter.hpp")).unwrap();
    assert!(header.contains("#pragma once"));
    assert!(header.contains("namespace greeter"));

    let src = fs::read_to_string(target.join("src/greeter.cc")).unwrap();
    assert!(src.contains(r#"#include "greeter/greeter.hpp""#));
    assert!(src.contains("namespace greeter"));

    assert!(
        !target.join("src/main.cc").exists(),
        "library scaffold must not emit src/main.cc"
    );
}

#[test]
fn new_with_bin_and_lib_conflicts() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("either");
    cabin()
        .current_dir(parent.path())
        .args(["new", "either", "--bin", "--lib"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--bin").and(predicate::str::contains("--lib")));
    assert!(
        !target.exists(),
        "no directory should be created on conflict"
    );
}

#[test]
fn init_with_lib_generates_library_layout() {
    let dir = TempDir::new().expect("tempdir should be created");
    cabin()
        .current_dir(dir.path())
        .args(["init", "--lib", "--name", "lib-pkg"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Created library `lib-pkg` package",
        ));

    let manifest = fs::read_to_string(dir.path().join("cabin.toml")).unwrap();
    assert!(manifest.contains(r#"type = "library""#));
    assert!(manifest.contains(r#"sources = ["src/lib-pkg.cc"]"#));
    assert!(manifest.contains(r#"include-dirs = ["include"]"#));

    let header = fs::read_to_string(dir.path().join("include/lib-pkg/lib-pkg.hpp")).unwrap();
    assert!(header.contains("namespace lib_pkg"));

    let src = fs::read_to_string(dir.path().join("src/lib-pkg.cc")).unwrap();
    assert!(src.contains("namespace lib_pkg"));
}

#[test]
fn init_with_bin_explicit_matches_default() {
    let dir = TempDir::new().expect("tempdir should be created");
    cabin()
        .current_dir(dir.path())
        .args(["init", "--bin", "--name", "explicit-bin"])
        .assert()
        .success();

    let manifest = fs::read_to_string(dir.path().join("cabin.toml")).unwrap();
    assert!(manifest.contains(r#"type = "executable""#));
    assert!(dir.path().join("src/main.cc").exists());
}

#[test]
fn init_with_bin_and_lib_conflicts() {
    let dir = TempDir::new().expect("tempdir should be created");
    cabin()
        .current_dir(dir.path())
        .args(["init", "--bin", "--lib"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--bin").and(predicate::str::contains("--lib")));

    assert!(
        !dir.path().join("cabin.toml").exists(),
        "no manifest should be written on conflict"
    );
}

#[test]
fn new_generates_gitignore_with_build_and_dist() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("gi");
    cabin()
        .current_dir(parent.path())
        .args(["new", "gi"])
        .assert()
        .success();

    let body = fs::read_to_string(target.join(".gitignore")).unwrap();
    assert!(body.contains("/build/"), "missing build/ ignore:\n{body}");
    assert!(body.contains("/dist/"), "missing dist/ ignore:\n{body}");
    assert!(
        !body.contains("cabin.lock"),
        ".gitignore must not ignore cabin.lock:\n{body}"
    );
}

#[test]
fn init_creates_gitignore_when_missing() {
    let dir = TempDir::new().expect("tempdir should be created");
    cabin()
        .current_dir(dir.path())
        .args(["init", "--name", "fresh"])
        .assert()
        .success();
    let body = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(body.contains("/build/"), "{body}");
    assert!(body.contains("/dist/"), "{body}");
}

#[test]
fn init_preserves_existing_gitignore() {
    let dir = TempDir::new().expect("tempdir should be created");
    let preexisting = "# user gitignore\nfoo.tmp\n";
    assert_fs::fixture::ChildPath::new(dir.path().join(".gitignore"))
        .write_str(preexisting)
        .unwrap();

    cabin()
        .current_dir(dir.path())
        .args(["init", "--name", "preserve"])
        .assert()
        .success();

    let after = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(after, preexisting);
}

#[test]
fn new_lib_with_hyphenated_name_uses_sanitized_namespace() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("two-words");
    cabin()
        .current_dir(parent.path())
        .args(["new", "two-words", "--lib"])
        .assert()
        .success();

    let header = fs::read_to_string(target.join("include/two-words/two-words.hpp")).unwrap();
    assert!(header.contains("namespace two_words"));
    let src = fs::read_to_string(target.join("src/two-words.cc")).unwrap();
    assert!(src.contains("namespace two_words"));
    assert!(src.contains(r#"#include "two-words/two-words.hpp""#));
}

#[test]
fn new_lib_generated_files_are_deterministic() {
    let parent_a = TempDir::new().expect("tempdir should be created");
    let parent_b = TempDir::new().expect("tempdir should be created");
    cabin()
        .current_dir(parent_a.path())
        .args(["new", "twin", "--lib"])
        .assert()
        .success();
    cabin()
        .current_dir(parent_b.path())
        .args(["new", "twin", "--lib"])
        .assert()
        .success();

    for relative in [
        "cabin.toml",
        "include/twin/twin.hpp",
        "src/twin.cc",
        ".gitignore",
    ] {
        let a = fs::read(parent_a.path().join("twin").join(relative)).unwrap();
        let b = fs::read(parent_b.path().join("twin").join(relative)).unwrap();
        assert_eq!(a, b, "byte mismatch for {relative} between two runs");
    }
}

#[test]
fn new_lib_metadata_view_reports_library_target() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("metalib");
    cabin()
        .current_dir(parent.path())
        .args(["new", "metalib", "--lib"])
        .assert()
        .success();

    let value = run_metadata(&target.join("cabin.toml"));
    let pkg = package_in(&value, "metalib");
    assert_eq!(pkg["targets"][0]["kind"], "library");
}

#[test]
fn new_lib_builds_successfully() {
    require_cxx_build_tools();
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("buildlib");
    cabin()
        .current_dir(parent.path())
        .args(["new", "buildlib", "--lib"])
        .assert()
        .success();

    let build_dir = target.join("build");
    cabin()
        .current_dir(&target)
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();

    let lib_path = build_dir
        .join("dev")
        .join("packages")
        .join("buildlib")
        .join(host_static_lib("buildlib"));
    assert!(
        lib_path.is_file(),
        "expected library archive at {lib_path:?}"
    );
}

#[test]
fn new_bin_builds_successfully() {
    require_cxx_build_tools();
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("buildbin");
    cabin()
        .current_dir(parent.path())
        .args(["new", "buildbin"])
        .assert()
        .success();

    let build_dir = target.join("build");
    cabin()
        .current_dir(&target)
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();

    let bin_path = build_dir
        .join("dev")
        .join("packages")
        .join("buildbin")
        .join(host_exe("buildbin"));
    assert!(bin_path.is_file(), "expected executable at {bin_path:?}");
}

#[test]
fn new_bin_runs_and_prints_greeting() {
    require_cxx_build_tools();
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("hello_world");
    cabin()
        .current_dir(parent.path())
        .args(["new", "hello_world"])
        .assert()
        .success();

    let build_dir = target.join("build");
    let output = cabin()
        .current_dir(&target)
        .args(["run", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("Hello from Cabin"),
        "`cabin new` -> `cabin run` should print the scaffold greeting, got: {stdout}"
    );
}

#[test]
fn new_verbose_lists_created_files() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("verb");
    let output = cabin()
        .current_dir(parent.path())
        .args(["new", "verb", "--lib", "--verbose"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("cabin: wrote cabin.toml"),
        "verbose stdout missing cabin.toml: {stdout}"
    );
    assert!(
        stdout.contains("cabin: wrote include/verb/verb.hpp"),
        "verbose stdout missing header path: {stdout}"
    );
    assert!(
        stdout.contains("cabin: wrote src/verb.cc"),
        "verbose stdout missing source path: {stdout}"
    );
    assert!(
        stdout.contains("cabin: wrote .gitignore"),
        "verbose stdout missing .gitignore: {stdout}"
    );
    assert!(target.join("cabin.toml").exists());
}

// ---------------------------------------------------------------------------
// global verbosity (-v / --verbose / -q / --quiet)
// ---------------------------------------------------------------------------

#[path = "cli/verbosity.rs"]
mod verbosity;

// ---------------------------------------------------------------------------
// cabin clean
// ---------------------------------------------------------------------------

#[path = "cli/clean_cmd.rs"]
mod clean_cmd;

// ---------------------------------------------------------------------------
// cabin add / cabin remove
// ---------------------------------------------------------------------------

#[path = "cli/add_cmd.rs"]
mod add_cmd;

#[path = "cli/remove_cmd.rs"]
mod remove_cmd;

// ---------------------------------------------------------------------------
// single-package builds
// ---------------------------------------------------------------------------

/// Set up a hello-world C++ package in `dir` and run a default build.
/// Returns `dir/build/packages/hello/` for output assertions.
fn build_simple_executable(dir: &Path, extra_args: &[&str]) {
    write_hello_project(dir);

    let build_dir = dir.join("build");
    let mut cmd = cabin();
    cmd.current_dir(dir).arg("build");
    cmd.args(extra_args);
    cmd.arg("--build-dir").arg(&build_dir);
    cmd.assert().success();
}

#[test]
fn build_writes_ninja_and_compile_commands_for_simple_executable() {
    require_cxx_build_tools();

    let dir = TempDir::new().unwrap();
    build_simple_executable(dir.path(), &[]);

    let build_dir = dir.path().join("build");
    let pkg_dir = build_dir.join("dev").join("packages").join("hello");
    assert!(build_dir.join("dev").join("build.ninja").is_file());
    assert!(
        build_dir
            .join("dev")
            .join("compile_commands.json")
            .is_file()
    );
    assert!(
        pkg_dir.join(host_exe("hello")).is_file(),
        "executable should exist"
    );

    let output = std::process::Command::new(pkg_dir.join("hello"))
        .output()
        .expect("running hello should succeed");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Hello from Cabin"), "got: {stdout}");
}

#[test]
fn compile_commands_json_contains_expected_fields() {
    require_cxx_build_tools();

    let dir = TempDir::new().unwrap();
    build_simple_executable(dir.path(), &[]);

    let cc_path = dir
        .path()
        .join("build")
        .join("dev")
        .join("compile_commands.json");
    let body = fs::read_to_string(&cc_path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).expect("must be valid JSON");
    let arr = value.as_array().expect("must be array");
    assert_eq!(arr.len(), 1);
    let entry = &arr[0];
    assert!(entry["file"].as_str().unwrap().ends_with("src/main.cc"));
    assert!(
        entry["output"]
            .as_str()
            .unwrap()
            .ends_with(&host_path(&format!("src/main.cc.{}", host_obj_ext())))
    );
    let command = entry["command"].as_str().unwrap();
    assert!(command.contains(host_std_cxx_flag()));
    assert!(command.contains("src/main.cc"));
}

#[test]
fn build_links_executable_against_same_package_library() {
    require_cxx_build_tools();

    let dir = TempDir::new().unwrap();
    let manifest = r#"[package]
name = "hello"
version = "0.1.0"
cxx-standard = "c++17"

[target.greet]
type = "library"
sources = ["src/greet.cc"]
include-dirs = ["include"]

[target.hello]
type = "executable"
sources = ["src/main.cc"]
deps = ["greet"]
"#;
    let header = "#pragma once\nvoid greet();\n";
    let lib_src = "#include <iostream>\n#include \"greet.h\"\nvoid greet() { std::cout << \"hello from greet\\n\"; }\n";
    let main_src = "#include \"greet.h\"\nint main() { greet(); return 0; }\n";

    dir.child("cabin.toml").write_str(manifest).unwrap();
    dir.child("include/greet.h").write_str(header).unwrap();
    dir.child("src/greet.cc").write_str(lib_src).unwrap();
    dir.child("src/main.cc").write_str(main_src).unwrap();

    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();

    let pkg_dir = build_dir.join("dev").join("packages").join("hello");
    assert!(pkg_dir.join(host_static_lib("greet")).is_file());
    assert!(pkg_dir.join(host_exe("hello")).is_file());

    let output = std::process::Command::new(pkg_dir.join(host_exe("hello")))
        .output()
        .expect("running hello should succeed");
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello from greet"));
}

#[test]
fn release_flag_changes_compile_commands() {
    require_cxx_build_tools();

    let dir = TempDir::new().unwrap();
    build_simple_executable(dir.path(), &["--release"]);

    let release_dir = dir.path().join("build").join("release");
    let body = fs::read_to_string(release_dir.join("compile_commands.json"))
        .expect("compile_commands.json should be readable");
    let release_opt = host_release_opt_flag();
    let ndebug = host_define_ndebug_flag();
    let no_opt = host_no_opt_flag();
    assert!(
        body.contains(release_opt),
        "expected {release_opt} in: {body}"
    );
    assert!(body.contains(ndebug), "expected {ndebug} in: {body}");
    assert!(!body.contains(no_opt), "did not expect {no_opt} in: {body}");

    let ninja_body = fs::read_to_string(release_dir.join("build.ninja")).unwrap();
    assert!(ninja_body.contains(release_opt));
    assert!(ninja_body.contains(ndebug));
}

#[test]
fn cabin_build_rejects_target_flag_as_unknown_argument() {
    // `--target` is reserved for a future platform/toolchain
    // target selector.  Cabin no longer accepts it as a
    // manifest-target selector on `cabin build`, so clap must
    // reject the flag outright.  Pinning the rejection here keeps
    // the historic overload from quietly returning.
    cabin()
        .args(["build", "--target", "foo"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "unexpected argument '--target' found",
        ));
}

#[test]
fn target_dependency_cycle_fails_with_clear_error() {
    let dir = TempDir::new().unwrap();
    let manifest = r#"[package]
name = "cyc"
version = "0.1.0"
cxx-standard = "c++17"

[target.a]
type = "library"
sources = ["a.cc"]
deps = ["b"]

[target.b]
type = "library"
sources = ["b.cc"]
deps = ["a"]
"#;
    dir.child("cabin.toml").write_str(manifest).unwrap();
    dir.child("a.cc").write_str("// a\n").unwrap();
    dir.child("b.cc").write_str("// b\n").unwrap();

    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .failure()
        .stderr(predicate::str::contains("dependency cycle"));
}

// ---------------------------------------------------------------------------
// local path dependencies, workspaces, qualified targets
// ---------------------------------------------------------------------------

const GREET_HEADER: &str = "#pragma once\nvoid greet();\n";
const GREET_SRC: &str = "#include <iostream>\n#include \"greet.h\"\nvoid greet() { std::cout << \"hello from greet\\n\"; }\n";
const APP_MAIN: &str = "#include \"greet.h\"\nint main() { greet(); return 0; }\n";

/// Build the canonical app-depends-on-greet layout in `root`.
fn write_path_dep_project(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("greet/cabin.toml"))
        .write_str(
            r#"[package]
name = "greet"
version = "0.1.0"
cxx-standard = "c++17"

[target.greet]
type = "library"
sources = ["src/greet.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("greet/include/greet.h"))
        .write_str(GREET_HEADER)
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("greet/src/greet.cc"))
        .write_str(GREET_SRC)
        .unwrap();

    assert_fs::fixture::ChildPath::new(root.join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
greet = { path = "../greet" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["greet"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.cc"))
        .write_str(APP_MAIN)
        .unwrap();
}

#[test]
fn build_with_local_path_dependency_builds_executable() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_path_dep_project(dir.path());

    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();

    let app_pkg_dir = build_dir.join("dev").join("packages").join("app");
    let greet_pkg_dir = build_dir.join("dev").join("packages").join("greet");
    assert!(build_dir.join("dev").join("build.ninja").is_file());
    assert!(
        build_dir
            .join("dev")
            .join("compile_commands.json")
            .is_file()
    );
    assert!(greet_pkg_dir.join(host_static_lib("greet")).is_file());
    assert!(
        app_pkg_dir.join(host_exe("app")).is_file(),
        "app executable missing"
    );

    let output = std::process::Command::new(app_pkg_dir.join(host_exe("app")))
        .output()
        .expect("running app should succeed");
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello from greet"));
}

#[test]
fn metadata_includes_local_path_dependency() {
    let dir = TempDir::new().unwrap();
    write_path_dep_project(dir.path());

    let value = run_metadata(&dir.path().join("app/cabin.toml"));
    // No [workspace] -> workspace is null.
    assert!(value["workspace"].is_null());
    let app = package_in(&value, "app");
    let greet = package_in(&value, "greet");
    let app_deps = app["dependencies"].as_array().unwrap();
    assert_eq!(app_deps.len(), 1);
    assert_eq!(app_deps[0]["name"], "greet");
    assert_eq!(app_deps[0]["kind"], "path");
    assert!(app_deps[0]["path"].as_str().unwrap().ends_with("greet"));
    // The dep package is loaded too.
    assert!(
        greet["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "greet")
    );
    // Only `app` is primary; greet is pulled in as a dep.
    assert_eq!(app["is_primary"], true);
    assert_eq!(greet["is_primary"], false);
}

#[test]
fn compile_commands_includes_dependency_sources() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_path_dep_project(dir.path());

    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();

    let cc: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap(),
    )
    .unwrap();
    let arr = cc.as_array().unwrap();
    let files: Vec<String> = arr
        .iter()
        .map(|e| e["file"].as_str().unwrap().to_owned())
        .collect();
    // The compile-commands `file` field mixes separators on
    // Windows (backslash package-root boundary, forward-slash
    // manifest-relative source tail), so normalize to `/` before
    // matching the forward-slash expected suffix.
    let normalized: Vec<String> = files.iter().map(|f| f.replace('\\', "/")).collect();
    assert!(normalized.iter().any(|f| f.ends_with("greet/src/greet.cc")));
    assert!(normalized.iter().any(|f| f.ends_with("app/src/main.cc")));
}

#[test]
fn workspace_metadata_lists_members() {
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
    dir.child("packages/b/cabin.toml")
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"
"#,
        )
        .unwrap();
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let ws = &value["workspace"];
    assert!(!ws.is_null(), "workspace section missing in {value}");
    let members = ws["members"].as_array().unwrap();
    let names: Vec<&str> = members.iter().map(|m| m.as_str().unwrap()).collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
}

#[test]
fn package_dependency_cycle_fails_with_clear_error() {
    let dir = TempDir::new().unwrap();
    dir.child("a/cabin.toml")
        .write_str(
            r#"[package]
name = "a"
version = "0.1.0"

[dependencies]
b = { path = "../b" }
"#,
        )
        .unwrap();
    dir.child("b/cabin.toml")
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
a = { path = "../a" }
"#,
        )
        .unwrap();

    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("a/cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("package dependency cycle"));
}

#[test]
fn duplicate_workspace_package_names_fail() {
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
name = "shared"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("packages/b/cabin.toml")
        .write_str(
            r#"[package]
name = "shared"
version = "0.2.0"
"#,
        )
        .unwrap();

    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("duplicate package name"));
}

#[test]
fn dependency_name_mismatch_fails_with_clear_error() {
    let dir = TempDir::new().unwrap();
    dir.child("greet/cabin.toml")
        .write_str(
            r#"[package]
name = "actually-hello"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
"#,
        )
        .unwrap();

    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "dependency aliases are not supported",
        ));
}

// ---------------------------------------------------------------------------
// versioned dependencies, local JSON index, and `cabin resolve`
// ---------------------------------------------------------------------------

const FMT_INDEX: &str = r#"{
  "schema": 1,
  "name": "fmt",
  "versions": {
    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" },
    "10.1.0": { "dependencies": {}, "yanked": false, "checksum": "sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210" }
  }
}"#;

const SPDLOG_INDEX: &str = r#"{
  "schema": 1,
  "name": "spdlog",
  "versions": {
    "1.13.0": {
      "dependencies": { "fmt": ">=10.0.0 <11.0.0" },
      "yanked": false,
      "checksum": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  }
}"#;

#[test]
fn resolve_succeeds_for_direct_dependency() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json").write_str(FMT_INDEX).unwrap();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Resolved dependencies for app 0.1.0"));
    assert!(stdout.contains("fmt 10.2.1"), "stdout: {stdout}");
}

#[test]
fn resolve_emits_valid_json() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json").write_str(FMT_INDEX).unwrap();

    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .args(["--format", "json"]),
    );
    assert_eq!(value["root"]["name"], "app");
    assert_eq!(value["root"]["version"], "0.1.0");
    let packages = value["packages"].as_array().unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"], "fmt");
    assert_eq!(packages[0]["version"], "10.2.1");
    assert_eq!(packages[0]["source"], "index");
}

#[test]
fn resolve_handles_transitive_dependency() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"spdlog = "^1.13.0""#);
    dir.child("index/fmt.json").write_str(FMT_INDEX).unwrap();
    dir.child("index/spdlog.json")
        .write_str(SPDLOG_INDEX)
        .unwrap();

    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .args(["--format", "json"]),
    );
    let names: Vec<&str> = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"spdlog"));
    assert!(names.contains(&"fmt"));
}

#[test]
fn resolve_skips_yanked_versions() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    let yanked_index = r#"{
        "schema": 1,
        "name": "fmt",
        "versions": {
            "10.2.1": { "dependencies": {}, "yanked": true },
            "10.1.0": { "dependencies": {}, "yanked": false }
        }
    }"#;
    dir.child("index/fmt.json").write_str(yanked_index).unwrap();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("fmt 10.1.0"), "stdout: {stdout}");
    assert!(!stdout.contains("fmt 10.2.1"));
}

#[test]
fn resolve_reports_conflict_clearly() {
    let dir = TempDir::new().unwrap();
    let manifest = r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = "^10"
spdlog = "*"
"#;
    dir.child("app/cabin.toml").write_str(manifest).unwrap();
    // spdlog 1.0 wants fmt >=11; fmt only has 10.x available, so conflict.
    dir.child("index/fmt.json")
        .write_str(
            r#"{
            "schema": 1,
            "name": "fmt",
            "versions": { "10.2.1": { "dependencies": {} } }
        }"#,
        )
        .unwrap();
    dir.child("index/spdlog.json")
        .write_str(
            r#"{
            "schema": 1,
            "name": "spdlog",
            "versions": {
                "1.0.0": { "dependencies": { "fmt": ">=11, <12" } }
            }
        }"#,
        )
        .unwrap();

    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("dependency resolution failed"));
}

#[test]
fn resolve_without_index_path_fails_clearly() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = "^10""#);

    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("--index-path"));
}

#[test]
fn resolve_missing_package_fails_clearly() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"missing-pkg = "^1""#);
    dir.child("index/fmt.json")
        .write_str(r#"{ "schema": 1, "name": "fmt", "versions": {} }"#)
        .unwrap();

    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("missing-pkg"));
}

#[test]
fn build_with_versioned_dependency_requires_index_path() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = "^10""#);
    dir.child("app/src/main.cc")
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    // Add a target so the build would otherwise have something to do.
    let manifest = r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
fmt = "^10"

[target.app]
type = "executable"
sources = ["src/main.cc"]
"#;
    dir.child("app/cabin.toml").write_str(manifest).unwrap();

    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--index-path"));
}

#[test]
fn metadata_records_versioned_dependency() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);

    let value = run_metadata(&dir.path().join("app/cabin.toml"));
    let app = package_in(&value, "app");
    let deps = app["dependencies"].as_array().unwrap();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0]["name"], "fmt");
    assert_eq!(deps[0]["kind"], "version");
    assert!(
        deps[0]["requirement"]
            .as_str()
            .unwrap()
            .contains(">=10.0.0")
    );
}

#[test]
fn resolve_with_no_versioned_deps_succeeds() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "alone"
version = "0.1.0"
"#,
        )
        .unwrap();
    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json"]),
    );
    assert_eq!(value["root"]["name"], "alone");
    assert_eq!(value["packages"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// lockfile + `cabin update`
// ---------------------------------------------------------------------------

const FMT_INDEX_TWO_VERSIONS: &str = r#"{
  "schema": 1,
  "name": "fmt",
  "versions": {
    "10.2.0": { "dependencies": {}, "yanked": false, "checksum": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" },
    "10.1.0": { "dependencies": {}, "yanked": false, "checksum": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc" }
  }
}"#;

const FMT_INDEX_OLDER_ONLY: &str = r#"{
  "schema": 1,
  "name": "fmt",
  "versions": {
    "10.1.0": { "dependencies": {}, "yanked": false, "checksum": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc" }
  }
}"#;

fn write_app_with_dep(dir: &Path, dep: &str) {
    assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
{dep}
"#
        ))
        .unwrap();
}

#[test]
fn resolve_writes_lockfile() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .success();

    let lock_path = dir.path().join("app/cabin.lock");
    assert!(lock_path.is_file(), "cabin.lock should exist");
    let body = fs::read_to_string(&lock_path).unwrap();
    assert!(body.contains("version = 1"));
    assert!(body.contains(r#"name = "fmt""#));
    assert!(body.contains(r#"version = "10.2.0""#));
    assert!(body.contains(r#"source = "index""#));
}

#[test]
fn resolve_is_deterministic_across_runs() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    let manifest = dir.path().join("app/cabin.toml");
    let index = dir.path().join("index");
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();
    let first = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();
    let second = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
    assert_eq!(first, second);
}

#[test]
fn resolve_prefers_existing_lockfile() {
    // First, resolve against an index that only has 10.1.0; the
    // lockfile pins 10.1.0.  Then add 10.2.0 to the index and resolve
    // again; the lockfile should keep 10.1.0.
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    let index_dir = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index_dir.join("fmt.json"))
        .write_str(FMT_INDEX_OLDER_ONLY)
        .unwrap();

    let manifest = dir.path().join("app/cabin.toml");
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index_dir)
        .assert()
        .success();

    // Add 10.2.0 to the index.
    assert_fs::fixture::ChildPath::new(index_dir.join("fmt.json"))
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index_dir)
        .assert()
        .success();

    let body = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
    assert!(
        body.contains(r#"version = "10.1.0""#),
        "lockfile body: {body}"
    );
    assert!(!body.contains(r#"version = "10.2.0""#));
}

#[test]
fn cabin_update_refreshes_lockfile() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    let index_dir = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index_dir.join("fmt.json"))
        .write_str(FMT_INDEX_OLDER_ONLY)
        .unwrap();

    let manifest = dir.path().join("app/cabin.toml");
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index_dir)
        .assert()
        .success();
    let before = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
    assert!(before.contains(r#"version = "10.1.0""#));

    assert_fs::fixture::ChildPath::new(index_dir.join("fmt.json"))
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    cabin()
        .args(["update", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index_dir)
        .assert()
        .success();
    let after = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
    assert!(after.contains(r#"version = "10.2.0""#), "after: {after}");
}

#[test]
fn locked_fails_when_lockfile_missing() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    cabin()
        .args(["resolve", "--locked", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("--locked"))
        .stderr(predicate::str::contains("does not exist"));
}

#[test]
fn locked_succeeds_when_lockfile_is_current() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    let manifest = dir.path().join("app/cabin.toml");
    let index = dir.path().join("index");
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();
    let snapshot = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();

    cabin()
        .args(["resolve", "--locked", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();
    let after = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
    assert_eq!(snapshot, after);
}

#[test]
fn locked_fails_when_lockfile_is_stale() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_OLDER_ONLY)
        .unwrap();

    let manifest = dir.path().join("app/cabin.toml");
    let index = dir.path().join("index");
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();

    // Tighten the constraint to require >=10.2.0; the locked 10.1.0
    // can no longer satisfy it.
    write_app_with_dep(dir.path(), r#"fmt = ">=10.2.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    cabin()
        .args(["resolve", "--locked", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not satisfy"));
}

#[test]
fn frozen_does_not_write_lockfile() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    cabin()
        .args(["resolve", "--frozen", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .failure();
    assert!(
        !dir.path().join("app/cabin.lock").exists(),
        "cabin.lock must not be created in --frozen mode"
    );
}

// ---------------------------------------------------------------------------
// Resolver diagnostic rendering
// ---------------------------------------------------------------------------

/// A conflict between two callers should surface Cabin's
/// stable resolver diagnostic code, both package names
/// involved, and the conflicting version requirement -
/// rendered through the no-color path so the assertion stays
/// byte-stable on any terminal.
#[test]
fn resolve_conflict_renders_diagnostic_with_code_and_packages() {
    let dir = TempDir::new().unwrap();
    // The root pulls `a ^1` and `b ^1`. `b 1.0.0` requires
    // `a ^2`, which is unsatisfiable against `a 1.0.0`.
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
a = "^1"
b = "^1"
"#,
        )
        .unwrap();
    dir.child("index/a.json")
        .write_str(
            r#"{
  "schema": 1,
  "name": "a",
  "versions": {
    "1.0.0": { "dependencies": {}, "yanked": false }
  }
}"#,
        )
        .unwrap();
    dir.child("index/b.json")
        .write_str(
            r#"{
  "schema": 1,
  "name": "b",
  "versions": {
    "1.0.0": { "dependencies": { "a": "^2" }, "yanked": false }
  }
}"#,
        )
        .unwrap();

    cabin()
        .args(["resolve", "--color", "never", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("cabin::resolver::error"))
        .stderr(predicate::str::contains("a"))
        .stderr(predicate::str::contains("b"));
}

/// `--locked` failures keep their targeted message ("does not
/// satisfy") and still carry the stable diagnostic code, so the
/// preserved error specificity flows through the renderer.
#[test]
fn locked_failure_diagnostic_keeps_targeted_message_and_code() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_OLDER_ONLY)
        .unwrap();

    let manifest = dir.path().join("app/cabin.toml");
    let index = dir.path().join("index");
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();

    write_app_with_dep(dir.path(), r#"fmt = ">=10.2.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    cabin()
        .args(["resolve", "--locked", "--color", "never", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(&index)
        .assert()
        .failure()
        .stderr(predicate::str::contains("cabin::resolver::error"))
        .stderr(predicate::str::contains("does not satisfy"))
        .stderr(predicate::str::contains("cabin update"));
}

#[test]
fn metadata_includes_lockfile_when_present() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    let manifest = dir.path().join("app/cabin.toml");
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .success();

    let value = run_metadata(&manifest);
    let lockfile = &value["lockfile"];
    assert!(!lockfile.is_null(), "metadata should include lockfile");
    assert_eq!(lockfile["version"], 1);
    let pkgs = lockfile["packages"].as_array().unwrap();
    assert_eq!(pkgs.len(), 1);
    assert_eq!(pkgs[0]["name"], "fmt");
    assert_eq!(pkgs[0]["version"], "10.2.0");
    assert_eq!(pkgs[0]["source"], "index");
}

#[test]
fn metadata_lockfile_field_is_null_when_no_lockfile() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);

    let value = run_metadata(&dir.path().join("app/cabin.toml"));
    assert!(value["lockfile"].is_null());
}

#[test]
fn resolve_json_format_still_emits_valid_json_with_lockfile() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    let value = run_json(
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .args(["--format", "json"]),
    );
    assert_eq!(value["root"]["name"], "app");
    let pkgs = value["packages"].as_array().unwrap();
    assert_eq!(pkgs.len(), 1);
    assert_eq!(pkgs[0]["name"], "fmt");
}

#[test]
fn cabin_update_with_unknown_package_errors() {
    let dir = TempDir::new().unwrap();
    write_app_with_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json")
        .write_str(FMT_INDEX_TWO_VERSIONS)
        .unwrap();

    cabin()
        .args(["update", "--package", "nope", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("nope"));
}

// ---------------------------------------------------------------------------
// cabin fetch + registry-aware cabin build
// ---------------------------------------------------------------------------

#[path = "cli/artifact_fetch.rs"]
mod artifact_fetch;

// ---------------------------------------------------------------------------
// cabin package + cabin publish --dry-run
// ---------------------------------------------------------------------------

#[path = "cli/package_archive.rs"]
mod package_archive;

// ---------------------------------------------------------------------------
// cabin compgen + cabin mangen
// ---------------------------------------------------------------------------

#[path = "cli/distribution_artifacts.rs"]
mod distribution_artifacts;

// ---------------------------------------------------------------------------
// cabin publish --registry-dir
// ---------------------------------------------------------------------------

#[path = "cli/file_registry.rs"]
mod file_registry;

// ---------------------------------------------------------------------------
// cabin <cmd> --index-url against a static HTTP registry
// ---------------------------------------------------------------------------

#[path = "cli/sparse_http.rs"]
mod sparse_http;

// ---------------------------------------------------------------------------
// features foundation
// ---------------------------------------------------------------------------

#[path = "cli/features.rs"]
mod features;

// ---------------------------------------------------------------------------
// advanced workspace semantics - members/exclude/default-members,
// --workspace / -p / --exclude / --default-members selection flags,
// workspace dependency inheritance, root discovery from a member dir,
// nested workspace rejection.
// ---------------------------------------------------------------------------

#[path = "cli/workspace_semantics.rs"]
mod workspace_semantics;

// ---------------------------------------------------------------------------
// workspace-inherited language standards - `[workspace]` standard
// defaults + per-field `{ workspace = true }` member opt-ins.
// ---------------------------------------------------------------------------

#[path = "cli/workspace_language_standards.rs"]
mod workspace_language_standards;

// ---------------------------------------------------------------------------
// workspace-dependency archive normalization - `dep = { workspace = true }`
// markers rewritten to the root's literal requirement strings at
// `cabin package` / `cabin publish` time.
// ---------------------------------------------------------------------------

#[path = "cli/workspace_dependency_normalization.rs"]
mod workspace_dependency_normalization;

// ---------------------------------------------------------------------------
// post-merge regressions on the advanced-workspace-semantics surface.
// ---------------------------------------------------------------------------

#[path = "cli/workspace_review.rs"]
mod workspace_review;

// ---------------------------------------------------------------------------
// Workspace-selection hardening - selected-closure index requirement, target
// scoping, Cargo scoping, feature scoping, package/publish workspace
// dep resolution, registry path safety + name mismatch validation,
// nested-workspace consistency, --exclude policy, update --package
// back-compat.
// ---------------------------------------------------------------------------

#[path = "cli/workspace_selection_hardening.rs"]
mod workspace_selection_hardening;

#[path = "cli/strict_nested_workspace_discovery.rs"]
mod strict_nested_workspace_discovery;

#[path = "cli/workspace_selection_followups.rs"]
mod workspace_selection_followups;

#[path = "cli/dependency_kinds.rs"]
mod dependency_kinds;

#[path = "cli/dev_dependencies.rs"]
mod dev_dependencies;

#[path = "cli/optional_dependencies_and_features.rs"]
mod optional_dependencies_and_features;

#[path = "cli/required_features.rs"]
mod required_features;

#[path = "cli/explicit_target_deps.rs"]
mod explicit_target_deps;

#[path = "cli/target_dependencies.rs"]
mod target_dependencies;

#[path = "cli/profiles.rs"]
mod profiles;

#[path = "cli/toolchain.rs"]
mod toolchain;

#[path = "cli/compiler_detection.rs"]
mod compiler_detection;

#[path = "cli/compiler_conditions.rs"]
mod compiler_conditions;

#[path = "cli/compiler_cache.rs"]
mod compiler_cache;

#[path = "cli/config.rs"]
mod config;

#[path = "cli/patches.rs"]
mod patches;

#[path = "cli/test_targets.rs"]
mod test_targets;

#[path = "cli/c_language.rs"]
mod c_language;

#[path = "cli/language_standards.rs"]
mod language_standards;

#[path = "cli/vendor_offline.rs"]
mod vendor_offline;

// ---------------------------------------------------------------------------
// cabin tree + cabin explain
// ---------------------------------------------------------------------------

#[path = "cli/metadata_tree_explain.rs"]
mod metadata_tree_explain;

// ---------------------------------------------------------------------------
// cabin run + CABIN_* env vars
// ---------------------------------------------------------------------------

#[path = "cli/cargo_interface.rs"]
mod cargo_interface;

// ---------------------------------------------------------------------------
// post-Cargo-inspired-foundation help / env-var review tests
// ---------------------------------------------------------------------------

#[path = "cli/cargo_interface_cleanup.rs"]
mod cargo_interface_cleanup;

// ---------------------------------------------------------------------------
// Diagnostic / error-rendering refactor
// ---------------------------------------------------------------------------

#[path = "cli/diagnostics.rs"]
mod diagnostics;

/// `--color` / `CABIN_TERM_COLOR` integration tests.
///
/// The tests below exercise the user-visible color contract:
/// - `--color` parsing (clap rejects unknown values),
/// - `CABIN_TERM_COLOR` parsing (Cabin rejects unknown values
///   with a documented wording),
/// - `--color` overrides `CABIN_TERM_COLOR`,
/// - `--color always` produces ANSI escape sequences in
///   diagnostic output even when stderr is captured,
/// - `--color never` produces none even when the env says
///   `always`,
/// - help text exposes the option with the documented
///   possible-value list.
#[path = "cli/color_control.rs"]
mod color_control;

// ---------------------------------------------------------------------------
// `cabin fmt`
// ---------------------------------------------------------------------------

#[path = "cli/fmt_command.rs"]
mod fmt_command;

// ---------------------------------------------------------------------------
// `-j` / `--jobs <N>` for build / run / tidy
// ---------------------------------------------------------------------------

#[path = "cli/jobs_parallelism.rs"]
mod jobs_parallelism;

// ---------------------------------------------------------------------------
// `cabin tidy`
// ---------------------------------------------------------------------------

#[path = "cli/tidy_command.rs"]
mod tidy_command;

#[path = "cli/system_deps_pkg_config.rs"]
mod system_deps_pkg_config;

/// Integration tests for the conventional C/C++ build-flag
/// environment variables: `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, and
/// `LDFLAGS`.  These cover the parsing, ordering, fingerprint,
/// pkg-config interaction, and `cabin fmt` isolation
/// requirements.
#[path = "cli/env_build_flags.rs"]
mod env_build_flags;

#[path = "cli/version_output.rs"]
mod version_output;

#[path = "cli/environment_variable_docs.rs"]
mod environment_variable_docs;

#[path = "cli/vendoring_docs.rs"]
mod vendoring_docs;

#[path = "cli/toolchains_docs.rs"]
mod toolchains_docs;

#[path = "cli/architecture_docs.rs"]
mod architecture_docs;

#[path = "cli/installation_and_metadata_docs.rs"]
mod installation_and_metadata_docs;

#[path = "cli/profiles_docs.rs"]
mod profiles_docs;

#[path = "cli/workspaces_docs.rs"]
mod workspaces_docs;

#[path = "cli/cargo_interface_docs.rs"]
mod cargo_interface_docs;

#[path = "cli/curated_help_and_list.rs"]
mod curated_help_and_list;

// ---------------------------------------------------------------
// cabin port subcommand
// ---------------------------------------------------------------

#[test]
fn cabin_port_list_prints_zlib() {
    let assertion = cabin().args(["port", "list"]).assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    assert!(
        stdout.contains("zlib") && stdout.contains("1.3.1"),
        "expected name + version in output: {stdout}"
    );
}

// ---------------------------------------------------------------
// Foundation-port end-to-end pipeline
// ---------------------------------------------------------------

/// End-to-end coverage for the zlib foundation-port pipeline:
/// a downstream Cabin consumer declares
/// `{ port-path = "..." }`, the CLI downloads + verifies + extracts
/// the upstream archive, applies the overlay, and the planner
/// links an `executable` that calls `zlibVersion()`.
///
/// The tests are hermetic: a `tiny_http` loopback server serves
/// a synthesized "fake-zlib" archive whose layout matches the
/// real upstream archive (one `zlib.h` + one `zlib.c` under a
/// `zlib-1.3.1/` prefix dir).  The mock proves the mechanics
/// without touching `zlib.net` or GitHub.
#[path = "cli/foundation_port_zlib.rs"]
mod foundation_port_zlib;

#[path = "cli/foundation_port_cjson.rs"]
mod foundation_port_cjson;

#[path = "cli/foundation_port_xxhash.rs"]
mod foundation_port_xxhash;

#[path = "cli/foundation_port_tinyxml2.rs"]
mod foundation_port_tinyxml2;

#[path = "cli/foundation_port_sqlite.rs"]
mod foundation_port_sqlite;

#[path = "cli/foundation_port_mock_smoke.rs"]
mod foundation_port_mock_smoke;

#[path = "cli/foundation_port_libpng.rs"]
mod foundation_port_libpng;

#[path = "cli/foundation_port_fmt.rs"]
mod foundation_port_fmt;

#[path = "cli/foundation_port_spdlog.rs"]
mod foundation_port_spdlog;

#[path = "cli/foundation_port_googletest.rs"]
mod foundation_port_googletest;

#[path = "cli/foundation_port_catch2.rs"]
mod foundation_port_catch2;

#[path = "cli/foundation_port_nlohmann_json.rs"]
mod foundation_port_nlohmann_json;

#[path = "cli/foundation_port_cli11.rs"]
mod foundation_port_cli11;

#[path = "cli/foundation_port_miniz.rs"]
mod foundation_port_miniz;

#[path = "cli/foundation_port_stb.rs"]
mod foundation_port_stb;

#[path = "cli/foundation_port_uthash.rs"]
mod foundation_port_uthash;

#[path = "cli/foundation_port_inih.rs"]
mod foundation_port_inih;

#[path = "cli/foundation_port_picohttpparser.rs"]
mod foundation_port_picohttpparser;
