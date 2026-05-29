#![allow(
    clippy::needless_raw_string_hashes,
    clippy::uninlined_format_args,
    clippy::format_push_string,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::single_match_else,
    clippy::redundant_closure_for_method_calls,
    clippy::manual_let_else,
    clippy::map_unwrap_or,
    clippy::stable_sort_primitive,
    clippy::items_after_statements
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use assert_cmd::Command;
use assert_fs::TempDir;
use assert_fs::prelude::*;
use cabin::Cli;
use clap::CommandFactory;
use predicates::prelude::*;

const SKIP_EXTERNAL_TOOL_TESTS_ENV: &str = "CABIN_SKIP_EXTERNAL_TOOL_TESTS";

/// All top-level subcommand names registered with clap,
/// derived from `Cli::command()` so tests never hard-code the
/// list.  The `help` pseudo-subcommand that clap auto-injects
/// is filtered because Cabin never advertises it as a public
/// command.
fn all_subcommand_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .filter(|sub| sub.get_name() != "help")
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

/// Subset of [`all_subcommand_names`] that `cabin --help`
/// advertises — the visible, day-to-day surface.
fn visible_subcommand_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .filter(|sub| sub.get_name() != "help" && !sub.is_hide_set())
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

/// Names of subcommands hidden from `cabin --help` but still
/// reachable through `cabin --list`, shell completions, and
/// per-subcommand man pages.
fn hidden_subcommand_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .filter(|sub| sub.is_hide_set())
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

/// Extract the command-row names from a clap-rendered
/// `--help` payload.  Each row in the `Commands:` block has
/// the shape `  <name><spaces><description>`; this helper
/// returns just the `<name>` tokens so callers do not have to
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
        // The block ends at the first blank line — clap then
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

[target.hello]
type = "cpp_executable"
sources = ["src/main.cc"]
"#;

const HELLO_MAIN_CC: &str = "#include <iostream>\n\nint main() {\n    std::cout << \"Hello from Cabin\\n\";\n    return 0;\n}\n";

fn cabin() -> Command {
    let mut cmd = Command::cargo_bin("cabin").expect("the `cabin` binary should be built by cargo");
    // Isolate every integration test from a developer's own
    // `~/.config/cabin/config.toml`. Tests that exercise config
    // discovery on purpose explicitly re-enable it via
    // `.env_remove("CABIN_NO_CONFIG")` or a custom
    // `CABIN_CONFIG_HOME`.
    cmd.env("CABIN_NO_CONFIG", "1")
        .env_remove("CABIN_CONFIG")
        .env_remove("CABIN_CONFIG_HOME");
    // Toolchain / wrapper / build-flag environment leaks are
    // the most common reason a test that passes locally fails in
    // CI (or vice versa). Strip the high-impact variables this
    // helper owns so a test sees only those inputs it sets
    // explicitly. Tests that depend on build-dir, jobs, or
    // verbosity env vars set or remove those per test.
    // Tests that exercise env precedence opt back in by calling
    // `.env(KEY, VALUE)` *after* this helper — `assert_cmd`
    // applies env mutations in declaration order, so a later
    // `.env(...)` overrides this `.env_remove(...)`.
    for key in [
        "CC",
        "CXX",
        "AR",
        "NINJA",
        "CFLAGS",
        "CXXFLAGS",
        // CPPFLAGS is read by the build orchestration layer
        // and merged into per-package compile flags. Strip it
        // so a developer's `CPPFLAGS=-I/opt/...` shell state
        // cannot bleed into golden output or verbose-on-stderr
        // assertions.
        "CPPFLAGS",
        "LDFLAGS",
        "CABIN_NET_OFFLINE",
        "CABIN_COMPILER_WRAPPER",
        "CABIN_CACHE_DIR",
        // `CABIN_CACHE_HOME` redirects the per-user cache home;
        // strip it so a developer's environment can't bleed into
        // tests that observe cache state. Tests that exercise
        // foundation-port HTTP traffic pass an explicit
        // `--cache-dir` instead.
        "CABIN_CACHE_HOME",
        "CABIN_FMT",
        "CABIN_TIDY",
        // System dependency probing reads `CABIN_PKG_CONFIG`
        // and Cabin passes the rest of the standard pkg-config
        // environment through to its child process. Strip every
        // one of them so an integration test sees only the
        // overrides it sets explicitly.
        "CABIN_PKG_CONFIG",
        "PKG_CONFIG_PATH",
        "PKG_CONFIG_LIBDIR",
        "PKG_CONFIG_SYSROOT_DIR",
        // termcolor's `Auto` decision honors `NO_COLOR`,
        // `CLICOLOR`, and `CLICOLOR_FORCE`. Strip them so a
        // developer's shell configuration does not flip the
        // default away from "no color".
        "NO_COLOR",
        "CLICOLOR",
        "CLICOLOR_FORCE",
    ] {
        cmd.env_remove(key);
    }
    // Force the default test binary to emit no color so
    // existing substring-based assertions stay byte-stable
    // regardless of whether the test harness's stderr
    // ultimately resolves to a terminal. Tests that exercise
    // the color contract (in `mod color_control`) override
    // this with `--color` or `CABIN_TERM_COLOR` explicitly;
    // assert_cmd applies env mutations in declaration order so
    // a later `.env(...)` overrides this default.
    cmd.env("CABIN_TERM_COLOR", "never");
    pin_test_cache_home(&mut cmd);
    cmd
}

/// Pin `CABIN_CACHE_HOME` to a deterministic temp path. Tests
/// routinely strip `HOME` for config isolation, which would
/// otherwise leave the user-global cache fallback
/// (`$CABIN_CACHE_HOME` ▶ `$XDG_CACHE_HOME/cabin` ▶
/// `$HOME/.cache/cabin`) with nothing to resolve to in CI,
/// where `XDG_CACHE_HOME` is unset. The cache is
/// content-addressed, so parallel writers to the same path are
/// safe. Tests that observe cache state still pass an explicit
/// `--cache-dir`, which takes precedence.
fn pin_test_cache_home(cmd: &mut Command) {
    cmd.env(
        "CABIN_CACHE_HOME",
        std::env::temp_dir().join("cabin-tests-cache-home"),
    );
}

/// Pin `HOME` and `XDG_CONFIG_HOME` to deterministic temp paths
/// that contain no Cabin config. The `xdg` crate falls back to
/// `getpwuid_r` when `HOME` is unset, so simply removing `HOME`
/// from the subprocess environment would leave a developer's
/// real `~/.config/cabin/config.toml` reachable. Pointing both
/// variables at empty temp paths is the robust equivalent of
/// "no user config home" for tests that exercise config
/// discovery. Tests that exercise specific config-home arms
/// override these with later `.env(...)` calls (`assert_cmd`
/// applies env mutations in declaration order).
///
/// The path is wiped once per `cargo test` invocation so a
/// stale `config.toml` written by a previous run can never leak
/// in. Cabin does not write into the user config home itself, so
/// a single cleanup at the first call is sufficient — later
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

fn command_exists(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn use_fake_external_tools() -> bool {
    std::env::var_os(SKIP_EXTERNAL_TOOL_TESTS_ENV).is_some_and(|value| !value.is_empty())
}

fn require_external_tool(name: &str) {
    assert!(
        command_exists(name),
        "external tool `{name}` is required for this test; install it or set {SKIP_EXTERNAL_TOOL_TESTS_ENV}=1 to run the external-tool smoke tests against bundled fake tools"
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

/// Whether Ninja is available on `PATH`. Cabin invokes Ninja
/// directly for every `cabin build` / `cabin test` integration
/// test that produces real artifacts; tests gate on this
/// helper to skip cleanly on environments without it.
fn ninja_available() -> bool {
    command_exists("ninja")
}

/// Whether at least one of Cabin's documented C compiler
/// fallbacks is on `PATH` (`cc` / `clang` / `gcc`). Tests that
/// compile `.c` translation units gate on this helper so they
/// do not silently fall through to a `MissingCCompiler` error
/// at planner time on a system that has only a C++ compiler.
fn c_compiler_available() -> bool {
    ["cc", "clang", "gcc"]
        .iter()
        .any(|name| command_exists(name))
}

/// Whether at least one of Cabin's documented C++ compiler
/// fallbacks is on `PATH` (`c++` / `clang++` / `g++`).
fn cxx_compiler_available() -> bool {
    ["c++", "clang++", "g++"]
        .iter()
        .any(|name| command_exists(name))
}

/// Whether the integration tests that build C++ targets via
/// real Ninja can run. Use this for tests that link only C++
/// translation units. Tests that touch C must use
/// [`c_and_cxx_build_tools_available`] instead.
fn build_tools_available() -> bool {
    ninja_available() && cxx_compiler_available()
}

/// Whether the integration tests that build *both* C and C++
/// targets via real Ninja can run. Required by every test that
/// compiles `.c` sources alongside C++ sources, and by pure-C
/// tests (Cabin still requires a C++ compiler at toolchain
/// resolution time even when only C is built).
fn c_and_cxx_build_tools_available() -> bool {
    ninja_available() && c_compiler_available() && cxx_compiler_available()
}

fn skip(test_name: &str, reason: &str) {
    eprintln!("test `{test_name}` skipped: {reason}");
}

mod external_tool_smoke {
    use super::*;

    fn external_tool(real: &str, fake: &str) -> PathBuf {
        if use_fake_external_tools() {
            workspace_test_bin(fake)
        } else {
            require_external_tool(real);
            PathBuf::from(real)
        }
    }

    fn assert_process_success(tool: &Path, args: &[&str], label: &str) {
        let output = std::process::Command::new(tool)
            .args(args)
            .output()
            .unwrap_or_else(|err| panic!("failed to spawn {label} at {}: {err}", tool.display()));
        assert!(
            output.status.success(),
            "{label} at {} exited with {:?}\nstdout:\n{}\nstderr:\n{}",
            tool.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_cpp_project(root: &TempDir, manifest_tail: &str, source: &str) {
        root.child("cabin.toml")
            .write_str(&format!("{VALID_MANIFEST}\n{manifest_tail}"))
            .unwrap();
        root.child("src/main.cc").write_str(source).unwrap();
    }

    #[test]
    fn ninja_is_available_or_fake_backend_is_selected() {
        let tool = external_tool("ninja", "cabin-ninja-fake-ninja");
        assert_process_success(&tool, &["--version"], "ninja");
    }

    #[test]
    fn pkg_config_is_available_or_fake_probe_is_selected() {
        let tool = external_tool("pkg-config", "cabin-system-deps-fake-pkg-config");
        assert_process_success(&tool, &["--version"], "pkg-config");
    }

    #[test]
    fn cabin_fmt_reaches_real_formatter_or_fake_formatter() {
        let dir = TempDir::new().unwrap();
        let source = if use_fake_external_tools() {
            "int main() { return 0; }\n/* FORMATTED */\n"
        } else {
            "int main() { return 0; }\n"
        };
        write_cpp_project(&dir, "", source);
        dir.child(".clang-format")
            .write_str("BasedOnStyle: LLVM\n")
            .unwrap();

        let mut cmd = cabin();
        if use_fake_external_tools() {
            cmd.env("CABIN_FMT", workspace_test_bin("cabin-fmt-fake-formatter"));
        } else {
            require_external_tool("clang-format");
        }
        cmd.current_dir(dir.path())
            .args(["fmt", "--check"])
            .assert()
            .success();
    }

    /// `cabin lint` was removed alongside the cpplint wrapper.
    /// Pin the absence so a regression cannot reintroduce a
    /// hidden command path or alias.
    #[test]
    fn cabin_lint_subcommand_no_longer_exists() {
        let dir = TempDir::new().unwrap();
        write_cpp_project(&dir, "", "int main() { return 0; }\n");
        cabin()
            .current_dir(dir.path())
            .arg("lint")
            .assert()
            .failure()
            .stderr(predicate::str::contains("unrecognized subcommand"));
    }

    #[test]
    fn cabin_tidy_reaches_real_tidy_or_fake_tidy() {
        let dir = TempDir::new().unwrap();
        write_cpp_project(&dir, "", "int main() { return 0; }\n");
        dir.child(".clang-tidy")
            .write_str("Checks: '-*,clang-diagnostic-*,clang-analyzer-core.*'\n")
            .unwrap();

        let mut cmd = cabin();
        if use_fake_external_tools() {
            cmd.env("CABIN_TIDY", workspace_test_bin("cabin-tidy-fake-tidy"));
            let dummy_tool = workspace_test_bin("cabin-ninja-fake-ninja");
            cmd.env("CXX", &dummy_tool);
            cmd.env("CC", &dummy_tool);
            cmd.env("AR", &dummy_tool);
        } else {
            require_external_tool("run-clang-tidy");
            assert!(
                build_tools_available(),
                "real `cabin tidy` smoke test requires ninja and a C++ compiler; install them or set {SKIP_EXTERNAL_TOOL_TESTS_ENV}=1 to use bundled fake tools"
            );
        }
        cmd.current_dir(dir.path()).arg("tidy").assert().success();
    }
}

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
    assert!(manifest.contains("[target.hello]"));

    let main_cc = dir.path().join("src").join("main.cc");
    assert!(main_cc.is_file(), "src/main.cc should exist");
    let main_contents = fs::read_to_string(&main_cc).unwrap();
    assert!(main_contents.contains("int main"));
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
    // The metadata view wraps the package list. For a single-package
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
    assert_eq!(targets[0]["kind"], "cpp_executable");
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
    assert_eq!(pkg["targets"][0]["kind"], "cpp_executable");
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
    // is only meaningful after stripping the name. We compare
    // structure instead by checking the explicit-bin manifest
    // declares `cpp_executable` like the default.
    let _ = default;
    let body = String::from_utf8(explicit).unwrap();
    assert!(body.contains(r#"type = "cpp_executable""#), "{body}");
    assert!(
        !body.contains(r#"type = "cpp_library""#),
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
    assert!(manifest.contains(r#"type = "cpp_library""#));
    assert!(manifest.contains(r#"sources = ["src/greeter.cc"]"#));
    assert!(manifest.contains(r#"include_dirs = ["include"]"#));

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
    assert!(manifest.contains(r#"type = "cpp_library""#));
    assert!(manifest.contains(r#"sources = ["src/lib-pkg.cc"]"#));
    assert!(manifest.contains(r#"include_dirs = ["include"]"#));

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
    assert!(manifest.contains(r#"type = "cpp_executable""#));
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
fn new_lib_metadata_view_reports_cpp_library_target() {
    let parent = TempDir::new().expect("tempdir should be created");
    let target = parent.path().join("metalib");
    cabin()
        .current_dir(parent.path())
        .args(["new", "metalib", "--lib"])
        .assert()
        .success();

    let value = run_metadata(&target.join("cabin.toml"));
    let pkg = package_in(&value, "metalib");
    assert_eq!(pkg["targets"][0]["kind"], "cpp_library");
}

#[test]
fn new_lib_builds_successfully() {
    if !build_tools_available() {
        skip(
            "new_lib_builds_successfully",
            "ninja or a C++ compiler is not available",
        );
        return;
    }
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
        .join("libbuildlib.a");
    assert!(
        lib_path.is_file(),
        "expected library archive at {:?}",
        lib_path
    );
}

#[test]
fn new_bin_builds_successfully() {
    if !build_tools_available() {
        skip(
            "new_bin_builds_successfully",
            "ninja or a C++ compiler is not available",
        );
        return;
    }
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
        .join("buildbin");
    assert!(bin_path.is_file(), "expected executable at {:?}", bin_path);
}

#[test]
fn new_bin_runs_and_prints_greeting() {
    if !build_tools_available() {
        skip(
            "new_bin_runs_and_prints_greeting",
            "ninja or a C++ compiler is not available",
        );
        return;
    }
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

mod verbosity {
    use super::*;

    /// Set up a fresh single-package package at `dir` so commands
    /// that load the workspace (`cabin clean`, `cabin metadata`)
    /// have a manifest to read.
    fn populate_project(dir: &Path) {
        assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
            .write_str(VALID_MANIFEST)
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("src/main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
    }

    /// Read both stdout and stderr from a `cabin <args>` run that
    /// is expected to succeed.
    fn run_capture(cwd: &Path, args: &[&str]) -> (String, String) {
        let output = cabin()
            .current_dir(cwd)
            .args(args)
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");
        let stderr = String::from_utf8(output.stderr).expect("stderr utf-8");
        (stdout, stderr)
    }

    #[test]
    fn quiet_flag_suppresses_init_status_message() {
        let dir = TempDir::new().unwrap();
        let (stdout, _) = run_capture(dir.path(), &["init", "--name", "hello", "--quiet"]);
        assert!(
            !stdout.contains("Created binary"),
            "quiet must suppress init status:\n{stdout}"
        );
        // Quiet must not interfere with the actual scaffolding
        // side-effect.
        assert!(dir.path().join("cabin.toml").exists());
    }

    #[test]
    fn quiet_flag_suppresses_clean_status_message() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());
        let (stdout, _) = run_capture(dir.path(), &["clean", "--quiet"]);
        assert!(
            stdout.is_empty(),
            "quiet must suppress clean status:\n{stdout}"
        );
    }

    #[test]
    fn quiet_short_flag_works_the_same() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());
        let (stdout, _) = run_capture(dir.path(), &["clean", "-q"]);
        assert!(stdout.is_empty(), "stdout should be empty: {stdout}");
    }

    #[test]
    fn quiet_does_not_suppress_errors() {
        let dir = TempDir::new().unwrap();
        // Missing manifest; clean will fail with a typed
        // diagnostic that must remain visible under --quiet.
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["clean", "--quiet"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("error[cabin::workspace::manifest_not_found]")
                || stderr.contains("could not find a Cabin workspace"),
            "errors must survive --quiet:\n{stderr}"
        );
    }

    #[test]
    fn verbose_flag_adds_build_dir_and_profile_lines_to_build() {
        if !ninja_available() || !cxx_compiler_available() {
            skip(
                "verbose_flag_adds_build_dir_and_profile_lines_to_build",
                "requires ninja + a C++ compiler",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let build_dir = dir.path().join("build");
        let (stdout, _) = run_capture(
            dir.path(),
            &[
                "build",
                "--verbose",
                "--build-dir",
                build_dir.to_str().unwrap(),
            ],
        );
        assert!(
            stdout.contains("cabin: profile = "),
            "verbose must add a profile line:\n{stdout}"
        );
        assert!(
            stdout.contains("cabin: build dir = "),
            "verbose must add a build dir line:\n{stdout}"
        );
        assert!(
            stdout.contains("cabin: c++ compiler = "),
            "verbose must add a c++ compiler line:\n{stdout}"
        );
    }

    #[test]
    fn very_verbose_flag_adds_archiver_line_to_build() {
        if !ninja_available() || !cxx_compiler_available() {
            skip(
                "very_verbose_flag_adds_archiver_line_to_build",
                "requires ninja + a C++ compiler",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let build_dir = dir.path().join("build");
        let (stdout, _) = run_capture(
            dir.path(),
            &["build", "-vv", "--build-dir", build_dir.to_str().unwrap()],
        );
        assert!(
            stdout.contains("cabin: archiver = "),
            "very verbose must add an archiver line:\n{stdout}"
        );
    }

    #[test]
    fn repeated_short_verbose_flags_clamp_to_very_verbose() {
        if !ninja_available() || !cxx_compiler_available() {
            skip(
                "repeated_short_verbose_flags_clamp_to_very_verbose",
                "requires ninja + a C++ compiler",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let build_dir = dir.path().join("build");
        // Five `-v`s saturate at VeryVerbose without erroring.
        let (stdout, _) = run_capture(
            dir.path(),
            &[
                "build",
                "-vvvvv",
                "--build-dir",
                build_dir.to_str().unwrap(),
            ],
        );
        assert!(
            stdout.contains("cabin: archiver = "),
            "five -v should clamp to very verbose:\n{stdout}"
        );
    }

    #[test]
    fn separate_verbose_flags_also_count() {
        if !ninja_available() || !cxx_compiler_available() {
            skip(
                "separate_verbose_flags_also_count",
                "requires ninja + a C++ compiler",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let build_dir = dir.path().join("build");
        let (stdout, _) = run_capture(
            dir.path(),
            &[
                "build",
                "-v",
                "-v",
                "--build-dir",
                build_dir.to_str().unwrap(),
            ],
        );
        assert!(
            stdout.contains("cabin: archiver = "),
            "`-v -v` should reach very verbose:\n{stdout}"
        );
    }

    #[test]
    fn quiet_with_verbose_is_rejected_by_clap() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["clean", "--quiet", "--verbose"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("cannot be used with"),
            "clap must report the conflict:\n{stderr}"
        );
    }

    #[test]
    fn resolve_json_stdout_stays_clean_under_verbose() {
        // `cabin resolve --format json` writes a JSON document
        // to stdout and routes its lockfile-status line to
        // stderr via `Reporter::aux_status`.  Verbose flags
        // must not reverse that split.
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());
        let manifest = dir.path().join("cabin.toml");
        let (stdout, _stderr) = run_capture(
            dir.path(),
            &[
                "resolve",
                "--manifest-path",
                manifest.to_str().unwrap(),
                "--format",
                "json",
                "--verbose",
            ],
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("resolve --format json stdout must parse");
        assert!(parsed.is_object(), "resolve returned non-object: {stdout}");
    }

    #[test]
    fn metadata_stdout_stays_clean_under_verbose() {
        // `cabin metadata` is a JSON-emitting command.  The
        // verbose flag must never add human-readable lines to
        // stdout; otherwise consumers piping the output into
        // `jq` break.
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());
        let (stdout, _stderr) = run_capture(
            dir.path(),
            &[
                "metadata",
                "--manifest-path",
                dir.path().join("cabin.toml").to_str().unwrap(),
                "--verbose",
            ],
        );
        // The stdout must parse as a single JSON document; if
        // verbose injected text, this would fail.
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("metadata stdout must be valid JSON under -v");
        assert!(parsed.is_object(), "metadata returned non-object: {stdout}");
    }

    #[test]
    fn env_var_verbose_takes_effect_when_cli_silent() {
        if !ninja_available() || !cxx_compiler_available() {
            skip(
                "env_var_verbose_takes_effect_when_cli_silent",
                "requires ninja + a C++ compiler",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let build_dir = dir.path().join("build");
        let output = cabin()
            .current_dir(dir.path())
            .env("CABIN_TERM_VERBOSE", "1")
            .args(["build", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains("cabin: profile = "),
            "CABIN_TERM_VERBOSE=1 must enable verbose:\n{stdout}"
        );
    }

    #[test]
    fn invalid_env_value_is_rejected_with_actionable_error() {
        let dir = TempDir::new().unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .env("CABIN_TERM_VERBOSE", "loud")
            .args(["init", "--name", "hello"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("invalid CABIN_TERM_VERBOSE value 'loud'"),
            "env error must name the variable and value:\n{stderr}"
        );
    }

    #[test]
    fn help_lists_global_verbose_and_quiet_flags() {
        let output = cabin()
            .args(["--help"])
            .assert()
            .success()
            .get_output()
            .clone();
        let body = String::from_utf8(output.stdout).unwrap();
        assert!(
            body.contains("-v") && body.contains("--verbose"),
            "top-level help missing -v / --verbose:\n{body}"
        );
        assert!(
            body.contains("-q") && body.contains("--quiet"),
            "top-level help missing -q / --quiet:\n{body}"
        );
    }

    #[test]
    fn subcommand_help_inherits_verbose_and_quiet_flags() {
        for sub in ["build", "clean", "init", "new"] {
            let output = cabin()
                .args([sub, "--help"])
                .assert()
                .success()
                .get_output()
                .clone();
            let body = String::from_utf8(output.stdout).unwrap();
            assert!(
                body.contains("--verbose"),
                "{sub} --help missing --verbose:\n{body}"
            );
            assert!(
                body.contains("--quiet"),
                "{sub} --help missing --quiet:\n{body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// cabin clean
// ---------------------------------------------------------------------------

mod clean_cmd {
    use super::*;

    /// Set up a single-package package at `dir` with a fully
    /// populated build directory layout that mirrors what `cabin
    /// build` would produce (`<build>/<profile>/{build.ninja,
    /// packages/<pkg>/..., cargo/<pkg>/...}`).
    fn populate_project(dir: &Path) {
        assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
            .write_str(VALID_MANIFEST)
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("src/main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("cabin.lock"))
            .write_str("# lock\n")
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("vendor/.keep"))
            .write_str("")
            .unwrap();
        assert_fs::fixture::ChildPath::new(
            dir.join("build")
                .join("dev")
                .join("packages")
                .join("hello")
                .join("hello"),
        )
        .write_str("obj")
        .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("build").join("dev").join("build.ninja"))
            .write_str("ninja")
            .unwrap();
        assert_fs::fixture::ChildPath::new(
            dir.join("build")
                .join("release")
                .join("packages")
                .join("hello")
                .join("hello"),
        )
        .write_str("obj")
        .unwrap();
    }

    #[test]
    fn clean_removes_build_dir() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());

        cabin()
            .current_dir(dir.path())
            .args(["clean"])
            .assert()
            .success()
            .stdout(predicate::str::contains("removed"));

        assert!(
            !dir.path().join("build").exists(),
            "build dir should be gone"
        );
        assert!(dir.path().join("cabin.toml").exists(), "manifest preserved");
        assert!(
            dir.path().join("src").join("main.cc").exists(),
            "src preserved"
        );
        assert!(dir.path().join("cabin.lock").exists(), "lockfile preserved");
        assert!(dir.path().join("vendor").exists(), "vendor preserved");
    }

    #[test]
    fn clean_succeeds_when_build_dir_missing() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

        cabin()
            .current_dir(dir.path())
            .args(["clean"])
            .assert()
            .success()
            .stdout(predicate::str::contains("does not exist"));
    }

    #[test]
    fn clean_dry_run_lists_paths_and_keeps_files() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());

        let output = cabin()
            .current_dir(dir.path())
            .args(["clean", "--dry-run"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("dry run"));
        assert!(stdout.contains("would remove"));
        assert!(stdout.contains("/build"));

        assert!(
            dir.path().join("build").exists(),
            "dry run must not delete files"
        );
    }

    #[test]
    fn clean_profile_narrows_to_one_profile_dir() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());

        cabin()
            .current_dir(dir.path())
            .args(["clean", "--profile", "release"])
            .assert()
            .success();

        assert!(
            !dir.path().join("build").join("release").exists(),
            "release tree should be gone"
        );
        assert!(
            dir.path().join("build").join("dev").exists(),
            "dev tree must remain"
        );
    }

    #[test]
    fn clean_release_alias_matches_profile_release() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());

        cabin()
            .current_dir(dir.path())
            .args(["clean", "--release"])
            .assert()
            .success();
        assert!(!dir.path().join("build").join("release").exists());
        assert!(dir.path().join("build").join("dev").exists());
    }

    #[test]
    fn clean_package_targets_per_package_paths_across_profiles() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());
        // Add a sibling package output that must survive a -p
        // selection that names only `hello`.
        assert_fs::fixture::ChildPath::new(
            dir.path()
                .join("build")
                .join("dev")
                .join("packages")
                .join("other")
                .join("libother.a"),
        )
        .write_str("obj")
        .unwrap();

        cabin()
            .current_dir(dir.path())
            .args(["clean", "-p", "hello"])
            .assert()
            .success();

        assert!(
            !dir.path()
                .join("build")
                .join("dev")
                .join("packages")
                .join("hello")
                .exists()
        );
        assert!(
            !dir.path()
                .join("build")
                .join("release")
                .join("packages")
                .join("hello")
                .exists()
        );
        assert!(
            dir.path()
                .join("build")
                .join("dev")
                .join("packages")
                .join("other")
                .exists(),
            "non-selected package output must remain"
        );
        assert!(
            dir.path()
                .join("build")
                .join("dev")
                .join("build.ninja")
                .exists(),
            "profile-level files must remain"
        );
    }

    #[test]
    fn clean_profile_and_package_combine() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());

        cabin()
            .current_dir(dir.path())
            .args(["clean", "--profile", "dev", "-p", "hello"])
            .assert()
            .success();

        assert!(
            !dir.path()
                .join("build")
                .join("dev")
                .join("packages")
                .join("hello")
                .exists()
        );
        assert!(
            dir.path()
                .join("build")
                .join("release")
                .join("packages")
                .join("hello")
                .exists(),
            "release tree untouched when profile narrowed to dev"
        );
    }

    #[test]
    fn clean_respects_custom_build_dir() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let custom = dir.path().join("custom-build-dir");
        assert_fs::fixture::ChildPath::new(custom.join("dev").join("build.ninja"))
            .write_str("x")
            .unwrap();

        cabin()
            .current_dir(dir.path())
            .args(["clean", "--build-dir"])
            .arg(&custom)
            .assert()
            .success();

        assert!(!custom.exists(), "custom build dir should be removed");
        // Default `build/` was never created and must remain absent.
        assert!(!dir.path().join("build").exists());
    }

    #[test]
    fn clean_rejects_build_dir_that_contains_source_files() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

        cabin()
            .current_dir(dir.path())
            .args(["clean", "--build-dir", "src"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("source file"));

        assert!(
            dir.path().join("src").join("main.cc").exists(),
            "source file must not be removed"
        );
    }

    #[test]
    fn clean_rejects_workspace_root_as_build_dir() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

        cabin()
            .current_dir(dir.path())
            .args(["clean", "--build-dir"])
            .arg(dir.path())
            .assert()
            .failure()
            .stderr(predicate::str::contains("refusing to clean"));
        assert!(dir.path().join("cabin.toml").exists());
        assert!(dir.path().join("src").join("main.cc").exists());
    }

    /// Set up a two-member workspace at `dir` with build-tree
    /// fixtures for both members under `dev/` and `release/`.
    fn populate_workspace(dir: &Path) {
        assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
            .write_str("[workspace]\nmembers = [\"hello\", \"util\"]\n")
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("hello").join("cabin.toml"))
            .write_str(
                "[package]\n\
             name = \"hello\"\n\
             version = \"0.1.0\"\n\
             \n\
             [target.hello]\n\
             type = \"cpp_executable\"\n\
             sources = [\"src/main.cc\"]\n",
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("hello").join("src").join("main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("util").join("cabin.toml"))
            .write_str(
                "[package]\n\
             name = \"util\"\n\
             version = \"0.1.0\"\n\
             \n\
             [target.util]\n\
             type = \"cpp_library\"\n\
             sources = [\"src/util.cc\"]\n",
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("util").join("src").join("util.cc"))
            .write_str("void util(){}\n")
            .unwrap();
        for profile in ["dev", "release"] {
            for pkg in ["hello", "util"] {
                assert_fs::fixture::ChildPath::new(
                    dir.join("build")
                        .join(profile)
                        .join("packages")
                        .join(pkg)
                        .join("artifact"),
                )
                .write_str("x")
                .unwrap();
            }
        }
    }

    #[test]
    fn clean_workspace_with_exclude_skips_excluded_package() {
        let dir = TempDir::new().unwrap();
        populate_workspace(dir.path());

        cabin()
            .current_dir(dir.path())
            .args(["clean", "--workspace", "--exclude", "hello"])
            .assert()
            .success();

        for profile in ["dev", "release"] {
            assert!(
                dir.path()
                    .join("build")
                    .join(profile)
                    .join("packages")
                    .join("hello")
                    .exists(),
                "excluded `hello` output must remain ({profile})"
            );
            assert!(
                !dir.path()
                    .join("build")
                    .join(profile)
                    .join("packages")
                    .join("util")
                    .exists(),
                "non-excluded `util` output should be removed ({profile})"
            );
        }
    }

    #[test]
    fn clean_rejects_root_path() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

        cabin()
            .current_dir(dir.path())
            .args(["clean", "--build-dir", "/"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("root path"));
    }

    #[cfg(unix)]
    #[test]
    fn clean_rejects_symlink_build_dir() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let real = dir.path().join("real-build");
        let link = dir.path().join("build");
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_fs::fixture::ChildPath::new(real.join("dev").join("build.ninja"))
            .write_str("x")
            .unwrap();

        cabin()
            .current_dir(dir.path())
            .args(["clean"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("symlink"));

        assert!(real.exists(), "real build dir untouched");
        assert!(link.exists(), "symlink itself untouched");
    }

    #[test]
    fn clean_dry_run_output_is_sorted_and_deterministic() {
        let dir = TempDir::new().unwrap();
        populate_project(dir.path());
        assert_fs::fixture::ChildPath::new(
            dir.path()
                .join("build")
                .join("dev")
                .join("packages")
                .join("zeta")
                .join("libzeta.a"),
        )
        .write_str("x")
        .unwrap();
        assert_fs::fixture::ChildPath::new(
            dir.path()
                .join("build")
                .join("dev")
                .join("packages")
                .join("alpha")
                .join("libalpha.a"),
        )
        .write_str("x")
        .unwrap();

        let stdout = capture_dry_run(dir.path(), &["clean", "--dry-run"]);
        let stdout_again = capture_dry_run(dir.path(), &["clean", "--dry-run"]);
        assert_eq!(stdout, stdout_again, "dry-run output must be deterministic");
    }

    #[test]
    fn clean_help_describes_dry_run_and_profile() {
        let stdout = cabin()
            .args(["clean", "--help"])
            .assert()
            .success()
            .get_output()
            .clone();
        let body = String::from_utf8(stdout.stdout).unwrap();
        for needle in ["--dry-run", "--profile", "--build-dir", "--package"] {
            assert!(
                body.contains(needle),
                "clean --help missing `{needle}`:\n{body}"
            );
        }
    }

    fn capture_dry_run(cwd: &Path, args: &[&str]) -> String {
        let output = cabin()
            .current_dir(cwd)
            .args(args)
            .assert()
            .success()
            .get_output()
            .clone();
        String::from_utf8(output.stdout).unwrap()
    }
}

// ---------------------------------------------------------------------------
// single-package builds
// ---------------------------------------------------------------------------

/// Set up a hello-world C++ package in `dir` and run a default build.
/// Returns `dir/build/packages/hello/` for output assertions.
fn build_simple_executable(dir: &Path, extra_args: &[&str]) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str(VALID_MANIFEST)
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/main.cc"))
        .write_str(HELLO_MAIN_CC)
        .unwrap();

    let build_dir = dir.join("build");
    let mut cmd = cabin();
    cmd.current_dir(dir).arg("build");
    cmd.args(extra_args);
    cmd.arg("--build-dir").arg(&build_dir);
    cmd.assert().success();
}

#[test]
fn build_writes_ninja_and_compile_commands_for_simple_executable() {
    if !build_tools_available() {
        skip(
            "build_writes_ninja_and_compile_commands_for_simple_executable",
            "ninja or a C++ compiler is not available",
        );
        return;
    }

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
    assert!(pkg_dir.join("hello").is_file(), "executable should exist");

    let output = std::process::Command::new(pkg_dir.join("hello"))
        .output()
        .expect("running hello should succeed");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Hello from Cabin"), "got: {stdout}");
}

#[test]
fn compile_commands_json_contains_expected_fields() {
    if !build_tools_available() {
        skip(
            "compile_commands_json_contains_expected_fields",
            "ninja or a C++ compiler is not available",
        );
        return;
    }

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
    assert!(entry["output"].as_str().unwrap().ends_with("src/main.cc.o"));
    let command = entry["command"].as_str().unwrap();
    assert!(command.contains("-std=c++17"));
    assert!(command.contains("src/main.cc"));
}

#[test]
fn build_links_executable_against_same_package_library() {
    if !build_tools_available() {
        skip(
            "build_links_executable_against_same_package_library",
            "ninja or a C++ compiler is not available",
        );
        return;
    }

    let dir = TempDir::new().unwrap();
    let manifest = r#"[package]
name = "hello"
version = "0.1.0"

[target.greet]
type = "cpp_library"
sources = ["src/greet.cc"]
include_dirs = ["include"]

[target.hello]
type = "cpp_executable"
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
    assert!(pkg_dir.join("libgreet.a").is_file());
    assert!(pkg_dir.join("hello").is_file());

    let output = std::process::Command::new(pkg_dir.join("hello"))
        .output()
        .expect("running hello should succeed");
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello from greet"));
}

#[test]
fn release_flag_changes_compile_commands() {
    if !build_tools_available() {
        skip(
            "release_flag_changes_compile_commands",
            "ninja or a C++ compiler is not available",
        );
        return;
    }

    let dir = TempDir::new().unwrap();
    build_simple_executable(dir.path(), &["--release"]);

    let release_dir = dir.path().join("build").join("release");
    let body = fs::read_to_string(release_dir.join("compile_commands.json"))
        .expect("compile_commands.json should be readable");
    assert!(body.contains("-O3"), "expected -O3 in: {body}");
    assert!(body.contains("-DNDEBUG"), "expected -DNDEBUG in: {body}");
    assert!(!body.contains("-O0"), "did not expect -O0 in: {body}");

    let ninja_body = fs::read_to_string(release_dir.join("build.ninja")).unwrap();
    assert!(ninja_body.contains("-O3"));
    assert!(ninja_body.contains("-DNDEBUG"));
}

#[test]
fn cabin_build_rejects_target_flag_as_unknown_argument() {
    // `--target` is reserved for a future platform/toolchain
    // target selector. Cabin no longer accepts it as a
    // manifest-target selector on `cabin build`, so clap must
    // reject the flag outright. Pinning the rejection here keeps
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

[target.a]
type = "cpp_library"
sources = ["a.cc"]
deps = ["b"]

[target.b]
type = "cpp_library"
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

[target.greet]
type = "cpp_library"
sources = ["src/greet.cc"]
include_dirs = ["include"]
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

[dependencies]
greet = { path = "../greet" }

[target.app]
type = "cpp_executable"
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
    if !build_tools_available() {
        skip(
            "build_with_local_path_dependency_builds_executable",
            "ninja or a C++ compiler is not available",
        );
        return;
    }
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
    assert!(greet_pkg_dir.join("libgreet.a").is_file());
    assert!(app_pkg_dir.join("app").is_file(), "app executable missing");

    let output = std::process::Command::new(app_pkg_dir.join("app"))
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
    if !build_tools_available() {
        skip(
            "compile_commands_includes_dependency_sources",
            "ninja or a C++ compiler is not available",
        );
        return;
    }
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
    assert!(files.iter().any(|f| f.ends_with("greet/src/greet.cc")));
    assert!(files.iter().any(|f| f.ends_with("app/src/main.cc")));
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

fn write_app_with_versioned_dep(dir: &Path, dep_line: &str) {
    let manifest = format!(
        r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
{dep_line}
"#
    );
    assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
        .write_str(&manifest)
        .unwrap();
}

#[test]
fn resolve_succeeds_for_direct_dependency() {
    let dir = TempDir::new().unwrap();
    write_app_with_versioned_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
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
    write_app_with_versioned_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
    dir.child("index/fmt.json").write_str(FMT_INDEX).unwrap();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("output is JSON");
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
    write_app_with_versioned_dep(dir.path(), r#"spdlog = "^1.13.0""#);
    dir.child("index/fmt.json").write_str(FMT_INDEX).unwrap();
    dir.child("index/spdlog.json")
        .write_str(SPDLOG_INDEX)
        .unwrap();

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
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
    write_app_with_versioned_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);
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
    write_app_with_versioned_dep(dir.path(), r#"fmt = "^10""#);

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
    write_app_with_versioned_dep(dir.path(), r#"missing-pkg = "^1""#);
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
    write_app_with_versioned_dep(dir.path(), r#"fmt = "^10""#);
    dir.child("app/src/main.cc")
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    // Add a target so the build would otherwise have something to do.
    let manifest = r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = "^10"

[target.app]
type = "cpp_executable"
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
    write_app_with_versioned_dep(dir.path(), r#"fmt = ">=10.0.0 <11.0.0""#);

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
    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
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
    // lockfile pins 10.1.0. Then add 10.2.0 to the index and resolve
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
/// involved, and the conflicting version requirement —
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

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .args(["--format", "json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
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

mod artifact_fetch {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use sha2::Digest;
    use std::fs::File;
    use std::io::Write;

    fn manifest_for(name: &str, version: &str, deps: &[(&str, &str)]) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\n"
        ));
        if !deps.is_empty() {
            out.push_str("\n[dependencies]\n");
            for (name, req) in deps {
                out.push_str(&format!("{name} = \"{req}\"\n"));
            }
        }
        out
    }

    /// Build a `.tar.gz` containing the given file entries (relative
    /// path -> body). Returns the archive path and its `sha256` hex.
    fn make_archive(path: &Path, entries: &[(&str, &str)]) -> String {
        if let Some(parent) = path.parent() {
            assert_fs::fixture::ChildPath::new(parent)
                .create_dir_all()
                .unwrap();
        }
        let f = File::create(path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
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
        sha256_hex(path)
    }

    /// Same as [`make_archive`] but the caller chooses the entry type
    /// and writes the path bytes directly so we can construct unsafe
    /// archive entries that the tar crate's safe API would refuse.
    fn make_archive_with_raw_name(
        path: &Path,
        raw_name: &str,
        entry_type: tar::EntryType,
        body: &[u8],
    ) -> String {
        if let Some(parent) = path.parent() {
            assert_fs::fixture::ChildPath::new(parent)
                .create_dir_all()
                .unwrap();
        }
        let f = File::create(path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
        let mut builder = tar::Builder::new(enc);
        let mut header = tar::Header::new_old();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(entry_type);
        {
            let bytes = raw_name.as_bytes();
            let old = header.as_old_mut();
            for b in &mut old.name[..] {
                *b = 0;
            }
            let n = bytes.len().min(old.name.len());
            old.name[..n].copy_from_slice(&bytes[..n]);
        }
        header.set_cksum();
        builder.append(&header, body).unwrap();
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap().flush().unwrap();
        sha256_hex(path)
    }

    fn sha256_hex(path: &Path) -> String {
        let bytes = fs::read(path).unwrap();
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        format!("{:x}", hasher.finalize())
    }

    fn fmt_archive_entries() -> Vec<(&'static str, &'static str)> {
        vec![
            ("cabin.toml", FMT_PKG_MANIFEST),
            ("include/fmt.h", FMT_HEADER),
            ("src/fmt.cc", FMT_SRC),
        ]
    }

    const FMT_PKG_MANIFEST: &str = r#"[package]
name = "fmt"
version = "10.2.1"

[target.fmt]
type = "cpp_library"
sources = ["src/fmt.cc"]
include_dirs = ["include"]
"#;

    const FMT_HEADER: &str = "#pragma once\nvoid say_hello();\n";

    const FMT_SRC: &str = "#include <iostream>\n#include \"fmt.h\"\nvoid say_hello() { std::cout << \"hello from fmt\\n\"; }\n";

    const APP_MAIN: &str = "#include \"fmt.h\"\nint main() { say_hello(); return 0; }\n";

    /// Write an `app/` package whose root manifest depends on
    /// `fmt = ">=10 <11"` plus a `[target.app]` linking against `fmt`.
    fn write_app_using_fmt(dir: &Path) {
        let manifest = r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
deps = ["fmt"]
"#;
        assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
            .write_str(manifest)
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("app/src/main.cc"))
            .write_str(APP_MAIN)
            .unwrap();
    }

    fn write_index_entry(
        index_dir: &Path,
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

    fn write_index_entry_no_source(index_dir: &Path, package: &str, version: &str, checksum: &str) {
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

    #[test]
    fn fetch_extracts_registry_package_into_cache() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(&archive, &fmt_archive_entries());
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );

        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .success();

        // Lockfile written next to root manifest.
        let lock_path = dir.path().join("app/cabin.lock");
        assert!(lock_path.is_file(), "cabin.lock should exist");
        let lock_body = fs::read_to_string(&lock_path).unwrap();
        assert!(lock_body.contains(r#"name = "fmt""#));
        assert!(lock_body.contains(&format!("checksum = \"sha256:{hex}\"")));

        // Archive present in the checksum-addressed cache.
        let archive_in_cache = cache.join("archives/sha256").join(format!("{hex}.tar.gz"));
        assert!(archive_in_cache.is_file(), "archive should be cached");
        // Source extracted with cabin.toml at root.
        let source_in_cache = cache.join("sources/sha256").join(&hex);
        assert!(source_in_cache.join("cabin.toml").is_file());
    }

    #[test]
    fn fetch_emits_json_when_requested() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(&archive, &fmt_archive_entries());
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );
        let cache = dir.path().join("cache");
        let output = cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
        let pkgs = value["packages"].as_array().unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0]["name"], "fmt");
        assert_eq!(pkgs[0]["version"], "10.2.1");
        assert_eq!(pkgs[0]["checksum"], format!("sha256:{hex}"));
    }

    #[test]
    fn build_links_against_registry_package() {
        if !build_tools_available() {
            skip(
                "build_links_against_registry_package",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(&archive, &fmt_archive_entries());
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );

        let build_dir = dir.path().join("build");
        let cache = dir.path().join("cache");
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .arg("--build-dir")
            .arg(&build_dir)
            .assert()
            .success();

        assert!(build_dir.join("dev").join("build.ninja").is_file());
        assert!(
            build_dir
                .join("dev")
                .join("compile_commands.json")
                .is_file()
        );
        let exe = build_dir.join("dev/packages/app/app");
        assert!(exe.is_file(), "executable should exist at {exe:?}");
        let output = std::process::Command::new(&exe).output().unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello from fmt"));
    }

    #[test]
    fn build_handles_transitive_registry_dependency() {
        if !build_tools_available() {
            skip(
                "build_handles_transitive_registry_dependency",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();

        // Root depends only on spdlog; spdlog depends on fmt.
        let app_manifest = r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
spdlog = ">=1.0.0 <2.0.0"

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
deps = ["spdlog"]
"#;
        let app_main = "#include \"spdlog.h\"\nint main() { log_hello(); return 0; }\n";
        dir.child("app/cabin.toml").write_str(app_manifest).unwrap();
        dir.child("app/src/main.cc").write_str(app_main).unwrap();

        // fmt archive (cpp_library).
        let fmt_archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let fmt_hex = make_archive(&fmt_archive, &fmt_archive_entries());

        // spdlog archive: cpp_library that depends on fmt.
        let spdlog_manifest = r#"[package]
name = "spdlog"
version = "1.13.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[target.spdlog]
type = "cpp_library"
sources = ["src/spdlog.cc"]
include_dirs = ["include"]
deps = ["fmt"]
"#;
        let spdlog_header = "#pragma once\nvoid log_hello();\n";
        let spdlog_src =
            "#include \"spdlog.h\"\n#include \"fmt.h\"\nvoid log_hello() { say_hello(); }\n";
        let spdlog_archive = dir.path().join("artifacts/spdlog-1.13.0.tar.gz");
        let spdlog_hex = make_archive(
            &spdlog_archive,
            &[
                ("cabin.toml", spdlog_manifest),
                ("include/spdlog.h", spdlog_header),
                ("src/spdlog.cc", spdlog_src),
            ],
        );

        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &fmt_hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );
        write_index_entry(
            &dir.path().join("index"),
            "spdlog",
            "1.13.0",
            r#"{ "fmt": ">=10.0.0 <11.0.0" }"#,
            &spdlog_hex,
            "../artifacts/spdlog-1.13.0.tar.gz",
        );

        let build_dir = dir.path().join("build");
        let cache = dir.path().join("cache");
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .arg("--build-dir")
            .arg(&build_dir)
            .assert()
            .success();

        // Both packages should have been fetched and built.
        assert!(
            cache
                .join("sources/sha256")
                .join(&fmt_hex)
                .join("cabin.toml")
                .is_file()
        );
        assert!(
            cache
                .join("sources/sha256")
                .join(&spdlog_hex)
                .join("cabin.toml")
                .is_file()
        );
        assert!(build_dir.join("dev/packages/fmt/libfmt.a").is_file());
        assert!(build_dir.join("dev/packages/spdlog/libspdlog.a").is_file());
        assert!(build_dir.join("dev/packages/app/app").is_file());
    }

    #[test]
    fn fetch_fails_on_checksum_mismatch() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        make_archive(&archive, &fmt_archive_entries());
        // Index advertises a checksum that doesn't match the archive's
        // actual bytes.
        let bogus_hex = "0".repeat(64);
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &bogus_hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );

        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .failure()
            .stderr(predicate::str::contains("checksum mismatch"));
    }

    #[test]
    fn fetch_rejects_unsafe_archive() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex =
            make_archive_with_raw_name(&archive, "../escape.txt", tar::EntryType::Regular, b"evil");
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );
        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .failure()
            .stderr(predicate::str::contains("unsafe archive entry"));
        // Nothing escaped the cache.
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn fetch_fails_when_index_has_no_source() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(&archive, &fmt_archive_entries());
        write_index_entry_no_source(&dir.path().join("index"), "fmt", "10.2.1", &hex);

        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .failure()
            .stderr(predicate::str::contains("no source artifact"));
    }

    #[test]
    fn frozen_uses_cache_after_initial_fetch() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(&archive, &fmt_archive_entries());
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );

        let cache = dir.path().join("cache");
        // Populate cache normally.
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .success();
        // Now move the source archive away and re-run with --frozen;
        // cache hit should let it succeed.
        fs::remove_file(&archive).unwrap();
        cabin()
            .args(["fetch", "--frozen", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .success();
    }

    #[test]
    fn frozen_fails_on_cache_miss() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(&archive, &fmt_archive_entries());
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );
        // Pre-populate a lockfile so --frozen can run resolution.
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .assert()
            .success();

        let empty_cache = dir.path().join("empty-cache");
        cabin()
            .args(["fetch", "--frozen", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&empty_cache)
            .assert()
            .failure()
            .stderr(predicate::str::contains("--frozen"))
            .stderr(predicate::str::contains("not cached"));
    }

    #[test]
    fn frozen_does_not_write_lockfile_or_cache() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(&archive, &fmt_archive_entries());
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );

        // No lockfile, no cache pre-populated. --frozen must refuse.
        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--frozen", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .failure();
        // Lockfile must not have been created by the failed run.
        assert!(!dir.path().join("app/cabin.lock").exists());
        // Cache must not have been populated by the failed run.
        let archive_in_cache = cache.join("archives/sha256").join(format!("{hex}.tar.gz"));
        assert!(!archive_in_cache.exists());
    }

    #[test]
    fn fetch_fails_when_archive_manifest_disagrees() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        // Archive declares fmt 10.1.0 but the index promises 10.2.1.
        let mut entries = fmt_archive_entries();
        entries[0].1 = r#"[package]
name = "fmt"
version = "10.1.0"
"#;
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(&archive, &entries);
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );
        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .failure()
            .stderr(predicate::str::contains("contains package"));
    }

    #[test]
    fn fetch_with_no_versioned_deps_succeeds() {
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_for("solo", "0.1.0", &[]))
            .unwrap();
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .success()
            .stdout(predicate::str::contains("(no registry dependencies"));
    }

    #[test]
    fn build_uses_separate_cache_dir_when_specified() {
        let dir = TempDir::new().unwrap();
        write_app_using_fmt(dir.path());
        let archive = dir.path().join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(&archive, &fmt_archive_entries());
        write_index_entry(
            &dir.path().join("index"),
            "fmt",
            "10.2.1",
            "{}",
            &hex,
            "../artifacts/fmt-10.2.1.tar.gz",
        );

        let cache = dir.path().join("alt-cache");
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .success();
        assert!(
            cache
                .join("archives/sha256")
                .join(format!("{hex}.tar.gz"))
                .is_file()
        );
        // Default cache must NOT have been populated.
        assert!(!dir.path().join("app/.cabin/cache").exists());
    }
}

// ---------------------------------------------------------------------------
// cabin package + cabin publish --dry-run
// ---------------------------------------------------------------------------

mod package_archive {
    use super::*;
    use flate2::read::GzDecoder;
    use std::collections::BTreeSet;

    fn write_simple_package(root: &Path) {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(
                r#"[package]
name = "fmt"
version = "10.2.1"

[target.fmt]
type = "cpp_library"
sources = ["src/fmt.cc"]
include_dirs = ["include"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("include/example.h"))
            .write_str("#pragma once\nvoid say_hello();\n")
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("src/fmt.cc"))
            .write_str("#include \"example.h\"\nvoid say_hello() {}\n")
            .unwrap();
    }

    fn read_archive_entries(archive: &Path) -> BTreeSet<String> {
        let f = fs::File::open(archive).unwrap();
        let dec = GzDecoder::new(f);
        let mut tar = tar::Archive::new(dec);
        let mut out = BTreeSet::new();
        for entry in tar.entries().unwrap() {
            let entry = entry.unwrap();
            out.insert(entry.path().unwrap().to_string_lossy().into_owned());
        }
        out
    }

    #[test]
    fn package_creates_archive_and_metadata() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        let dist = dir.path().join("dist");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .success();

        let archive = dist.join("fmt-10.2.1.tar.gz");
        let metadata = dist.join("fmt-10.2.1.json");
        assert!(archive.is_file(), "archive missing: {archive:?}");
        assert!(metadata.is_file(), "metadata missing: {metadata:?}");

        let body = fs::read_to_string(&metadata).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["name"], "fmt");
        assert_eq!(value["version"], "10.2.1");
        assert_eq!(value["yanked"], false);
        assert!(value["checksum"].as_str().unwrap().starts_with("sha256:"));
        assert_eq!(value["source"]["type"], "archive");
        assert_eq!(value["source"]["format"], "tar.gz");
        assert!(
            value["source"]["path"]
                .as_str()
                .unwrap()
                .ends_with("fmt-10.2.1.tar.gz")
        );
    }

    #[test]
    fn package_metadata_preserves_manifest_compiler_cache_settings() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        let manifest_path = dir.path().join("cabin.toml");
        let mut manifest = fs::read_to_string(&manifest_path).unwrap();
        manifest.push_str(
            r#"
[profile.cache]
compiler-wrapper = "ccache"
"#,
        );
        assert_fs::fixture::ChildPath::new(&manifest_path)
            .write_str(&manifest)
            .unwrap();
        let dist = dir.path().join("dist");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(&manifest_path)
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .success();

        let body = fs::read_to_string(dist.join("fmt-10.2.1.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value["compiler_wrapper"]["general"],
            serde_json::json!({"kind": "use", "wrapper": "ccache"})
        );
    }

    #[test]
    fn package_json_format_emits_machine_readable_summary() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        let dist = dir.path().join("dist");
        let output = cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
        assert_eq!(value["name"], "fmt");
        assert_eq!(value["version"], "10.2.1");
        assert!(
            value["archive_path"]
                .as_str()
                .unwrap()
                .ends_with("fmt-10.2.1.tar.gz")
        );
        assert!(
            value["metadata_path"]
                .as_str()
                .unwrap()
                .ends_with("fmt-10.2.1.json")
        );
        assert!(value["checksum"].as_str().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn package_excludes_generated_and_vcs_files() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        // Files that must NOT appear in the archive.
        dir.child(".git/config").write_str("leak-this").unwrap();
        dir.child("build/build.ninja")
            .write_str("leak-this")
            .unwrap();
        dir.child("dist/old.tar.gz").write_str("leak-this").unwrap();
        dir.child(".cabin/cache/x").write_str("leak-this").unwrap();
        dir.child("node_modules/foo/x")
            .write_str("leak-this")
            .unwrap();
        dir.child("compile_commands.json")
            .write_str("leak-this")
            .unwrap();
        dir.child("cabin.lock").write_str("leak-this").unwrap();
        dir.child("build.ninja").write_str("leak-this").unwrap();

        let dist = dir.path().join("artifact-out");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .success();

        let entries = read_archive_entries(&dist.join("fmt-10.2.1.tar.gz"));
        assert!(entries.contains("cabin.toml"));
        assert!(entries.contains("src/fmt.cc"));
        assert!(entries.contains("include/example.h"));
        for forbidden in &[
            ".git/config",
            "build/build.ninja",
            "dist/old.tar.gz",
            ".cabin/cache/x",
            "node_modules/foo/x",
            "compile_commands.json",
            "cabin.lock",
            "build.ninja",
        ] {
            assert!(
                !entries.iter().any(|e| e == forbidden),
                "archive leaked {forbidden}: {entries:?}"
            );
        }
    }

    #[test]
    fn package_excludes_in_tree_custom_output_dir() {
        // A custom --output-dir living inside the package source
        // tree (and not on the hard-coded EXCLUDED_DIR_NAMES list)
        // must be skipped during staging so the next archive does
        // not embed last run's `.tar.gz` / `.json` and the
        // idempotent-rewrite check stays meaningful.
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        let out = dir.path().join("myoutput");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&out)
            .assert()
            .success();
        // Second run uses the same in-tree output dir. With the
        // bug present, the staging walker pulls last run's
        // archive into the new archive, the bytes drift, and the
        // idempotent rewrite refuses the differing existing
        // archive. With the fix, the second run is a no-op.
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&out)
            .assert()
            .success();
        let entries = read_archive_entries(&out.join("fmt-10.2.1.tar.gz"));
        assert!(entries.contains("cabin.toml"));
        assert!(entries.contains("src/fmt.cc"));
        assert!(
            !entries.iter().any(|e| e.starts_with("myoutput/")),
            "custom output dir leaked into archive: {entries:?}"
        );
    }

    #[test]
    fn package_rejects_output_dir_equal_to_package_root() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        let assertion = cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(dir.path())
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("equals the package source root"),
            "expected output_dir == package_root rejection, got: {stderr}"
        );
    }

    #[test]
    fn package_is_byte_deterministic_across_runs() {
        // Write the package and the two output directories in
        // *separate* trees: `pkg-a/` and `pkg-b/`. Both packages have
        // identical source content. Each run targets an output dir
        // outside the package root so neither archive picks up the
        // other run's `dist-*/` contents.
        let dir = TempDir::new().unwrap();
        let pkg_a = dir.path().join("pkg-a");
        let pkg_b = dir.path().join("pkg-b");
        write_simple_package(&pkg_a);
        write_simple_package(&pkg_b);

        let dist_a = dir.path().join("dist-a");
        let dist_b = dir.path().join("dist-b");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(pkg_a.join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist_a)
            .assert()
            .success();
        cabin()
            .args(["package", "--manifest-path"])
            .arg(pkg_b.join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist_b)
            .assert()
            .success();

        let bytes_a = fs::read(dist_a.join("fmt-10.2.1.tar.gz")).unwrap();
        let bytes_b = fs::read(dist_b.join("fmt-10.2.1.tar.gz")).unwrap();
        assert_eq!(bytes_a, bytes_b, "archives must be byte-identical");
    }

    #[test]
    fn publish_dry_run_creates_archive_and_reports_no_registry_modified() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        let dist = dir.path().join("dist");
        let output = cabin()
            .args(["publish", "--dry-run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("Publish dry-run"));
        assert!(stdout.contains("No registry was modified"));
        assert!(dist.join("fmt-10.2.1.tar.gz").is_file());
        assert!(dist.join("fmt-10.2.1.json").is_file());
    }

    #[test]
    fn publish_dry_run_json_format_is_valid_json() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        let dist = dir.path().join("dist");
        let output = cabin()
            .args(["publish", "--dry-run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
        assert_eq!(value["dry_run"], true);
        assert_eq!(value["registry_modified"], false);
        assert_eq!(value["name"], "fmt");
        assert_eq!(value["version"], "10.2.1");
    }

    #[test]
    fn publish_without_dry_run_fails_clearly() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        cabin()
            .args(["publish", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("--dry-run"));
    }

    #[test]
    fn package_with_path_dependency_fails_clearly() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
local = { path = "../local" }
"#,
            )
            .unwrap();
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("path dependencies"));
    }

    #[test]
    fn package_workspace_root_without_project_fails_clearly() {
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
        // `cabin package` against a workspace root must refuse
        // without a single `--package <name>` selection.
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("--package <name>"));
    }

    #[test]
    fn package_overwrite_with_identical_bytes_succeeds() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        let dist = dir.path().join("dist");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .success();
        // Second run with the same input must succeed silently.
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .success();
    }

    #[test]
    fn package_overwrite_with_different_bytes_fails() {
        let dir = TempDir::new().unwrap();
        write_simple_package(dir.path());
        let dist = dir.path().join("dist");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .success();
        // Stomp on the existing archive with junk; a re-run must fail.
        assert_fs::fixture::ChildPath::new(dist.join("fmt-10.2.1.tar.gz"))
            .write_binary(b"not the same bytes")
            .unwrap();
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .failure()
            .stderr(predicate::str::contains("already exists"));
    }
}

// ---------------------------------------------------------------------------
// cabin compgen + cabin mangen
// ---------------------------------------------------------------------------

mod distribution_artifacts {
    use super::*;

    #[test]
    fn compgen_bash_writes_completion_script_to_stdout() {
        let output = cabin()
            .args(["compgen", "bash"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.is_empty(), "bash completion should be non-empty");
        assert!(stdout.contains("cabin"), "bash completion mentions cabin");
        // Generated completions list every visible subcommand; pick a
        // few stable ones so the assertion is not brittle.
        assert!(stdout.contains("build"));
        assert!(stdout.contains("package"));
    }

    #[test]
    fn compgen_zsh_writes_completion_script_to_stdout() {
        let output = cabin()
            .args(["compgen", "zsh"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.is_empty(), "zsh completion should be non-empty");
        // Zsh completion either mentions the binary or the standard
        // `_cabin` function name.
        assert!(
            stdout.contains("cabin") || stdout.contains("_cabin"),
            "expected zsh script to reference cabin or _cabin"
        );
    }

    #[test]
    fn compgen_all_writes_files_for_every_supported_shell() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("completions");
        cabin()
            .args(["compgen", "--all", "--output-dir"])
            .arg(&out)
            .assert()
            .success();
        for filename in &[
            "cabin.bash",
            "_cabin",
            "cabin.fish",
            "cabin.ps1",
            "cabin.elv",
        ] {
            let path = out.join(filename);
            assert!(path.is_file(), "expected {filename} to exist at {path:?}");
            let body = fs::read(&path).unwrap();
            assert!(!body.is_empty(), "{filename} must be non-empty");
        }
    }

    #[test]
    fn compgen_all_without_output_dir_fails_clearly() {
        cabin()
            .args(["compgen", "--all"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("--output-dir"))
            .stderr(predicate::str::contains("--all"));
    }

    #[test]
    fn compgen_invalid_shell_fails() {
        cabin()
            .args(["compgen", "klingon"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("klingon"));
    }

    #[test]
    fn compgen_writes_single_shell_to_output_dir() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("completions");
        cabin()
            .args(["compgen", "fish", "--output-dir"])
            .arg(&out)
            .assert()
            .success();
        let path = out.join("cabin.fish");
        assert!(path.is_file());
        let body = fs::read(&path).unwrap();
        assert!(!body.is_empty());
        // Other shells should NOT have been written for a single-shell
        // invocation.
        assert!(!out.join("cabin.bash").exists());
    }

    #[test]
    fn mangen_writes_root_man_page_to_stdout() {
        let output = cabin()
            .args(["mangen"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.is_empty(), "man page should be non-empty");
        assert!(
            stdout.contains(".TH"),
            "stdout should look like a ROFF man page"
        );
        assert!(stdout.contains("cabin"));
    }

    #[test]
    fn mangen_writes_root_and_subcommand_pages_to_output_dir() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("man");
        cabin()
            .args(["mangen", "--output-dir"])
            .arg(&out)
            .assert()
            .success();
        let root = out.join("cabin.1");
        assert!(root.is_file(), "expected cabin.1 at {root:?}");
        let root_body = fs::read_to_string(&root).unwrap();
        assert!(!root_body.is_empty());

        // Every top-level subcommand — including the ones
        // hidden from `cabin --help` — gets its own
        // `cabin-<sub>.1` page. The expected list is derived
        // from clap so adding a subcommand updates this test
        // automatically.
        for sub in all_subcommand_names() {
            let path = out.join(format!("cabin-{sub}.1"));
            assert!(path.is_file(), "expected cabin-{sub}.1");
            let body = fs::read(&path).unwrap();
            assert!(!body.is_empty(), "cabin-{sub}.1 must be non-empty");
        }
    }

    #[test]
    fn mangen_root_page_mentions_known_subcommands() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("man");
        cabin()
            .args(["mangen", "--output-dir"])
            .arg(&out)
            .assert()
            .success();
        let body = fs::read_to_string(out.join("cabin.1")).unwrap();
        assert!(body.contains("build"));
        assert!(body.contains("package"));
    }
}

// ---------------------------------------------------------------------------
// cabin publish --registry-dir
// ---------------------------------------------------------------------------

mod file_registry {
    use super::*;

    fn write_simple_package(root: &Path) {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(
                r#"[package]
name = "fmt"
version = "10.2.1"

[target.fmt]
type = "cpp_library"
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

        let output = cabin()
            .args(["publish", "--manifest-path"])
            .arg(pkg_root.join("cabin.toml"))
            .arg("--registry-dir")
            .arg(&registry)
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
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

        let output = cabin()
            .args(["publish", "--dry-run", "--manifest-path"])
            .arg(pkg_root.join("cabin.toml"))
            .arg("--registry-dir")
            .arg(&registry)
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
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
type = "cpp_executable"
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

        let output = cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(&registry)
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
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
        if !build_tools_available() {
            skip(
                "published_registry_can_be_built",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
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
        let exe = build_dir.join("dev/packages/app/app");
        assert!(exe.is_file());
        let output = std::process::Command::new(&exe).output().unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello from fmt"));
    }
}

// ---------------------------------------------------------------------------
// cabin <cmd> --index-url against a static HTTP registry
// ---------------------------------------------------------------------------

mod sparse_http {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread::JoinHandle;

    /// Tiny static HTTP server backed by `tiny_http`. Serves files
    /// from a directory; missing files yield 404.
    struct TestServer {
        server: Arc<tiny_http::Server>,
        thread: Option<JoinHandle<()>>,
        url: String,
    }

    impl TestServer {
        fn serve(root: PathBuf) -> Self {
            let server = Arc::new(
                tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
            );
            let addr = server.server_addr().to_ip().expect("loopback addr");
            let url = format!("http://{addr}");
            let server_for_thread = Arc::clone(&server);
            let thread = std::thread::spawn(move || {
                loop {
                    let req = match server_for_thread.recv() {
                        Ok(req) => req,
                        Err(_) => break,
                    };
                    let raw_url = req.url().to_string();
                    let path = raw_url
                        .split('?')
                        .next()
                        .unwrap_or("")
                        .trim_start_matches('/')
                        .to_owned();
                    if path.contains("..") {
                        let _ = req.respond(tiny_http::Response::empty(400));
                        continue;
                    }
                    let file_path = root.join(&path);
                    if file_path.is_file() {
                        match fs::read(&file_path) {
                            Ok(bytes) => {
                                let _ = req.respond(tiny_http::Response::from_data(bytes));
                            }
                            Err(_) => {
                                let _ = req.respond(tiny_http::Response::empty(500));
                            }
                        }
                    } else {
                        let _ = req.respond(tiny_http::Response::empty(404));
                    }
                }
            });
            Self {
                server,
                thread: Some(thread),
                url,
            }
        }

        fn url(&self) -> &str {
            &self.url
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }

    fn publish_fmt_to_registry(dir: &Path) -> PathBuf {
        let pkg_root = dir.join("pkg");
        assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
            .write_str(
                r#"[package]
name = "fmt"
version = "10.2.1"

[target.fmt]
type = "cpp_library"
sources = ["src/fmt.cc"]
include_dirs = ["include"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(pkg_root.join("include/fmt.h"))
            .write_str("#pragma once\nvoid say_hello();\n")
            .unwrap();
        assert_fs::fixture::ChildPath::new(pkg_root.join("src/fmt.cc"))
            .write_str("#include <iostream>\n#include \"fmt.h\"\nvoid say_hello() { std::cout << \"hello from fmt\\n\"; }\n")
            .unwrap();
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
type = "cpp_executable"
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
    fn resolve_via_index_url_finds_published_package() {
        let dir = TempDir::new().unwrap();
        let registry = publish_fmt_to_registry(dir.path());
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(registry);

        let output = cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
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
    fn fetch_via_index_url_extracts_archive_into_cache() {
        let dir = TempDir::new().unwrap();
        let registry = publish_fmt_to_registry(dir.path());
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(registry);

        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .success();
        let sources = cache.join("sources/sha256");
        assert!(sources.is_dir());
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
    fn build_via_index_url_builds_executable() {
        if !build_tools_available() {
            skip(
                "build_via_index_url_builds_executable",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        let registry = publish_fmt_to_registry(dir.path());
        let app_main = "#include \"fmt.h\"\nint main() { say_hello(); return 0; }\n";
        write_app_using_fmt(dir.path(), Some(app_main));
        let server = TestServer::serve(registry);

        let cache = dir.path().join("cache");
        let build_dir = dir.path().join("build");
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .arg("--cache-dir")
            .arg(&cache)
            .arg("--build-dir")
            .arg(&build_dir)
            .assert()
            .success();
        let exe = build_dir.join("dev/packages/app/app");
        assert!(exe.is_file());
        let output = std::process::Command::new(&exe).output().unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello from fmt"));
    }

    #[test]
    fn index_path_and_index_url_together_fail() {
        let dir = TempDir::new().unwrap();
        let registry = publish_fmt_to_registry(dir.path());
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(registry.clone());
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-path")
            .arg(&registry)
            .arg("--index-url")
            .arg(server.url())
            .assert()
            .failure()
            .stderr(predicate::str::contains("--index-path"))
            .stderr(predicate::str::contains("--index-url"));
    }

    #[test]
    fn http_package_not_found_surfaces_clear_error() {
        let dir = TempDir::new().unwrap();
        let empty_registry = dir.path().join("registry");
        assert_fs::fixture::ChildPath::new(empty_registry.join("packages"))
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(empty_registry.join("artifacts"))
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(empty_registry.join("config.json"))
            .write_str(
                r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts"}"#,
            )
            .unwrap();
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(empty_registry);
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .assert()
            .failure()
            .stderr(predicate::str::contains("not found in HTTP index"));
    }

    #[test]
    fn http_invalid_metadata_surfaces_clear_error() {
        let dir = TempDir::new().unwrap();
        let registry = dir.path().join("registry");
        assert_fs::fixture::ChildPath::new(registry.join("packages"))
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(registry.join("artifacts"))
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(registry.join("config.json"))
            .write_str(
                r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts"}"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(registry.join("packages/fmt.json"))
            .write_binary(b"{ not really json")
            .unwrap();
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(registry);
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid package metadata"));
    }

    #[test]
    fn cross_origin_http_artifact_url_is_rejected() {
        let dir = TempDir::new().unwrap();
        let registry = publish_fmt_to_registry(dir.path());
        let pkg_index = registry.join("packages/fmt.json");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&pkg_index).unwrap()).unwrap();
        value["versions"]["10.2.1"]["source"]["path"] =
            serde_json::Value::String("http://127.0.0.1/artifacts/fmt.tar.gz".into());
        assert_fs::fixture::ChildPath::new(&pkg_index)
            .write_str(&(serde_json::to_string_pretty(&value).unwrap() + "\n"))
            .unwrap();
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(registry);
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .assert()
            .failure()
            .stderr(predicate::str::contains("same origin"));
    }

    #[test]
    fn http_artifact_checksum_mismatch_fails() {
        let dir = TempDir::new().unwrap();
        let registry = publish_fmt_to_registry(dir.path());
        // Tamper with the published `fmt.json` to advertise a wrong
        // checksum so the artifact bytes the server returns will
        // mismatch what the index claims.
        let pkg_index = registry.join("packages/fmt.json");
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&pkg_index).unwrap()).unwrap();
        value["versions"]["10.2.1"]["checksum"] =
            serde_json::Value::String(format!("sha256:{}", "0".repeat(64)));
        assert_fs::fixture::ChildPath::new(&pkg_index)
            .write_str(&(serde_json::to_string_pretty(&value).unwrap() + "\n"))
            .unwrap();
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(registry);
        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .failure()
            .stderr(predicate::str::contains("checksum mismatch"));
    }

    #[test]
    fn relative_artifact_path_resolves_correctly() {
        // A successful resolve confirms the HTTP loader resolves
        // `../artifacts/<name>/<name>-<version>.tar.gz` against the
        // package metadata URL.
        let dir = TempDir::new().unwrap();
        let registry = publish_fmt_to_registry(dir.path());
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(registry);
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .assert()
            .success();
    }

    #[test]
    fn frozen_with_index_url_fails_clearly() {
        let dir = TempDir::new().unwrap();
        let registry = publish_fmt_to_registry(dir.path());
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(registry);
        // Pre-populate a lockfile so `--frozen` reaches the
        // documented HTTP-metadata-cache check rather than the
        // "missing lockfile" path.
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .assert()
            .success();
        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--frozen", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .arg("--index-url")
            .arg(server.url())
            .arg("--cache-dir")
            .arg(&cache)
            .assert()
            .failure()
            .stderr(predicate::str::contains("--index-url"))
            .stderr(predicate::str::contains("--frozen"));
    }

    #[test]
    fn resolve_frozen_rejects_config_index_url() {
        let dir = TempDir::new().unwrap();
        let registry = publish_fmt_to_registry(dir.path());
        write_app_using_fmt(dir.path(), None);
        let server = TestServer::serve(registry);
        assert_fs::fixture::ChildPath::new(dir.path().join("app/.cabin/config.toml"))
            .write_str(&format!("[registry]\nindex-url = \"{}\"\n", server.url()))
            .unwrap();
        let mut cmd = cabin();
        super::pin_test_user_config_home_to_empty(&mut cmd);
        cmd.args(["resolve", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .env_remove("CABIN_NO_CONFIG")
            .env_remove("CABIN_CONFIG")
            .env_remove("CABIN_CONFIG_HOME")
            .assert()
            .success();

        let mut cmd = cabin();
        super::pin_test_user_config_home_to_empty(&mut cmd);
        cmd.args(["resolve", "--frozen", "--manifest-path"])
            .arg(dir.path().join("app/cabin.toml"))
            .env_remove("CABIN_NO_CONFIG")
            .env_remove("CABIN_CONFIG")
            .env_remove("CABIN_CONFIG_HOME")
            .assert()
            .failure()
            .stderr(predicate::str::contains("--index-url"))
            .stderr(predicate::str::contains("--frozen"));
    }
}

// ---------------------------------------------------------------------------
// features foundation
// ---------------------------------------------------------------------------

mod features {
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
type = "cpp_executable"
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
        let out = cabin()
            .current_dir(dir.path())
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .success()
            .get_output()
            .clone();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata json");
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
        let out = cabin()
            .current_dir(dir.path())
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--all-features"])
            .assert()
            .success()
            .get_output()
            .clone();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata json");
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
type = "cpp_executable"
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
type = "cpp_executable"
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
}

// ---------------------------------------------------------------------------
// advanced workspace semantics — members/exclude/default-members,
// --workspace / -p / --exclude / --default-members selection flags,
// workspace dependency inheritance, root discovery from a member dir,
// nested workspace rejection.
// ---------------------------------------------------------------------------

mod workspace_semantics {
    use super::*;

    /// Workspace with three members named `alpha`, `beta`, `gamma`,
    /// each with one cpp_executable and a shared `src/main.cc`. The
    /// caller can request that `default-members` and an `exclude`
    /// pattern be added, and gets back the manifest path.
    fn write_three_member_workspace(
        root: &Path,
        default_members: Option<&[&str]>,
        exclude: Option<&[&str]>,
    ) {
        let mut manifest = String::from("[workspace]\nmembers = [\"packages/*\"]\n");
        if let Some(dm) = default_members {
            let entries: Vec<String> = dm.iter().map(|n| format!("\"packages/{n}\"")).collect();
            manifest.push_str(&format!("default-members = [{}]\n", entries.join(", ")));
        }
        if let Some(ex) = exclude {
            let entries: Vec<String> = ex.iter().map(|n| format!("\"packages/{n}\"")).collect();
            manifest.push_str(&format!("exclude = [{}]\n", entries.join(", ")));
        }
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(&manifest)
            .unwrap();
        for name in ["alpha", "beta", "gamma"] {
            assert_fs::fixture::ChildPath::new(root.join(format!("packages/{name}/cabin.toml")))

                .write_str(&format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[target.{name}]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\"]\n"
                ))

                .unwrap();
            assert_fs::fixture::ChildPath::new(root.join(format!("packages/{name}/src/main.cc")))
                .write_str(HELLO_MAIN_CC)
                .unwrap();
        }
    }

    #[test]
    fn metadata_reports_workspace_members_default_excluded_selected() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), Some(&["alpha"]), Some(&["gamma"]));
        let value = run_metadata(&dir.path().join("cabin.toml"));
        let ws = &value["workspace"];
        assert!(!ws.is_null());
        let members: Vec<&str> = ws["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(members, vec!["alpha", "beta"]);
        let default_members: Vec<&str> = ws["default_members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(default_members, vec!["alpha"]);
        let excluded: Vec<&str> = ws["excluded_members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(excluded, vec!["packages/gamma"]);
        let selected: Vec<&str> = ws["selected_packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        // No CLI selection flags + default-members declared = the
        // current-package fallback selects default-members.
        assert_eq!(selected, vec!["alpha"]);
    }

    #[test]
    fn metadata_inside_member_directory_finds_root() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        let out = cabin()
            .current_dir(dir.path().join("packages/beta"))
            .args(["metadata"])
            .assert()
            .success()
            .get_output()
            .clone();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata JSON");
        let ws = &json["workspace"];
        assert!(
            !ws.is_null(),
            "workspace section missing — root discovery failed"
        );
        let root = ws["root"].as_str().unwrap();
        assert!(
            root.ends_with(dir.path().file_name().unwrap().to_str().unwrap()),
            "root mismatch: {root}"
        );
    }

    #[test]
    fn metadata_workspace_flag_selects_all_members_minus_exclude() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        let out = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--workspace", "--exclude", "beta"])
            .assert()
            .success()
            .get_output()
            .clone();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let selected: Vec<&str> = json["workspace"]["selected_packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(selected, vec!["alpha", "gamma"]);
    }

    #[test]
    fn metadata_explicit_packages_selects_named_members() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        let out = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "alpha", "-p", "gamma"])
            .assert()
            .success()
            .get_output()
            .clone();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let selected: Vec<&str> = json["workspace"]["selected_packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(selected, vec!["alpha", "gamma"]);
    }

    #[test]
    fn metadata_unknown_package_fails_clearly() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "nope"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("not a member"));
    }

    #[test]
    fn metadata_default_members_mode_errors_when_undeclared() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--default-members"])
            .assert()
            .failure()
            .stderr(
                predicate::str::contains("default member")
                    .or(predicate::str::contains("default-members")),
            );
    }

    #[test]
    fn metadata_exclude_with_explicit_package_errors() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "alpha", "--exclude", "beta"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("--exclude"));
    }

    #[test]
    fn workspace_default_member_missing_member_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
default-members = ["packages/missing"]
"#,
            )
            .unwrap();
        dir.child("packages/keep/cabin.toml")
            .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
            .unwrap();
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("default member"));
    }

    #[test]
    fn unused_exclude_pattern_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/keep"]
exclude = ["packages/missing"]
"#,
            )
            .unwrap();
        dir.child("packages/keep/cabin.toml")
            .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
            .unwrap();
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("packages/missing"));
    }

    #[test]
    fn nested_workspace_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["nested"]
"#,
            )
            .unwrap();
        dir.child("nested/cabin.toml")
            .write_str("[workspace]\nmembers = []\n")
            .unwrap();
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("nested workspace"));
    }

    #[test]
    fn workspace_dependency_inheritance_resolves_in_metadata() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10 <11"
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
            )
            .unwrap();
        let json = run_metadata(&dir.path().join("cabin.toml"));
        let app = json["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "app")
            .unwrap();
        let deps = app["dependencies"].as_array().unwrap();
        assert_eq!(deps.len(), 1);
        // The Workspace marker has been resolved into a Version
        // source by the workspace loader.
        assert_eq!(deps[0]["kind"], "version");
    }

    #[test]
    fn workspace_dependency_unresolved_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
            )
            .unwrap();
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("workspace = true"));
    }

    #[test]
    fn build_workspace_flag_builds_every_member() {
        if !build_tools_available() {
            skip(
                "workspace_semantics build --workspace",
                "ninja or C++ compiler missing",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        let build_dir = dir.path().join("build");
        cabin()
            .args(["build", "--workspace", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(&build_dir)
            .assert()
            .success();
        for name in ["alpha", "beta", "gamma"] {
            let exe = build_dir.join("dev").join("packages").join(name).join(name);
            assert!(exe.is_file(), "missing built binary {}", exe.display());
        }
    }

    #[test]
    fn build_with_explicit_packages_builds_only_those() {
        if !build_tools_available() {
            skip(
                "workspace_semantics build -p",
                "ninja or C++ compiler missing",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        let build_dir = dir.path().join("build");
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "beta"])
            .arg("--build-dir")
            .arg(&build_dir)
            .assert()
            .success();
        assert!(build_dir.join("dev/packages/beta/beta").is_file());
        // alpha and gamma must not have been built.
        assert!(!build_dir.join("dev/packages/alpha/alpha").exists());
        assert!(!build_dir.join("dev/packages/gamma/gamma").exists());
    }

    #[test]
    fn build_workspace_with_exclude_skips_member() {
        if !build_tools_available() {
            skip(
                "workspace_semantics build --workspace --exclude",
                "ninja or C++ compiler missing",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        let build_dir = dir.path().join("build");
        cabin()
            .args([
                "build",
                "--workspace",
                "--exclude",
                "gamma",
                "--manifest-path",
            ])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(&build_dir)
            .assert()
            .success();
        assert!(build_dir.join("dev/packages/alpha/alpha").is_file());
        assert!(build_dir.join("dev/packages/beta/beta").is_file());
        assert!(!build_dir.join("dev/packages/gamma/gamma").exists());
    }

    #[test]
    fn build_unknown_package_fails_clearly() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "nope", "--build-dir"])
            .arg(dir.path().join("build"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("not a member"));
    }

    #[test]
    fn package_in_workspace_requires_explicit_selection() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        // Without --package, packaging the workspace root must fail.
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("--package <name>"));

        // With a single --package, packaging the chosen member works.
        let dist = dir.path().join("dist");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "beta", "--output-dir"])
            .arg(&dist)
            .assert()
            .success();
        assert!(dist.join("beta-0.1.0.tar.gz").is_file());
    }

    #[test]
    fn publish_in_workspace_requires_explicit_selection() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace(dir.path(), None, None);
        let registry = dir.path().join("registry");

        // Without --package, publishing the workspace root must
        // fail with the workspace-boundary error.
        cabin()
            .args(["publish", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--registry-dir")
            .arg(&registry)
            .assert()
            .failure()
            .stderr(predicate::str::contains("--package <name>"));

        // With a single --package, publishing the chosen member
        // succeeds.
        cabin()
            .args(["publish", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "alpha", "--registry-dir"])
            .arg(&registry)
            .assert()
            .success();
        assert!(registry.join("packages/alpha.json").is_file());
    }
}

// ---------------------------------------------------------------------------
// post-merge regressions on the advanced-workspace-semantics surface.
// ---------------------------------------------------------------------------

mod workspace_review {
    use super::*;

    fn write_three_member_workspace_no_default(root: &Path) {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        for name in ["alpha", "beta", "gamma"] {
            assert_fs::fixture::ChildPath::new(root.join(format!("packages/{name}/cabin.toml")))

                .write_str(&format!(
                    "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[target.{name}]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\"]\n"
                ))

                .unwrap();
            assert_fs::fixture::ChildPath::new(root.join(format!("packages/{name}/src/main.cc")))
                .write_str(HELLO_MAIN_CC)
                .unwrap();
        }
    }

    /// Blocking 1: workspace.members with `..` must be rejected.
    #[test]
    fn member_with_parent_dir_rejected_at_cli() {
        let dir = TempDir::new().unwrap();
        let workspace_dir = dir.path().join("ws");
        let outside_dir = dir.path().join("outside");
        assert_fs::fixture::ChildPath::new(&workspace_dir)
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(&outside_dir)
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(workspace_dir.join("cabin.toml"))
            .write_str(
                r#"[workspace]
members = ["../outside"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(outside_dir.join("cabin.toml"))
            .write_str("[package]\nname = \"sneaky\"\nversion = \"0.1.0\"\n")
            .unwrap();
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(workspace_dir.join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("workspace.members"))
            .stderr(predicate::str::contains("../outside"));
    }

    /// Blocking 1: workspace.exclude with absolute path must be
    /// rejected.
    #[test]
    fn exclude_absolute_path_rejected_at_cli() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/keep"]
exclude = ["/tmp/outside"]
"#,
            )
            .unwrap();
        dir.child("packages/keep/cabin.toml")
            .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
            .unwrap();
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure()
            .stderr(predicate::str::contains("workspace.exclude"));
    }

    /// Blocking 3: building a package whose dep tree has no C/C++
    /// targets must not silently build every other package.
    #[test]
    fn select_package_without_cpp_target_errors_clearly() {
        if !build_tools_available() {
            skip(
                "workspace_semantics review empty selection",
                "ninja or C++ compiler missing",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        // empty: declares no targets at all.
        dir.child("packages/empty/cabin.toml")
            .write_str("[package]\nname = \"empty\"\nversion = \"0.1.0\"\n")
            .unwrap();
        // peer: a real C++ executable that should NOT be built when
        // the user selects only `empty`.
        dir.child("packages/peer/cabin.toml")
            .write_str(
                r#"[package]
name = "peer"
version = "0.1.0"

[target.peer]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("packages/peer/src/main.cc")
            .write_str(HELLO_MAIN_CC)
            .unwrap();
        let build_dir = dir.path().join("build");
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "empty", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .failure()
            .stderr(predicate::str::contains("no C/C++ targets"));
        assert!(
            !build_dir.join("dev/packages/peer/peer").exists(),
            "selecting `empty` must not have built `peer`"
        );
    }

    /// Blocking 2: `cabin fetch -p missing` must fail at the
    /// selection-validation step even when the workspace has no
    /// versioned dependencies (and thus no fetch happens).
    #[test]
    fn fetch_unknown_package_errors_without_versioned_deps() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace_no_default(dir.path());
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "missing"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("not a member"));
    }

    /// Blocking 2: `cabin resolve` over a pure-workspace root
    /// (no `[package]`) collects member versioned deps and writes a
    /// lockfile. An earlier baseline failed with "pure-workspace
    /// roots are not supported".
    #[test]
    fn resolve_pure_workspace_root_with_member_versioned_deps() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        // A minimal local index with a single fmt version that
        // satisfies the requirement.
        dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "dependencies": {},
                        "yanked": false,
                        "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    }
                }
            }"#)

            .unwrap();
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .assert()
            .success();
        let lockfile = fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
        assert!(
            lockfile.contains(r#"name = "fmt""#),
            "lockfile missing fmt: {lockfile}"
        );
    }

    /// Blocking 2: `cabin resolve -p app` selects exactly one
    /// member's deps. With only `app` selected, sibling members'
    /// requirements do not contribute.
    #[test]
    fn resolve_explicit_package_selection() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app", "packages/sibling"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        // sibling depends on a name that is NOT in the index. If
        // the resolver sees both, it would error out. With
        // `-p app`, only fmt should be considered.
        dir.child("packages/sibling/cabin.toml")
            .write_str(
                r#"[package]
name = "sibling"
version = "0.1.0"

[dependencies]
unknown = ">=1"
"#,
            )
            .unwrap();
        dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "dependencies": {},
                        "yanked": false,
                        "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    }
                }
            }"#)

            .unwrap();
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "app", "--index-path"])
            .arg(dir.path().join("index"))
            .assert()
            .success();
        let lockfile = fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
        assert!(lockfile.contains(r#"name = "fmt""#));
        assert!(!lockfile.contains("unknown"));
    }

    /// Blocking 2: `cabin update --update-package <name>` is the
    /// renamed dep-update flag that used to be `cabin update
    /// --package <name>`. The new `--package` is the workspace
    /// selector and is validated against the graph.
    #[test]
    fn update_split_flag_names() {
        // `cabin update --package <name>` is the dep-targeted
        // refresh flag. Workspace member scoping on
        // `cabin update` uses `--workspace`, `--default-members`,
        // and `--exclude` — not `-p`. The workspace here declares
        // no versioned deps, so any `--package` value reports
        // "not a versioned dependency" rather than "not a member";
        // the test asserts that the back-compat flag spelling
        // stays accepted.
        let dir = TempDir::new().unwrap();
        write_three_member_workspace_no_default(dir.path());
        cabin()
            .args(["update", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--package", "anything"])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "not a direct versioned dependency",
            ));
    }

    /// Non-blocking 4: `--manifest-path cabin.toml` from inside a
    /// workspace member must load the *member* manifest, not the
    /// workspace root. The default-no-flag invocation still walks
    /// up to the workspace root (covered by another upward-walk
    /// test in this file).
    #[test]
    fn explicit_manifest_path_overrides_root_discovery() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace_no_default(dir.path());
        let out = cabin()
            .current_dir(dir.path().join("packages/beta"))
            .args(["metadata", "--manifest-path", "cabin.toml"])
            .assert()
            .success()
            .get_output()
            .clone();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata JSON");
        // The metadata document for the *member* manifest has no
        // workspace section.
        assert!(
            json["workspace"].is_null(),
            "expected member-scoped metadata, got: {json}"
        );
        let pkgs = json["packages"].as_array().unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0]["name"], "beta");
    }

    /// Non-blocking 4 corollary: with no `--manifest-path`, root
    /// discovery still finds the workspace root from a member
    /// directory.
    #[test]
    fn default_manifest_path_walks_up_to_workspace_root() {
        let dir = TempDir::new().unwrap();
        write_three_member_workspace_no_default(dir.path());
        let out = cabin()
            .current_dir(dir.path().join("packages/beta"))
            .args(["metadata"])
            .assert()
            .success()
            .get_output()
            .clone();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata JSON");
        assert!(
            !json["workspace"].is_null(),
            "expected workspace section, got: {json}"
        );
    }
}

// ---------------------------------------------------------------------------
// Workspace-selection hardening — selected-closure index requirement, target
// scoping, Cargo scoping, feature scoping, package/publish workspace
// dep resolution, registry path safety + name mismatch validation,
// nested-workspace consistency, --exclude policy, update --package
// back-compat.
// ---------------------------------------------------------------------------

mod workspace_selection_hardening {
    use super::*;

    /// Workspace with `app` (C++ executable) plus an unrelated
    /// member `b` that declares a versioned dep. `cabin build -p
    /// app` must not require an index in this case.
    fn write_workspace_with_app_and_versioned_unrelated(root: &Path) {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("packages/app/cabin.toml"))
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("packages/app/src/main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("packages/b/cabin.toml"))
            .write_str(
                r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
    }

    #[test]
    fn build_p_app_does_not_require_index_when_unrelated_member_has_versioned_dep() {
        if !build_tools_available() {
            skip(
                "workspace_semantics.5 build -p app no index",
                "ninja or C++ compiler missing",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_workspace_with_app_and_versioned_unrelated(dir.path());
        let build_dir = dir.path().join("build");
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "app", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        assert!(build_dir.join("dev/packages/app/app").is_file());
    }

    #[test]
    fn fetch_p_app_does_not_require_index_when_unrelated_member_has_versioned_dep() {
        let dir = TempDir::new().unwrap();
        write_workspace_with_app_and_versioned_unrelated(dir.path());
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "app"])
            .assert()
            .success();
    }

    /// Path-dep transitive registry deps reach the resolver when
    /// the user selects only `app`.
    #[test]
    fn resolve_p_app_includes_registry_deps_from_path_dep_lib() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
lib = { path = "../lib" }
"#,
            )
            .unwrap();
        dir.child("packages/lib/cabin.toml")
            .write_str(
                r#"[package]
name = "lib"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
                }
            }"#)

            .unwrap();
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "app", "--index-path"])
            .arg(dir.path().join("index"))
            .assert()
            .success();
        let lock = fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
        assert!(lock.contains(r#"name = "fmt""#), "lockfile: {lock}");
    }

    /// Feature CLI requests apply only to selected packages.
    /// Unrelated packages that do not declare the requested
    /// feature must not fail the build.
    #[test]
    fn features_apply_only_to_selected_packages() {
        if !build_tools_available() {
            skip(
                "workspace_semantics.5 features scoped",
                "ninja or C++ compiler missing",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        // a declares ssl; b does not. Selecting -p a --features ssl
        // must succeed.
        dir.child("packages/a/cabin.toml")
            .write_str(
                r#"[package]
name = "a"
version = "0.1.0"

[features]
ssl = []

[target.a]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("packages/a/src/main.cc")
            .write_str(HELLO_MAIN_CC)
            .unwrap();
        dir.child("packages/b/cabin.toml")
            .write_str(
                r#"[package]
name = "b"
version = "0.1.0"

[target.b]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("packages/b/src/main.cc")
            .write_str(HELLO_MAIN_CC)
            .unwrap();
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "a", "--features", "ssl", "--build-dir"])
            .arg(dir.path().join("build"))
            .assert()
            .success();
    }

    /// `package` / `publish` in workspace context must see
    /// `dep = { workspace = true }` resolved against
    /// `[workspace.dependencies]`. Otherwise the package metadata
    /// would silently omit the dep.
    #[test]
    fn package_resolves_workspace_dep_inheritance() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10 <11"
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
            )
            .unwrap();
        let dist = dir.path().join("dist");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["-p", "app", "--output-dir"])
            .arg(&dist)
            .assert()
            .success();
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dist.join("app-0.1.0.json")).unwrap())
                .unwrap();
        assert!(
            json["dependencies"]["fmt"].is_string(),
            "fmt missing from package metadata: {json}"
        );
    }

    /// Standalone `cabin package` against a manifest with
    /// `dep = { workspace = true }` must error rather than
    /// silently drop the dep.
    #[test]
    fn package_standalone_workspace_dep_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
            )
            .unwrap();
        let dist = dir.path().join("dist");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .assert()
            .failure()
            .stderr(predicate::str::contains("workspace = true"));
    }

    /// Registry path safety. A package called `../evil` must not
    /// be allowed to publish.
    #[test]
    fn publish_unsafe_package_name_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "../evil"
version = "0.1.0"
"#,
            )
            .unwrap();
        let registry = dir.path().join("registry");
        cabin()
            .args(["publish", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--registry-dir")
            .arg(&registry)
            .assert()
            .failure();
        // The cabin-package layer rejects the name before any
        // registry write happens.
    }

    /// `--exclude` requires `--workspace` or
    /// `--default-members`. Using it with the no-flag default
    /// errors clearly.
    #[test]
    fn exclude_without_workspace_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        dir.child("packages/a/cabin.toml")
            .write_str("[package]\nname = \"a\"\nversion = \"0.1.0\"\n")
            .unwrap();
        dir.child("packages/b/cabin.toml")
            .write_str("[package]\nname = \"b\"\nversion = \"0.1.0\"\n")
            .unwrap();
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--exclude", "b"])
            .assert()
            .failure()
            .stderr(
                predicate::str::contains("--workspace")
                    .or(predicate::str::contains("--default-members")),
            );
    }

    /// A nested workspace invoked from inside is rejected by the
    /// strict nested-workspace discovery rule: discovery itself
    /// errors when it finds two `[workspace]` manifests above the
    /// starting path, naming both roots.
    #[test]
    fn nested_workspace_from_inside_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["nested"]
"#,
            )
            .unwrap();
        dir.child("nested/cabin.toml")
            .write_str(
                r#"[workspace]
members = []
"#,
            )
            .unwrap();
        cabin()
            .current_dir(dir.path().join("nested"))
            .args(["metadata"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("nested workspace"));
    }

    /// `cabin update --package <name>` keeps its
    /// dep-targeted-update meaning. Unknown name reports the
    /// "not a versioned dependency" error consistently.
    #[test]
    fn update_package_back_compat() {
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
        dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
                }
            }"#)

            .unwrap();
        cabin()
            .args(["update", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--package", "missing", "--index-path"])
            .arg(dir.path().join("index"))
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "not a direct versioned dependency",
            ));
    }
}

mod strict_nested_workspace_discovery {
    use super::*;

    /// When the user is sandwiched between two `[workspace]`
    /// roots — and the outer does NOT list the nested directory
    /// as a member — discovery still errors rather than silently
    /// picking one. The strict rule names both roots, so the
    /// user can disambiguate by passing `--manifest-path`
    /// explicitly; an earlier rule only rejected the nested case
    /// via the loader and only when the outer claimed the nested
    /// as a member.
    #[test]
    fn metadata_inside_nested_workspace_with_unrelated_outer_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = []
"#,
            )
            .unwrap();
        dir.child("nested/cabin.toml")
            .write_str(
                r#"[workspace]
members = []
"#,
            )
            .unwrap();
        cabin()
            .current_dir(dir.path().join("nested"))
            .args(["metadata"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("nested workspace detected"));
    }

    /// Selection-aware materialization. With workspace
    /// `app + b`, where `b` (unrelated to `app`) declares a
    /// versioned dep `spdlog` that is *not* in the registry, and
    /// the registry only carries `fmt` (which `app` uses),
    /// `cabin resolve -p app` must not error on the missing
    /// `spdlog` because `b` is outside the selected closure.
    #[test]
    fn resolve_p_app_does_not_require_unrelated_dep_in_registry() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        // `b` declares a dep on `spdlog` that the registry does
        // not carry. Selection-aware materialization must skip it.
        dir.child("packages/b/cabin.toml")
            .write_str(
                r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
spdlog = "^1"
"#,
            )
            .unwrap();
        dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
                }
            }"#)

            .unwrap();
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--package", "app", "--index-path"])
            .arg(dir.path().join("index"))
            .assert()
            .success();
    }

    /// `cabin update --package <name>` only refreshes direct
    /// versioned deps. Even if a transitive locked package would
    /// otherwise be reachable via the lockfile, the CLI rejects
    /// it explicitly.
    #[test]
    fn update_package_rejects_transitive() {
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
        dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
                }
            }"#)

            .unwrap();
        cabin()
            .args(["update", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--package", "spdlog", "--index-path"])
            .arg(dir.path().join("index"))
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "only refreshes direct dependencies",
            ));
    }
}

mod workspace_selection_followups {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use sha2::Digest;
    use std::fs::File;
    use std::io::Write;

    const FMT_PKG_MANIFEST: &str = r#"[package]
name = "fmt"
version = "10.2.1"

[target.fmt]
type = "cpp_library"
sources = ["src/fmt.cc"]
include_dirs = ["include"]
"#;
    const FMT_HEADER: &str = "#pragma once\nvoid say_hello();\n";
    const FMT_SRC: &str = "#include <iostream>\n#include \"fmt.h\"\nvoid say_hello() { std::cout << \"hello\\n\"; }\n";
    const APP_MAIN_USING_FMT: &str = "#include \"fmt.h\"\nint main() { say_hello(); return 0; }\n";

    /// Build a `.tar.gz` archive at `path` containing the given
    /// `(relative_path, body)` entries and return its sha256 hex.
    fn make_archive(path: &Path, entries: &[(&str, &str)]) -> String {
        if let Some(parent) = path.parent() {
            assert_fs::fixture::ChildPath::new(parent)
                .create_dir_all()
                .unwrap();
        }
        let f = File::create(path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
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
        let bytes = fs::read(path).unwrap();
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        format!("{:x}", hasher.finalize())
    }

    /// Selection-aware fixture: `app` (which declares a versioned
    /// dep on `fmt`) plus an unrelated workspace member `b` which
    /// declares a versioned dep on `spdlog` that the index does
    /// *not* cover. The fixture builds a real `fmt-10.2.1.tar.gz`
    /// archive and writes a matching index entry pointing at it,
    /// so `cabin fetch -p app` and `cabin build -p app` can
    /// succeed end-to-end without ever consulting `spdlog`.
    ///
    /// Returns the sha256 hex of the produced archive so callers
    /// can assert against the cache layout.
    fn write_workspace_with_real_fmt_archive(root: &Path) -> String {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("packages/app/cabin.toml"))
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
deps = ["fmt"]

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("packages/app/src/main.cc"))
            .write_str(APP_MAIN_USING_FMT)
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("packages/b/cabin.toml"))
            .write_str(
                r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
spdlog = "^1"
"#,
            )
            .unwrap();
        let archive_path = root.join("artifacts/fmt-10.2.1.tar.gz");
        let hex = make_archive(
            &archive_path,
            &[
                ("cabin.toml", FMT_PKG_MANIFEST),
                ("include/fmt.h", FMT_HEADER),
                ("src/fmt.cc", FMT_SRC),
            ],
        );
        let index_body = format!(
            r#"{{
  "schema": 1,
  "name": "fmt",
  "versions": {{
    "10.2.1": {{
      "dependencies": {{}},
      "yanked": false,
      "checksum": "sha256:{hex}",
      "source": {{ "type": "archive", "path": "../artifacts/fmt-10.2.1.tar.gz", "format": "tar.gz" }}
    }}
  }}
}}"#
        );
        assert_fs::fixture::ChildPath::new(root.join("index/fmt.json"))
            .write_str(&index_body)
            .unwrap();
        hex
    }

    /// `cabin resolve -p app` must succeed when only `app`'s
    /// versioned deps are covered by the index, even if an
    /// unrelated workspace member declares a versioned dep that
    /// the index does not know about.
    #[test]
    fn resolve_p_app_succeeds_when_unrelated_dep_missing_from_index() {
        let dir = TempDir::new().unwrap();
        write_workspace_with_real_fmt_archive(dir.path());
        let assertion = cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--package", "app", "--index-path"])
            .arg(dir.path().join("index"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        assert!(
            stdout.contains("fmt"),
            "resolve -p app should report `fmt` in its output: {stdout}"
        );
    }

    /// `cabin fetch -p app` against the same fixture must fully
    /// succeed: the `fmt` archive is in the index, has a real
    /// checksum, and selection-aware loading must skip the
    /// unrelated `spdlog` dep declared by `b`. We verify both
    /// cache state (the archive lands in `archives/sha256/<hex>`)
    /// and lockfile state (the lockfile pins `fmt` at the
    /// archive's checksum).
    #[test]
    fn fetch_p_app_extracts_fmt_and_skips_unrelated_dep() {
        let dir = TempDir::new().unwrap();
        let hex = write_workspace_with_real_fmt_archive(dir.path());
        let cache = dir.path().join("cache");
        cabin()
            .args(["fetch", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--package", "app", "--index-path"])
            .arg(dir.path().join("index"))
            .args(["--cache-dir"])
            .arg(&cache)
            .assert()
            .success();
        let archive_in_cache = cache.join("archives/sha256").join(format!("{hex}.tar.gz"));
        assert!(
            archive_in_cache.is_file(),
            "fmt archive must be cached at {archive_in_cache:?}"
        );
        let source_in_cache = cache.join("sources/sha256").join(&hex);
        assert!(
            source_in_cache.join("cabin.toml").is_file(),
            "fmt source must be extracted with cabin.toml at root"
        );
        let lock_path = dir.path().join("cabin.lock");
        assert!(lock_path.is_file(), "workspace lockfile should be written");
        let lock_body = fs::read_to_string(&lock_path).unwrap();
        assert!(
            lock_body.contains(r#"name = "fmt""#),
            "lockfile must pin fmt: {lock_body}"
        );
        assert!(
            lock_body.contains(&format!("checksum = \"sha256:{hex}\"")),
            "lockfile must record fmt's archive checksum: {lock_body}"
        );
        assert!(
            !lock_body.contains("spdlog"),
            "selection-aware fetch must not pin spdlog: {lock_body}"
        );
    }

    /// `cabin build -p app` against the same fixture must succeed
    /// end-to-end when the host toolchain is available: the
    /// `fmt` archive is fetched and extracted, the C++ link picks
    /// up its `cpp_library` target, and the resulting `app`
    /// executable lands under the build directory. `b` and its
    /// unindexed `spdlog` dep never enter the build graph.
    #[test]
    fn build_p_app_links_against_real_fmt_archive() {
        if !build_tools_available() {
            skip(
                "workspace_semantics.7 build -p app selection-aware",
                "ninja or C++ compiler missing",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_workspace_with_real_fmt_archive(dir.path());
        let build_dir = dir.path().join("build");
        let cache = dir.path().join("cache");
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--package", "app", "--build-dir"])
            .arg(&build_dir)
            .args(["--cache-dir"])
            .arg(&cache)
            .args(["--index-path"])
            .arg(dir.path().join("index"))
            .assert()
            .success();
        let app_exe = build_dir.join("dev/packages/app/app");
        assert!(
            app_exe.is_file(),
            "app executable must be produced at {app_exe:?}"
        );
    }

    /// An unsafe package name in a workspace member manifest must
    /// fail at manifest parsing time, *before* any sparse-HTTP
    /// URL is constructed. This pins the rule that
    /// `PackageName::new` is the structural gate.
    #[test]
    fn unsafe_package_name_in_manifest_rejected_before_http_url() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "foo?bar"
version = "0.1.0"
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
            stderr.contains("\"foo?bar\""),
            "error must echo the offending name: {stderr}"
        );
        assert!(
            stderr.contains("ASCII letters") && stderr.contains("ASCII digits"),
            "error must describe the allowed alphabet: {stderr}"
        );
    }

    /// The manifest dependency *name* is also validated up-front.
    /// A direct dep named `foo#bar` (a URL-reserved character) is
    /// rejected at parse time so a later `--index-url` flow
    /// cannot expand it into a hostile URL.
    #[test]
    fn unsafe_dep_name_in_manifest_rejected_before_http_url() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")

            .write_str("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\n\"foo#bar\" = \"1.0.0\"\n")

            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("foo#bar"),
            "expected error mentioning unsafe dep name foo#bar; stderr was: {stderr}"
        );
    }
}

mod dependency_kinds {
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
    fn dep_entry<'a>(
        package: &'a serde_json::Value,
        name: &str,
        kind: &str,
    ) -> &'a serde_json::Value {
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
        // Canonical kind order: normal, dev. Within each kind,
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
        // the index does *not* declare. With dev correctly
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
        // Index covers fmt but *not* gtest. If dev deps were
        // resolved, `gtest` would be missing.
        dir.child("index/fmt.json")

            .write_str(r#"{ "schema": 1, "name": "fmt", "versions": { "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" } } }"#)

            .unwrap();
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
        dir.child("index/fmt.json")

            .write_str(r#"{ "schema": 1, "name": "fmt", "versions": { "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" } } }"#)

            .unwrap();
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
        // normal kind. System dependencies (`system = true`)
        // remain declaration-only and may *not* carry `optional =
        // true`. Mixing the flags surfaces an explicit `system =
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
}

mod optional_dependencies_and_features {
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
        assert_fs::fixture::ChildPath::new(root.join("index/fmt.json"))

            .write_str(r#"{ "schema": 1, "name": "fmt", "versions": { "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" } } }"#)

            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("index/openssl.json"))

            .write_str(r#"{ "schema": 1, "name": "openssl", "versions": { "3.2.0": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" } } }"#)

            .unwrap();
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
        dir.child("index/openssl.json")

            .write_str(r#"{ "schema": 1, "name": "openssl", "versions": { "3.2.0": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" } } }"#)

            .unwrap();
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
            serde_json::from_str(&fs::read_to_string(out.join("demo-0.1.0.json")).unwrap())
                .unwrap();
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
}

mod target_dependencies {
    //! End-to-end coverage for `[target.'cfg(...)'.<kind>]`
    //! handling. The tests exercise the full pipeline: parser,
    //! workspace loader, resolver, fetch, package metadata,
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
    fn resolve_filters_inactive_target_dependency() {
        // Even though the manifest declares `spdlog`, only the
        // `fmt` constraint reaches the resolver because the
        // `spdlog` declaration is gated by a non-matching `cfg`.
        // This proves the index does not need to know about
        // `spdlog` for `cabin resolve` to succeed.
        let dir = TempDir::new().unwrap();
        let manifest = format!(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"

[target.'cfg(os = "{other}")'.dependencies]
spdlog = "^1"
"#,
            other = other_os_value(),
        );
        dir.child("cabin.toml").write_str(&manifest).unwrap();
        dir.child("index/fmt.json")

            .write_str(r#"{ "schema": 1, "name": "fmt", "versions": { "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" } } }"#)

            .unwrap();
        let assertion = cabin()
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--index-path"])
            .arg(dir.path().join("index"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        assert!(stdout.contains("fmt"), "fmt should resolve: {stdout}");
        assert!(
            !stdout.contains("spdlog"),
            "inactive spdlog must not enter resolution: {stdout}",
        );
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
            serde_json::from_str(&fs::read_to_string(out.join("demo-0.1.0.json")).unwrap())
                .unwrap();
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
}

mod profiles {
    //! End-to-end coverage for build profiles. The tests exercise
    //! the full pipeline: parser, resolver, build, metadata view,
    //! and the per-profile output directory.

    use super::*;

    #[test]
    fn metadata_reports_default_dev_profile_when_unselected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let selected = &value["profiles"]["selected"];
        assert_eq!(selected["name"].as_str(), Some("dev"));
        assert_eq!(selected["debug"].as_bool(), Some(true));
        assert_eq!(selected["opt_level"].as_str(), Some("0"));
        assert_eq!(selected["assertions"].as_bool(), Some(true));
        assert_eq!(selected["source"].as_str(), Some("builtin"));
        let available: Vec<&str> = value["profiles"]["available"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(available, vec!["dev", "release"]);
    }

    #[test]
    fn metadata_reports_release_when_selected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--profile", "release"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let selected = &value["profiles"]["selected"];
        assert_eq!(selected["name"].as_str(), Some("release"));
        assert_eq!(selected["opt_level"].as_str(), Some("3"));
        assert_eq!(selected["debug"].as_bool(), Some(false));
        assert_eq!(selected["assertions"].as_bool(), Some(false));
    }

    #[test]
    fn metadata_reports_custom_profile_definitions_and_resolved_fields() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[profile.relwithdebinfo]
inherits = "release"
debug = true
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--profile", "relwithdebinfo"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let selected = &value["profiles"]["selected"];
        assert_eq!(selected["name"].as_str(), Some("relwithdebinfo"));
        assert_eq!(selected["opt_level"].as_str(), Some("3"));
        assert_eq!(selected["debug"].as_bool(), Some(true));
        assert_eq!(selected["source"].as_str(), Some("custom"));
        let chain: Vec<&str> = selected["inherits_chain"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(chain, vec!["release", "relwithdebinfo"]);
        let available: Vec<&str> = value["profiles"]["available"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(available, vec!["dev", "release", "relwithdebinfo"]);
        assert!(
            value["profiles"]["definitions"]["relwithdebinfo"].is_object(),
            "manifest definition preserved",
        );
    }

    #[test]
    fn unknown_profile_errors_clearly_at_cli() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--profile", "fastdebug"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("unknown profile") && stderr.contains("fastdebug"),
            "expected unknown-profile error, got: {stderr}"
        );
    }

    #[test]
    fn invalid_profile_name_errors_clearly_at_cli() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--profile", ".release"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("invalid profile name"),
            "expected invalid-profile-name error, got: {stderr}"
        );
    }

    #[test]
    fn release_flag_and_profile_flag_conflict() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let assertion = cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--release", "--profile", "release"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("cannot be used with") || stderr.contains("conflicts"),
            "expected clap conflict error, got: {stderr}"
        );
    }

    #[test]
    fn dev_and_release_use_distinct_output_directories() {
        if !build_tools_available() {
            skip(
                "dev_and_release_use_distinct_output_directories",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .args(["build", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        cabin()
            .current_dir(dir.path())
            .args(["build", "--release", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();

        assert!(build_dir.join("dev").join("build.ninja").is_file());
        assert!(build_dir.join("release").join("build.ninja").is_file());
        assert!(
            build_dir
                .join("dev")
                .join("packages")
                .join("hello")
                .join("hello")
                .is_file()
        );
        assert!(
            build_dir
                .join("release")
                .join("packages")
                .join("hello")
                .join("hello")
                .is_file()
        );

        let dev_cc =
            std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
        let release_cc =
            std::fs::read_to_string(build_dir.join("release").join("compile_commands.json"))
                .unwrap();
        assert!(dev_cc.contains("-O0") && dev_cc.contains("-g"));
        assert!(release_cc.contains("-O3") && release_cc.contains("-DNDEBUG"));
    }

    #[test]
    fn custom_profile_uses_its_own_output_directory() {
        if !build_tools_available() {
            skip(
                "custom_profile_uses_its_own_output_directory",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "cpp_executable"
sources = ["src/main.cc"]

[profile.relwithdebinfo]
inherits = "release"
debug = true
"#,
            )
            .unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .args(["build", "--profile", "relwithdebinfo", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let cc = std::fs::read_to_string(
            build_dir
                .join("relwithdebinfo")
                .join("compile_commands.json"),
        )
        .unwrap();
        // Inherits release defaults (-O3 -DNDEBUG) but turns
        // debug info back on (-g).
        assert!(cc.contains("-O3"), "{cc}");
        assert!(cc.contains("-DNDEBUG"), "{cc}");
        assert!(cc.contains("-g"), "{cc}");
    }

    #[test]
    fn metadata_build_config_appends_inherited_profile_flags() {
        // Top-level [profile] flags, the selected profile's
        // inherits chain, and the leaf [profile.<name>] block
        // must compose with **append** semantics — root → leaf —
        // so the resolved build configuration carries every
        // contributing layer in declaration order.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[profile]
cxxflags = ["-Wall"]

[profile.release]
cxxflags = ["-O3"]

[profile.bench]
inherits = "release"
cxxflags = ["-pg"]
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--profile", "bench"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let per_package = value["toolchain"]["build_flags_per_package"]
            .as_object()
            .expect("toolchain.build_flags_per_package object");
        let pkg = per_package
            .values()
            .next()
            .expect("at least one package with build flags");
        let cxx: Vec<String> = pkg["cxxflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            cxx,
            vec!["-Wall".to_owned(), "-O3".to_owned(), "-pg".to_owned(),],
            "[profile] → inherited parent → selected must append in that order",
        );
    }

    #[test]
    fn metadata_build_config_orders_all_four_layers() {
        // Pin the full layer order documented in
        // `docs/profiles.md`:
        //   [profile] → matching [target.'cfg()'.profile]
        //             → inherited profile parent → selected profile
        // The conditional layer must land between the top-level
        // [profile] block and the profile inherits chain.
        let host_os = std::env::consts::OS;
        let dir = TempDir::new().unwrap();
        let manifest = format!(
            r#"[package]
name = "demo"
version = "0.1.0"

[profile]
cxxflags = ["-Wall"]

[target.'cfg(os = "{host_os}")'.profile]
cxxflags = ["-DCFG"]

[profile.release]
cxxflags = ["-O3"]

[profile.bench]
inherits = "release"
cxxflags = ["-pg"]
"#
        );
        dir.child("cabin.toml").write_str(&manifest).unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--profile", "bench"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let pkg = value["toolchain"]["build_flags_per_package"]
            .as_object()
            .expect("toolchain.build_flags_per_package object")
            .values()
            .next()
            .expect("at least one package with build flags");
        let cxx: Vec<String> = pkg["cxxflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            cxx,
            vec![
                "-Wall".to_owned(),
                "-DCFG".to_owned(),
                "-O3".to_owned(),
                "-pg".to_owned(),
            ],
            "documented order: [profile] → cfg → inherited → selected",
        );
    }

    #[test]
    fn old_manifest_without_profile_tables_still_metadata_works() {
        // Regression: older manifests have no profile
        // tables. Metadata view must still work and report the
        // built-in dev profile.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "old"
version = "0.1.0"

[dependencies]
fmt = ">=10"
"#,
            )
            .unwrap();
        cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .success();
    }
}

mod toolchain {
    //! End-to-end coverage for explicit toolchain selection and
    //! conditional build flags.

    use super::*;
    use std::path::PathBuf;

    /// Helper: write a fake compiler/archiver `name` into `dir`
    /// and return its absolute path. The fake binary is a
    /// minimal POSIX shell script so `--cxx /path/to/it` can be
    /// resolved; the tests never actually invoke it.
    #[cfg(unix)]
    fn fake_tool(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        assert_fs::fixture::ChildPath::new(&path)
            .write_str("#!/bin/sh\nexit 0\n")
            .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn metadata_reports_default_toolchain_source() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let cxx = &value["toolchain"]["tools"]["cxx"];
        assert_eq!(cxx["kind"].as_str(), Some("cxx"));
        assert_eq!(cxx["source"].as_str(), Some("default"));
    }

    #[test]
    fn metadata_requires_resolvable_cxx_before_detection() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let empty_path = TempDir::new().unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .env("PATH", empty_path.path())
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("no usable C++ compiler found on PATH"),
            "expected missing-CXX diagnostic, got: {stderr}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_cxx_flag_overrides_default() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let cxx = fake_tool(bin.path(), "my-cxx");
        let _ar = fake_tool(bin.path(), "ar");
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--cxx"])
            .arg(&cxx)
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let entry = &value["toolchain"]["tools"]["cxx"];
        assert_eq!(entry["source"].as_str(), Some("cli"));
        assert_eq!(entry["spec"].as_str().unwrap(), cxx.to_str().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn cxx_env_var_is_respected_when_no_cli_flag() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let cxx = fake_tool(bin.path(), "env-cxx");
        let _ar = fake_tool(bin.path(), "ar");
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .env("PATH", bin.path())
            .env("CXX", &cxx)
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let entry = &value["toolchain"]["tools"]["cxx"];
        assert_eq!(entry["source"].as_str(), Some("env"));
    }

    #[cfg(unix)]
    #[test]
    fn missing_explicit_cxx_errors_clearly() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _ar = fake_tool(bin.path(), "ar");
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--cxx", "definitely-not-a-real-compiler-99"])
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("definitely-not-a-real-compiler-99"),
            "expected toolchain error mentioning the spec, got: {stderr}",
        );
        assert!(
            stderr.contains("could not be found"),
            "expected `could not be found` wording, got: {stderr}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn manifest_toolchain_table_is_honored_when_no_cli_or_env() {
        let dir = TempDir::new().unwrap();
        let bin = TempDir::new().unwrap();
        let _g = fake_tool(bin.path(), "g++");
        let _c = fake_tool(bin.path(), "clang++");
        let _ar = fake_tool(bin.path(), "ar");
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[toolchain]
cxx = "clang++"
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let entry = &value["toolchain"]["tools"]["cxx"];
        assert_eq!(entry["source"].as_str(), Some("manifest"));
        assert_eq!(entry["spec"].as_str(), Some("clang++"));
    }

    #[test]
    fn unsupported_toolchain_field_is_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[toolchain]
compiler-family = "clang"
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
            stderr.contains("compiler-family"),
            "expected unknown-field error mentioning compiler-family, got: {stderr}",
        );
    }

    #[test]
    fn invalid_include_path_with_parent_traversal_is_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[profile]
include-dirs = ["../sneaky"]
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
            stderr.contains("..") || stderr.contains("sneaky"),
            "expected include-dir traversal rejection, got: {stderr}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn target_conditioned_build_flags_apply_to_compile_commands() {
        if !build_tools_available() {
            skip(
                "target_conditioned_build_flags_apply_to_compile_commands",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let host_os = std::env::consts::OS;
        let other_os = if host_os == "linux" { "macos" } else { "linux" };
        let dir = TempDir::new().unwrap();
        let manifest = format!(
            r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "cpp_executable"
sources = ["src/main.cc"]

[target.'cfg(os = "{host_os}")'.profile]
defines = ["CABIN_HOST_MATCHED"]

[target.'cfg(os = "{other_os}")'.profile]
defines = ["CABIN_HOST_NOT_MATCHED"]
"#
        );
        dir.child("cabin.toml").write_str(&manifest).unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .args(["build", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let cc =
            std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
        assert!(
            cc.contains("-DCABIN_HOST_MATCHED"),
            "expected matching cfg define present: {cc}"
        );
        assert!(
            !cc.contains("CABIN_HOST_NOT_MATCHED"),
            "expected non-matching cfg define absent: {cc}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn build_includes_dirs_from_build_table() {
        if !build_tools_available() {
            skip(
                "build_includes_dirs_from_build_table",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "cpp_executable"
sources = ["src/main.cc"]

[profile]
defines = ["CABIN_BUILD_DEFINE"]
include-dirs = ["include"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        dir.child("include/.gitkeep").write_str("").unwrap();

        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .args(["build", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let cc =
            std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
        assert!(
            cc.contains("-DCABIN_BUILD_DEFINE"),
            "expected build define in compile_commands: {cc}"
        );
        assert!(
            cc.contains("/include"),
            "expected include dir in compile_commands: {cc}"
        );
    }

    #[test]
    fn member_manifest_with_toolchain_table_is_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[toolchain]
cxx = "clang++"
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
            stderr.contains("toolchain"),
            "expected member-toolchain rejection, got: {stderr}",
        );
    }
}

mod compiler_detection {
    //! End-to-end coverage for compiler / tool capability
    //! detection. Each test stages a fake compiler / archiver in
    //! a `TempDir`, points `--cxx` / `--ar` at it, and inspects
    //! either the metadata JSON or the build error message.

    use super::*;
    use std::path::PathBuf;

    /// Write a fake tool that, when invoked with any args,
    /// prints `stdout` and `stderr` and exits with `status`. The
    /// shell wrapper is used because the CLI invokes
    /// `tool --version` directly; staging real compilers would
    /// be flaky on different CI hosts.
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
    fn metadata_reports_detected_clang_identity() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let cxx = fake_tool_with_output(
            bin.path(),
            "fake-clang++",
            "clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\nThread model: posix\n",
            "",
            0,
        );
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--cxx"])
            .arg(&cxx)
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let detected = &value["toolchain"]["detected"];
        assert_eq!(detected["cxx"]["identity"]["kind"].as_str(), Some("clang"));
        assert_eq!(
            detected["cxx"]["identity"]["version"].as_str(),
            Some("17.0.6")
        );
        assert!(
            detected["cxx"]["capabilities"]["gcc_style_flags"]["supported"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(detected["ar"]["identity"]["kind"].as_str(), Some("ar"));
    }

    #[cfg(unix)]
    #[test]
    fn build_with_msvc_compiler_errors_clearly() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let bin = TempDir::new().unwrap();
        let cxx = fake_tool_with_output(
            bin.path(),
            "fake-cl",
            "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.0 for x64\n",
            "",
            0,
        );
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["build", "--cxx"])
            .arg(&cxx)
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("MSVC") || stderr.contains("GCC- or Clang-like"),
            "expected MSVC unsupported error, got: {stderr}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn build_with_unknown_compiler_errors_clearly() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let bin = TempDir::new().unwrap();
        let cxx = fake_tool_with_output(
            bin.path(),
            "fake-funky-cxx",
            "my funky compiler 0.1\n",
            "",
            0,
        );
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["build", "--cxx"])
            .arg(&cxx)
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("could not be identified"),
            "expected unknown-compiler error, got: {stderr}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn package_metadata_does_not_serialize_local_detection() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "cpp_library"
sources = ["src/lib.cc"]
"#,
            )
            .unwrap();
        dir.child("src/lib.cc")
            .write_str("int demo() { return 0; }\n")
            .unwrap();
        let out = dir.path().join("dist");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--output-dir"])
            .arg(&out)
            .assert()
            .success();
        let body = fs::read_to_string(out.join("demo-0.1.0.json")).unwrap();
        // Detected info must never leak into published metadata.
        assert!(!body.contains("CABIN_CXX_COMPILER_KIND"), "{body}");
        assert!(!body.contains("\"detected\""), "{body}");
        assert!(!body.contains("clang version"), "{body}");
        assert!(!body.contains("Apple clang"), "{body}");
    }

    #[cfg(unix)]
    #[test]
    fn metadata_toolchain_block_is_a_stable_golden_for_a_fixed_toolchain() {
        // Pin the JSON shape of `cabin metadata`'s
        // `toolchain.detected` block end-to-end. Uses fake
        // compiler / archiver wrappers so the golden does not
        // depend on whichever clang/gcc happens to be installed;
        // absolute paths are normalized back to placeholders
        // before the snapshot comparison so the assertion does
        // not embed machine-specific data.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let cxx = fake_tool_with_output(
            bin.path(),
            "fake-clang++",
            "clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\nThread model: posix\n",
            "",
            0,
        );
        let ar =
            fake_tool_with_output(bin.path(), "fake-ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--cxx"])
            .arg(&cxx)
            .args(["--ar"])
            .arg(&ar)
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

        let cxx_str = cxx.to_string_lossy().into_owned();
        let ar_str = ar.to_string_lossy().into_owned();
        let detected = serde_json::to_string_pretty(&value["toolchain"]["detected"]).unwrap();
        let detected = detected.replace(&cxx_str, "<CXX>").replace(&ar_str, "<AR>");

        let expected = r#"{
  "cxx": {
    "path": "<CXX>",
    "identity": {
      "kind": "clang",
      "version": "17.0.6",
      "target": "x86_64-unknown-linux-gnu",
      "raw_version_line": "clang version 17.0.6"
    },
    "capabilities": {
      "color_diagnostics_flag": {
        "supported": true,
        "source": "version"
      },
      "cxx_standard_17": {
        "supported": true,
        "source": "version"
      },
      "depfile_mmd_mf": {
        "supported": true,
        "source": "version"
      },
      "gcc_style_flags": {
        "supported": true,
        "source": "version"
      },
      "json_diagnostics": {
        "supported": true,
        "source": "version"
      },
      "msvc_style_flags": {
        "supported": false,
        "source": "assumed-default"
      },
      "response_files": {
        "supported": true,
        "source": "version"
      },
      "sarif_diagnostics": {
        "supported": false,
        "source": "assumed-default"
      },
      "std_flag": {
        "supported": true,
        "source": "version"
      }
    }
  },
  "ar": {
    "path": "<AR>",
    "identity": {
      "kind": "ar",
      "version": "2.40",
      "raw_version_line": "GNU ar (GNU Binutils) 2.40"
    },
    "capabilities": {
      "ar_crs": {
        "supported": true,
        "source": "version"
      },
      "static_library_output": {
        "supported": true,
        "source": "version"
      }
    }
  }
}"#;
        assert_eq!(detected, expected);
    }
}

mod compiler_cache {
    //! End-to-end coverage for the compiler-cache wrapper feature
    //! (`ccache` / `sccache`). Each test stages a fake wrapper +
    //! compiler / archiver, points the CLI at them, and inspects
    //! either the metadata JSON or a stub `cabin build` invocation.

    use super::*;
    use std::path::PathBuf;

    /// Re-implementation of `compiler_detection::fake_tool_with_output`.
    /// The detection module is private to its `mod`, so the helper
    /// is duplicated here rather than reaching across module
    /// boundaries.
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
    fn metadata_reports_no_wrapper_by_default() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .env_remove("CABIN_COMPILER_WRAPPER")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert!(
            value["toolchain"]["compiler_wrapper"].is_null(),
            "expected null compiler_wrapper, got: {}",
            value["toolchain"]["compiler_wrapper"],
        );
    }

    #[cfg(unix)]
    #[test]
    fn metadata_reports_cli_selected_ccache() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
        let _ccache = fake_tool_with_output(
            bin.path(),
            "ccache",
            "ccache version 4.10.2\nFeatures: file-storage http-storage\n",
            "",
            0,
        );
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--compiler-wrapper", "ccache"])
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
        assert_eq!(wrapper["source"].as_str(), Some("cli"));
        assert_eq!(wrapper["version"].as_str(), Some("4.10.2"));
    }

    #[cfg(unix)]
    #[test]
    fn no_compiler_wrapper_overrides_manifest_selection() {
        let dir = TempDir::new().unwrap();
        // Manifest selects ccache, but `--no-compiler-wrapper`
        // wins. The wrapper executable is intentionally absent so
        // a regression that ignored the override would surface as
        // a NotFound error instead of a silent pass.
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[profile.cache]
compiler-wrapper = "ccache"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--no-compiler-wrapper"])
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .env_remove("CABIN_COMPILER_WRAPPER")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert!(value["toolchain"]["compiler_wrapper"].is_null());
    }

    #[cfg(unix)]
    #[test]
    fn manifest_build_cache_selects_wrapper_when_no_override() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[profile.cache]
compiler-wrapper = "sccache"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
        let _sccache = fake_tool_with_output(bin.path(), "sccache", "sccache 0.7.7\n", "", 0);
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
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
        assert_eq!(wrapper["kind"].as_str(), Some("sccache"));
        assert_eq!(wrapper["source"].as_str(), Some("manifest"));
        assert_eq!(wrapper["version"].as_str(), Some("0.7.7"));
    }

    #[cfg(unix)]
    #[test]
    fn env_overrides_manifest_compiler_wrapper() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[profile.cache]
compiler-wrapper = "sccache"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
        let _ccache = fake_tool_with_output(bin.path(), "ccache", "ccache version 4.10.2\n", "", 0);
        let _sccache = fake_tool_with_output(bin.path(), "sccache", "sccache 0.7.7\n", "", 0);
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .env("PATH", bin.path())
            .env("CABIN_COMPILER_WRAPPER", "ccache")
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let wrapper = &value["toolchain"]["compiler_wrapper"];
        assert_eq!(wrapper["kind"].as_str(), Some("ccache"));
        assert_eq!(wrapper["source"].as_str(), Some("env"));
    }

    #[cfg(unix)]
    #[test]
    fn missing_wrapper_executable_yields_clear_build_error() {
        // CLI requests ccache, but PATH has no `ccache` binary —
        // the build orchestration must surface a typed
        // "not found" error rather than silently dropping the
        // wrapper. A `ninja` stub is staged so the build path
        // reaches the wrapper-resolution step before bailing on
        // missing tools.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "cpp_library"
sources = ["src/lib.cc"]
"#,
            )
            .unwrap();
        dir.child("src/lib.cc")
            .write_str("int demo() { return 0; }\n")
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
        let _ninja = fake_tool_with_output(bin.path(), "ninja", "1.11.1\n", "", 0);
        // No ccache staged.
        let assertion = cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--compiler-wrapper", "ccache"])
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .env_remove("CABIN_COMPILER_WRAPPER")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("ccache") && stderr.contains("could not be found"),
            "expected NotFound message naming ccache, got: {stderr}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsupported_cli_value_is_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--compiler-wrapper", "fastcache"])
            .env("PATH", bin.path())
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .env_remove("CABIN_COMPILER_WRAPPER")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("fastcache") && stderr.contains("not supported"),
            "expected unsupported-wrapper error, got: {stderr}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cli_flags_are_mutually_exclusive() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"
"#,
            )
            .unwrap();
        // Clap rejects the combination before any orchestration
        // runs, which makes the test fully hermetic.
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--compiler-wrapper", "ccache", "--no-compiler-wrapper"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("--no-compiler-wrapper")
                || stderr.contains("--compiler-wrapper")
                || stderr.contains("cannot be used"),
            "expected mutually-exclusive error, got: {stderr}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn build_fingerprint_changes_when_wrapper_changes() {
        // Two `cabin metadata` runs differing only in the wrapper
        // selection must produce different `fingerprint` values
        // so cache layers can distinguish them.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[features]
default = []
fast = []
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
        let _ccache = fake_tool_with_output(bin.path(), "ccache", "ccache version 4.10.2\n", "", 0);
        let common = |extra: &[&str]| {
            let mut cmd = cabin();
            cmd.args(["metadata", "--manifest-path"])
                .arg(dir.path().join("cabin.toml"))
                .args(extra)
                .env("PATH", bin.path())
                .env_remove("CXX")
                .env_remove("CC")
                .env_remove("AR")
                .env_remove("CABIN_COMPILER_WRAPPER");
            cmd
        };

        let baseline = common(&[]).assert().success();
        let baseline_value: serde_json::Value =
            serde_json::from_str(&String::from_utf8_lossy(&baseline.get_output().stdout)).unwrap();
        let baseline_fp = baseline_value["packages"][0]["configuration"]["fingerprint"]
            .as_str()
            .expect("baseline fingerprint")
            .to_owned();

        let with_wrapper = common(&["--compiler-wrapper", "ccache"]).assert().success();
        let with_wrapper_value: serde_json::Value =
            serde_json::from_str(&String::from_utf8_lossy(&with_wrapper.get_output().stdout))
                .unwrap();
        let with_wrapper_fp = with_wrapper_value["packages"][0]["configuration"]["fingerprint"]
            .as_str()
            .expect("wrapper fingerprint");

        assert_ne!(
            baseline_fp, with_wrapper_fp,
            "fingerprint must differ when a wrapper is selected"
        );
    }

    #[cfg(unix)]
    #[test]
    fn member_manifest_with_build_cache_is_rejected() {
        // Wrapper settings must only appear at the workspace
        // root. A member declaring `[profile.cache]` should surface
        // a clear `MemberDeclaresCompilerWrapper`-shaped error.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["member"]

[package]
name = "root"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("member/cabin.toml")
            .write_str(
                r#"[package]
name = "member"
version = "0.1.0"

[profile.cache]
compiler-wrapper = "ccache"
"#,
            )
            .unwrap();
        let bin = TempDir::new().unwrap();
        let _cxx = fake_tool_with_output(bin.path(), "c++", "clang version 17.0.6\n", "", 0);
        let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar 2.40\n", "", 0);
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .env("PATH", bin.path())
            .env_remove("CABIN_COMPILER_WRAPPER")
            .env_remove("CXX")
            .env_remove("CC")
            .env_remove("AR")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("compiler-cache wrapper")
                || stderr.contains("[profile.cache]")
                || stderr.contains("workspace root"),
            "expected member-rejection error, got: {stderr}"
        );
    }
}

mod config {
    //! End-to-end coverage for the typed config layer:
    //! discovery, parsing, merging, precedence, and metadata
    //! reporting. Tests stage temp directories for the user
    //! config home (via `CABIN_CONFIG_HOME`) and the workspace
    //! root so they never read or write a developer's real
    //! `~/.config/cabin/config.toml`.

    use super::*;
    use std::path::PathBuf;

    /// Build a `cabin` command that re-enables config discovery
    /// for a single test. Mirrors the default test-harness
    /// helper but drops the `CABIN_NO_CONFIG=1` opt-out applied
    /// to every other integration test.
    fn cabin_with_config() -> Command {
        let mut cmd =
            Command::cargo_bin("cabin").expect("the `cabin` binary should be built by cargo");
        cmd.env_remove("CABIN_NO_CONFIG")
            .env_remove("CABIN_CONFIG")
            .env_remove("CABIN_CONFIG_HOME");
        super::pin_test_user_config_home_to_empty(&mut cmd);
        super::pin_test_cache_home(&mut cmd);
        cmd
    }

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
    fn invalid_compiler_wrapper_in_config_yields_clear_error() {
        let dir = project_dir(MINIMAL_PROJECT);
        write_workspace_config(
            dir.path(),
            r#"[build.cache]
compiler-wrapper = "fastcache"
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
            stderr.contains("not supported") && stderr.contains("none, ccache, sccache"),
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
            resolved.ends_with("/.cabin/registry"),
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
type = "cpp_library"
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

[build.cache]
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
        // metadata — `cabin package` is supposed to drop local
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
        // honored. We point the CLI at a temp index and the
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
        // touching the index, so success here just confirms the
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

    /// Stage a fake tool that prints fixed `--version` output —
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
        let cxx =
            fake_tool_with_output(bin.path(), "fake-clang++", "clang version 17.0.6\n", "", 0);
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
        let env_cxx =
            fake_tool_with_output(bin.path(), "env-clang++", "clang version 17.0.6\n", "", 0);
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
            r#"[build.cache]
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
            r#"[build.cache]
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
}

mod patches {
    //! End-to-end coverage for the patch / override layer.
    //!
    //! Each test stages a temp workspace plus a sibling
    //! "patched fork" directory that holds a real `cabin.toml`,
    //! then drives `cabin metadata` (or `cabin package`) and
    //! inspects the resulting JSON / errors. No tests here
    //! perform network access; the patch path is the only source
    //! of truth.

    use super::*;
    use std::path::PathBuf;

    fn cabin_with_config() -> Command {
        let mut cmd =
            Command::cargo_bin("cabin").expect("the `cabin` binary should be built by cargo");
        cmd.env_remove("CABIN_NO_CONFIG")
            .env_remove("CABIN_CONFIG")
            .env_remove("CABIN_CONFIG_HOME");
        super::pin_test_user_config_home_to_empty(&mut cmd);
        super::pin_test_cache_home(&mut cmd);
        cmd
    }

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

[target.app]
type = "cpp_library"
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
            // with a `│` continuation prefix. Pin the load-bearing
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

    #[test]
    fn locked_fails_when_patch_policy_changed_after_lockfile() {
        // Lock the package once with no patches, then add a
        // `[patch]` table whose fork still satisfies the original
        // requirement. The package set is unchanged; only the
        // patch state differs. `cabin resolve --locked` must
        // detect that and refuse to proceed.
        let parent = TempDir::new().unwrap();
        let root = parent.path().join("app");
        write_root_manifest(
            &root,
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        );
        let index = parent.path().join("index");
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

        // Add a manifest patch that supplies fmt at 10.2.0 — the
        // same version the resolver picked from the index. The locked
        // package set is identical, but the `[[patch]]` array
        // changed, so --locked must bail.
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
version = "10.2.0"
"#,
        );

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

    #[test]
    fn source_replacement_self_loop_yields_clear_cycle_error() {
        // A workspace config whose source-replacement entry
        // points back at its own original triggers cycle detection
        // the moment the CLI tries to resolve the index source for
        // a fetch / resolve invocation that needs versioned deps.
        // `cabin metadata` is intentionally lazy here — it never
        // walks the replacement chain — so we drive `resolve`,
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
        // index. The post-replacement check must catch the
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
        // mention of fmt is a *dev* dep with `>= 99` — clearly
        // unsatisfiable, but dev deps are inactive for the
        // default invocation, so patch validation must skip the
        // edge and metadata succeeds. This is the end-to-end
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
        // package. After the fix, `cabin resolve` must include
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
        // dep on `spdlog`. Without the patched-deps fix, this
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
        let output = cabin()
            .args(["resolve", "--manifest-path"])
            .arg(root.join("cabin.toml"))
            .arg("--index-path")
            .arg(parent.path().join("index"))
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
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

[target.demo]
type = "cpp_library"
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
}

mod test_targets {
    //! End-to-end coverage for `cpp_test` / `cpp_example` target
    //! kinds and the `cabin test` command.

    use super::*;

    /// Single-package fixture with one library plus one passing test
    /// target. Returns the temp dir guard so the caller can drive
    /// commands against it.
    fn passing_test_project() -> TempDir {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "cpp_library"
sources = ["src/lib.cc"]

[target.demo_test]
type = "cpp_test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]
"#,
            )
            .unwrap();
        dir.child("src/lib.cc")
            .write_str("int demo() { return 42; }\n")
            .unwrap();
        dir.child("tests/lib_test.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        dir
    }

    fn project_with_dev_kinds() -> TempDir {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "cpp_library"
sources = ["src/lib.cc"]

[target.demo_test]
type = "cpp_test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]

[target.hello_example]
type = "cpp_example"
sources = ["examples/hello.cc"]
deps = ["demo"]
"#,
            )
            .unwrap();
        dir.child("src/lib.cc")
            .write_str("int demo() { return 1; }\n")
            .unwrap();
        dir.child("tests/lib_test.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        dir.child("examples/hello.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        dir
    }

    #[test]
    fn metadata_lists_test_and_example_target_kinds() {
        let dir = project_with_dev_kinds();
        let value = run_metadata(&dir.path().join("cabin.toml"));
        let demo = package_in(&value, "demo");
        let kinds: std::collections::BTreeMap<String, String> = demo["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t["name"].as_str().unwrap().to_owned(),
                    t["kind"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(kinds.get("demo").map(String::as_str), Some("cpp_library"));
        assert_eq!(kinds.get("demo_test").map(String::as_str), Some("cpp_test"));
        assert_eq!(
            kinds.get("hello_example").map(String::as_str),
            Some("cpp_example")
        );
    }

    #[test]
    fn invalid_target_kind_is_rejected_with_helpful_message() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.broken]
type = "cpp_tests"
sources = ["src/x.cc"]
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        // Wording is stable: enumerate the supported kinds so the
        // user can correct the typo without reading docs.
        assert!(
            stderr.contains("cpp_test")
                && stderr.contains("cpp_library")
                && stderr.contains("cpp_executable"),
            "expected target-type error mentioning the supported kinds, got: {stderr}"
        );
    }

    #[test]
    fn build_default_does_not_build_dev_only_targets() {
        if !build_tools_available() {
            skip(
                "build_default_does_not_build_dev_only_targets",
                "ninja or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = project_with_dev_kinds();
        // `-v` keeps Ninja's `[N/M] AR / CXX / LINK …` progress
        // lines on stdout so the assertion below can pin the
        // archive action.  At the default verbosity the lines
        // are filtered to match cargo's terser banner shape.
        let assertion = cabin()
            .args(["build", "-v", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        // The library object/archive must build; the dev-only
        // targets must NOT appear in the ninja output.
        assert!(
            stdout.contains("AR"),
            "library archive should build: {stdout}"
        );
        for forbidden in ["demo_test", "hello_example"] {
            assert!(
                !stdout.contains(forbidden),
                "default build must not produce {forbidden}: {stdout}"
            );
        }
    }

    #[test]
    fn cabin_test_builds_and_runs_passing_test() {
        if !build_tools_available() {
            skip(
                "cabin_test_builds_and_runs_passing_test",
                "ninja or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = passing_test_project();
        let assertion = cabin()
            .args(["test", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        assert!(
            stdout.contains("test demo:demo_test ... ok"),
            "expected per-test result line, got: {stdout}"
        );
        assert!(
            stdout.contains("test result: ok. 1 passed; 0 failed"),
            "expected passing summary, got: {stdout}"
        );
    }

    #[test]
    fn cabin_test_sets_per_test_cabin_env_overlay() {
        if !build_tools_available() {
            skip(
                "cabin_test_sets_per_test_cabin_env_overlay",
                "ninja or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "env_demo"
version = "0.1.0"

[target.env_test]
type = "cpp_test"
sources = ["tests/env_test.cc"]
"#,
            )
            .unwrap();
        dir.child("tests/env_test.cc")
            .write_str(
                r#"#include <cstdio>
#include <cstdlib>
#include <cstring>

static int status = 0;

void keep(const char* name, const char* expected) {
    const char* v = std::getenv(name);
    if (v == nullptr) {
        std::printf("MISSING %s\n", name);
        status |= 1;
        return;
    }
    std::printf("KEEP %s=%s\n", name, v);
    if (expected != nullptr && std::strcmp(v, expected) != 0) {
        status |= 2;
    }
}

void keep_present(const char* name) {
    const char* v = std::getenv(name);
    if (v == nullptr || v[0] == '\0') {
        std::printf("MISSING %s\n", name);
        status |= 1;
        return;
    }
    std::printf("KEEP %s\n", name);
}

void must_be_absent(const char* name) {
    if (std::getenv(name) != nullptr) {
        std::printf("LEAK %s\n", name);
        status |= 4;
    } else {
        std::printf("ABSENT %s\n", name);
    }
}

int main() {
    keep("CABIN_PACKAGE_NAME", "env_demo");
    keep("CABIN_PACKAGE_VERSION", "0.1.0");
    keep("CABIN_PROFILE", "dev");
    keep_present("CABIN_MANIFEST_DIR");
    keep_present("CABIN_MANIFEST_PATH");
    keep_present("CABIN_BUILD_DIR");
    must_be_absent("CABIN");
    must_be_absent("CABIN_PACKAGE_NAME_CANONICAL");
    must_be_absent("CABIN_BIN_NAME");
    must_be_absent("CABIN_BIN_NAME_CANONICAL");
    must_be_absent("CABIN_TEST_NAME");
    must_be_absent("CABIN_TEST_NAME_CANONICAL");
    must_be_absent("CABIN_TARGET_KIND");
    must_be_absent("CABIN_TARGET_TRIPLE");
    must_be_absent("CABIN_HOST_TRIPLE");
    must_be_absent("CABIN_BUILD_CONFIGURATION_FINGERPRINT");
    return status;
}
"#,
            )
            .unwrap();

        let assertion = cabin()
            .args(["test", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        for expected in [
            "KEEP CABIN_PACKAGE_NAME=env_demo",
            "KEEP CABIN_PACKAGE_VERSION=0.1.0",
            "KEEP CABIN_PROFILE=dev",
            "KEEP CABIN_MANIFEST_DIR",
            "KEEP CABIN_MANIFEST_PATH",
            "KEEP CABIN_BUILD_DIR",
            "ABSENT CABIN",
            "ABSENT CABIN_PACKAGE_NAME_CANONICAL",
            "ABSENT CABIN_BIN_NAME",
            "ABSENT CABIN_BIN_NAME_CANONICAL",
            "ABSENT CABIN_TEST_NAME",
            "ABSENT CABIN_TEST_NAME_CANONICAL",
            "ABSENT CABIN_TARGET_KIND",
            "ABSENT CABIN_TARGET_TRIPLE",
            "ABSENT CABIN_HOST_TRIPLE",
            "ABSENT CABIN_BUILD_CONFIGURATION_FINGERPRINT",
            "test env_demo:env_test ... ok",
        ] {
            assert!(
                stdout.contains(expected),
                "expected `{expected}` in test output, got: {stdout}"
            );
        }
        assert!(
            !stdout.contains("LEAK "),
            "no removed CABIN_* variable may be injected, got: {stdout}"
        );
    }

    #[test]
    fn cabin_test_exits_non_zero_on_failure() {
        if !build_tools_available() {
            skip(
                "cabin_test_exits_non_zero_on_failure",
                "ninja or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = passing_test_project();
        dir.child("tests/lib_test.cc")
            .write_str("int main() { return 17; }\n")
            .unwrap();
        let assertion = cabin()
            .args(["test", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .failure();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stdout.contains("test demo:demo_test ... FAILED (exit 17)"),
            "expected per-test failure line, got stdout: {stdout}"
        );
        assert!(
            stderr.contains("test failures: 1 of 1"),
            "expected failure summary in stderr, got: {stderr}"
        );
    }

    #[test]
    fn cabin_test_no_targets_errors_by_default() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "lib_only"
version = "0.1.0"

[target.lib_only]
type = "cpp_library"
sources = ["src/lib.cc"]
"#,
            )
            .unwrap();
        dir.child("src/lib.cc")
            .write_str("int x() { return 1; }\n")
            .unwrap();
        let assertion = cabin()
            .args(["test", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("no test targets found"),
            "expected no-test-targets error, got: {stderr}"
        );
    }

    #[test]
    fn cabin_test_no_targets_succeeds_with_allow_no_tests() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "lib_only"
version = "0.1.0"

[target.lib_only]
type = "cpp_library"
sources = ["src/lib.cc"]
"#,
            )
            .unwrap();
        dir.child("src/lib.cc")
            .write_str("int x() { return 1; }\n")
            .unwrap();
        let assertion = cabin()
            .args(["test", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .arg("--allow-no-tests")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        assert!(
            stdout.contains("no test targets found"),
            "expected explanatory line, got: {stdout}"
        );
    }

    #[test]
    fn cabin_test_runs_in_deterministic_package_then_target_order() {
        if !build_tools_available() {
            skip(
                "cabin_test_runs_in_deterministic_package_then_target_order",
                "ninja or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        // Workspace with two members; member `b` declares its
        // tests *before* member `a` in TOML order, but the runner
        // must sort by package then target.
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/b", "packages/a"]
"#,
            )
            .unwrap();
        for (member, deps_table) in [("a", "[target.a_z_test]"), ("b", "[target.b_a_test]")] {
            assert_fs::fixture::ChildPath::new(
                dir.path().join(format!("packages/{member}/cabin.toml")),
            )
            .write_str(&format!(
                r#"[package]
name = "{member}"
version = "0.1.0"

[target.{member}]
type = "cpp_library"
sources = ["src/lib.cc"]

{deps_table}
type = "cpp_test"
sources = ["tests/lib_test.cc"]
deps = ["{member}"]
"#
            ))
            .unwrap();
            dir.child(format!("packages/{member}/src/lib.cc"))
                .write_str("int x() { return 0; }\n")
                .unwrap();
            dir.child(format!("packages/{member}/tests/lib_test.cc"))
                .write_str("int main() { return 0; }\n")
                .unwrap();
        }
        let assertion = cabin()
            .args(["test", "--workspace", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        // Both tests must appear, with `a:a_z_test` before
        // `b:b_a_test` regardless of TOML declaration order.
        let a_pos = stdout
            .find("test a:a_z_test ... ok")
            .unwrap_or_else(|| panic!("missing a:a_z_test in: {stdout}"));
        let b_pos = stdout
            .find("test b:b_a_test ... ok")
            .unwrap_or_else(|| panic!("missing b:b_a_test in: {stdout}"));
        assert!(
            a_pos < b_pos,
            "tests must run in (package, target) ascending order; got: {stdout}"
        );
    }

    #[test]
    fn cabin_test_rejects_target_flag_as_unknown_argument() {
        // `cabin test` mirrors `cabin build`: the historic
        // `--target` manifest-target selector is gone, with the
        // flag name reserved for a future platform/toolchain
        // target. clap must reject the flag at parse time so the
        // overload cannot creep back in.
        cabin()
            .args(["test", "--target", "foo"])
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains(
                "unexpected argument '--target' found",
            ));
    }

    #[test]
    fn package_archive_includes_test_and_example_sources() {
        let dir = project_with_dev_kinds();
        let out = dir.path().join("dist");
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--output-dir"])
            .arg(&out)
            .assert()
            .success();
        // The archive must carry every declared source — including
        // dev-only target sources — so the package round-trips.
        let archive = out.join("demo-0.1.0.tar.gz");
        let bytes = fs::read(&archive).expect("archive readable");
        let listing = list_tar_gz_paths(&bytes);
        for expected in ["src/lib.cc", "tests/lib_test.cc", "examples/hello.cc"] {
            assert!(
                listing.iter().any(|p| p.ends_with(expected)),
                "archive missing {expected}; got: {listing:?}"
            );
        }
    }

    fn list_tar_gz_paths(bytes: &[u8]) -> Vec<String> {
        let decoder = flate2::read::GzDecoder::new(bytes);
        let mut archive = tar::Archive::new(decoder);
        archive
            .entries()
            .expect("entries iterator")
            .map(|e| {
                e.expect("entry")
                    .path()
                    .expect("path")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

mod c_language {
    //! End-to-end coverage for first-class C support.
    //!
    //! These tests exercise the C / C++ source-language model
    //! across the manifest parser, build planner, Ninja
    //! generator, and `cabin test`. Each test stages a small
    //! temp package rather than depending on a fixed fixture
    //! tree so failures point at the actual source / manifest
    //! that broke.

    use super::*;

    fn write_c_only_library(dir: &Path) {
        assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
            .write_str(
                r#"[package]
name = "cdemo"
version = "0.1.0"

[target.cdemo]
type = "cpp_library"
sources = ["src/lib.c"]
include_dirs = ["include"]

[target.runner]
type = "cpp_executable"
sources = ["src/main.c"]
deps = ["cdemo"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("include/cdemo.h"))
            .write_str("#pragma once\nint cdemo(void);\n")
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("src/lib.c"))
            .write_str("#include \"cdemo.h\"\nint cdemo(void) { return 7; }\n")
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("src/main.c"))
            .write_str("#include \"cdemo.h\"\nint main(void) { return cdemo() == 7 ? 0 : 1; }\n")
            .unwrap();
    }

    fn write_mixed_library(dir: &Path) {
        assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
            .write_str(
                r#"[package]
name = "mixed"
version = "0.1.0"

[target.mixedlib]
type = "cpp_library"
sources = ["src/c_part.c", "src/cpp_part.cc"]
include_dirs = ["include"]

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
deps = ["mixedlib"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("include/mixed.h"))
            .write_str("#pragma once\n#ifdef __cplusplus\nextern \"C\" {\n#endif\nint c_value(void);\n#ifdef __cplusplus\n}\n#endif\nint cpp_value();\n")
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("src/c_part.c"))
            .write_str("#include \"mixed.h\"\nint c_value(void) { return 21; }\n")
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("src/cpp_part.cc"))
            .write_str("#include \"mixed.h\"\nint cpp_value() { return 21; }\n")
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("src/main.cc"))
            .write_str("#include \"mixed.h\"\nint main() { return (c_value() + cpp_value()) == 42 ? 0 : 1; }\n")
            .unwrap();
    }

    #[test]
    fn metadata_reports_target_kinds_for_c_only_project() {
        let dir = TempDir::new().unwrap();
        write_c_only_library(dir.path());
        let value = run_metadata(&dir.path().join("cabin.toml"));
        let pkg = package_in(&value, "cdemo");
        let target_kinds: std::collections::BTreeMap<String, String> = pkg["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t["name"].as_str().unwrap().to_owned(),
                    t["kind"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            target_kinds.get("cdemo").map(String::as_str),
            Some("cpp_library")
        );
        assert_eq!(
            target_kinds.get("runner").map(String::as_str),
            Some("cpp_executable")
        );
    }

    #[test]
    fn build_c_only_project_emits_c_compile_rule_and_c_link_driver() {
        if !c_and_cxx_build_tools_available() {
            skip(
                "build_c_only_project_emits_c_compile_rule_and_c_link_driver",
                "ninja, a C compiler, or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_c_only_library(dir.path());
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
        // Only the C compile rule is exercised on a pure-C package.
        // The link line must use the C compiler driver — never `c++`
        // — so the binary stays off the C++ runtime.
        assert!(
            ninja.contains("c_compile"),
            "expected c_compile rule to be referenced: {ninja}"
        );
        assert!(
            !ninja
                .lines()
                .any(|l| l.contains("cxx_compile") && l.starts_with("build ")),
            "no cxx_compile build edges expected for pure-C package: {ninja}"
        );
        // Link command line: must include `cc` (or `clang` / `gcc`)
        // not `c++` / `clang++` / `g++`.
        let link_line = ninja
            .lines()
            .find(|l| l.contains("link_executable") && l.contains("/runner"))
            .expect("link edge for runner");
        let next = ninja
            .lines()
            .skip_while(|l| *l != link_line)
            .nth(1)
            .expect("link edge has a command line");
        assert!(
            !next.contains("c++") && !next.contains("g++") && !next.contains("clang++"),
            "C-only link must not use a C++ driver, got: {next}"
        );
    }

    #[test]
    fn build_mixed_project_uses_cxx_link_driver_when_any_object_is_cxx() {
        if !c_and_cxx_build_tools_available() {
            skip(
                "build_mixed_project_uses_cxx_link_driver_when_any_object_is_cxx",
                "ninja, a C compiler, or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_mixed_library(dir.path());
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
        // Both compile rules are exercised — one per language.
        assert!(
            ninja.contains("c_compile"),
            "expected a C compile edge for mixed package: {ninja}"
        );
        assert!(
            ninja.contains("cxx_compile"),
            "expected a C++ compile edge for mixed package: {ninja}"
        );
        // Link line must use the C++ driver because the closure
        // contains a C++ object.
        let link_line = ninja
            .lines()
            .find(|l| l.contains("link_executable") && l.ends_with("libmixedlib.a"))
            .expect("link edge for app");
        let cmd = ninja
            .lines()
            .skip_while(|l| *l != link_line)
            .nth(1)
            .expect("link edge has a command line");
        assert!(
            cmd.contains("c++") || cmd.contains("g++") || cmd.contains("clang++"),
            "mixed link must use a C++ driver, got: {cmd}"
        );
    }

    #[test]
    fn link_driver_path_matches_resolved_cc_path_for_pure_c_target() {
        // Structural variant of
        // `build_c_only_project_emits_c_compile_rule_and_c_link_driver`:
        // instead of pattern-matching driver-name substrings
        // (`cc` / `clang` / `gcc`) on the link command, this
        // test reads the resolved CC path from `cabin metadata`
        // and asserts the link command's first argument equals
        // it. Decouples the assertion from how the host names
        // its C compiler.
        if !c_and_cxx_build_tools_available() {
            skip(
                "link_driver_path_matches_resolved_cc_path_for_pure_c_target",
                "ninja, a C compiler, or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_c_only_library(dir.path());
        // First, ask metadata for the resolved toolchain so the
        // assertion below knows the host's *actual* CC path.
        let metadata = run_metadata(&dir.path().join("cabin.toml"));
        // The resolved CC path lives under
        // `toolchain.detected.cc.path` — `toolchain.tools.cc`
        // carries the user-visible spec / source / kind, while
        // `toolchain.detected.cc.path` is the absolute path the
        // planner threads into the build graph.
        let cc_path = metadata["toolchain"]["detected"]["cc"]["path"]
            .as_str()
            .expect("metadata must report a resolved cc path on this host");
        // Then build, and inspect the link edge's command.
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
        let link_cmds = compile_command_lines_for_rule(&ninja, "link_executable");
        assert_eq!(link_cmds.len(), 1, "expected one link edge");
        let link_argv: Vec<&str> = link_cmds[0].split_whitespace().collect();
        assert_eq!(
            link_argv[0], cc_path,
            "pure-C target must link with the resolved C compiler, got: {}",
            link_cmds[0]
        );
    }

    #[test]
    fn cabin_test_runs_pure_c_test_executable() {
        if !c_and_cxx_build_tools_available() {
            skip(
                "cabin_test_runs_pure_c_test_executable",
                "ninja, a C compiler, or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "cdemo"
version = "0.1.0"

[target.cdemo]
type = "cpp_library"
sources = ["src/lib.c"]
include_dirs = ["include"]

[target.cdemo_test]
type = "cpp_test"
sources = ["tests/lib_test.c"]
deps = ["cdemo"]
"#,
            )
            .unwrap();
        dir.child("include/cdemo.h")
            .write_str("#pragma once\nint cdemo(void);\n")
            .unwrap();
        dir.child("src/lib.c")
            .write_str("#include \"cdemo.h\"\nint cdemo(void) { return 9; }\n")
            .unwrap();
        dir.child("tests/lib_test.c")
            .write_str("#include \"cdemo.h\"\nint main(void) { return cdemo() == 9 ? 0 : 1; }\n")
            .unwrap();
        let assertion = cabin()
            .args(["test", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        assert!(
            stdout.contains("test cdemo:cdemo_test ... ok"),
            "expected passing C test, got: {stdout}"
        );
    }

    #[test]
    fn unrecognized_source_extension_is_rejected() {
        // Cabin rejects an unrecognized source extension during
        // build planning, before any compile is invoked.
        // Toolchain validation does run before the planner,
        // though, so a C++ compiler must be present on PATH.
        if !build_tools_available() {
            skip(
                "unrecognized_source_extension_is_rejected",
                "ninja or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "broken"
version = "0.1.0"

[target.broken]
type = "cpp_library"
sources = ["src/file.txt"]
"#,
            )
            .unwrap();
        dir.child("src/file.txt")
            .write_str("not a source\n")
            .unwrap();
        let assertion = cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("unrecognized extension"),
            "expected explicit extension diagnostic, got: {stderr}"
        );
        assert!(
            stderr.contains(".c") && stderr.contains(".cc"),
            "diagnostic should list supported extensions, got: {stderr}"
        );
    }

    #[test]
    fn cflags_and_cxxflags_do_not_leak_across_languages() {
        if !c_and_cxx_build_tools_available() {
            skip(
                "cflags_and_cxxflags_do_not_leak_across_languages",
                "ninja, a C compiler, or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "splitflags"
version = "0.1.0"

[profile]
cflags = ["-DCABIN_TEST_C_FLAG=1"]
cxxflags = ["-DCABIN_TEST_CXX_FLAG=1"]

[target.splitflags]
type = "cpp_library"
sources = ["src/c_part.c", "src/cpp_part.cc"]
"#,
            )
            .unwrap();
        dir.child("src/c_part.c")
            .write_str("int c_part_value(void) { return 0; }\n")
            .unwrap();
        dir.child("src/cpp_part.cc")
            .write_str("int cpp_part_value() { return 0; }\n")
            .unwrap();
        cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
        // Locate compile command lines by walking the build edges
        // and inspecting the rule name on each `build` line.
        // Anchoring on the rule (rather than on a hardcoded
        // standard flag like `-std=c11`) keeps the test stable if
        // the planner's default standard ever changes.
        let c_compile_lines = compile_command_lines_for_rule(&ninja, "c_compile");
        let cxx_compile_lines = compile_command_lines_for_rule(&ninja, "cxx_compile");
        assert!(
            !c_compile_lines.is_empty(),
            "expected at least one c_compile edge: {ninja}"
        );
        assert!(
            !cxx_compile_lines.is_empty(),
            "expected at least one cxx_compile edge: {ninja}"
        );
        for line in &c_compile_lines {
            assert!(
                line.contains("-DCABIN_TEST_C_FLAG=1"),
                "C compile must include the C-only define, got: {line}"
            );
            assert!(
                !line.contains("-DCABIN_TEST_CXX_FLAG=1"),
                "C-only define must NOT leak into the C++ compile, got: {line}"
            );
        }
        for line in &cxx_compile_lines {
            assert!(
                line.contains("-DCABIN_TEST_CXX_FLAG=1"),
                "C++ compile must include the C++-only define, got: {line}"
            );
            assert!(
                !line.contains("-DCABIN_TEST_C_FLAG=1"),
                "C++-only define must NOT leak into the C compile, got: {line}"
            );
        }
    }

    #[test]
    fn cabin_test_runs_cpp_test_depending_on_c_library() {
        // A C++ test target consumes a pure-C library through the
        // ordinary `[target.X].deps` mechanism. The build planner
        // must compile each source through its language-appropriate
        // driver and link the test executable through the C++
        // driver.
        if !c_and_cxx_build_tools_available() {
            skip(
                "cabin_test_runs_cpp_test_depending_on_c_library",
                "ninja or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "interop"
version = "0.1.0"

[target.clib]
type = "cpp_library"
sources = ["src/clib.c"]
include_dirs = ["include"]

[target.cpp_test]
type = "cpp_test"
sources = ["tests/clib_test.cc"]
deps = ["clib"]
"#,
            )
            .unwrap();
        dir.child("include/clib.h")

            .write_str("#pragma once\n#ifdef __cplusplus\nextern \"C\" {\n#endif\nint c_value(void);\n#ifdef __cplusplus\n}\n#endif\n")

            .unwrap();
        dir.child("src/clib.c")
            .write_str("#include \"clib.h\"\nint c_value(void) { return 99; }\n")
            .unwrap();
        dir.child("tests/clib_test.cc")
            .write_str("#include \"clib.h\"\nint main() { return c_value() == 99 ? 0 : 1; }\n")
            .unwrap();
        let assertion = cabin()
            .args(["test", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        assert!(
            stdout.contains("test interop:cpp_test ... ok"),
            "expected passing C++ test that consumes a C library, got: {stdout}"
        );
        // Both compile rules must have been used and the link
        // must use a C++ driver because the test sources are C++.
        let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
        let c_compile_lines = compile_command_lines_for_rule(&ninja, "c_compile");
        let cxx_compile_lines = compile_command_lines_for_rule(&ninja, "cxx_compile");
        assert!(!c_compile_lines.is_empty(), "expected C compile edge");
        assert!(!cxx_compile_lines.is_empty(), "expected C++ compile edge");
        let link_cmds = compile_command_lines_for_rule(&ninja, "link_executable");
        assert_eq!(link_cmds.len(), 1, "expected one link edge");
        let link = &link_cmds[0];
        assert!(
            link.contains("c++") || link.contains("g++") || link.contains("clang++"),
            "C++ test target must link with a C++ driver, got: {link}"
        );
    }

    #[test]
    fn cabin_test_runs_mixed_c_and_cpp_tests_in_deterministic_order() {
        // A workspace with two test targets — one C, one C++ —
        // must run in `(package, target)` ascending order
        // regardless of TOML declaration order.
        if !c_and_cxx_build_tools_available() {
            skip(
                "cabin_test_runs_mixed_c_and_cpp_tests_in_deterministic_order",
                "ninja, a C compiler, or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "mixedtests"
version = "0.1.0"

[target.zz_cpp_test]
type = "cpp_test"
sources = ["tests/zz_cpp.cc"]

[target.aa_c_test]
type = "cpp_test"
sources = ["tests/aa_c.c"]
"#,
            )
            .unwrap();
        dir.child("tests/zz_cpp.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        dir.child("tests/aa_c.c")
            .write_str("int main(void) { return 0; }\n")
            .unwrap();
        let assertion = cabin()
            .args(["test", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
        let aa_pos = stdout
            .find("test mixedtests:aa_c_test ... ok")
            .expect("aa_c_test result must be present");
        let zz_pos = stdout
            .find("test mixedtests:zz_cpp_test ... ok")
            .expect("zz_cpp_test result must be present");
        assert!(
            aa_pos < zz_pos,
            "tests must run in (package, target) ascending order regardless of language; got: {stdout}"
        );
    }

    #[test]
    fn missing_c_compiler_yields_actionable_diagnostic() {
        // Cabin's toolchain resolver requires a C++ compiler
        // unconditionally; this test points `--cc` at a path
        // that does not exist so we can observe the
        // user-visible diagnostic without depending on the
        // host's `cc` / `clang` / `gcc` PATH state.
        if !build_tools_available() {
            skip(
                "missing_c_compiler_yields_actionable_diagnostic",
                "ninja or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "needscc"
version = "0.1.0"

[target.needscc]
type = "cpp_library"
sources = ["src/lib.c"]
"#,
            )
            .unwrap();
        dir.child("src/lib.c")
            .write_str("int needscc_value(void) { return 0; }\n")
            .unwrap();
        // Build a non-existent path inside the temp dir so the
        // test does not depend on a hardcoded host-specific
        // path like `/this/path/does/not/exist/cc`. The path
        // simply must not resolve to an executable; nothing
        // here is invoked.
        let missing_cc = dir.path().join("missing-cc");
        let assertion = cabin()
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .arg("--cc")
            .arg(&missing_cc)
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("C compiler") || stderr.contains("`cc`"),
            "expected error to mention the C compiler, got: {stderr}"
        );
    }

    /// Return the `command = ...` lines for every Ninja edge
    /// whose rule equals `rule_name`. The returned slices are
    /// owned `String`s for ergonomics. Anchoring on the rule
    /// name decouples assertions from incidental command-line
    /// content (standard flag, optimization level, etc.).
    fn compile_command_lines_for_rule(ninja: &str, rule_name: &str) -> Vec<String> {
        let needle = format!(": {rule_name} ");
        let mut out: Vec<String> = Vec::new();
        let mut lines = ninja.lines();
        while let Some(line) = lines.next() {
            if !line.starts_with("build ") || !line.contains(&needle) {
                continue;
            }
            // The next non-blank line of an edge starts with
            // `  command = ...`. Walk forward until we find it,
            // stopping at the next blank line that terminates
            // the edge so a malformed `build.ninja` doesn't
            // silently hide regressions.
            for inner in lines.by_ref() {
                if inner.is_empty() {
                    break;
                }
                if let Some(rest) = inner.strip_prefix("  command = ") {
                    out.push(rest.to_owned());
                    break;
                }
            }
        }
        out
    }
}

mod vendor_offline {
    //! End-to-end coverage for `cabin vendor` and `--offline`
    //! mode. Each test stages a real `.tar.gz` archive plus the
    //! file-registry index that publishes its checksum, so the
    //! tests exercise the full vendor → offline build pipeline.

    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use sha2::Digest;
    use std::path::PathBuf;

    /// Build a `.tar.gz` containing the given `(relative_path,
    /// body)` entries and return the archive's `sha256` hex.
    fn make_archive(path: &Path, entries: &[(&str, &str)]) -> String {
        if let Some(parent) = path.parent() {
            assert_fs::fixture::ChildPath::new(parent)
                .create_dir_all()
                .unwrap();
        }
        let f = std::fs::File::create(path).unwrap();
        let enc = GzEncoder::new(f, Compression::default());
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
        use std::io::Write;
        enc.finish().unwrap().flush().unwrap();
        let bytes = fs::read(path).unwrap();
        let mut hasher = sha2::Sha256::new();
        hasher.update(&bytes);
        format!("{:x}", hasher.finalize())
    }

    /// Stage a one-package file-registry index at `<root>/index`
    /// containing a single `fmt 10.2.1` entry. Returns the
    /// directory the index lives in.
    fn stage_fmt_index(root: &Path) -> PathBuf {
        let index = root.join("index");
        assert_fs::fixture::ChildPath::new(index.join("config.json"))

            .write_str("{\"schema\":1,\"kind\":\"file-registry\",\"packages\":\"packages\",\"artifacts\":\"artifacts\"}\n")

            .unwrap();
        let archive = index.join("artifacts/fmt/fmt-10.2.1.tar.gz");
        let manifest = "[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n\n[target.fmt]\ntype = \"cpp_library\"\nsources = [\"src/fmt.cc\"]\ninclude_dirs = [\"include\"]\n";
        let header = "#pragma once\nint fmt_value();\n";
        let body = "#include \"fmt.h\"\nint fmt_value() { return 42; }\n";
        let checksum = make_archive(
            &archive,
            &[
                ("cabin.toml", manifest),
                ("include/fmt.h", header),
                ("src/fmt.cc", body),
            ],
        );
        let entry = format!(
            "{{\n  \"schema\": 1,\n  \"name\": \"fmt\",\n  \"versions\": {{\n    \"10.2.1\": {{\n      \"dependencies\": {{}},\n      \"yanked\": false,\n      \"checksum\": \"sha256:{checksum}\",\n      \"source\": {{\"type\": \"archive\", \"path\": \"../artifacts/fmt/fmt-10.2.1.tar.gz\", \"format\": \"tar.gz\"}}\n    }}\n  }}\n}}\n",
        );
        assert_fs::fixture::ChildPath::new(index.join("packages/fmt.json"))
            .write_str(&entry)
            .unwrap();
        index
    }

    /// Stage a small consuming package that depends on `fmt
    /// 10.2.1`. Includes a working `main.cc` so a follow-up
    /// `cabin build` can succeed.
    fn stage_consumer_project(root: &Path) {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = "10.2.1"

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
deps = ["fmt"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
            .write_str(
                "extern int fmt_value();\nint main() { return fmt_value() == 42 ? 0 : 1; }\n",
            )
            .unwrap();
    }

    #[test]
    fn vendor_writes_deterministic_file_registry() {
        let dir = TempDir::new().unwrap();
        let index = stage_fmt_index(dir.path());
        stage_consumer_project(&dir.path().join("proj"));
        cabin()
            .args(["vendor", "--manifest-path"])
            .arg(dir.path().join("proj/cabin.toml"))
            .arg("--vendor-dir")
            .arg(dir.path().join("proj/vendor"))
            .arg("--index-path")
            .arg(&index)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .assert()
            .success();
        let vendor = dir.path().join("proj/vendor");
        // file-registry skeleton + artifact + per-package index +
        // vendor summary.
        assert!(vendor.join("config.json").is_file());
        assert!(vendor.join("packages/fmt.json").is_file());
        assert!(vendor.join("artifacts/fmt/fmt-10.2.1.tar.gz").is_file());
        assert!(vendor.join("cabin-vendor.json").is_file());

        // The vendored per-package index points at the *vendor's*
        // relative archive path, not at the source index's. This
        // is what makes the directory portable.
        let body = fs::read_to_string(vendor.join("packages/fmt.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let path = parsed["versions"]["10.2.1"]["source"]["path"]
            .as_str()
            .unwrap();
        assert_eq!(path, "../artifacts/fmt/fmt-10.2.1.tar.gz");

        // Re-running with the same inputs must be byte-identical.
        let summary = fs::read(vendor.join("cabin-vendor.json")).unwrap();
        cabin()
            .args(["vendor", "--manifest-path"])
            .arg(dir.path().join("proj/cabin.toml"))
            .arg("--vendor-dir")
            .arg(&vendor)
            .arg("--index-path")
            .arg(&index)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .assert()
            .success();
        let summary_again = fs::read(vendor.join("cabin-vendor.json")).unwrap();
        assert_eq!(summary, summary_again);
    }

    #[test]
    fn vendor_then_offline_build_links_against_the_vendored_dependency() {
        if !build_tools_available() {
            skip(
                "vendor_then_offline_build_links_against_the_vendored_dependency",
                "ninja or a C++ compiler is unavailable on PATH",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        let index = stage_fmt_index(dir.path());
        stage_consumer_project(&dir.path().join("proj"));

        cabin()
            .args(["vendor", "--manifest-path"])
            .arg(dir.path().join("proj/cabin.toml"))
            .arg("--vendor-dir")
            .arg(dir.path().join("proj/vendor"))
            .arg("--index-path")
            .arg(&index)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .assert()
            .success();

        // Offline build using only the vendored directory and a
        // fresh cache. Must NOT touch the source index (we
        // delete it to be sure).
        fs::remove_dir_all(&index).unwrap();
        cabin()
            .args(["build", "--offline", "--manifest-path"])
            .arg(dir.path().join("proj/cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("proj/build"))
            .arg("--index-path")
            .arg(dir.path().join("proj/vendor"))
            .arg("--cache-dir")
            .arg(dir.path().join("vendor-cache"))
            .assert()
            .success();
        let exe = dir.path().join("proj/build/dev/packages/app/app");
        assert!(exe.is_file(), "offline build must link the executable");
    }

    #[test]
    fn offline_rejects_index_url() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "lone"
version = "0.1.0"
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args(["build", "--offline", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--index-url")
            .arg("https://example.com/index")
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("--offline forbids network access"),
            "expected offline rejection, got: {stderr}"
        );
        assert!(
            stderr.contains("https://example.com/index"),
            "diagnostic should name the rejected URL, got: {stderr}"
        );
    }

    #[test]
    fn vendor_with_no_versioned_deps_writes_skeleton_only() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "lone"
version = "0.1.0"

[target.lone]
type = "cpp_library"
sources = ["src/lib.cc"]
"#,
            )
            .unwrap();
        dir.child("src/lib.cc")
            .write_str("int lone_value() { return 0; }\n")
            .unwrap();
        let vendor = dir.path().join("vendor");
        cabin()
            .args(["vendor", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--vendor-dir")
            .arg(&vendor)
            .assert()
            .success();
        // Empty plan still writes the file-registry skeleton so a
        // follow-up `cabin build --offline --index-path ./vendor`
        // can be a no-op rather than an error.
        assert!(vendor.join("config.json").is_file());
        assert!(vendor.join("cabin-vendor.json").is_file());
        assert!(vendor.join("packages").is_dir());
        assert!(vendor.join("artifacts").is_dir());
    }

    #[test]
    fn vendor_locked_succeeds_when_lockfile_is_current() {
        // First a vanilla vendor run writes both the lockfile
        // and the vendor directory, then a follow-up `--locked`
        // run must succeed without rewriting the lockfile.
        let dir = TempDir::new().unwrap();
        let index = stage_fmt_index(dir.path());
        stage_consumer_project(&dir.path().join("proj"));
        cabin()
            .args(["vendor", "--manifest-path"])
            .arg(dir.path().join("proj/cabin.toml"))
            .arg("--vendor-dir")
            .arg(dir.path().join("proj/vendor"))
            .arg("--index-path")
            .arg(&index)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .assert()
            .success();
        let lock_before = fs::read_to_string(dir.path().join("proj/cabin.lock")).unwrap();
        cabin()
            .args(["vendor", "--locked", "--manifest-path"])
            .arg(dir.path().join("proj/cabin.toml"))
            .arg("--vendor-dir")
            .arg(dir.path().join("proj/vendor"))
            .arg("--index-path")
            .arg(&index)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .assert()
            .success();
        let lock_after = fs::read_to_string(dir.path().join("proj/cabin.lock")).unwrap();
        assert_eq!(lock_before, lock_after);
    }
}

// ---------------------------------------------------------------------------
// cabin tree + cabin explain
// ---------------------------------------------------------------------------

mod metadata_tree_explain {
    use super::*;

    /// Workspace fixture: an `app` (cpp_executable) that depends on
    /// a path-local `lib` (cpp_library). Used to exercise tree
    /// rendering and explain queries that need at least one
    /// dependency edge.
    fn write_app_with_path_dep(dir: &Path) {
        assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
            .write_str(
                r#"[workspace]
members = ["app", "lib"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("app/cabin.toml"))
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
lib = { path = "../lib" }

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("app/src/main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("lib/cabin.toml"))
            .write_str(
                r#"[package]
name = "lib"
version = "0.1.0"

[target.lib]
type = "cpp_library"
sources = ["src/lib.cc"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("lib/src/lib.cc"))
            .write_str("int lib_value() { return 1; }\n")
            .unwrap();
    }

    #[test]
    fn tree_human_format_default_emits_box_drawing() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let output = cabin()
            .args(["tree", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        // Both packages must appear, app as a workspace root with
        // its workspace label, lib as a normal-kind child of app.
        assert!(stdout.contains("app v0.1.0"), "got: {stdout}");
        assert!(stdout.contains("lib v0.1.0"), "got: {stdout}");
        assert!(stdout.contains("[normal]"), "got: {stdout}");
        // Box-drawing must be emitted for the child edge.
        assert!(
            stdout.contains("└── lib") || stdout.contains("├── lib"),
            "got: {stdout}"
        );
    }

    #[test]
    fn tree_json_format_is_valid_structured_document() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let output = cabin()
            .args(["tree", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|err| panic!("expected valid JSON, got error {err} for: {stdout}"));
        let arr = value.as_array().expect("forest must be a JSON array");
        let app = arr
            .iter()
            .find(|n| n["name"] == "app")
            .expect("app must be a root in tree forest");
        assert_eq!(app["version"], "0.1.0");
        // Source provenance is a tagged enum; the workspace
        // member case has no extra fields.
        assert_eq!(app["source"]["kind"], "workspace-member");
        let children = app["children"].as_array().expect("children must be array");
        assert_eq!(children.len(), 1, "app should have exactly one child");
        assert_eq!(children[0]["name"], "lib");
        assert_eq!(children[0]["edge_kind"], "normal");
    }

    #[test]
    fn tree_default_roots_honor_workspace_default_members() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["app", "lib"]
default-members = ["app"]
"#,
            )
            .unwrap();

        let output = cabin()
            .args(["tree", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let roots = value.as_array().expect("forest must be a JSON array");
        let root_names: Vec<&str> = roots
            .iter()
            .map(|node| node["name"].as_str().unwrap())
            .collect();

        assert_eq!(
            root_names,
            vec!["app"],
            "implicit tree selection should use default-members, got: {stdout}"
        );
        assert_eq!(roots[0]["children"][0]["name"], "lib");
    }

    #[test]
    fn tree_kind_filter_restricts_to_normal_edges() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let output = cabin()
            .args(["tree", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--kind", "normal"])
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let app = value
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == "app")
            .expect("app must be present");
        let children = app["children"].as_array().unwrap();
        assert!(
            children.iter().all(|c| c["edge_kind"] == "normal"),
            "--kind normal should restrict to normal edges, got: {children:?}"
        );
    }

    #[test]
    fn explain_package_marks_selected_root() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let output = cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json", "package", "app"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value["kind"], "package");
        assert_eq!(value["name"], "app");
        assert_eq!(value["is_selected_root"], true);
        assert_eq!(value["source"]["kind"], "workspace-member");
    }

    #[test]
    fn explain_package_traces_dep_path_from_root() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        // Constrain selection to `app` so the only reachable
        // path to lib is via `app -> lib`. Without this the
        // workspace's other primary package (lib itself) would
        // contribute a length-1 self-path that sorts first.
        let output = cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--package", "app"])
            .args(["--format", "json", "package", "lib"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let paths = value["paths"].as_array().unwrap();
        assert!(!paths.is_empty(), "lib must be reachable from a root");
        let first = paths[0].as_array().unwrap();
        assert_eq!(first[0]["name"], "app");
        assert_eq!(first[1]["name"], "lib");
        assert_eq!(first[1]["edge_kind"], "normal");
    }

    #[test]
    fn explain_unknown_package_returns_diagnostic() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let assertion = cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["package", "missing"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("`missing`") && stderr.contains("not found"),
            "expected package-not-found diagnostic, got: {stderr}"
        );
        assert!(
            stderr.contains("cabin::explain::error"),
            "the typed `ExplainError` must reach the diagnostic dispatcher so the stable code is emitted, got: {stderr}",
        );
    }

    #[test]
    fn explain_target_reports_languages_and_kind() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let output = cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json", "target", "lib"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        // Outer tag (the Explanation discriminator).
        assert_eq!(value["kind"], "target");
        assert_eq!(value["package"], "lib");
        assert_eq!(value["target"], "lib");
        // Inner target_kind field carries the Cabin TargetKind
        // string. Renamed from `kind` so it does not collide
        // with the outer discriminator.
        assert_eq!(value["target_kind"], "cpp_library");
        assert!(value["is_buildable"].as_bool().unwrap());
        let langs = value["languages"].as_array().unwrap();
        assert!(
            langs.iter().any(|v| v == "cxx"),
            "expected cxx in languages, got: {langs:?}"
        );
    }

    #[test]
    fn explain_source_reports_workspace_member_provenance() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let output = cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json", "source", "app"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value["kind"], "source");
        assert_eq!(value["name"], "app");
        assert_eq!(value["source"]["kind"], "workspace-member");
    }

    #[test]
    fn explain_feature_query_without_separator_errors() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let assertion = cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["feature", "no-separator"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("`package/feature`"),
            "expected feature query diagnostic, got: {stderr}"
        );
    }

    #[test]
    fn explain_build_config_emits_fingerprint_field() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let output = cabin()
            .args(["explain", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .args(["--format", "json", "build-config", "app"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(value["kind"], "build-config");
        assert_eq!(value["package"], "app");
        let cfg = &value["configuration"];
        assert!(
            cfg["fingerprint"].is_string(),
            "fingerprint must be present, got: {cfg}"
        );
        assert!(cfg["profile"].is_object());
    }

    #[test]
    fn tree_renders_deterministically_across_runs() {
        let dir = TempDir::new().unwrap();
        write_app_with_path_dep(dir.path());
        let manifest = dir.path().join("cabin.toml");
        let first = cabin()
            .args(["tree", "--manifest-path"])
            .arg(&manifest)
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        let second = cabin()
            .args(["tree", "--manifest-path"])
            .arg(&manifest)
            .args(["--format", "json"])
            .assert()
            .success()
            .get_output()
            .clone();
        assert_eq!(
            first.stdout, second.stdout,
            "tree output must be byte-stable"
        );
    }
}

// ---------------------------------------------------------------------------
// cabin run + CABIN_* env vars
// ---------------------------------------------------------------------------

mod cargo_interface {
    use super::*;
    use std::path::PathBuf;

    /// `main.cc` that prints argv (one arg per line) plus the
    /// injected `CABIN_PACKAGE_NAME` / `CABIN_PROFILE` env vars
    /// so a single run can confirm `--` arg forwarding and the
    /// `CABIN_*` overlay together.
    const ARGV_AND_ENV_MAIN_CC: &str = r#"
#include <cstdio>
#include <cstdlib>
int main(int argc, char** argv) {
    for (int i = 1; i < argc; ++i) {
        std::printf("ARG %d %s\n", i, argv[i]);
    }
    if (const char* p = std::getenv("CABIN_PACKAGE_NAME")) {
        std::printf("PKG %s\n", p);
    }
    if (const char* p = std::getenv("CABIN_PROFILE")) {
        std::printf("PROFILE %s\n", p);
    }
    return 0;
}
"#;

    fn write_run_fixture(root: &Path, package: &str, target: &str) {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(&format!(
                r#"[package]
name = "{package}"
version = "0.1.0"

[target.{target}]
type = "cpp_executable"
sources = ["src/main.cc"]
"#
            ))
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
            .write_str(ARGV_AND_ENV_MAIN_CC)
            .unwrap();
    }

    #[test]
    fn run_executes_default_binary_and_forwards_trailing_args() {
        if !ninja_available() || !cxx_compiler_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        write_run_fixture(dir.path(), "demo", "demo_app");
        let output = cabin()
            .args(["run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .args(["--", "first", "second"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("ARG 1 first"), "got: {stdout}");
        assert!(stdout.contains("ARG 2 second"), "got: {stdout}");
        assert!(stdout.contains("PKG demo"), "got: {stdout}");
        assert!(stdout.contains("PROFILE dev"), "got: {stdout}");
    }

    #[test]
    fn run_does_not_materialize_dev_dependencies() {
        if !build_tools_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[dev-dependencies]
test_only = "1.0.0"

[target.demo]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc")
            .write_str(ARGV_AND_ENV_MAIN_CC)
            .unwrap();

        let output = cabin()
            .args(["run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("PKG demo"), "got: {stdout}");
    }

    #[test]
    fn run_with_bin_flag_picks_named_target() {
        if !ninja_available() || !cxx_compiler_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        // Two cpp_executable targets with distinct sources;
        // --bin selects which one runs.
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "two-bins"
version = "0.1.0"

[target.alpha]
type = "cpp_executable"
sources = ["src/alpha.cc"]

[target.beta]
type = "cpp_executable"
sources = ["src/beta.cc"]
"#,
            )
            .unwrap();
        dir.child("src/alpha.cc")
            .write_str(
                "#include <cstdio>\nint main() { std::printf(\"WHICH alpha\\n\"); return 0; }\n",
            )
            .unwrap();
        dir.child("src/beta.cc")
            .write_str(
                "#include <cstdio>\nint main() { std::printf(\"WHICH beta\\n\"); return 0; }\n",
            )
            .unwrap();
        let output = cabin()
            .args(["run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .args(["--bin", "beta"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("WHICH beta"), "got: {stdout}");
    }

    #[test]
    fn run_in_pure_workspace_omits_fingerprint_env() {
        if !ninja_available() || !cxx_compiler_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[features]
default = []
fast = []

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("packages/app/src/main.cc")
            .write_str(ARGV_AND_ENV_MAIN_CC)
            .unwrap();
        let output = cabin()
            .args(["run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .args(["--package", "app", "--features", "fast"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains("PKG app"),
            "the selected package's binary should have run: {stdout}"
        );
        assert!(
            !stdout.contains("FINGERPRINT"),
            "CABIN_BUILD_CONFIGURATION_FINGERPRINT must not be injected: {stdout}"
        );
    }

    #[test]
    fn run_without_bin_when_multiple_executables_errors() {
        if !ninja_available() || !cxx_compiler_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "ambiguous"
version = "0.1.0"

[target.alpha]
type = "cpp_executable"
sources = ["src/main.cc"]

[target.beta]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let assertion = cabin()
            .args(["run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("multiple `cpp_executable` targets"),
            "expected ambiguous-executable error, got: {stderr}"
        );
    }

    #[test]
    fn run_unknown_bin_returns_actionable_error() {
        if !ninja_available() || !cxx_compiler_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        write_run_fixture(dir.path(), "demo", "demo_app");
        let assertion = cabin()
            .args(["run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .args(["--bin", "missing"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("`missing`") && stderr.contains("not found"),
            "expected --bin missing error, got: {stderr}"
        );
    }

    #[test]
    fn run_for_c_executable_target() {
        if !ninja_available() || !c_compiler_available() {
            eprintln!("test skipped: requires ninja + a C compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "c-app"
version = "0.1.0"

[target.c_app]
type = "cpp_executable"
sources = ["src/main.c"]
"#,
            )
            .unwrap();
        dir.child("src/main.c")

            .write_str("#include <stdio.h>\nint main(int argc, char** argv) { (void)argc; (void)argv; puts(\"c-ok\"); return 0; }\n")

            .unwrap();
        let output = cabin()
            .args(["run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("c-ok"), "got: {stdout}");
    }

    #[test]
    fn run_for_mixed_c_and_cpp_executable_target() {
        if !ninja_available() || !cxx_compiler_available() || !c_compiler_available() {
            eprintln!("test skipped: requires ninja + C and C++ compilers");
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "mixed-app"
version = "0.1.0"

[target.mixed_app]
type = "cpp_executable"
sources = ["src/main.cc", "src/util.c"]
"#,
            )
            .unwrap();
        dir.child("src/util.c")
            .write_str("int util_value(void) { return 42; }\n")
            .unwrap();
        dir.child("src/main.cc")
            .write_str(
                r#"#include <cstdio>
extern "C" int util_value();
int main() { std::printf("util=%d\n", util_value()); return 0; }
"#,
            )
            .unwrap();
        let output = cabin()
            .args(["run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("util=42"), "got: {stdout}");
    }

    #[test]
    fn cabin_build_dir_env_var_overrides_default_directory() {
        if !ninja_available() || !cxx_compiler_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        write_run_fixture(dir.path(), "envbin", "envbin");
        let custom_dir: PathBuf = dir.path().join("custom-build");
        cabin()
            .env("CABIN_BUILD_DIR", &custom_dir)
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .success();
        assert!(
            custom_dir.join("dev/build.ninja").is_file(),
            "expected build.ninja under {}",
            custom_dir.display()
        );
        assert!(
            !dir.path().join("build/dev/build.ninja").is_file(),
            "default `build/` should not have been created"
        );
    }

    #[test]
    fn cli_build_dir_flag_wins_over_cabin_build_dir_env() {
        if !ninja_available() || !cxx_compiler_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        write_run_fixture(dir.path(), "winflag", "winflag");
        let env_dir = dir.path().join("env-build");
        let cli_dir = dir.path().join("cli-build");
        cabin()
            .env("CABIN_BUILD_DIR", &env_dir)
            .args(["build", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(&cli_dir)
            .assert()
            .success();
        assert!(
            cli_dir.join("dev/build.ninja").is_file(),
            "CLI flag must win — expected build.ninja under {}",
            cli_dir.display()
        );
        assert!(
            !env_dir.exists(),
            "env-supplied build dir should not have been used"
        );
    }

    /// An explicit `--build-dir build` that happens to spell the
    /// built-in default literal must still beat `CABIN_BUILD_DIR`.
    /// Without value-source-aware detection the precedence check
    /// can't tell the user-supplied value from the clap default
    /// and silently routes outputs to the env-supplied directory.
    #[test]
    fn cli_build_dir_default_literal_wins_over_cabin_build_dir_env() {
        if !ninja_available() || !cxx_compiler_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        write_run_fixture(dir.path(), "explicit_default", "explicit_default");
        let env_dir = dir.path().join("env-build");
        cabin()
            .current_dir(dir.path())
            .env("CABIN_BUILD_DIR", &env_dir)
            .args(["build", "--build-dir", "build"])
            .assert()
            .success();
        assert!(
            dir.path().join("build/dev/build.ninja").is_file(),
            "explicit `--build-dir build` must win — expected build.ninja under build/dev/"
        );
        assert!(
            !env_dir.exists(),
            "env-supplied build dir should not have been used"
        );
    }

    #[test]
    fn cabin_net_offline_env_var_blocks_url_index() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "needs-fmt"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        let assertion = cabin()
            .env("CABIN_NET_OFFLINE", "1")
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--index-url")
            .arg("https://example.com/index")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("--offline forbids network access"),
            "expected offline diagnostic, got: {stderr}"
        );
    }

    #[test]
    fn invalid_cabin_net_offline_env_value_is_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "needs-fmt"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
            )
            .unwrap();
        dir.child("index/fmt.json").write_str(FMT_INDEX).unwrap();
        let assertion = cabin()
            .env("CABIN_NET_OFFLINE", "nope")
            .args(["resolve", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--index-path")
            .arg(dir.path().join("index"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("CABIN_NET_OFFLINE") && stderr.contains("nope"),
            "invalid offline env diagnostic should name the variable and value: {stderr}"
        );
    }

    #[test]
    fn cabin_run_exits_with_program_status_code() {
        if !ninja_available() || !cxx_compiler_available() {
            eprintln!("test skipped: requires ninja + a C++ compiler");
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "exit-code"
version = "0.1.0"

[target.exit_code]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() { return 42; }\n")
            .unwrap();
        let output = cabin()
            .args(["run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--build-dir")
            .arg(dir.path().join("build"))
            .assert()
            .failure()
            .get_output()
            .clone();
        assert_eq!(output.status.code(), Some(42));
    }
}

// ---------------------------------------------------------------------------
// post-Cargo-inspired-foundation help / env-var review tests
// ---------------------------------------------------------------------------

mod cargo_interface_cleanup {
    use super::*;

    /// Capture `cabin <args> --help` stdout for a focused
    /// substring assertion. Any non-zero exit fails the test.
    fn help_text(args: &[&str]) -> String {
        let mut full: Vec<&str> = args.to_vec();
        full.push("--help");
        let output = cabin().args(&full).assert().success().get_output().clone();
        String::from_utf8(output.stdout).expect("help output should be utf-8")
    }

    #[test]
    fn top_level_help_lists_day_to_day_subcommands() {
        let out = help_text(&[]);
        // `cabin --help` is curated to match cargo's `--help`
        // pattern: only commands users type day-to-day.  The
        // inspection, offline, scripting, packaging, and
        // distribution helpers live under `cabin --list`.  See
        // the dedicated `curated_help_and_list` module for the
        // full curation contract.  The visible set is derived
        // from clap (`Cli::command()`); we never hard-code it.
        for cmd in visible_subcommand_names() {
            assert!(out.contains(&cmd), "top-level help missing `{cmd}`:\n{out}");
        }
        // Cabin describes itself for C and C++, not Rust.
        assert!(
            out.contains("C and C++") || out.contains("C/C++"),
            "top-level help should mention C and C++:\n{out}"
        );
    }

    #[test]
    fn cabin_run_help_mentions_documented_flags() {
        let out = help_text(&["run"]);
        for needle in [
            "--bin",
            "--manifest-path",
            "--build-dir",
            "--profile",
            "--release",
            "--features",
            "--all-features",
            "--no-default-features",
            "--locked",
            "--frozen",
            "--offline",
            "--no-patches",
        ] {
            assert!(
                out.contains(needle),
                "`cabin run --help` missing `{needle}`:\n{out}"
            );
        }
    }

    #[test]
    fn cabin_build_help_uses_build_dir_not_target_dir() {
        let out = help_text(&["build"]);
        assert!(out.contains("--build-dir"), "build help: {out}");
        assert!(
            !out.contains("--target-dir"),
            "Cabin must not surface `--target-dir`:\n{out}"
        );
    }

    #[test]
    fn cabin_build_and_test_help_do_not_advertise_target_flag() {
        // `--target` is reserved for a future platform/toolchain
        // target. The historic manifest-target selector overload
        // is gone, so neither help screen should advertise the
        // flag.
        for sub in ["build", "test"] {
            let out = help_text(&[sub]);
            assert!(
                !out.contains("--target"),
                "`cabin {sub} --help` must not advertise `--target`:\n{out}"
            );
        }
    }

    #[test]
    fn cabin_metadata_help_lists_format_flag() {
        let out = help_text(&["metadata"]);
        assert!(out.contains("--format"), "metadata help: {out}");
    }

    #[test]
    fn cabin_tree_help_lists_kind_filter() {
        let out = help_text(&["tree"]);
        assert!(out.contains("--kind"), "tree help: {out}");
        assert!(out.contains("--format"), "tree help: {out}");
    }

    #[test]
    fn cabin_explain_help_lists_subcommand_set() {
        let out = help_text(&["explain"]);
        for sub in ["package", "target", "source", "feature", "build-config"] {
            assert!(out.contains(sub), "explain help missing `{sub}`:\n{out}");
        }
    }

    #[test]
    fn cabin_vendor_help_mentions_vendor_dir() {
        let out = help_text(&["vendor"]);
        assert!(
            out.contains("external versioned dependencies"),
            "vendor help should not claim to vendor local path deps: {out}"
        );
        assert!(out.contains("--vendor-dir"), "vendor help: {out}");
        assert!(out.contains("--offline"), "vendor help: {out}");
    }

    #[test]
    fn cabin_version_reports_workspace_release_version() {
        let output = cabin()
            .arg("--version")
            .assert()
            .success()
            .get_output()
            .clone();
        let stdout = String::from_utf8(output.stdout).expect("--version should be utf-8");
        // The workspace's `[workspace.package] version` drives
        // every crate's `--version`; if a future release bumps
        // it, this test must be updated alongside the bump.
        assert!(
            stdout.contains("0.14.0"),
            "expected `cabin --version` to mention the 0.14.0 release, got: {stdout}"
        );
    }

    #[test]
    fn cabin_about_describes_c_and_cpp_not_just_cpp() {
        let out = help_text(&[]);
        // Cabin's about text must describe the package as
        // serving both C and C++; the older "for C++" branding
        // is gone.
        assert!(
            out.contains("for C and C++"),
            "expected `--help` to describe Cabin for C and C++; got: {out}"
        );
    }
}

// ---------------------------------------------------------------------------
// Diagnostic / error-rendering refactor
// ---------------------------------------------------------------------------

mod diagnostics {
    use super::*;

    #[test]
    fn cli_sources_do_not_write_directly_to_stderr() {
        fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).expect("read source directory") {
                let entry = entry.expect("read source entry");
                let path = entry.path();
                if path.is_dir() {
                    collect_rs_files(&path, out);
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }

        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src_dir, &mut files);

        let mut offenders = Vec::new();
        for path in files {
            let body = fs::read_to_string(&path).expect("read source file");
            if body.contains("eprintln!(") {
                offenders.push(
                    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                );
            }
        }

        assert!(
            offenders.is_empty(),
            "production CLI sources must route human output through Reporter or cabin-diagnostics, not direct eprintln!: {offenders:#?}",
        );
    }

    /// Replace the absolute test-tempdir path in `text` with a
    /// stable placeholder so a golden assertion is byte-stable
    /// across CI / developer machines. macOS canonicalizes
    /// `/tmp/...` to `/private/tmp/...`, so we strip both
    /// prefixes.
    fn normalize(text: &str, tmpdir: &std::path::Path) -> String {
        let canonical = tmpdir
            .canonicalize()
            .unwrap_or_else(|_| tmpdir.to_path_buf());
        let canonical_str = canonical.to_string_lossy();
        let original_str = tmpdir.to_string_lossy();
        let mut out = text.replace(canonical_str.as_ref(), "<TMPDIR>");
        out = out.replace(original_str.as_ref(), "<TMPDIR>");
        out
    }

    #[test]
    fn missing_manifest_emits_typed_diagnostic_with_help() {
        let dir = TempDir::new().unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        let normalized = normalize(&stderr, dir.path());
        // miette's fancy renderer emits the stable code on its
        // own line, then a blank line, then `  × <message>`,
        // and finally `  help: <help text>`. Pin all three
        // components plus the no-cause-chain invariant: the
        // raw `os error 2` must not appear anywhere because
        // the typed error sets its own message.
        assert!(
            normalized.contains("cabin::workspace::manifest_not_found"),
            "missing code: {normalized:?}"
        );
        assert!(
            normalized.contains("× could not find a Cabin workspace at <TMPDIR>/cabin.toml"),
            "missing primary message: {normalized:?}"
        );
        assert!(
            normalized.contains("help: run `cabin init`"),
            "missing help: {normalized:?}"
        );
        assert!(
            !normalized.contains("os error 2"),
            "raw OS error must not appear: {normalized:?}"
        );
    }

    #[test]
    fn invalid_toml_manifest_renders_source_snippet() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package\nname = broken\n")
            .unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        // The exact byte position the toml parser flags varies
        // between releases, so we assert on stable invariants:
        // - the `parse_error` code,
        // - the primary `× could not parse Cabin manifest`
        //   line,
        // - miette's box-drawing snippet header `╭─[path:l:c]`,
        // - the offending source line embedded in the snippet,
        // - a `help:` line.
        assert!(
            stderr.contains("cabin::manifest::parse_error"),
            "missing parse_error code: {stderr}"
        );
        assert!(
            stderr.contains("× could not parse Cabin manifest"),
            "missing primary message: {stderr}"
        );
        assert!(stderr.contains("╭─["), "missing snippet header: {stderr}");
        assert!(stderr.contains("[package"), "missing source line: {stderr}");
        assert!(
            stderr.contains("help: check that the manifest is valid TOML"),
            "missing help: {stderr}"
        );
    }

    #[test]
    fn cabin_help_works_outside_workspace() {
        // A user invoking `cabin --help` should get the help
        // text whether or not they are inside a Cabin
        // workspace. clap short-circuits `--help` before
        // dispatch, so we expect SUCCESS even from a tempdir
        // that has no `cabin.toml`.
        let dir = TempDir::new().unwrap();
        cabin()
            .current_dir(dir.path())
            .arg("--help")
            .assert()
            .success();
    }

    #[test]
    fn cabin_subcommand_help_works_outside_workspace() {
        // Same regression for `cabin <cmd> --help`. The classic
        // failure mode is: dispatcher tries to load the
        // workspace before clap sees the `--help` flag, and the
        // missing manifest fails the help invocation. Every
        // top-level subcommand is exercised — including the
        // hidden distribution helpers — so a regression in any
        // one of them surfaces here.  The list is derived from
        // clap so a future subcommand is covered automatically.
        let dir = TempDir::new().unwrap();
        for sub in all_subcommand_names() {
            cabin()
                .current_dir(dir.path())
                .args([sub.as_str(), "--help"])
                .assert()
                .success();
        }
    }

    #[test]
    fn manifest_path_pointing_at_directory_emits_unreadable_diagnostic() {
        // `--manifest-path <dir>` is not a missing manifest:
        // the path canonicalizes fine but the subsequent
        // `read_to_string` in the manifest crate returns
        // `IsADirectory`. The diagnostic must be a typed
        // `cabin::manifest::unreadable` with no chain
        // duplication.
        let dir = TempDir::new().unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(dir.path())
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("cabin::manifest::unreadable"),
            "expected manifest unreadable diagnostic code, got: {stderr}"
        );
        // The OS error must appear once, not twice. The old
        // anyhow chain rendered "failed to read X: failed to
        // read X: Is a directory: Is a directory" — and the
        // miette renderer is configured `.without_cause_chain()`
        // so it doesn't re-emit `╰─▶ Is a directory` either.
        let occurrences = stderr.matches("Is a directory").count();
        assert_eq!(
            occurrences, 1,
            "expected one `Is a directory` occurrence (no chain dup), got {occurrences}: {stderr}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn permission_denied_manifest_emits_unreadable_diagnostic() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let manifest = dir.path().join("cabin.toml");
        assert_fs::fixture::ChildPath::new(&manifest)
            .write_str(
                r#"[package]
name = "x"
version = "0.1.0"
"#,
            )
            .unwrap();
        // Strip every permission bit so `std::fs::canonicalize`
        // (the workspace loader's first read) returns
        // PermissionDenied rather than NotFound. Skip the
        // assertion on platforms (notably running as root in
        // CI) where chmod 0 still allows reads.
        let mut perms = std::fs::metadata(&manifest).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&manifest, perms).unwrap();
        let still_readable = std::fs::canonicalize(&manifest).is_ok();
        // Restore permissions so TempDir cleanup works.
        let mut restore = std::fs::metadata(&manifest).unwrap().permissions();
        restore.set_mode(0o644);
        let _ = std::fs::set_permissions(&manifest, restore);
        if still_readable {
            eprintln!("test skipped: chmod 0 did not block read access (running as root?)");
            return;
        }
        // Re-strip so the actual cabin invocation observes the
        // denial; restore again afterwards.
        let mut perms = std::fs::metadata(&manifest).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&manifest, perms).unwrap();
        let assertion = cabin()
            .args(["metadata", "--manifest-path"])
            .arg(&manifest)
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        let mut restore = std::fs::metadata(&manifest).unwrap().permissions();
        restore.set_mode(0o644);
        let _ = std::fs::set_permissions(&manifest, restore);
        assert!(
            stderr.contains("cabin::workspace::manifest_unreadable"),
            "expected manifest_unreadable code, got: {stderr}"
        );
    }
}

/// `--color` / `CABIN_TERM_COLOR` integration tests.
///
/// The tests below exercise the user-visible color contract:
///   - `--color` parsing (clap rejects unknown values),
///   - `CABIN_TERM_COLOR` parsing (Cabin rejects unknown values
///     with a documented wording),
///   - `--color` overrides `CABIN_TERM_COLOR`,
///   - `--color always` produces ANSI escape sequences in
///     diagnostic output even when stderr is captured,
///   - `--color never` produces none even when the env says
///     `always`,
///   - help text exposes the option with the documented
///     possible-value list.
mod color_control {
    use super::*;

    /// ANSI control sequence introducer; presence in the
    /// captured bytes is the test's signal for "the renderer
    /// emitted styling".
    const ESC: char = '\x1b';

    /// Drive `cabin metadata` against a non-existent manifest
    /// in `dir`. The workspace loader emits the
    /// `cabin::workspace::manifest_not_found` typed
    /// diagnostic, which the renderer paints when color is on.
    /// We use this path because it is the cheapest way to
    /// produce a Cabin-owned diagnostic without setting up a
    /// build environment.
    fn missing_manifest_command(dir: &TempDir) -> Command {
        let mut cmd = cabin();
        cmd.current_dir(dir.path()).arg("metadata");
        cmd
    }

    #[test]
    fn color_value_auto_is_accepted_by_clap() {
        let dir = TempDir::new().unwrap();
        // `--color auto` must parse cleanly. Failure is
        // expected (no manifest exists), but it must be a
        // workspace error, not a CLI parse error (clap exits
        // with code 2 on parse error; Cabin returns exit code
        // 1 on workspace failure). miette's fancy renderer
        // prints the diagnostic code on its own line; with
        // `auto` color the code may be wrapped in ANSI
        // escapes, so we match the bare code text rather than
        // a fixed prefix.
        let assertion = missing_manifest_command(&dir)
            .arg("--color")
            .arg("auto")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("cabin::workspace::manifest_not_found"),
            "expected workspace error reaching the diagnostic renderer, got: {stderr}"
        );
    }

    #[test]
    fn color_value_always_is_accepted_by_clap() {
        let dir = TempDir::new().unwrap();
        missing_manifest_command(&dir)
            .arg("--color")
            .arg("always")
            .assert()
            .failure();
    }

    #[test]
    fn color_value_never_is_accepted_by_clap() {
        let dir = TempDir::new().unwrap();
        missing_manifest_command(&dir)
            .arg("--color")
            .arg("never")
            .assert()
            .failure();
    }

    #[test]
    fn color_value_unknown_is_rejected_by_clap() {
        let dir = TempDir::new().unwrap();
        let assertion = missing_manifest_command(&dir)
            .arg("--color")
            .arg("sometimes")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        // clap's value-enum rejection mentions the offending
        // value and lists the valid set. Pin both substrings
        // rather than the exact message so a clap upgrade
        // does not break the test.
        assert!(
            stderr.contains("'sometimes'") || stderr.contains("\"sometimes\""),
            "expected the invalid value to appear in the error, got: {stderr}"
        );
        assert!(
            stderr.contains("auto") && stderr.contains("always") && stderr.contains("never"),
            "expected the valid set to appear in the error, got: {stderr}"
        );
    }

    #[test]
    fn cabin_term_color_invalid_value_yields_documented_wording() {
        let dir = TempDir::new().unwrap();
        let assertion = missing_manifest_command(&dir)
            .env("CABIN_TERM_COLOR", "sometimes")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains(
                "invalid CABIN_TERM_COLOR value 'sometimes'; expected one of: auto, always, never"
            ),
            "expected documented env-error wording, got: {stderr}"
        );
    }

    #[test]
    fn color_always_emits_ansi_escapes_for_workspace_diagnostic() {
        let dir = TempDir::new().unwrap();
        let assertion = missing_manifest_command(&dir)
            .arg("--color")
            .arg("always")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains(ESC),
            "expected ANSI escape with --color always, got: {stderr:?}"
        );
        // miette wraps the bare code line in ANSI; pin the
        // bare code text and the message body.
        assert!(
            stderr.contains("cabin::workspace::manifest_not_found"),
            "expected diagnostic code, got: {stderr}"
        );
        assert!(
            stderr.contains("could not find a Cabin workspace"),
            "expected diagnostic message body, got: {stderr}"
        );
    }

    #[test]
    fn color_never_emits_no_ansi_escapes_for_workspace_diagnostic() {
        let dir = TempDir::new().unwrap();
        let assertion = missing_manifest_command(&dir)
            .arg("--color")
            .arg("never")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains(ESC),
            "expected no ANSI escape with --color never, got: {stderr:?}"
        );
        assert!(
            stderr.contains("cabin::workspace::manifest_not_found"),
            "expected diagnostic body intact, got: {stderr}"
        );
        assert!(
            stderr.contains("could not find a Cabin workspace"),
            "expected diagnostic message body, got: {stderr}"
        );
    }

    #[test]
    fn cli_color_always_overrides_env_never() {
        let dir = TempDir::new().unwrap();
        let assertion = missing_manifest_command(&dir)
            .arg("--color")
            .arg("always")
            .env("CABIN_TERM_COLOR", "never")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains(ESC),
            "CLI --color always must override env CABIN_TERM_COLOR=never, got: {stderr:?}"
        );
    }

    #[test]
    fn cli_color_never_overrides_env_always() {
        let dir = TempDir::new().unwrap();
        let assertion = missing_manifest_command(&dir)
            .arg("--color")
            .arg("never")
            .env("CABIN_TERM_COLOR", "always")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains(ESC),
            "CLI --color never must override env CABIN_TERM_COLOR=always, got: {stderr:?}"
        );
    }

    #[test]
    fn env_always_applies_when_cli_omitted() {
        let dir = TempDir::new().unwrap();
        let assertion = missing_manifest_command(&dir)
            .env("CABIN_TERM_COLOR", "always")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains(ESC),
            "env CABIN_TERM_COLOR=always must apply when --color is omitted, got: {stderr:?}"
        );
    }

    #[test]
    fn env_never_applies_when_cli_omitted() {
        let dir = TempDir::new().unwrap();
        let assertion = missing_manifest_command(&dir)
            .env("CABIN_TERM_COLOR", "never")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains(ESC),
            "env CABIN_TERM_COLOR=never must apply when --color is omitted, got: {stderr:?}"
        );
    }

    #[test]
    fn metadata_json_output_remains_uncolored_with_color_always() {
        // Set up a real, parseable workspace so `cabin metadata`
        // emits its JSON document. The JSON path writes only
        // serde-formatted bytes to stdout and never touches the
        // diagnostic renderer, so even with `--color always`
        // the captured stdout must contain no ANSI escape.
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(VALID_MANIFEST)
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("src"))
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("src/main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["metadata", "--color", "always"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            !stdout.contains(ESC),
            "machine-readable JSON must not be colorised, got: {stdout:?}"
        );
        // Sanity-check it actually is JSON.
        assert!(stdout.trim_start().starts_with('{'), "got: {stdout:?}");
    }

    #[test]
    fn top_level_help_advertises_color_option() {
        let assertion = cabin().arg("--help").assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("--color"),
            "top-level --help should mention --color, got: {stdout}"
        );
        assert!(
            stdout.contains("Coloring: auto, always, never"),
            "top-level --help should carry our help text, got: {stdout}"
        );
        assert!(
            stdout.contains("auto") && stdout.contains("always") && stdout.contains("never"),
            "top-level --help should list the value set, got: {stdout}"
        );
    }

    #[test]
    fn subcommand_help_inherits_global_color_option() {
        let assertion = cabin().args(["build", "--help"]).assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("--color"),
            "subcommand --help should expose the global --color, got: {stdout}"
        );
    }

    #[test]
    fn config_term_color_always_applies_when_cli_and_env_silent() {
        // Drop a user-level `[term] color = "always"` into a
        // throw-away `CABIN_CONFIG_HOME`. The default test
        // helper sets `CABIN_TERM_COLOR=never`, so for this
        // test we explicitly remove the env var (and
        // `CABIN_NO_CONFIG`, which the helper otherwise sets
        // to `1` to isolate other tests from a developer's
        // own config). The expected behavior is that
        // discovery picks up the file, the resolver sees
        // config=always with no env or CLI override, and the
        // diagnostic renderer paints the output.
        let dir = TempDir::new().unwrap();
        let cfg_home = TempDir::new().unwrap();
        cfg_home
            .child("config.toml")
            .write_str("[term]\ncolor = \"always\"\n")
            .unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .arg("metadata")
            .env_remove("CABIN_NO_CONFIG")
            .env_remove("CABIN_TERM_COLOR")
            .env("CABIN_CONFIG_HOME", cfg_home.path())
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains(ESC),
            "config term.color=always should color output when CLI+env are silent, got: {stderr:?}"
        );
    }

    #[test]
    fn cli_color_overrides_config_term_color_always() {
        // CLI must beat a user-level config that says
        // `always`.
        let dir = TempDir::new().unwrap();
        let cfg_home = TempDir::new().unwrap();
        cfg_home
            .child("config.toml")
            .write_str("[term]\ncolor = \"always\"\n")
            .unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["--color", "never", "metadata"])
            .env_remove("CABIN_NO_CONFIG")
            .env_remove("CABIN_TERM_COLOR")
            .env("CABIN_CONFIG_HOME", cfg_home.path())
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains(ESC),
            "--color never must override config term.color=always, got: {stderr:?}"
        );
    }

    #[test]
    fn env_term_color_overrides_config_term_color() {
        // Config says `always`, env says `never`. Env wins.
        let dir = TempDir::new().unwrap();
        let cfg_home = TempDir::new().unwrap();
        cfg_home
            .child("config.toml")
            .write_str("[term]\ncolor = \"always\"\n")
            .unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .arg("metadata")
            .env_remove("CABIN_NO_CONFIG")
            .env("CABIN_TERM_COLOR", "never")
            .env("CABIN_CONFIG_HOME", cfg_home.path())
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains(ESC),
            "CABIN_TERM_COLOR=never must override config term.color=always, got: {stderr:?}"
        );
    }

    #[test]
    fn metadata_json_stderr_remains_uncolored_with_color_always() {
        // Companion to `metadata_json_output_remains_uncolored…`:
        // the JSON success path should not emit anything
        // colored on stderr either, even when `--color always`
        // forces color for any diagnostics.
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(VALID_MANIFEST)
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("src"))
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("src/main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["metadata", "--color", "always"])
            .assert()
            .success();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains(ESC),
            "successful metadata run must leave stderr clean, got: {stderr:?}"
        );
    }

    #[test]
    fn cli_color_always_paints_help_lead_in() {
        let dir = TempDir::new().unwrap();
        let assertion = missing_manifest_command(&dir)
            .arg("--color")
            .arg("always")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        // The diagnostic carries a `help:` line; under
        // `--color always` it must be wrapped in ANSI styling
        // (escape just before the `help:` token).
        let help_idx = stderr
            .find("help:")
            .expect("manifest_not_found diagnostic should include a help line");
        let leading = &stderr[..help_idx];
        assert!(
            leading.contains(ESC),
            "expected ANSI sequence before `help:`, got: {stderr:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// `cabin fmt`
// ---------------------------------------------------------------------------

mod fmt_command {
    use super::*;
    use std::path::PathBuf;

    /// Sentinel marker appended by the bundled fake clang-format.
    /// A file whose trimmed contents end with this marker is
    /// treated as "already formatted"; a file that lacks it is
    /// treated as "would be reformatted".
    const MARKER: &str = "/* FORMATTED */";

    fn cabin_with_fake_formatter() -> Command {
        let mut cmd = cabin();
        let path = fake_formatter_path();
        cmd.env("CABIN_FMT", path);
        cmd
    }

    fn fake_formatter_path() -> PathBuf {
        // `assert_cmd::cargo_bin!` only resolves binaries
        // declared in the *current* package, so we walk
        // alongside the test executable to find the workspace-
        // built `cabin-fmt-fake-formatter`. The binary lives
        // in the workspace target directory at the same depth
        // as the test binary itself (`target/<profile>/`).
        let test_exe = std::env::current_exe().expect("current_exe");
        let mut dir = test_exe
            .parent()
            .expect("test exe should live in a directory")
            .to_path_buf();
        if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
            dir.pop();
        }
        let candidate = dir.join("cabin-fmt-fake-formatter");
        assert!(
            candidate.is_file(),
            "expected fake formatter at {}; build cabin-fmt with `--features test-fake-formatter`",
            candidate.display()
        );
        candidate
    }

    /// Single-package fixture used by most fmt tests. The
    /// manifest declares a `cpp_executable` target so the
    /// surrounding workspace bits look real, but `cabin fmt`
    /// only cares about the on-disk source files.
    fn write_minimal_project(root: &Path) {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(VALID_MANIFEST)
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
            .write_str("int main() { return 0; }\n")
            .unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn top_level_help_lists_fmt() {
        let assertion = cabin().arg("--help").assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("fmt"),
            "top-level help should list fmt: {stdout}"
        );
    }

    #[test]
    fn fmt_help_documents_documented_flags() {
        let assertion = cabin().args(["fmt", "--help"]).assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        for snippet in ["--check", "--build-dir", "--exclude", "--no-ignore-vcs"] {
            assert!(
                stdout.contains(snippet),
                "`cabin fmt --help` should mention {snippet}: {stdout}"
            );
        }
    }

    #[test]
    fn write_mode_formats_in_place() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .arg("fmt")
            .assert()
            .success()
            .stdout(predicate::str::contains("formatted 1 file"));

        let body = read(&dir.path().join("src/main.cc"));
        assert!(body.contains(MARKER), "expected marker, got: {body:?}");
    }

    #[test]
    fn check_mode_passes_when_already_formatted() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc")
            .write_str("int main() { return 0; }\n/* FORMATTED */\n")
            .unwrap();

        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "--check"])
            .assert()
            .success()
            .stdout(predicate::str::contains("already formatted"));

        let body = read(&dir.path().join("src/main.cc"));
        assert!(body.contains(MARKER));
    }

    #[test]
    fn check_mode_fails_when_files_would_be_reformatted() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "--check"])
            .assert()
            .failure();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("formatting check failed"),
            "expected actionable status, got: {stdout}"
        );

        // Check mode must not modify files.
        let body = read(&dir.path().join("src/main.cc"));
        assert!(!body.contains(MARKER), "check mode mutated file: {body}");
    }

    #[test]
    fn exclude_path_skips_named_file() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        dir.child("src/generated.cc")
            .write_str("int gen() {}\n")
            .unwrap();

        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "--exclude", "src/generated.cc"])
            .assert()
            .success();

        assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
        assert!(!read(&dir.path().join("src/generated.cc")).contains(MARKER));
    }

    #[test]
    fn repeated_exclude_accumulates() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        dir.child("src/a.cc").write_str("int a() {}\n").unwrap();
        dir.child("src/b.cc").write_str("int b() {}\n").unwrap();

        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "--exclude", "src/a.cc", "--exclude", "src/b.cc"])
            .assert()
            .success();

        assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
        assert!(!read(&dir.path().join("src/a.cc")).contains(MARKER));
        assert!(!read(&dir.path().join("src/b.cc")).contains(MARKER));
    }

    #[test]
    fn vcs_ignored_files_are_skipped_by_default() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        dir.child("src/generated.cc")
            .write_str("int gen() {}\n")
            .unwrap();
        dir.child(".gitignore")
            .write_str("src/generated.cc\n")
            .unwrap();
        // Touch a `.git/HEAD` so the ignore crate's git-aware
        // walker activates without us needing to shell out to
        // `git init`.
        dir.child(".git/HEAD")
            .write_str("ref: refs/heads/main\n")
            .unwrap();

        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .arg("fmt")
            .assert()
            .success();

        assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
        assert!(!read(&dir.path().join("src/generated.cc")).contains(MARKER));
    }

    #[test]
    fn no_ignore_vcs_includes_gitignored_files() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        dir.child("src/generated.cc")
            .write_str("int gen() {}\n")
            .unwrap();
        dir.child(".gitignore")
            .write_str("src/generated.cc\n")
            .unwrap();
        dir.child(".git/HEAD")
            .write_str("ref: refs/heads/main\n")
            .unwrap();

        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "--no-ignore-vcs"])
            .assert()
            .success();

        assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
        assert!(read(&dir.path().join("src/generated.cc")).contains(MARKER));
    }

    #[test]
    fn build_directory_is_not_formatted() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        // Drop a fake artifact under the default build dir.
        dir.child("build/dev/scratch.cc")
            .write_str("int scratch() {}\n")
            .unwrap();

        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .arg("fmt")
            .assert()
            .success();

        assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
        assert!(!read(&dir.path().join("build/dev/scratch.cc")).contains(MARKER));
    }

    #[test]
    fn custom_build_directory_is_not_formatted() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        dir.child("out/dev/scratch.cc")
            .write_str("int s() {}\n")
            .unwrap();

        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "--build-dir", "out"])
            .assert()
            .success();

        assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
        assert!(!read(&dir.path().join("out/dev/scratch.cc")).contains(MARKER));
    }

    #[test]
    fn vendor_and_cache_directories_are_not_formatted() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        // `target/` and `dist/` are part of the built-in
        // excluded-name set in source discovery.
        dir.child("target/leftover.cc")
            .write_str("int t() {}\n")
            .unwrap();
        dir.child("dist/staging.cc")
            .write_str("int d() {}\n")
            .unwrap();
        // `node_modules` is on the excluded-name set too —
        // documentation sites sometimes ship one inside a
        // Cabin tree.
        dir.child("node_modules/dep/main.cc")
            .write_str("int dep() {}\n")
            .unwrap();

        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .arg("fmt")
            .assert()
            .success();

        assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
        for skipped in [
            "target/leftover.cc",
            "dist/staging.cc",
            "node_modules/dep/main.cc",
        ] {
            assert!(
                !read(&dir.path().join(skipped)).contains(MARKER),
                "{skipped} was unexpectedly formatted"
            );
        }
    }

    #[test]
    fn missing_formatter_produces_actionable_error() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin()
            .current_dir(dir.path())
            .env("CABIN_FMT", "/no-such/clang-format-binary")
            .arg("fmt")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("/no-such/clang-format-binary"),
            "diagnostic should name the missing executable: {stderr}"
        );
        assert!(
            stderr.contains("install `clang-format`"),
            "diagnostic should include the install hint: {stderr}"
        );
        assert!(
            stderr.contains("CABIN_FMT"),
            "diagnostic should reference the override env var: {stderr}"
        );
    }

    #[test]
    fn cabin_fmt_env_override_routes_through_named_binary() {
        // This test is the same surface as
        // `write_mode_formats_in_place`, but asserted from the
        // angle of the env-var contract: if `CABIN_FMT` is set
        // to the fake formatter's path, Cabin spawns *that*
        // binary, not the system clang-format. The fake
        // formatter's sentinel marker confirms it ran.
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        cabin()
            .current_dir(dir.path())
            .env("CABIN_FMT", fake_formatter_path())
            .arg("fmt")
            .assert()
            .success();
        assert!(read(&dir.path().join("src/main.cc")).contains(MARKER));
    }

    #[test]
    fn quiet_mode_suppresses_normal_status_lines() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "--quiet"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stdout.is_empty() && stderr.is_empty(),
            "quiet fmt run must produce no output, got stdout={stdout:?} stderr={stderr:?}"
        );
    }

    #[test]
    fn verbose_mode_lists_selected_package_and_file_count() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "--verbose"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("formatting 1 file"),
            "verbose fmt must report file count: {stdout}"
        );
        assert!(
            stdout.contains("hello"),
            "verbose fmt must name the selected package: {stdout}"
        );
    }

    #[test]
    fn very_verbose_lists_command_lines() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "-vv"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("--style=file"),
            "very-verbose fmt must echo the formatter command: {stdout}"
        );
        assert!(
            stdout.contains("-i"),
            "very-verbose fmt must echo the write-mode flag: {stdout}"
        );
    }

    #[test]
    fn nested_workspace_member_is_not_walked_from_outer_root() {
        // Verifies that when a workspace member lives inside
        // another package's directory, walking the outer
        // package does not pick up the inner package's
        // sources. The inner package would be walked
        // independently when selected.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["outer", "outer/nested"]
"#,
            )
            .unwrap();
        dir.child("outer/cabin.toml")
            .write_str(
                r#"[package]
name = "outer"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("outer/src/main.cc")
            .write_str("int outer() {}\n")
            .unwrap();
        dir.child("outer/nested/cabin.toml")
            .write_str(
                r#"[package]
name = "nested"
version = "0.1.0"
"#,
            )
            .unwrap();
        dir.child("outer/nested/src/main.cc")
            .write_str("int nested() {}\n")
            .unwrap();

        // Select only `outer`; the walker must skip
        // `outer/nested/...`.
        cabin_with_fake_formatter()
            .current_dir(dir.path())
            .args(["fmt", "-p", "outer"])
            .assert()
            .success();

        assert!(read(&dir.path().join("outer/src/main.cc")).contains(MARKER));
        assert!(!read(&dir.path().join("outer/nested/src/main.cc")).contains(MARKER));
    }
}

// ---------------------------------------------------------------------------
// `-j` / `--jobs <N>` for build / run / tidy
// ---------------------------------------------------------------------------

mod jobs_parallelism {
    use super::*;
    use std::path::PathBuf;

    const VALID_C_MANIFEST: &str = r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "cpp_executable"
sources = ["src/main.cc"]
"#;

    fn fake_ninja_path() -> PathBuf {
        let test_exe = std::env::current_exe().expect("current_exe");
        let mut dir = test_exe
            .parent()
            .expect("test exe should live in a directory")
            .to_path_buf();
        if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
            dir.pop();
        }
        let candidate = dir.join("cabin-ninja-fake-ninja");
        assert!(
            candidate.is_file(),
            "expected fake ninja at {}; build cabin-ninja with `--features test-fake-ninja`",
            candidate.display()
        );
        candidate
    }

    /// Stage a tempdir with a minimal C++ package and configure
    /// the returned [`Command`] to use the fake ninja recorded
    /// at `record`.  Callers add the subcommand and its flags.
    fn cabin_with_fake_ninja(record: &Path) -> Command {
        let mut cmd = cabin();
        cmd.env("NINJA", fake_ninja_path())
            .env("CABIN_FAKE_NINJA_RECORD", record);
        cmd
    }

    fn read_ninja_argvs(record: &Path) -> Vec<Vec<String>> {
        let body = fs::read_to_string(record).unwrap_or_default();
        body.lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                line.split('\u{001f}')
                    .map(|s| s.to_owned())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn write_minimal_project(root: &Path) {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(VALID_C_MANIFEST)
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
    }

    #[test]
    fn build_help_documents_jobs() {
        let assertion = cabin().args(["build", "--help"]).assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("-j, --jobs <N>"),
            "build help should advertise -j/--jobs:\n{stdout}"
        );
    }

    #[test]
    fn run_help_documents_jobs() {
        let assertion = cabin().args(["run", "--help"]).assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("-j, --jobs <N>"),
            "run help should advertise -j/--jobs:\n{stdout}"
        );
    }

    #[test]
    fn test_help_does_not_document_jobs() {
        let assertion = cabin().args(["test", "--help"]).assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        // `cabin test` deliberately does not expose `-j` / `--jobs`:
        // the test runner is sequential, so a `--jobs` knob would
        // only affect the build phase and mislead users into
        // expecting parallel test execution.
        assert!(
            !stdout.contains("-j, --jobs <N>"),
            "test help must not advertise -j/--jobs:\n{stdout}"
        );
    }

    #[test]
    fn jobs_zero_is_rejected_at_cli() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["build", "--jobs", "0"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("positive integer"),
            "diagnostic should mention 'positive integer':\n{stderr}"
        );
        assert!(
            stderr.contains('0'),
            "diagnostic should echo the offending value:\n{stderr}"
        );
    }

    #[test]
    fn jobs_non_numeric_is_rejected_at_cli() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["build", "--jobs", "many"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("many"),
            "diagnostic should echo the offending value:\n{stderr}"
        );
    }

    #[test]
    fn build_with_jobs_forwards_dash_j_to_ninja() {
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        write_minimal_project(dir.path());
        cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .args(["build", "--jobs", "1"])
            .assert()
            .success();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations.len(), 1, "expected one ninja invocation");
        assert_eq!(invocations[0][0], "-j1");
    }

    #[test]
    fn build_with_short_j_forwards_to_ninja() {
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        write_minimal_project(dir.path());
        cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .args(["build", "-j", "2"])
            .assert()
            .success();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0][0], "-j2");
    }

    #[test]
    fn build_without_jobs_does_not_pass_dash_j() {
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        write_minimal_project(dir.path());
        cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .arg("build")
            .assert()
            .success();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations.len(), 1);
        for arg in &invocations[0] {
            assert!(
                !arg.starts_with("-j"),
                "expected no -j argument when --jobs is omitted, got: {arg}"
            );
        }
    }

    #[test]
    fn test_rejects_jobs_flag() {
        // `cabin test` does not accept `--jobs` (or `-j`): the
        // test runner is sequential, so exposing the flag would
        // mislead users into expecting parallel test execution.
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["test", "--jobs", "2"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(
            stderr.contains("unexpected argument") && stderr.contains("--jobs"),
            "expected `unexpected argument '--jobs'` error, got: {stderr}"
        );
    }

    #[test]
    fn cabin_build_jobs_env_var_is_honored() {
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        write_minimal_project(dir.path());
        cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .env("CABIN_BUILD_JOBS", "3")
            .arg("build")
            .assert()
            .success();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0][0], "-j3");
    }

    #[test]
    fn cli_jobs_overrides_env() {
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        write_minimal_project(dir.path());
        cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .env("CABIN_BUILD_JOBS", "8")
            .args(["build", "--jobs", "2"])
            .assert()
            .success();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations[0][0], "-j2");
    }

    #[test]
    fn invalid_env_value_produces_actionable_error() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .env("NINJA", fake_ninja_path())
            .env("CABIN_BUILD_JOBS", "many")
            .arg("build")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("CABIN_BUILD_JOBS"),
            "diagnostic should name the env var:\n{stderr}"
        );
        assert!(
            stderr.contains("\"many\""),
            "diagnostic should echo the offending value:\n{stderr}"
        );
    }

    #[test]
    fn env_zero_is_rejected() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .env("NINJA", fake_ninja_path())
            .env("CABIN_BUILD_JOBS", "0")
            .arg("build")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("CABIN_BUILD_JOBS"),
            "diagnostic should name the env var:\n{stderr}"
        );
        assert!(
            stderr.contains("positive integer"),
            "diagnostic should mention 'positive integer':\n{stderr}"
        );
    }

    #[test]
    fn build_jobs_config_setting_is_honored() {
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        write_minimal_project(dir.path());
        let config_dir = dir.path().join(".cabin");
        assert_fs::fixture::ChildPath::new(config_dir.join("config.toml"))
            .write_str("[build]\njobs = 5\n")
            .unwrap();
        cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .env_remove("CABIN_NO_CONFIG")
            .env("CABIN_CONFIG", config_dir.join("config.toml"))
            .arg("build")
            .assert()
            .success();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0][0], "-j5");
    }

    #[test]
    fn env_overrides_config() {
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        write_minimal_project(dir.path());
        let config_dir = dir.path().join(".cabin");
        assert_fs::fixture::ChildPath::new(config_dir.join("config.toml"))
            .write_str("[build]\njobs = 5\n")
            .unwrap();
        cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .env_remove("CABIN_NO_CONFIG")
            .env("CABIN_CONFIG", config_dir.join("config.toml"))
            .env("CABIN_BUILD_JOBS", "9")
            .arg("build")
            .assert()
            .success();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations[0][0], "-j9");
    }

    #[test]
    fn config_zero_is_rejected_with_actionable_error() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let config_dir = dir.path().join(".cabin");
        assert_fs::fixture::ChildPath::new(config_dir.join("config.toml"))
            .write_str("[build]\njobs = 0\n")
            .unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .env("NINJA", fake_ninja_path())
            .env_remove("CABIN_NO_CONFIG")
            .env("CABIN_CONFIG", config_dir.join("config.toml"))
            .arg("build")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("cabin::config::invalid_build_jobs"),
            "config-layer failures should render with a stable diagnostic code:\n{stderr}"
        );
        assert!(
            stderr.contains("build.jobs"),
            "diagnostic should name the config key:\n{stderr}"
        );
        assert!(
            stderr.contains("positive integer"),
            "diagnostic should mention 'positive integer':\n{stderr}"
        );
    }

    #[test]
    fn run_jobs_before_doubledash_is_consumed_by_cabin() {
        // `cabin run --jobs 4 -- --help`: `--jobs 4` appears
        // before `--`, so Cabin parses it and forwards `-j4`
        // to ninja.  `--help` after `--` reaches the user
        // program — but the fake ninja never produces an
        // executable, so the run command exits non-zero
        // after the ninja step.  We only care about what
        // reached ninja.
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        write_minimal_project(dir.path());
        let _ = cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .args(["run", "--jobs", "4", "--", "--help"])
            .assert()
            .failure();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0][0], "-j4");
    }

    #[test]
    fn run_jobs_after_doubledash_does_not_reach_ninja() {
        // `cabin run -- --jobs 4`: `--jobs 4` appears after
        // `--` and is therefore destined for the user
        // program.  Cabin must not interpret it as its own
        // `--jobs` flag; ninja receives no `-j` argument.
        // (The program is never reached because the fake
        // ninja short-circuits, but Cabin's parse-time
        // treatment is what this test pins down.)
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        write_minimal_project(dir.path());
        let _ = cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .args(["run", "--", "--jobs", "4"])
            .assert()
            .failure();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations.len(), 1);
        for arg in &invocations[0] {
            assert!(
                !arg.starts_with("-j"),
                "ninja must not receive jobs when --jobs is after `--`: {arg}"
            );
        }
    }

    #[test]
    fn metadata_output_is_byte_identical_with_and_without_jobs_env() {
        // The `--jobs` contract forbids polluting machine-readable
        // output with jobs-related information.  The strongest
        // assertion is that `cabin metadata`'s stdout is
        // byte-identical whether `CABIN_BUILD_JOBS` is set or not
        // — that catches both accidental metadata extension and
        // any incidental status-line
        // leak.
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let baseline = cabin()
            .current_dir(dir.path())
            .args(["metadata"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let with_jobs = cabin()
            .current_dir(dir.path())
            .env("CABIN_BUILD_JOBS", "4")
            .args(["metadata"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            baseline, with_jobs,
            "CABIN_BUILD_JOBS must not change `cabin metadata` JSON",
        );
    }
}

// ---------------------------------------------------------------------------
// `cabin tidy`
// ---------------------------------------------------------------------------

mod tidy_command {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    /// Sentinel that the bundled fake tidy recognizes: a source
    /// file whose contents contain this marker triggers a
    /// non-zero exit and a fake clang-tidy diagnostic on stderr.
    const FAIL_MARKER: &str = "// CABIN-TIDY-FAIL";

    /// Process-wide lock for tests that rely on
    /// `CABIN_FAKE_TIDY_RECORD`: env vars are process-global
    /// and cargo runs tests in parallel.
    fn tidy_record_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Build the integration-test command with `CABIN_TIDY`
    /// pointing at the bundled fake tidy.  Also sets `CXX` /
    /// `CC` / `AR` to a path that exists on every Unix host so
    /// the tidy planner's toolchain resolver does not fail when
    /// the developer's PATH lacks `c++` / `clang++` / `g++`.
    /// The fake tidy never invokes the compiler — the value is
    /// only threaded into `compile_commands.json`.
    fn cabin_with_fake_tidy() -> Command {
        let mut cmd = cabin();
        cmd.env("CABIN_TIDY", fake_tidy_path());
        cmd.env("CXX", "/bin/sh");
        cmd.env("CC", "/bin/sh");
        cmd.env("AR", "/bin/sh");
        cmd
    }

    fn fake_tidy_path() -> PathBuf {
        let test_exe = std::env::current_exe().expect("current_exe");
        let mut dir = test_exe
            .parent()
            .expect("test exe should live in a directory")
            .to_path_buf();
        if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
            dir.pop();
        }
        let candidate = dir.join("cabin-tidy-fake-tidy");
        assert!(
            candidate.is_file(),
            "expected fake tidy at {}; build cabin-tidy with `--features test-fake-tidy`",
            candidate.display()
        );
        candidate
    }

    fn write_minimal_project(root: &Path) {
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(VALID_MANIFEST)
            .unwrap();
        assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
            .write_str("int main() { return 0; }\n")
            .unwrap();
    }

    /// Collect the raw record lines the fake tidy appended.
    fn read_record(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn top_level_help_lists_tidy() {
        let assertion = cabin().arg("--help").assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("Run clang-tidy"),
            "top-level help should describe the tidy subcommand:\n{stdout}"
        );
    }

    #[test]
    fn tidy_help_documents_documented_flags() {
        let assertion = cabin().args(["tidy", "--help"]).assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        for snippet in [
            "--fix",
            "--jobs",
            "--build-dir",
            "--exclude",
            "--no-ignore-vcs",
        ] {
            assert!(
                stdout.contains(snippet),
                "`cabin tidy --help` should mention {snippet}: {stdout}"
            );
        }
    }

    #[test]
    fn clean_project_tidies_successfully() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .arg("tidy")
            .assert()
            .success()
            .stdout(predicate::str::contains("checked 1 file"));
    }

    /// `cabin tidy` must analyze `cpp_test` and `cpp_example`
    /// sources, not just default-buildable ones.  `cabin fmt`
    /// already formats those files; tidy must match its surface.
    #[test]
    fn tidy_analyses_cpp_test_and_example_sources() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "cpp_library"
sources = ["src/lib.cc"]

[target.demo_test]
type = "cpp_test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]

[target.hello_example]
type = "cpp_example"
sources = ["examples/hello.cc"]
deps = ["demo"]
"#,
            )
            .unwrap();
        dir.child("src/lib.cc")
            .write_str("int demo() { return 1; }\n")
            .unwrap();
        dir.child("tests/lib_test.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        dir.child("examples/hello.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();

        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .arg("tidy")
            .assert()
            .success()
            .stdout(predicate::str::contains("checked 3 files"));
    }

    #[test]
    fn compile_database_is_generated_at_profile_root() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .arg("tidy")
            .assert()
            .success();

        let cdb = dir.path().join("build/dev/compile_commands.json");
        assert!(
            cdb.is_file(),
            "compile_commands.json must land at build/dev/compile_commands.json: missing {}",
            cdb.display()
        );
        let body = fs::read_to_string(&cdb).unwrap();
        assert!(
            body.contains("main.cc"),
            "compile database should mention the source file: {body}"
        );
    }

    #[test]
    fn dash_p_is_the_compile_database_directory() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let record = dir.path().join("argv.log");

        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .arg("tidy")
            .assert()
            .success();

        let lines = read_record(&record);
        assert_eq!(lines.len(), 1, "expected one tidy spawn, got {lines:?}");
        let mut parts = lines[0].split('\t');
        let argv = parts.next().unwrap();
        let _quiet = parts.next().unwrap();
        let _fix = parts.next().unwrap();
        let cdb = parts.next().unwrap();
        assert!(argv.contains("-p"), "argv should contain -p: {argv}");
        // Path canonicalization may resolve symlinks under
        // /tmp -> /private/tmp on macOS, so test for the
        // suffix instead of an exact match.
        assert!(
            cdb.ends_with("build/dev"),
            "compile DB dir should end at <build>/dev: {cdb}"
        );
    }

    #[test]
    fn custom_build_dir_is_the_compile_database_directory() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let record = dir.path().join("argv.log");

        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "--build-dir", "out"])
            .assert()
            .success();

        assert!(dir.path().join("out/dev/compile_commands.json").is_file());
        let lines = read_record(&record);
        assert_eq!(lines.len(), 1, "expected one tidy spawn, got {lines:?}");
        let cdb = lines[0].split('\t').nth(3).unwrap();
        assert!(
            cdb.ends_with("out/dev"),
            "compile DB dir should end at <custom-build>/dev: {cdb}"
        );
    }

    #[test]
    fn fix_flag_is_passed_to_tidy_driver() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let record = dir.path().join("argv.log");

        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "--fix"])
            .assert()
            .success();

        let lines = read_record(&record);
        let mut parts = lines[0].split('\t');
        let argv = parts.next().unwrap();
        let _quiet = parts.next().unwrap();
        let fix = parts.next().unwrap();
        assert!(argv.contains("-fix"), "argv should contain -fix: {argv}");
        assert_eq!(fix, "true");
    }

    #[test]
    fn fix_mode_clamps_jobs_to_one() {
        // The historical C++ tidy implementation forced `-j 1`
        // in `--fix` mode; preserve the policy here.  Verbose
        // mode prints the override notice so the user can see
        // why their `--jobs 4` was dropped.
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let record = dir.path().join("argv.log");

        let assertion = cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "--fix", "--jobs", "4", "--verbose"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();

        let lines = read_record(&record);
        let mut parts = lines[0].split('\t');
        let argv = parts.next().unwrap();
        let _quiet = parts.next().unwrap();
        let _fix = parts.next().unwrap();
        let _cdb = parts.next().unwrap();
        let jobs = parts.next().unwrap();
        assert_eq!(jobs, "1", "fix mode must clamp jobs to 1: argv={argv}");
        assert!(
            stdout.contains("--fix forces tidy parallelism to 1"),
            "verbose mode must surface the clamp: {stdout}"
        );
    }

    #[test]
    fn jobs_flag_is_passed_to_tidy_driver_in_check_mode() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let record = dir.path().join("argv.log");

        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "--jobs", "2"])
            .assert()
            .success();

        let lines = read_record(&record);
        let mut parts = lines[0].split('\t');
        let argv = parts.next().unwrap();
        let _quiet = parts.next().unwrap();
        let _fix = parts.next().unwrap();
        let _cdb = parts.next().unwrap();
        let jobs = parts.next().unwrap();
        assert!(argv.contains("-j 2"), "argv should contain -j 2: {argv}");
        assert_eq!(jobs, "2");
    }

    #[test]
    fn invalid_jobs_value_is_rejected_at_parse_time() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin_with_fake_tidy()
            .current_dir(dir.path())
            .args(["tidy", "--jobs", "0"])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("got 0") || stderr.contains("expected a positive integer"),
            "diagnostic should call out the rejected value: {stderr}"
        );
    }

    #[test]
    fn no_files_succeeds_with_message() {
        // No `[target.*]` declared → no compile commands → no
        // files to check.  Should succeed cleanly with a
        // status line, not error.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str("[package]\nname = \"empty\"\nversion = \"0.1.0\"\n")
            .unwrap();
        dir.child("docs/README.md")
            .write_str("no sources here\n")
            .unwrap();

        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .arg("tidy")
            .assert()
            .success()
            .stdout(predicate::str::contains("no C/C++ source files to check"));
    }

    #[test]
    fn fail_marker_causes_non_zero_exit_and_preserves_diagnostic() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        dir.child("src/main.cc")
            .write_str("// CABIN-TIDY-FAIL\nint main() {}\n")
            .unwrap();

        let assertion = cabin_with_fake_tidy()
            .current_dir(dir.path())
            .arg("tidy")
            .assert()
            .failure();
        let _ = FAIL_MARKER;
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("fake-clang-tidy diagnostic"),
            "clang-tidy diagnostic must reach stderr unchanged, got: {stderr}"
        );
    }

    #[test]
    fn exclude_path_skips_named_file() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\", \"src/extra.cc\"]\n")

            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        dir.child("src/extra.cc")
            .write_str("int extra() { return 0; }\n")
            .unwrap();

        let record = dir.path().join("argv.log");
        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "--exclude", "src/extra.cc"])
            .assert()
            .success();

        let body = std::fs::read_to_string(&record).unwrap();
        assert!(body.contains("src/main.cc"));
        assert!(
            !body.contains("src/extra.cc"),
            "excluded file leaked into argv: {body}"
        );
    }

    #[test]
    fn repeated_exclude_accumulates() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\", \"src/a.cc\", \"src/b.cc\"]\n")

            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        dir.child("src/a.cc")
            .write_str("int a() { return 0; }\n")
            .unwrap();
        dir.child("src/b.cc")
            .write_str("int b() { return 0; }\n")
            .unwrap();

        let record = dir.path().join("argv.log");
        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "--exclude", "src/a.cc", "--exclude", "src/b.cc"])
            .assert()
            .success();

        let body = std::fs::read_to_string(&record).unwrap();
        assert!(body.contains("src/main.cc"));
        assert!(!body.contains("src/a.cc"));
        assert!(!body.contains("src/b.cc"));
    }

    #[test]
    fn vcs_ignored_files_are_skipped_by_default() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\", \"src/generated.cc\"]\n")

            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        dir.child("src/generated.cc")
            .write_str("int gen() { return 0; }\n")
            .unwrap();
        dir.child(".gitignore")
            .write_str("src/generated.cc\n")
            .unwrap();
        dir.child(".git/HEAD")
            .write_str("ref: refs/heads/main\n")
            .unwrap();

        let record = dir.path().join("argv.log");
        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .arg("tidy")
            .assert()
            .success();

        let body = std::fs::read_to_string(&record).unwrap();
        assert!(body.contains("src/main.cc"));
        assert!(!body.contains("src/generated.cc"));
    }

    #[test]
    fn no_ignore_vcs_includes_gitignored_files() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\", \"src/generated.cc\"]\n")

            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();
        dir.child("src/generated.cc")
            .write_str("int gen() { return 0; }\n")
            .unwrap();
        dir.child(".gitignore")
            .write_str("src/generated.cc\n")
            .unwrap();
        dir.child(".git/HEAD")
            .write_str("ref: refs/heads/main\n")
            .unwrap();

        let record = dir.path().join("argv.log");
        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "--no-ignore-vcs"])
            .assert()
            .success();

        let body = std::fs::read_to_string(&record).unwrap();
        assert!(body.contains("src/main.cc"));
        assert!(body.contains("src/generated.cc"));
    }

    #[test]
    fn build_and_cache_directories_are_not_checked() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        // Drop fake artifacts under directories that source
        // discovery is contracted to skip.  None of these
        // declare a target so they could not appear in
        // compile_commands either, but the assertion is the
        // negative one: no path that includes any of them
        // should reach the tidy driver.
        dir.child("build/dev/scratch.cc")
            .write_str("int s() {}\n")
            .unwrap();
        dir.child("target/leftover.cc")
            .write_str("int t() {}\n")
            .unwrap();
        dir.child("dist/staging.cc")
            .write_str("int d() {}\n")
            .unwrap();
        dir.child("node_modules/dep/main.cc")
            .write_str("int dep() {}\n")
            .unwrap();

        let record = dir.path().join("argv.log");
        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .arg("tidy")
            .assert()
            .success();

        let body = std::fs::read_to_string(&record).unwrap();
        for skipped in [
            "build/dev/scratch.cc",
            "target/leftover.cc",
            "dist/staging.cc",
            "node_modules/dep/main.cc",
        ] {
            assert!(
                !body.contains(skipped),
                "{skipped} should not have been checked: {body}"
            );
        }
    }

    #[test]
    fn custom_build_directory_is_not_checked() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        dir.child("out/dev/scratch.cc")
            .write_str("int s() {}\n")
            .unwrap();

        let record = dir.path().join("argv.log");
        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "--build-dir", "out"])
            .assert()
            .success();

        let body = std::fs::read_to_string(&record).unwrap();
        assert!(body.contains("src/main.cc"));
        assert!(
            !body.contains("out/dev/scratch.cc"),
            "custom build dir should not have been checked: {body}"
        );
    }

    #[test]
    fn missing_tidy_produces_actionable_error() {
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin()
            .current_dir(dir.path())
            .env("CABIN_TIDY", "/no-such/run-clang-tidy-binary")
            .env("CXX", "/bin/sh")
            .env("CC", "/bin/sh")
            .env("AR", "/bin/sh")
            .arg("tidy")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("/no-such/run-clang-tidy-binary"),
            "diagnostic should name the missing executable: {stderr}"
        );
        assert!(
            stderr.contains("install `clang-tidy`"),
            "diagnostic should include the install hint: {stderr}"
        );
        assert!(
            stderr.contains("CABIN_TIDY"),
            "diagnostic should reference the override env var: {stderr}"
        );
    }

    #[test]
    fn cabin_tidy_env_override_routes_through_named_binary() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let record = dir.path().join("argv.log");
        cabin()
            .current_dir(dir.path())
            .env("CABIN_TIDY", fake_tidy_path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .env("CXX", "/bin/sh")
            .env("CC", "/bin/sh")
            .env("AR", "/bin/sh")
            .arg("tidy")
            .assert()
            .success();
        let body = std::fs::read_to_string(&record).unwrap();
        assert!(body.contains("src/main.cc"));
    }

    #[test]
    fn clang_tidy_config_survives_unmodified_after_tidy() {
        // Cabin must not generate, modify, or delete a
        // `.clang-tidy` file the user committed alongside their
        // sources.  The integration here is the negative
        // assertion that Cabin does not interfere with
        // clang-tidy's own config story.
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());
        let cfg_path = dir.path().join(".clang-tidy");
        let cfg_body = "Checks: 'modernize-*'\n";
        assert_fs::fixture::ChildPath::new(&cfg_path)
            .write_str(cfg_body)
            .unwrap();

        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .arg("tidy")
            .assert()
            .success();

        assert!(cfg_path.is_file(), "Cabin must not delete .clang-tidy");
        let after = fs::read_to_string(&cfg_path).unwrap();
        assert_eq!(after, cfg_body, "Cabin must not modify .clang-tidy");
    }

    #[test]
    fn quiet_mode_suppresses_cabin_status_but_not_tidy_output() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin_with_fake_tidy()
            .current_dir(dir.path())
            .args(["tidy", "--quiet"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            !stdout.contains("cabin: running clang-tidy"),
            "quiet mode must suppress Cabin status: {stdout}"
        );
        assert!(
            !stdout.contains("cabin: checked"),
            "quiet mode must suppress Cabin summary: {stdout}"
        );
    }

    #[test]
    fn verbose_mode_lists_selected_package_and_file_count() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin_with_fake_tidy()
            .current_dir(dir.path())
            .args(["tidy", "--verbose"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("tidying 1 file"),
            "verbose tidy should report file count: {stdout}"
        );
        assert!(
            stdout.contains("hello"),
            "verbose tidy should name the selected package: {stdout}"
        );
        assert!(
            stdout.contains("compile database ="),
            "verbose tidy should show compile DB path: {stdout}"
        );
    }

    #[test]
    fn very_verbose_lists_command_line() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        write_minimal_project(dir.path());

        let assertion = cabin_with_fake_tidy()
            .current_dir(dir.path())
            .args(["tidy", "-vv"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("cabin-tidy-fake-tidy"),
            "very-verbose tidy should echo the executable: {stdout}"
        );
        // Verbose drops the `-quiet` flag from the *spawned*
        // tidy command, but the very-verbose command-line echo
        // reflects what we *would* have run; verbose mode
        // suppresses `-quiet`, so the echo also omits it.
        assert!(
            stdout.contains("-p "),
            "very-verbose tidy should echo the -p flag: {stdout}"
        );
    }

    #[test]
    fn nested_workspace_member_is_not_walked_from_outer_root() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["outer", "outer/nested"]
"#,
            )
            .unwrap();
        dir.child("outer/cabin.toml")
            .write_str(
                r#"[package]
name = "outer"
version = "0.1.0"

[target.outer]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("outer/src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        dir.child("outer/nested/cabin.toml")
            .write_str(
                r#"[package]
name = "nested"
version = "0.1.0"

[target.nested]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("outer/nested/src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();

        let record = dir.path().join("argv.log");
        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "-p", "outer"])
            .assert()
            .success();

        let body = std::fs::read_to_string(&record).unwrap();
        assert!(body.contains("outer/src/main.cc"));
        assert!(
            !body.contains("outer/nested/src/main.cc"),
            "nested member's sources leaked into outer's tidy: {body}"
        );
    }

    #[test]
    fn unrelated_versioned_dependency_does_not_block_selected_package() {
        let _guard = tidy_record_lock();
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["clean", "registry-user"]
"#,
            )
            .unwrap();
        dir.child("clean/cabin.toml")
            .write_str(
                r#"[package]
name = "clean"
version = "0.1.0"

[target.clean]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        dir.child("clean/src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        dir.child("registry-user/cabin.toml")
            .write_str(
                r#"[package]
name = "registry-user"
version = "0.1.0"

[dependencies]
fmt = "1.0"
"#,
            )
            .unwrap();

        let record = dir.path().join("argv.log");
        cabin_with_fake_tidy()
            .current_dir(dir.path())
            .env("CABIN_FAKE_TIDY_RECORD", &record)
            .args(["tidy", "-p", "clean"])
            .assert()
            .success();

        let body = std::fs::read_to_string(&record).unwrap();
        assert!(body.contains("clean/src/main.cc"));
        assert!(!body.contains("registry-user"));
    }

    #[test]
    fn versioned_dependency_produces_actionable_error() {
        // `cabin tidy` does not run the artifact pipeline; if a
        // selected package declares a versioned registry dep we
        // surface a clear unsupported-shape diagnostic instead of
        // letting the planner fail with a confusing downstream
        // error.
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\"]\n\n[dependencies]\nfmt = \"1.0\"\n")

            .unwrap();
        dir.child("src/main.cc")
            .write_str("int main() {}\n")
            .unwrap();

        let assertion = cabin_with_fake_tidy()
            .current_dir(dir.path())
            .arg("tidy")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("versioned registry dependencies"),
            "diagnostic should name the offending situation: {stderr}"
        );
        assert!(
            stderr.contains("does not run the artifact pipeline"),
            "diagnostic should explain why tidy refuses: {stderr}"
        );
        assert!(
            !stderr.contains("cabin build"),
            "diagnostic must not suggest a build/fetch step that tidy cannot use: {stderr}"
        );
    }
}

mod system_deps_pkg_config {
    use super::*;
    use std::path::PathBuf;

    /// Locate the bundled fake `pkg-config` binary the same way
    /// `cabin-fmt` and `cabin-tidy` locate their fakes: walk up
    /// from the test executable to the target dir and look for
    /// the named bin. Build with the `test-fake-pkg-config`
    /// feature on `cabin-system-deps`.
    fn fake_pkg_config_path() -> PathBuf {
        let test_exe = std::env::current_exe().expect("current_exe");
        let mut dir = test_exe
            .parent()
            .expect("test exe should live in a directory")
            .to_path_buf();
        if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
            dir.pop();
        }
        let candidate = dir.join("cabin-system-deps-fake-pkg-config");
        assert!(
            candidate.is_file(),
            "expected fake pkg-config at {}; build cabin-system-deps with `--features test-fake-pkg-config`",
            candidate.display()
        );
        candidate
    }

    /// Pre-built TempDir holding fixture JSON files for the fake
    /// pkg-config. Tests call `.write` to publish a module's
    /// metadata, then point `CABIN_FAKE_PKG_CONFIG_FIXTURES` at
    /// the directory path through the command env.
    pub(super) struct Fixtures {
        dir: TempDir,
    }

    impl Fixtures {
        pub(super) fn new() -> Self {
            Self {
                dir: TempDir::new().expect("tempdir"),
            }
        }

        pub(super) fn write(&self, name: &str, body: &str) {
            assert_fs::fixture::ChildPath::new(self.dir.path().join(format!("{name}.json")))
                .write_str(body)
                .unwrap();
        }

        pub(super) fn path(&self) -> &Path {
            self.dir.path()
        }
    }

    /// Build a `cabin` command pre-loaded with the fake
    /// pkg-config and a freshly-created fixture directory. The
    /// caller publishes fixtures via the returned `Fixtures`
    /// handle.
    pub(super) fn cabin_with_fake_pkg_config(fixtures: &Fixtures) -> Command {
        let mut cmd = cabin();
        cmd.env("CABIN_PKG_CONFIG", fake_pkg_config_path());
        cmd.env("CABIN_FAKE_PKG_CONFIG_FIXTURES", fixtures.path());
        cmd
    }

    /// Manifest declaring exactly one system dependency. Tests
    /// override the requirement / required field by formatting
    /// it as needed.
    fn manifest_with_system_dep(version: &str, required_clause: &str) -> String {
        format!(
            "[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\"]\n\n[dependencies]\nzlib = {{ version = \"{version}\", system = true{required_clause} }}\n",
        )
    }

    fn write_hello_main(root: &Path) {
        assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
    }

    #[test]
    fn build_succeeds_with_no_system_deps_even_when_pkg_config_missing() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
        write_hello_main(dir.path());

        let mut cmd = cabin();
        // Point CABIN_PKG_CONFIG at a path that does not exist;
        // because the manifest has no `system = true` deps,
        // Cabin must not try to spawn pkg-config.
        cmd.env("CABIN_PKG_CONFIG", dir.path().join("missing-pkg-config"));
        // metadata exercises the same code path without
        // requiring a real toolchain.
        cmd.current_dir(dir.path())
            .arg("metadata")
            .assert()
            .success();
    }

    #[test]
    fn metadata_reflects_pkg_config_cflags_in_build_flags_per_package() {
        let fixtures = Fixtures::new();
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include -DZLIB_CONST",
                "libs": "-L/opt/zlib/lib -lz"
            }"#,
        );
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_with_system_dep("", ""))
            .unwrap();
        write_hello_main(dir.path());

        let assertion = cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        let view: serde_json::Value =
            serde_json::from_str(&stdout).expect("metadata output should be JSON");
        let pkg = package_build_flags(&view);
        let includes: Vec<String> = pkg["include_dirs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            includes.iter().any(|p| p == "/opt/zlib/include"),
            "include dirs must reflect pkg-config -I path: {includes:?}",
        );
        let extra_compile: Vec<String> = pkg["extra_compile_args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            extra_compile.contains(&"-DZLIB_CONST".to_owned()),
            "extra compile args must carry non-include cflags: {extra_compile:?}",
        );
        let extra_link: Vec<String> = pkg["ldflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            extra_link,
            vec!["-L/opt/zlib/lib".to_owned(), "-lz".to_owned()],
            "pkg-config --libs must reach the planner verbatim and in order",
        );
    }

    /// Lookup helper: `cabin metadata`'s build flags live under
    /// `toolchain.build_flags_per_package.<name>`. Returns the
    /// first package's block; only one package is declared in
    /// these fixtures.
    fn package_build_flags(view: &serde_json::Value) -> &serde_json::Value {
        let per_package = view["toolchain"]["build_flags_per_package"]
            .as_object()
            .expect("toolchain.build_flags_per_package object");
        per_package
            .values()
            .next()
            .expect("at least one package with build flags")
    }

    #[test]
    fn metadata_fails_when_system_dep_is_missing() {
        let fixtures = Fixtures::new();
        // No fixture published; fake pkg-config will report
        // "not found".
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_with_system_dep("", ""))
            .unwrap();
        write_hello_main(dir.path());

        let assertion = cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("zlib"),
            "diagnostic should name the missing dep: {stderr}",
        );
        assert!(
            stderr.contains("not found"),
            "diagnostic should describe the failure mode: {stderr}",
        );
    }

    #[test]
    fn metadata_fails_when_system_dep_version_unsatisfied() {
        let fixtures = Fixtures::new();
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.1.0",
                "cflags": "",
                "libs": "-lz"
            }"#,
        );
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_with_system_dep(">=2", ""))
            .unwrap();
        write_hello_main(dir.path());

        let assertion = cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("zlib"),
            "diagnostic should name the dep: {stderr}",
        );
        assert!(
            stderr.contains(">=2"),
            "diagnostic should quote the requirement: {stderr}",
        );
        assert!(
            stderr.contains("1.1.0"),
            "diagnostic should report the installed version: {stderr}",
        );
    }

    #[test]
    fn metadata_fails_when_pkg_config_missing_and_system_dep_declared() {
        let fixtures = Fixtures::new();
        let dir = TempDir::new().unwrap();
        let missing_pkg_config = dir.path().join("nope-pkg-config");
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_with_system_dep("", ""))
            .unwrap();
        write_hello_main(dir.path());

        let mut cmd = cabin();
        cmd.env("CABIN_PKG_CONFIG", &missing_pkg_config);
        cmd.env("CABIN_FAKE_PKG_CONFIG_FIXTURES", fixtures.path());
        let assertion = cmd
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("not found"),
            "diagnostic should mention `not found`: {stderr}",
        );
        assert!(
            stderr.contains("CABIN_PKG_CONFIG"),
            "diagnostic should mention the override env var: {stderr}",
        );
    }

    #[test]
    fn cabin_pkg_config_env_var_overrides_executable() {
        // A fixture-publishing test that depends on the env var
        // being honored. If the env var were ignored, the test
        // would fail to spawn pkg-config and metadata would error.
        let fixtures = Fixtures::new();
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include",
                "libs": "-lz"
            }"#,
        );
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_with_system_dep("", ""))
            .unwrap();
        write_hello_main(dir.path());

        cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .success();
    }

    #[test]
    fn manifest_rejects_required_field_on_system_dep() {
        // System dependencies are unconditionally required. The
        // CLI must reject any attempt to declare `required = …`
        // with a diagnostic that explicitly names the offending
        // field — the snippet alone is too weak because the source
        // line happens to contain the field name regardless.
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_with_system_dep(">=1", ", required = false"))
            .unwrap();
        write_hello_main(dir.path());

        let assertion = cabin()
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("unknown field `required`"),
            "diagnostic should call out the unknown field by name: {stderr}",
        );
    }

    #[test]
    fn build_compile_commands_carry_include_paths_from_pkg_config() {
        if !build_tools_available() {
            skip(
                "build_compile_commands_carry_include_paths_from_pkg_config",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let fixtures = Fixtures::new();
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include -DZLIB_CONST",
                "libs": "-L/opt/zlib/lib -lz"
            }"#,
        );
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_with_system_dep("", ""))
            .unwrap();
        write_hello_main(dir.path());

        cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("build")
            .assert()
            .success();

        let ccdb_path = dir.path().join("build/dev/compile_commands.json");
        let ccdb = std::fs::read_to_string(&ccdb_path).expect("compile_commands.json");
        // The planner emits include directories as two argv
        // tokens (`-I` followed by the path) so the rendered
        // command string contains them with a space between.
        assert!(
            ccdb.contains("-I /opt/zlib/include"),
            "compile_commands.json must carry pkg-config -I: {ccdb}",
        );
        assert!(
            ccdb.contains("-DZLIB_CONST"),
            "compile_commands.json must carry pkg-config -D: {ccdb}",
        );

        let ninja_path = dir.path().join("build/dev/build.ninja");
        let ninja = std::fs::read_to_string(&ninja_path).expect("build.ninja");
        assert!(
            ninja.contains("-lz"),
            "build.ninja link command must carry pkg-config -l: {ninja}",
        );
        assert!(
            ninja.contains("-L/opt/zlib/lib"),
            "build.ninja link command must carry pkg-config -L: {ninja}",
        );
    }

    #[test]
    fn fingerprint_moves_when_pkg_config_flags_change() {
        let fixtures = Fixtures::new();
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include",
                "libs": "-lz"
            }"#,
        );
        let dir = TempDir::new().unwrap();
        // The metadata view only emits `configuration` (and
        // hence the fingerprint) when the package declares at
        // least one feature. Declare a trivial feature so the
        // fingerprint surface is populated.
        dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\"]\n\n[features]\ndefault = []\nflag-a = []\n\n[dependencies]\nzlib = { version = \"\", system = true }\n")

            .unwrap();
        write_hello_main(dir.path());

        let stdout1 = String::from_utf8_lossy(
            &cabin_with_fake_pkg_config(&fixtures)
                .current_dir(dir.path())
                .arg("metadata")
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .to_string();
        let view1: serde_json::Value = serde_json::from_str(&stdout1).unwrap();
        let fp1 = find_fingerprint(&view1);

        // Republish with different libs — the discovered link
        // args change, so the fingerprint must move.
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include",
                "libs": "-lz -lother"
            }"#,
        );
        let stdout2 = String::from_utf8_lossy(
            &cabin_with_fake_pkg_config(&fixtures)
                .current_dir(dir.path())
                .arg("metadata")
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .to_string();
        let view2: serde_json::Value = serde_json::from_str(&stdout2).unwrap();
        let fp2 = find_fingerprint(&view2);

        assert_ne!(
            fp1, fp2,
            "fingerprint must move when discovered pkg-config flags change",
        );
    }

    /// Walk the metadata view looking for the first build-config
    /// fingerprint. Build-configurations live under
    /// `configurations.<package>.fingerprint`; the value is a
    /// hex string. Robust against schema reshuffles.
    fn find_fingerprint(value: &serde_json::Value) -> String {
        fn walk(v: &serde_json::Value) -> Option<String> {
            if let Some(map) = v.as_object() {
                if let Some(fp) = map.get("fingerprint").and_then(|f| f.as_str()) {
                    return Some(fp.to_owned());
                }
                for child in map.values() {
                    if let Some(found) = walk(child) {
                        return Some(found);
                    }
                }
            }
            if let Some(arr) = v.as_array() {
                for item in arr {
                    if let Some(found) = walk(item) {
                        return Some(found);
                    }
                }
            }
            None
        }
        walk(value).expect("metadata view should expose a fingerprint")
    }

    #[test]
    fn non_matching_target_conditional_system_dep_does_not_require_pkg_config() {
        // Declare a system dep gated on a condition that the
        // host platform cannot match. Cabin must not spawn
        // pkg-config — and the integration test exercises that
        // by pointing `CABIN_PKG_CONFIG` at a non-existent path.
        let dir = TempDir::new().unwrap();
        let unreachable = dir.path().join("never-reached-pkg-config");
        dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\"]\n\n[target.'cfg(os = \"none-such\")'.dependencies]\nzlib = { version = \"\", system = true }\n")

            .unwrap();
        write_hello_main(dir.path());

        let mut cmd = cabin();
        cmd.env("CABIN_PKG_CONFIG", &unreachable);
        cmd.current_dir(dir.path())
            .arg("metadata")
            .assert()
            .success();
    }

    #[test]
    fn matching_target_conditional_system_dep_is_probed() {
        let fixtures = Fixtures::new();
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include",
                "libs": "-lz"
            }"#,
        );
        let dir = TempDir::new().unwrap();
        let host_os = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            "linux"
        };
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))

            .write_str(&format!(
                "[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"cpp_executable\"\nsources = [\"src/main.cc\"]\n\n[target.'cfg(os = \"{host_os}\")'.dependencies]\nzlib = {{ version = \"\", system = true }}\n",
            ))

            .unwrap();
        write_hello_main(dir.path());

        let assertion = cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        let view: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let pkg = package_build_flags(&view);
        let includes: Vec<String> = pkg["include_dirs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            includes.iter().any(|p| p == "/opt/zlib/include"),
            "matching conditional system dep must contribute flags: {includes:?}",
        );
    }

    #[test]
    fn verbose_mode_prints_probe_progress() {
        let fixtures = Fixtures::new();
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.2.13",
                "cflags": "",
                "libs": "-lz"
            }"#,
        );
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_with_system_dep("", ""))
            .unwrap();
        write_hello_main(dir.path());

        let assertion = cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("-v")
            .arg("metadata")
            .assert()
            .success();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("probing"),
            "verbose stderr should mention probing: {stderr}",
        );
        assert!(
            stderr.contains("zlib"),
            "verbose stderr should mention the dep name: {stderr}",
        );
        assert!(
            stderr.contains("1.2.13"),
            "verbose stderr should mention the resolved version: {stderr}",
        );
    }

    #[test]
    fn metadata_stdout_stays_clean_under_verbose_with_system_deps() {
        let fixtures = Fixtures::new();
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.2.13",
                "cflags": "",
                "libs": "-lz"
            }"#,
        );
        let dir = TempDir::new().unwrap();
        assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
            .write_str(&manifest_with_system_dep("", ""))
            .unwrap();
        write_hello_main(dir.path());

        let assertion = cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("-v")
            .arg("metadata")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        // Stdout must still be parseable JSON — probe chatter
        // belongs on stderr only.
        let _view: serde_json::Value =
            serde_json::from_str(&stdout).expect("metadata stdout must remain valid JSON under -v");
    }
}

/// Integration tests for the conventional C / C++ build-flag
/// environment variables: `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, and
/// `LDFLAGS`.  These cover the parsing, ordering, fingerprint,
/// pkg-config interaction, and `cabin fmt` isolation
/// requirements.
mod env_build_flags {
    use super::*;

    /// `metadata`'s JSON view exposes per-package build flags
    /// under `toolchain.build_flags_per_package`.  Returns the
    /// first package's block; tests in this module declare one
    /// primary package.
    fn package_build_flags(view: &serde_json::Value) -> &serde_json::Value {
        let per_package = view["toolchain"]["build_flags_per_package"]
            .as_object()
            .expect("toolchain.build_flags_per_package object");
        per_package
            .values()
            .next()
            .expect("at least one package with build flags")
    }

    /// Read the build-configuration fingerprint of the
    /// (sole) primary package via `cabin explain build-config`.
    /// `metadata`'s JSON view only exposes per-package
    /// configurations when features are non-empty; `explain
    /// build-config` always renders the full configuration block
    /// including the fingerprint, so we route the fingerprint
    /// assertions through it.
    fn fingerprint_for(dir: &Path, cmd_env: &[(&str, &str)], package: &str) -> String {
        let mut cmd = cabin();
        for (k, v) in cmd_env {
            cmd.env(k, v);
        }
        let assertion = cmd
            .current_dir(dir)
            .args(["explain", "build-config", package, "--format", "json"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        let v: serde_json::Value = serde_json::from_str(&stdout).expect("explain JSON");
        v["configuration"]["fingerprint"]
            .as_str()
            .expect("fingerprint string")
            .to_owned()
    }

    fn write_simple_project(dir: &Path) {
        assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
            .write_str(VALID_MANIFEST)
            .unwrap();
        assert_fs::fixture::ChildPath::new(dir.join("src/main.cc"))
            .write_str(HELLO_MAIN_CC)
            .unwrap();
    }

    fn metadata_view(cmd_env: &[(&str, &str)], dir: &Path) -> serde_json::Value {
        let mut cmd = cabin();
        for (k, v) in cmd_env {
            cmd.env(k, v);
        }
        let assertion = cmd.current_dir(dir).arg("metadata").assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        serde_json::from_str(&stdout).expect("metadata stdout must be JSON")
    }

    #[test]
    fn cppflags_appear_in_language_neutral_compile_args() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let view = metadata_view(
            &[("CPPFLAGS", "-DENV_FROM_CPP=1 -I/opt/include")],
            dir.path(),
        );
        let pkg = package_build_flags(&view);
        let extras: Vec<String> = pkg["extra_compile_args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            extras.contains(&"-DENV_FROM_CPP=1".to_owned()),
            "CPPFLAGS must reach language-neutral bucket: {extras:?}",
        );
        assert!(
            extras.contains(&"-I/opt/include".to_owned()),
            "CPPFLAGS tokens preserved verbatim: {extras:?}",
        );
        // CPPFLAGS must not leak into the C-only / C++-only
        // buckets — that would defeat the documented per-bucket
        // routing.
        let c_only: Vec<String> = pkg["cflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let cxx_only: Vec<String> = pkg["cxxflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            c_only.is_empty(),
            "CPPFLAGS must not enter C-only bucket: {c_only:?}"
        );
        assert!(
            cxx_only.is_empty(),
            "CPPFLAGS must not enter C++-only bucket: {cxx_only:?}"
        );
    }

    #[test]
    fn cflags_only_reach_c_compile_bucket() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let view = metadata_view(&[("CFLAGS", "-std=c11 -Wmissing-prototypes")], dir.path());
        let pkg = package_build_flags(&view);
        let c_only: Vec<String> = pkg["cflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(c_only, vec!["-std=c11", "-Wmissing-prototypes"]);
        let cxx_only: Vec<String> = pkg["cxxflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            cxx_only.is_empty(),
            "CFLAGS must never reach C++ bucket: {cxx_only:?}"
        );
        let link: Vec<String> = pkg["ldflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            link.is_empty(),
            "CFLAGS must never reach link bucket: {link:?}"
        );
    }

    #[test]
    fn cxxflags_only_reach_cxx_compile_bucket() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let view = metadata_view(&[("CXXFLAGS", "-fno-rtti -fno-exceptions")], dir.path());
        let pkg = package_build_flags(&view);
        let cxx_only: Vec<String> = pkg["cxxflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(cxx_only, vec!["-fno-rtti", "-fno-exceptions"]);
        let c_only: Vec<String> = pkg["cflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            c_only.is_empty(),
            "CXXFLAGS must never reach C bucket: {c_only:?}"
        );
    }

    #[test]
    fn ldflags_only_reach_link_bucket() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let view = metadata_view(&[("LDFLAGS", "-L/opt/lib -lextra")], dir.path());
        let pkg = package_build_flags(&view);
        let link: Vec<String> = pkg["ldflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(link, vec!["-L/opt/lib", "-lextra"]);
        let extras: Vec<String> = pkg["extra_compile_args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert!(
            extras.is_empty(),
            "LDFLAGS must never reach compile bucket: {extras:?}"
        );
    }

    #[test]
    fn empty_and_whitespace_env_vars_are_ignored() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        // Metadata only emits a per-package build-flags block
        // when the package contributes at least one non-empty
        // bucket. Empty / whitespace env vars must produce no
        // contribution, so the map stays empty and the
        // fingerprint matches the no-env baseline.
        let view_empty = metadata_view(
            &[
                ("CPPFLAGS", ""),
                ("CFLAGS", "  \t  "),
                ("CXXFLAGS", "\n"),
                ("LDFLAGS", ""),
            ],
            dir.path(),
        );
        let per_package = view_empty["toolchain"]["build_flags_per_package"]
            .as_object()
            .expect("toolchain.build_flags_per_package object");
        assert!(
            per_package.is_empty(),
            "empty / whitespace env vars must produce no flag contribution: {per_package:?}",
        );

        // Mirror through the fingerprint: identical to the
        // unset baseline.
        let base_fp = fingerprint_for(dir.path(), &[], "hello");
        let empty_fp = fingerprint_for(
            dir.path(),
            &[
                ("CPPFLAGS", ""),
                ("CFLAGS", "  \t  "),
                ("CXXFLAGS", "\n"),
                ("LDFLAGS", ""),
            ],
            "hello",
        );
        assert_eq!(base_fp, empty_fp);
    }

    #[test]
    fn quoted_and_escaped_arguments_parse_correctly() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        // Single-quoted run preserves spaces verbatim; the whole
        // -DNAME="hello world" is one argv element.
        let view = metadata_view(&[("CXXFLAGS", "-DNAME='hello world' -O\\ 2")], dir.path());
        let pkg = package_build_flags(&view);
        let cxx: Vec<String> = pkg["cxxflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            cxx,
            vec!["-DNAME=hello world".to_owned(), "-O 2".to_owned(),],
        );
    }

    #[test]
    fn malformed_quote_errors_name_variable() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .env("CXXFLAGS", "'oops")
            .arg("metadata")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("CXXFLAGS"),
            "error must name CXXFLAGS: {stderr}"
        );
        assert!(
            stderr.contains("shell"),
            "error must explain the parse issue: {stderr}",
        );
    }

    #[test]
    fn malformed_escape_errors_name_variable() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .env("LDFLAGS", "-L/lib\\")
            .arg("metadata")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("LDFLAGS"),
            "error must name LDFLAGS: {stderr}"
        );
        assert!(
            stderr.contains("shell"),
            "error must explain the parse issue: {stderr}",
        );
    }

    #[test]
    fn order_preserved_within_a_single_variable() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let view = metadata_view(&[("CPPFLAGS", "-Dfirst -Dsecond -Dthird")], dir.path());
        let pkg = package_build_flags(&view);
        let extras: Vec<String> = pkg["extra_compile_args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(extras, vec!["-Dfirst", "-Dsecond", "-Dthird"]);
    }

    #[test]
    fn env_flags_append_after_manifest_layer() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "cpp_executable"
sources = ["src/main.cc"]

[profile]
cxxflags = ["-DFROM_MANIFEST"]
ldflags = ["-Wl,--as-needed"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
        let view = metadata_view(
            &[("CXXFLAGS", "-DFROM_ENV"), ("LDFLAGS", "-L/from/env")],
            dir.path(),
        );
        let pkg = package_build_flags(&view);
        let cxx: Vec<String> = pkg["cxxflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            cxx,
            vec!["-DFROM_MANIFEST", "-DFROM_ENV"],
            "env flags must append *after* manifest [profile] flags",
        );
        let link: Vec<String> = pkg["ldflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(link, vec!["-Wl,--as-needed", "-L/from/env"]);
    }

    #[test]
    fn fingerprint_changes_when_env_flag_changes() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());

        let base_fp = fingerprint_for(dir.path(), &[], "hello");

        let cpp_fp = fingerprint_for(dir.path(), &[("CPPFLAGS", "-DENV1=1")], "hello");
        assert_ne!(base_fp, cpp_fp, "CPPFLAGS change must move the fingerprint");

        let cflags_fp = fingerprint_for(dir.path(), &[("CFLAGS", "-std=c11")], "hello");
        assert_ne!(
            base_fp, cflags_fp,
            "CFLAGS change must move the fingerprint"
        );
        assert_ne!(
            cpp_fp, cflags_fp,
            "CFLAGS and CPPFLAGS must hash into different fingerprints because they route to different buckets",
        );

        let cxx_fp = fingerprint_for(dir.path(), &[("CXXFLAGS", "-fno-rtti")], "hello");
        assert_ne!(base_fp, cxx_fp, "CXXFLAGS change must move the fingerprint");

        let ld_fp = fingerprint_for(dir.path(), &[("LDFLAGS", "-L/opt")], "hello");
        assert_ne!(base_fp, ld_fp, "LDFLAGS change must move the fingerprint");
    }

    #[test]
    fn fingerprint_is_deterministic_for_identical_env() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let env: &[(&str, &str)] = &[
            ("CPPFLAGS", "-DSHARED=1"),
            ("CFLAGS", "-std=c11"),
            ("CXXFLAGS", "-std=c++20"),
            ("LDFLAGS", "-L/opt/lib"),
        ];
        assert_eq!(
            fingerprint_for(dir.path(), env, "hello"),
            fingerprint_for(dir.path(), env, "hello"),
        );
    }

    #[cfg(unix)]
    #[test]
    fn cabin_build_emits_cppflags_into_compile_commands_for_cxx_sources() {
        if !build_tools_available() {
            skip(
                "cabin_build_emits_cppflags_into_compile_commands_for_cxx_sources",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .env("CPPFLAGS", "-DBUILD_FROM_ENV")
            .args(["build", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let cc =
            std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
        assert!(
            cc.contains("-DBUILD_FROM_ENV"),
            "CPPFLAGS must appear in compile_commands.json: {cc}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn cabin_build_emits_cxxflags_only_for_cxx_translation_units() {
        if !build_tools_available() || !c_compiler_available() {
            skip(
                "cabin_build_emits_cxxflags_only_for_cxx_translation_units",
                "ninja or a C / C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "mixed"
version = "0.1.0"

[target.mixed]
type = "cpp_executable"
sources = ["src/main.cc", "src/helper.c"]
"#,
            )
            .unwrap();
        dir.child("src/main.cc")
            .write_str("extern \"C\" void helper(void);\nint main() { helper(); return 0; }\n")
            .unwrap();
        dir.child("src/helper.c")
            .write_str("void helper(void) {}\n")
            .unwrap();
        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .env("CXXFLAGS", "-DSEEN_BY_CXX_ONLY")
            .env("CFLAGS", "-DSEEN_BY_C_ONLY")
            .args(["build", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let cc =
            std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
        let view: serde_json::Value = serde_json::from_str(&cc).unwrap();
        let entries = view.as_array().expect("compile_commands is an array");
        let mut cxx_seen = false;
        let mut c_seen = false;
        for entry in entries {
            let file = entry["file"].as_str().unwrap();
            // The compile DB stores each invocation as a single
            // `command` string; the planner does not emit the
            // alternate `arguments` array form.
            let command = entry["command"].as_str().unwrap();
            if file.ends_with("main.cc") {
                cxx_seen = true;
                assert!(
                    command.contains("-DSEEN_BY_CXX_ONLY"),
                    "C++ compile must include CXXFLAGS: {command}",
                );
                assert!(
                    !command.contains("-DSEEN_BY_C_ONLY"),
                    "C++ compile must NOT include CFLAGS: {command}",
                );
            } else if file.ends_with("helper.c") {
                c_seen = true;
                assert!(
                    command.contains("-DSEEN_BY_C_ONLY"),
                    "C compile must include CFLAGS: {command}",
                );
                assert!(
                    !command.contains("-DSEEN_BY_CXX_ONLY"),
                    "C compile must NOT include CXXFLAGS: {command}",
                );
            }
        }
        assert!(
            cxx_seen && c_seen,
            "expected both C and C++ entries in the compile DB"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cabin_build_ldflags_appear_in_ninja_link_command() {
        if !build_tools_available() {
            skip(
                "cabin_build_ldflags_appear_in_ninja_link_command",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        // Use a benign LDFLAG the host linker accepts silently
        // so the build phase succeeds and we can read the
        // generated artifacts.  `-L<path>` adds a library search
        // path with no requirement that the path exist.
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let build_dir = dir.path().join("build");
        let distinctive = "-L/this/path/should/not/exist/very-distinctive";
        cabin()
            .current_dir(dir.path())
            .env("LDFLAGS", distinctive)
            .args(["build", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let ninja_text =
            std::fs::read_to_string(build_dir.join("dev").join("build.ninja")).unwrap();
        assert!(
            ninja_text.contains(distinctive),
            "LDFLAGS must reach the link command in build.ninja: {ninja_text}",
        );
        // And must NOT contaminate compile lines.
        let cc =
            std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
        assert!(
            !cc.contains(distinctive),
            "LDFLAGS must NOT appear in compile_commands.json: {cc}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn ninja_rebuilds_when_cxxflags_change() {
        if !build_tools_available() {
            skip(
                "ninja_rebuilds_when_cxxflags_change",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .env("CXXFLAGS", "-DFIRST_BUILD")
            .args(["build", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let first = std::fs::read_to_string(build_dir.join("dev").join("build.ninja")).unwrap();
        cabin()
            .current_dir(dir.path())
            .env("CXXFLAGS", "-DSECOND_BUILD")
            .args(["build", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let second = std::fs::read_to_string(build_dir.join("dev").join("build.ninja")).unwrap();
        assert!(
            first.contains("-DFIRST_BUILD") && !first.contains("-DSECOND_BUILD"),
            "first build.ninja should pin the first flag value",
        );
        assert!(
            second.contains("-DSECOND_BUILD") && !second.contains("-DFIRST_BUILD"),
            "second build.ninja should pin the second flag value",
        );
    }

    #[cfg(unix)]
    #[test]
    fn cabin_run_build_phase_uses_env_flags() {
        if !build_tools_available() {
            skip(
                "cabin_run_build_phase_uses_env_flags",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .env("CPPFLAGS", "-DRUN_PHASE_FLAG")
            .args(["run", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let cc =
            std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
        assert!(
            cc.contains("-DRUN_PHASE_FLAG"),
            "cabin run must propagate CPPFLAGS to the build phase: {cc}",
        );
    }

    #[cfg(unix)]
    #[test]
    fn cabin_test_build_phase_uses_env_flags() {
        if !build_tools_available() {
            skip(
                "cabin_test_build_phase_uses_env_flags",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "hello"
version = "0.1.0"

[target.smoke]
type = "cpp_test"
sources = ["src/test.cc"]
"#,
            )
            .unwrap();
        dir.child("src/test.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .env("CXXFLAGS", "-DTEST_PHASE_FLAG")
            .args(["test", "--build-dir"])
            .arg(&build_dir)
            .assert()
            .success();
        let cc =
            std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
        assert!(
            cc.contains("-DTEST_PHASE_FLAG"),
            "cabin test must propagate CXXFLAGS to the build phase: {cc}",
        );
    }

    /// `cabin tidy` regenerates the compile database from the
    /// same build planner the other commands use, so env flags
    /// must reach the on-disk `compile_commands.json` it writes.
    #[cfg(unix)]
    #[test]
    fn cabin_tidy_compile_db_sees_env_flags() {
        if !build_tools_available() {
            skip(
                "cabin_tidy_compile_db_sees_env_flags",
                "ninja or a C++ compiler is not available",
            );
            return;
        }
        // Use the fake tidy so the test does not require a real
        // clang-tidy install; cabin still regenerates the
        // compile database before invoking the tool.  `cabin
        // tidy` reads the build directory via `CABIN_BUILD_DIR`.
        let fake_tidy = fake_tidy_path();
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let build_dir = dir.path().join("build");
        cabin()
            .current_dir(dir.path())
            .env("CABIN_TIDY", &fake_tidy)
            .env("CABIN_BUILD_DIR", &build_dir)
            .env("CPPFLAGS", "-DTIDY_DB_SEES_THIS")
            .arg("tidy")
            .assert()
            .success();
        let cc =
            std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
        assert!(
            cc.contains("-DTIDY_DB_SEES_THIS"),
            "cabin tidy compile DB must include CPPFLAGS: {cc}",
        );
    }

    /// Mirrors the bundled fake-binary lookup the tidy module
    /// uses; we keep it local rather than re-export across mod
    /// boundaries.
    fn fake_tidy_path() -> std::path::PathBuf {
        let test_exe = std::env::current_exe().expect("current_exe");
        let mut dir = test_exe
            .parent()
            .expect("test exe should live in a directory")
            .to_path_buf();
        if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
            dir.pop();
        }
        let candidate = dir.join("cabin-tidy-fake-tidy");
        assert!(
            candidate.is_file(),
            "expected fake tidy at {}; build cabin-tidy with `--features test-fake-tidy`",
            candidate.display(),
        );
        candidate
    }

    /// pkg-config and env-flag layers must coexist deterministically.
    /// pkg-config goes in first (already merged into
    /// `ResolvedProfileFlags` by `augment_build_flags_with_system_deps`),
    /// then env flags append.
    #[test]
    fn pkg_config_and_env_flags_coexist_in_documented_order() {
        let fixtures = system_deps_pkg_config::Fixtures::new();
        fixtures.write(
            "zlib",
            r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include -DZLIB_CONST",
                "libs": "-L/opt/zlib/lib -lz"
            }"#,
        );
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "cpp_executable"
sources = ["src/main.cc"]

[dependencies]
zlib = { version = "", system = true }
"#,
            )
            .unwrap();
        dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

        let mut cmd = system_deps_pkg_config::cabin_with_fake_pkg_config(&fixtures);
        cmd.env("CPPFLAGS", "-DFROM_ENV");
        cmd.env("LDFLAGS", "-L/from/env");
        let assertion = cmd
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        let view: serde_json::Value = serde_json::from_str(&stdout).expect("metadata JSON");
        let pkg = package_build_flags(&view);
        let extras: Vec<String> = pkg["extra_compile_args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        // pkg-config contributed `-DZLIB_CONST`; env adds
        // `-DFROM_ENV`. The pkg-config entry must come first.
        let env_pos = extras
            .iter()
            .position(|s| s == "-DFROM_ENV")
            .expect("env CPPFLAGS present");
        let pkg_pos = extras
            .iter()
            .position(|s| s == "-DZLIB_CONST")
            .expect("pkg-config define present");
        assert!(
            pkg_pos < env_pos,
            "pkg-config define must precede env CPPFLAGS in deterministic order: {extras:?}",
        );

        let link: Vec<String> = pkg["ldflags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let env_link = link
            .iter()
            .position(|s| s == "-L/from/env")
            .expect("env LDFLAGS present in link args");
        let pkg_link = link
            .iter()
            .position(|s| s == "-L/opt/zlib/lib")
            .expect("pkg-config -L present in link args");
        assert!(
            pkg_link < env_link,
            "pkg-config link flags must precede env LDFLAGS: {link:?}",
        );
    }

    /// `cabin fmt` must ignore the build-flag environment.
    /// Regression: a bad CFLAGS should not block formatter
    /// invocations.
    #[test]
    fn cabin_fmt_unaffected_by_build_flag_env() {
        // `cabin fmt --check` may succeed or fail depending on
        // whether a real `clang-format` is on PATH; either is
        // acceptable.  The only behavior we forbid is the
        // env-flag parser leaking through — stderr must never
        // name CXXFLAGS for an `fmt` invocation.
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .env("CXXFLAGS", "'never parsed")
            .arg("fmt")
            .arg("--check")
            .assert();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains("CXXFLAGS"),
            "cabin fmt must not parse CXXFLAGS: {stderr}",
        );
    }

    /// `cabin clean`, `cabin new`, `cabin init` are workspace /
    /// scaffold commands that must not be affected by build
    /// flags.
    #[test]
    fn cabin_new_unaffected_by_build_flag_env() {
        let dir = TempDir::new().unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .env("CXXFLAGS", "'unterminated")
            .args(["new", "demo", "--bin"])
            .assert();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains("CXXFLAGS"),
            "cabin new must not parse CXXFLAGS: {stderr}",
        );
    }

    #[test]
    fn cabin_init_unaffected_by_build_flag_env() {
        let dir = TempDir::new().unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .env("CPPFLAGS", "'unterminated")
            .args(["init", "--bin"])
            .assert();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains("CPPFLAGS"),
            "cabin init must not parse CPPFLAGS: {stderr}",
        );
    }

    #[test]
    fn cabin_clean_unaffected_by_build_flag_env() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .env("LDFLAGS", "-L/lib\\")
            .arg("clean")
            .assert();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            !stderr.contains("LDFLAGS"),
            "cabin clean must not parse LDFLAGS: {stderr}",
        );
    }

    /// `cabin metadata --format json` must stay parseable JSON
    /// on stdout even with env flags active and verbose enabled.
    #[test]
    fn metadata_stdout_stays_clean_with_env_flags_under_verbose() {
        let dir = TempDir::new().unwrap();
        write_simple_project(dir.path());
        let assertion = cabin()
            .current_dir(dir.path())
            .env("CPPFLAGS", "-DCHATTY=1")
            .env("LDFLAGS", "-L/opt/lib")
            .args(["-v", "metadata"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        let _view: serde_json::Value =
            serde_json::from_str(&stdout).expect("metadata stdout JSON under -v");
    }
}

mod version_output {
    //! End-to-end coverage for the `cabin version` subcommand.
    //!
    //! `cabin --version` is the clap-framework spelling and
    //! continues to work.  `cabin version` is the dedicated
    //! subcommand:
    //! - concise output by default;
    //! - verbose key/value block under `-v` (or the global
    //!   `cabin -v version` form);
    //! - quiet does not suppress the requested version output;
    //! - missing optional metadata renders as `unknown`, never
    //!   as an error;
    //! - output never leaks private local paths, usernames, or
    //!   hostnames.

    use super::*;

    /// Run `cabin version <args>`, assert success, and return
    /// the captured stdout as a `String`.  Stderr is checked to
    /// be empty so a future regression that prints status
    /// chatter to the wrong stream is caught.
    fn run_version(args: &[&str]) -> String {
        let assertion = cabin().args(args).assert().success();
        let out = assertion.get_output();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            stderr.is_empty(),
            "cabin {args:?} stderr should be empty: {stderr}"
        );
        String::from_utf8(out.stdout.clone()).expect("stdout should be utf-8")
    }

    #[test]
    fn version_subcommand_prints_concise_release_name() {
        let stdout = run_version(&["version"]);
        // Matches the wording of `cabin --version`: `cabin
        // <semver>` followed by a newline.  The workspace
        // version drives the value.
        assert_eq!(stdout, "cabin 0.14.0\n");
    }

    #[test]
    fn top_level_dash_dash_version_still_works() {
        let stdout = run_version(&["--version"]);
        // clap renders the same line; `cabin --version` and
        // `cabin version` agree on the concise wording.
        assert_eq!(stdout, "cabin 0.14.0\n");
    }

    #[test]
    fn top_level_dash_v_short_still_works() {
        // The clap-framework `-V` short alias must keep working
        // even after the new `version` subcommand is added.
        let stdout = run_version(&["-V"]);
        assert_eq!(stdout, "cabin 0.14.0\n");
    }

    #[test]
    fn version_verbose_emits_cargo_style_block() {
        let stdout = run_version(&["version", "-v"]);
        // The header line is `cabin <semver>` plus an optional
        // `(<short-hash> <date>)` parenthetical when git
        // metadata is available.  Pin the prefix without
        // coupling the test to the build's git state.
        let first_line = stdout.lines().next().expect("at least one line");
        assert!(
            first_line.starts_with("cabin 0.14.0"),
            "first line should be the release banner: {first_line}"
        );
        // `release:` is always emitted; `commit-hash:` /
        // `commit-date:` / `host:` / `os:` are conditional on
        // their underlying source being available.
        assert!(
            stdout.contains("release: 0.14.0"),
            "verbose version missing `release:` line: {stdout}"
        );
    }

    #[test]
    fn version_verbose_long_form_matches_short_form() {
        let short = run_version(&["version", "-v"]);
        let long = run_version(&["version", "--verbose"]);
        assert_eq!(
            short, long,
            "`-v` and `--verbose` should produce identical output"
        );
    }

    #[test]
    fn global_verbose_before_subcommand_also_triggers_verbose() {
        // The global `-v` flag is documented as the verbosity
        // entry point; `cabin -v version` should match
        // `cabin version -v` byte-for-byte.
        let trailing = run_version(&["version", "-v"]);
        let leading = run_version(&["-v", "version"]);
        assert_eq!(
            trailing, leading,
            "`cabin -v version` and `cabin version -v` should agree"
        );
    }

    #[test]
    fn verbose_emits_fields_in_deterministic_order() {
        // Two consecutive runs must produce identical output —
        // build-time fields are captured once and the runtime
        // OS probe is deterministic on a stable host.
        let first = run_version(&["version", "-v"]);
        let second = run_version(&["version", "-v"]);
        assert_eq!(first, second);
        // The released cargo-style banner is:
        //
        //     cabin <semver> [(<short-hash> <date>)]
        //     release: <semver>
        //     commit-hash: <full-hash>    (optional)
        //     commit-date: <date>         (optional)
        //     host: <triple>              (optional)
        //     os: <os string>             (optional)
        //
        // Walk the optional rows in order; whichever are
        // present must appear in this sequence.
        let canonical_order = ["release", "commit-hash", "commit-date", "host", "os"];
        let observed: Vec<&str> = first
            .lines()
            .skip(1) // header line has no label
            .filter_map(|line| line.split(':').next())
            .collect();
        let mut next = 0;
        for label in &observed {
            let position = canonical_order
                .iter()
                .position(|known| known == label)
                .unwrap_or_else(|| panic!("unexpected label `{label}` in {first}"));
            assert!(
                position >= next,
                "labels appeared out of order: `{label}` before expected slot ({first})"
            );
            next = position;
        }
        // `release:` must always appear; the others are
        // conditional but at least one further row should print
        // on a typical developer or CI host.
        assert!(observed.contains(&"release"));
    }

    #[test]
    fn version_quiet_does_not_suppress_output() {
        // Quiet only suppresses Cabin-owned status chatter;
        // version output is the requested command output and
        // must still print.
        let stdout = run_version(&["version", "-q"]);
        assert_eq!(stdout, "cabin 0.14.0\n");
        let stdout_leading = run_version(&["-q", "version"]);
        assert_eq!(stdout_leading, "cabin 0.14.0\n");
    }

    #[test]
    fn version_verbose_host_matches_target_triple_shape() {
        let stdout = run_version(&["version", "-v"]);
        // The host triple uses cargo's canonical
        // `<arch>-<vendor>-<os>[-<env>]` shape; pin only the
        // structural hyphen count so the test stays portable
        // across CI architectures.
        let host_line = stdout
            .lines()
            .find(|line| line.starts_with("host:"))
            .unwrap_or_else(|| panic!("verbose version missing `host:` line: {stdout}"));
        let triple = host_line["host:".len()..].trim();
        assert!(
            triple.matches('-').count() >= 2,
            "host triple should have at least two dashes: {triple}"
        );
    }

    #[test]
    fn version_verbose_never_leaks_local_filesystem_paths() {
        let stdout = run_version(&["version", "-v"]);
        // Hard-coded prefixes used by macOS, Linux, and the
        // common temp / opt directories: none of these belong
        // anywhere in stable version output.
        for needle in ["/Users/", "/home/", "/private/", "/opt/", "/tmp/", "/root/"] {
            assert!(
                !stdout.contains(needle),
                "verbose version unexpectedly contains `{needle}`: {stdout}"
            );
        }
    }

    #[test]
    fn version_verbose_never_leaks_username_or_hostname() {
        let stdout = run_version(&["version", "-v"]);
        // The developer's username should never appear in
        // version output even by coincidence.  Skip the assertion
        // gracefully when the value is too generic (e.g. empty
        // or "user") to avoid false positives.
        if let Ok(user) = std::env::var("USER")
            && !user.is_empty()
            && user.len() >= 3
            && user != "user"
        {
            assert!(
                !stdout.contains(&user),
                "verbose version unexpectedly contains $USER ({user}): {stdout}"
            );
        }
        if let Ok(home) = std::env::var("HOME") {
            // Trim to the final path component to spot the
            // developer's home directory name even when the
            // full path was suppressed.
            if let Some(name) = std::path::Path::new(&home)
                .file_name()
                .and_then(|s| s.to_str())
                && !name.is_empty()
                && name.len() >= 3
            {
                assert!(
                    !stdout.contains(name),
                    "verbose version unexpectedly contains $HOME basename ({name}): {stdout}"
                );
            }
        }
    }

    #[test]
    fn version_verbose_never_leaks_internal_planning_or_prompt_text() {
        let stdout = run_version(&["version", "-v"]);
        // The implementation must not leak planning labels, prompt
        // text, or marker strings that belong in development notes
        // rather than the user surface.
        for forbidden in ["TODO", "FIXME", "roadmap"] {
            assert!(
                !stdout.contains(forbidden),
                "verbose version unexpectedly contains `{forbidden}`: {stdout}"
            );
        }
    }

    #[test]
    fn top_level_help_lists_version_subcommand() {
        let assertion = cabin().arg("--help").assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("version"),
            "top-level help should list the `version` subcommand: {stdout}"
        );
    }

    #[test]
    fn version_help_documents_inheriting_global_verbosity() {
        // `cabin version --help` should surface the same global
        // `-v` / `--verbose` and `-q` / `--quiet` flags every
        // other subcommand inherits.
        let assertion = cabin().args(["version", "--help"]).assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        assert!(
            stdout.contains("--verbose"),
            "`cabin version --help` should mention --verbose: {stdout}"
        );
        assert!(
            stdout.contains("--quiet"),
            "`cabin version --help` should mention --quiet: {stdout}"
        );
    }

    #[test]
    fn version_works_outside_workspace() {
        // The version command must not require a workspace; it
        // is a self-describing CLI surface.
        let dir = TempDir::new().unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .arg("version")
            .assert()
            .success();
        let stdout = String::from_utf8(assertion.get_output().stdout.clone())
            .expect("stdout should be utf-8");
        assert_eq!(stdout, "cabin 0.14.0\n");
    }

    #[test]
    fn version_verbose_works_outside_workspace() {
        let dir = TempDir::new().unwrap();
        let assertion = cabin()
            .current_dir(dir.path())
            .args(["version", "-v"])
            .assert()
            .success();
        let stdout = String::from_utf8(assertion.get_output().stdout.clone())
            .expect("stdout should be utf-8");
        // The verbose banner does not depend on the working
        // directory; the header always starts with the release
        // line.  Whether the parenthetical git metadata appears
        // depends on the build, not on the current directory.
        assert!(stdout.starts_with("cabin 0.14.0"));
        assert!(stdout.contains("\nrelease: 0.14.0\n"));
    }

    /// Preservation: every command that `cabin --help`
    /// advertises today must remain advertised after the
    /// version-output work.  The expected set is derived from clap
    /// — the visible subcommands are exactly the ones that
    /// will appear in the help block.
    #[test]
    fn other_cargo_interface_commands_still_appear_in_help() {
        let assertion = cabin().arg("--help").assert().success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        for cmd in visible_subcommand_names() {
            assert!(
                stdout.contains(&cmd),
                "top-level help should still list `{cmd}`: {stdout}"
            );
        }
    }
}

mod environment_variable_docs {
    #[test]
    fn environment_variables_doc_lists_terminal_env_controls() {
        let docs = include_str!("../../../docs/environment-variables.md");
        for name in [
            cabin_env::CABIN_TERM_COLOR,
            cabin_env::CABIN_TERM_VERBOSE,
            cabin_env::CABIN_TERM_QUIET,
        ] {
            assert!(
                docs.contains(&format!("`{name}`")),
                "docs/environment-variables.md must list `{name}` because Cabin reads it"
            );
        }
    }

    #[test]
    fn environment_variables_doc_lists_build_time_version_metadata() {
        let docs = include_str!("../../../docs/environment-variables.md");
        for name in [
            "CABIN_BUILD_COMMIT",
            "CABIN_BUILD_COMMIT_DATE",
            "CABIN_BUILD_HOST",
        ] {
            assert!(
                docs.contains(&format!("`{name}`")),
                "docs/environment-variables.md claims to list every CABIN_* variable Cabin reads, so it must list build-time `{name}`"
            );
        }
        assert!(
            docs.contains("not runtime controls"),
            "build-time metadata docs must make clear these variables are not runtime controls"
        );
    }
}

mod vendoring_docs {
    #[test]
    fn quickstart_does_not_advertise_generic_offline_test_after_vendor() {
        let docs = include_str!("../../../docs/vendoring-offline.md");
        let quickstart = docs
            .split("## What `cabin vendor` produces")
            .next()
            .expect("vendoring docs should have a quickstart section");
        assert!(
            !quickstart.contains("cabin test   --offline --index-path ./vendor"),
            "the top-level vendor workflow must not imply that dev-dependency test closures are vendored: {quickstart}"
        );

        let glue = include_str!("../src/vendor_glue.rs");
        assert!(
            !glue.contains("cabin test   --offline --index-path ./vendor"),
            "vendor_glue module docs must not advertise a generic offline test workflow"
        );
    }
}

mod toolchains_docs {
    #[test]
    fn deferred_section_does_not_mark_implemented_surfaces_out_of_scope() {
        let docs = include_str!("../../../docs/toolchains.md");
        let deferred = docs
            .split("## Deferred / out of scope")
            .nth(1)
            .expect("toolchains docs should keep a deferred section");
        for implemented in [
            "Config files (`~/.cabin/config.toml`-style overrides)",
            "patch / override / source replacement, vendoring",
        ] {
            assert!(
                !deferred.contains(implemented),
                "toolchains deferred section still lists implemented surface `{implemented}`: {deferred}"
            );
        }
    }
}

mod architecture_docs {
    #[test]
    fn config_section_does_not_mark_source_replacement_or_vendoring_out_of_scope() {
        let docs = include_str!("../../../docs/architecture.md");
        assert!(
            !docs.contains("Auth tokens, source replacement, vendoring, and new registry"),
            "architecture docs still group implemented source replacement and vendoring with unsupported auth/protocol work"
        );
    }
}

mod installation_and_metadata_docs {
    #[test]
    fn installation_docs_list_the_c_compiler_slot() {
        let docs = include_str!("../../../docs/installation.md");
        assert!(
            docs.contains("GCC- or Clang-style C compiler (`cc`, `clang`, `gcc`)"),
            "installation docs must list the separate C compiler requirement for selected `.c` sources"
        );
    }

    #[test]
    fn contributing_docs_list_the_c_compiler_slot() {
        let docs = include_str!("../../../CONTRIBUTING.md");
        assert!(
            docs.contains("a **C compiler**"),
            "contributing docs must not describe end-to-end C/C++ coverage as requiring only a C++ compiler"
        );
        assert!(
            docs.contains("tests that exercise `.c` sources"),
            "contributing docs should explain when the C compiler is required"
        );
    }

    #[test]
    fn install_source_docs_do_not_describe_runtime_tools_as_cxx_only() {
        let docs = include_str!("../../../INSTALL.md");
        assert!(
            docs.contains("C / C++ toolchains"),
            "source install docs must not point users at runtime requirements as C++-only"
        );
    }

    #[test]
    fn toolchain_crate_description_mentions_c_and_cxx() {
        let manifest = include_str!("../../../crates/cabin-toolchain/Cargo.toml");
        assert!(
            manifest.contains("C / C++ toolchain"),
            "cabin-toolchain crate metadata should not describe the crate as C++-only: {manifest}"
        );
    }
}

mod profiles_docs {
    #[test]
    fn built_in_profile_table_lists_c_and_cxx_standard_flags() {
        let docs = include_str!("../../../docs/profiles.md");
        assert!(
            docs.contains("| Profile   | `debug` | `opt-level` | `assertions` | C compile flags"),
            "profile docs should distinguish C and C++ standard flags"
        );
        assert!(
            docs.contains("`-std=c11 -O0 -g`") && docs.contains("`-std=c++17 -O0 -g`"),
            "profile docs must show both C and C++ built-in standard flags"
        );
    }
}

mod workspaces_docs {
    #[test]
    fn build_selection_note_uses_c_and_cxx_target_wording() {
        let docs = include_str!("../../../docs/workspaces.md");
        assert!(
            docs.contains("plans only the C/C++ targets in the selected"),
            "workspace docs should not describe build selection as C++-only"
        );
    }
}

mod cargo_interface_docs {
    #[test]
    fn version_row_matches_verbose_version_fields() {
        let docs = include_str!("../../../docs/cargo-inspired-interface.md");
        let row = docs
            .lines()
            .find(|line| line.starts_with("| `cabin version`"))
            .expect("cargo-inspired interface docs must list `cabin version`");
        for supported in ["commit", "host", "OS"] {
            assert!(
                row.contains(supported),
                "`cabin version` docs should mention the actual verbose {supported} field: {row}"
            );
        }
        for unsupported in ["rustc", "profile"] {
            assert!(
                !row.contains(unsupported),
                "`cabin version -v` does not print {unsupported}; docs row is misleading: {row}"
            );
        }
    }
}

mod curated_help_and_list {
    //! Contract pinning the curation between `cabin --help` and
    //! `cabin --list`.
    //!
    //! `cabin --help` is the curated, day-to-day surface: it
    //! advertises the commands aimed at normal users.  The
    //! advanced and distribution-helper subcommands are hidden from
    //! `--help` so the block stays short and skimmable, and
    //! surface through `cabin --list` instead. Hidden commands still
    //! run normally, still
    //! produce shell completions, and still ship per-command
    //! man pages — only the help listing is curated.

    use super::*;

    fn run_ok(args: &[&str]) -> String {
        let assertion = cabin().args(args).assert().success();
        String::from_utf8(assertion.get_output().stdout.clone()).expect("stdout should be utf-8")
    }

    #[test]
    fn help_does_not_list_hidden_subcommands_in_commands_block() {
        let out = run_ok(&["--help"]);
        // Words like "package" and "version" also appear in
        // command descriptions ("Create a new cabin package
        // …"), so a naive `contains` check is too loose.
        // Parse the Commands block into row names and assert
        // against the structured set.
        let listed = parse_help_commands_block(&out);
        for sub in hidden_subcommand_names() {
            assert!(
                !listed.contains(&sub),
                "`--help` Commands block must not list hidden `{sub}`. Listed: {listed:?}"
            );
        }
    }

    #[test]
    fn list_includes_every_subcommand_including_hidden() {
        let out = run_ok(&["--list"]);
        // Every documented subcommand, hidden or not, appears
        // in `cabin --list`.  The expected set comes from clap
        // so a new subcommand is covered automatically.  Order
        // is alphabetical and deterministic — see the
        // dedicated test below.
        for sub in all_subcommand_names() {
            assert!(out.contains(&sub), "`--list` missing `{sub}`: {out}");
        }
    }

    #[test]
    fn list_heading_is_present() {
        let out = run_ok(&["--list"]);
        assert!(
            out.starts_with("Installed Commands:\n"),
            "`--list` must lead with the Installed Commands heading: {out}"
        );
    }

    #[test]
    fn list_output_is_deterministic_across_runs() {
        // Two consecutive runs must produce byte-identical
        // output.  Every input is captured at compile time so
        // there is no legitimate source of non-determinism.
        let first = run_ok(&["--list"]);
        let second = run_ok(&["--list"]);
        assert_eq!(first, second);
    }

    #[test]
    fn list_rows_are_alphabetically_sorted() {
        let out = run_ok(&["--list"]);
        let names: Vec<&str> = out
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "`--list` rows must be sorted: {names:?}");
    }

    #[test]
    fn list_includes_help_subcommand() {
        // `cabin --list` is exhaustive and surfaces `help` so
        // users discover `cabin help <command>`; this matches
        // cargo's `cargo --list` behavior even though
        // `cabin --help` hides the `help` row to keep its
        // Commands block short.
        let out = run_ok(&["--list"]);
        let names: Vec<&str> = out
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        assert!(
            names.contains(&"help"),
            "`--list` should surface the `help` subcommand: {names:?}"
        );
    }

    #[test]
    fn list_does_not_expose_future_or_internal_commands() {
        let out = run_ok(&["--list"]);
        // Sanity: nothing prefixed with `internal-`, `debug-`,
        // or `experimental-` belongs in user-visible surface.
        // No such command exists today; the assertion guards
        // against drift.
        for forbidden in ["internal-", "debug-", "experimental-", "step-", "TODO"] {
            assert!(
                !out.contains(forbidden),
                "`--list` exposed unexpected `{forbidden}`: {out}"
            );
        }
    }

    #[test]
    fn hidden_subcommands_still_run_normally() {
        // Hiding is purely a help-display concern; the actual
        // subcommands still parse and execute.
        for sub in hidden_subcommand_names() {
            let _ = cabin().args([sub.as_str(), "--help"]).assert().success();
        }
    }

    #[test]
    fn hidden_subcommands_still_appear_in_shell_completions() {
        // clap_complete walks the canonical command tree and
        // emits hidden subcommands so completions still work
        // for users who learn the name.
        let out = run_ok(&["compgen", "bash"]);
        for sub in hidden_subcommand_names() {
            assert!(
                out.contains(&sub),
                "bash completion must still know about hidden `{sub}`: {out}"
            );
        }
    }

    #[test]
    fn hidden_subcommands_still_get_individual_man_pages() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("man");
        cabin()
            .args(["mangen", "--output-dir"])
            .arg(&out)
            .assert()
            .success();
        for sub in hidden_subcommand_names() {
            let path = out.join(format!("cabin-{sub}.1"));
            assert!(
                path.is_file(),
                "expected per-subcommand man page for hidden `{sub}` at {path:?}"
            );
            let body = std::fs::read(&path).unwrap();
            assert!(!body.is_empty(), "cabin-{sub}.1 must be non-empty");
        }
    }

    #[test]
    fn cabin_no_args_prints_curated_help_and_exits_zero() {
        // The dispatcher hands the help printing to clap; the
        // exit code remains 0 so a CI script that runs `cabin`
        // standalone does not flap.
        let out = run_ok(&[]);
        assert!(
            out.contains("Usage: cabin"),
            "no-arg cabin should print help: {out}"
        );
        // The curated help still omits the hidden subcommands.
        let listed = parse_help_commands_block(&out);
        for sub in hidden_subcommand_names() {
            assert!(
                !listed.contains(&sub),
                "no-arg cabin help must not list hidden `{sub}`. Listed: {listed:?}"
            );
        }
    }

    #[test]
    fn cabin_dash_dash_list_documented_in_top_level_help() {
        let out = run_ok(&["--help"]);
        assert!(
            out.contains("--list"),
            "`--help` must mention the `--list` flag: {out}"
        );
    }

    #[test]
    fn help_renders_visible_aliases_in_cargo_style() {
        // Cargo's `cargo --help` shows `build, b` rather than
        // clap's default `[aliases: b]`.  Each visible-aliased
        // subcommand should advertise its alias inline.
        let out = run_ok(&["--help"]);
        for (canonical, alias) in [("build", "b"), ("run", "r"), ("test", "t")] {
            let pattern = format!("{canonical}, {alias}");
            assert!(
                out.contains(&pattern),
                "`--help` should advertise `{pattern}`: {out}"
            );
        }
        assert!(
            !out.contains("[aliases:"),
            "`--help` must not use clap's `[aliases: ...]` form: {out}"
        );
    }

    #[test]
    fn aliases_route_to_their_canonical_subcommand() {
        // `cabin b`, `cabin t`, `cabin r` should parse and
        // dispatch to the canonical build / test / run
        // subcommands; `--help` against each alias should
        // therefore print the canonical command's help.
        for (alias, expected_summary) in [
            ("b", "Compile a local package and all of its dependencies"),
            ("t", "Run the tests of a local package"),
            ("r", "Run a binary of the local package"),
        ] {
            let out = run_ok(&[alias, "--help"]);
            assert!(
                out.contains(expected_summary),
                "`cabin {alias} --help` should mirror the canonical help: {out}"
            );
        }
    }

    #[test]
    fn list_renders_visible_aliases_in_cargo_style() {
        let out = run_ok(&["--list"]);
        for (canonical, alias) in [("build", "b"), ("run", "r"), ("test", "t")] {
            let pattern = format!("{canonical}, {alias}");
            assert!(
                out.contains(&pattern),
                "`--list` should advertise `{pattern}`: {out}"
            );
        }
    }

    #[test]
    fn help_ends_with_cargo_style_see_more_hint() {
        // The curated Commands block ends with a cargo-style
        // `... See all commands with --list` row so users
        // immediately know how to find every subcommand.  The
        // row must be the *last* visible entry — clap's
        // auto-injected `help` subcommand is hidden from the
        // listing so the cargo-style ordering is preserved.
        let out = run_ok(&["--help"]);
        let listed = parse_help_commands_block(&out);
        assert!(
            listed.contains(&"...".to_owned()),
            "`--help` Commands block must include the `...` hint row: {listed:?}"
        );
        assert_eq!(
            listed.last().map(String::as_str),
            Some("..."),
            "`...` must be the last row in the Commands block: {listed:?}"
        );
        assert!(
            !listed.contains(&"help".to_owned()),
            "auto-injected `help` row must be hidden from the curated Commands block: {listed:?}"
        );
        assert!(
            out.contains("See all commands with --list"),
            "`--help` must spell out the `--list` hint next to the `...` row: {out}"
        );
    }

    #[test]
    fn help_subcommand_still_works_even_when_hidden() {
        // Hiding the `help` row is purely cosmetic — `cabin
        // help <subcommand>` should still surface the same
        // long help that `cabin <subcommand> --help` shows.
        let from_help_sub = run_ok(&["help", "build"]);
        let from_long_flag = run_ok(&["build", "--help"]);
        assert_eq!(
            from_help_sub, from_long_flag,
            "`cabin help build` should mirror `cabin build --help`"
        );
    }

    #[test]
    fn cabin_dot_dot_dot_is_a_shortcut_for_list() {
        // Typing `cabin ...` should land the user on the same
        // page `cabin --list` does, so the help-block row is
        // also a working command and not a footgun.
        let from_dots = run_ok(&["..."]);
        let from_list = run_ok(&["--list"]);
        assert_eq!(
            from_dots, from_list,
            "`cabin ...` should mirror `cabin --list` output"
        );
    }

    #[test]
    fn dots_hint_does_not_leak_into_list() {
        // The `...` row is purely a help-view affordance; it
        // must not appear in `cabin --list`.
        let out = run_ok(&["--list"]);
        let names: Vec<&str> = out
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        assert!(
            !names.contains(&"..."),
            "`--list` must not list the `...` help-only hint: {names:?}"
        );
    }

    #[test]
    fn dots_hint_does_not_leak_into_shell_completions() {
        // Shell completions are derived from the canonical
        // clap tree (no `...` injection), so the completion
        // script must not advertise `...` as a subcommand.
        let out = run_ok(&["compgen", "bash"]);
        // The bash completion script uses
        // `tool__subcmd__<name>` markers for each subcommand;
        // assert no `...`-flavored marker appears.
        assert!(
            !out.contains("cabin__subcmd________"),
            "bash completion must not expose `...` as a subcommand: {out}"
        );
    }

    #[test]
    fn dots_hint_does_not_get_its_own_man_page() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("man");
        cabin()
            .args(["mangen", "--output-dir"])
            .arg(&out)
            .assert()
            .success();
        let dots_path = out.join("cabin-....1");
        assert!(
            !dots_path.exists(),
            "expected no man page for the `...` hint at {dots_path:?}"
        );
    }

    #[test]
    fn subcommand_help_never_leaks_rustdoc_intra_doc_links() {
        // clap renders `///` doc comments verbatim, so an
        // intra-doc link like `[`BuildArgs::offline`]` appears
        // in the user-facing `--help` output as literal
        // `` [`BuildArgs::offline`] `` text.  That looks like
        // source code, not documentation.  Scan every visible
        // subcommand's `--help` plus the top-level `--help` for
        // the `[\`` opener so a future doc-comment regression
        // surfaces here rather than in the user's terminal.
        let mut surfaces: Vec<Vec<&str>> = vec![vec!["--help"]];
        let owned_help_args: Vec<[String; 2]> = visible_subcommand_names()
            .into_iter()
            .map(|name| [name, "--help".to_owned()])
            .collect();
        for pair in &owned_help_args {
            surfaces.push(vec![pair[0].as_str(), pair[1].as_str()]);
        }
        for args in surfaces {
            let assertion = cabin().args(&args).assert().success();
            let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
            assert!(
                !stdout.contains("[`"),
                "`cabin {args:?} --help` leaks a rustdoc intra-doc link \
                 (pattern `[\\`...\\``): {stdout}"
            );
        }
    }
}

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
/// links a `cpp_executable` that calls `zlibVersion()`.
///
/// The tests are hermetic: a `tiny_http` loopback server serves
/// a synthesized "fake-zlib" archive whose layout matches the
/// real upstream archive (one `zlib.h` + one `zlib.c` under a
/// `zlib-1.3.1/` prefix dir). The mock proves the mechanics
/// without touching `zlib.net` or GitHub.
mod foundation_port_zlib {
    use super::*;
    use cabin_core::{DependencySource, PortDepSource};
    use cabin_manifest::load_manifest;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use sha2::{Digest, Sha256};
    use std::io::Write as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread::JoinHandle;

    /// Minimal `zlib.h` and `zlib.c` placed under the
    /// `zlib-1.3.1/` prefix. The C source exports a
    /// `zlibVersion()` function with the canonical signature so
    /// the downstream consumer can link against it.
    const FAKE_ZLIB_HEADER: &str = r#"#ifndef ZLIB_H
#define ZLIB_H
#ifdef __cplusplus
extern "C" {
#endif
const char *zlibVersion(void);
#ifdef __cplusplus
}
#endif
#endif
"#;

    const FAKE_ZLIB_SOURCE: &str = r#"#include "zlib.h"
const char *zlibVersion(void) { return "1.3.1"; }
"#;

    /// Build a `.tar.gz` archive containing the given entries
    /// and return `(path, hex_sha256, request_counter)`. The
    /// counter is unused; the test server tracks its own count.
    fn make_archive(dir: &Path, name: &str, entries: &[(&str, &str)]) -> (PathBuf, String) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("archive parent dir");
        }
        let f = fs::File::create(&path).expect("create archive");
        let enc = GzEncoder::new(f, Compression::default());
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
                .expect("append entry");
        }
        let enc = builder.into_inner().expect("finalize tar");
        enc.finish().expect("finalize gzip").flush().expect("flush");
        let bytes = fs::read(&path).expect("hash archive");
        let mut h = Sha256::new();
        h.update(&bytes);
        (path, format!("{:x}", h.finalize()))
    }

    /// Loopback HTTP server that serves a single archive file
    /// and counts the number of GET requests it handles.
    struct ArchiveServer {
        server: Arc<tiny_http::Server>,
        thread: Option<JoinHandle<()>>,
        url: String,
        request_count: Arc<AtomicUsize>,
    }

    impl ArchiveServer {
        fn start(archive_bytes: Vec<u8>) -> Self {
            let server = Arc::new(
                tiny_http::Server::http("127.0.0.1:0").expect("bind tiny_http on loopback"),
            );
            let addr = server.server_addr().to_ip().expect("loopback addr");
            let url = format!("http://{addr}");
            let request_count = Arc::new(AtomicUsize::new(0));
            let count_for_thread = Arc::clone(&request_count);
            let server_for_thread = Arc::clone(&server);
            let bytes = Arc::new(archive_bytes);
            let bytes_for_thread = Arc::clone(&bytes);
            let thread = std::thread::spawn(move || {
                while let Ok(req) = server_for_thread.recv() {
                    let path = req.url().to_string();
                    if path.ends_with("/zlib-1.3.1.tar.gz") {
                        count_for_thread.fetch_add(1, Ordering::SeqCst);
                        let body = (*bytes_for_thread).clone();
                        let _ = req.respond(tiny_http::Response::from_data(body));
                    } else {
                        let _ = req.respond(tiny_http::Response::empty(404));
                    }
                }
            });
            Self {
                server,
                thread: Some(thread),
                url,
                request_count,
            }
        }

        fn url(&self) -> &str {
            &self.url
        }

        fn request_count(&self) -> usize {
            self.request_count.load(Ordering::SeqCst)
        }
    }

    impl Drop for ArchiveServer {
        fn drop(&mut self) {
            self.server.unblock();
            if let Some(handle) = self.thread.take() {
                let _ = handle.join();
            }
        }
    }

    /// Lay out a fake-zlib port + consumer fixture and return the
    /// consumer manifest path.
    fn lay_fixture(
        tmp: &Path,
        archive_url: &str,
        sha256_hex: &str,
        strip_prefix: Option<&str>,
        port_type: &str,
    ) -> PathBuf {
        let mut port_toml = String::new();
        port_toml.push_str("[port]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[source]\n");
        port_toml.push_str(&format!("type = \"{port_type}\"\n"));
        port_toml.push_str(&format!("url = \"{archive_url}\"\n"));
        port_toml.push_str(&format!("sha256 = \"{sha256_hex}\"\n"));
        if let Some(prefix) = strip_prefix {
            port_toml.push_str(&format!("strip_prefix = \"{prefix}\"\n"));
        }
        port_toml.push_str("\n[overlay]\nmanifest = \"cabin.toml\"\n");
        assert_fs::fixture::ChildPath::new(tmp.join("ports/zlib/1.3.1/port.toml"))
            .write_str(&port_toml)
            .unwrap();

        assert_fs::fixture::ChildPath::new(tmp.join("ports/zlib/1.3.1/cabin.toml"))
            .write_str(
                r#"[package]
name = "zlib"
version = "1.3.1"

[target.zlib]
type = "cpp_library"
sources = ["zlib.c"]
include_dirs = ["."]
"#,
            )
            .unwrap();

        let consumer_manifest = tmp.join("consumer/cabin.toml");
        assert_fs::fixture::ChildPath::new(&consumer_manifest)
            .write_str(
                r#"[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.consumer]
type = "cpp_executable"
sources = ["src/main.c"]
deps = ["zlib"]
"#,
            )
            .unwrap();
        assert_fs::fixture::ChildPath::new(tmp.join("consumer/src/main.c"))
            .write_str(
                r#"#include <zlib.h>
#include <stdio.h>

int main(void) {
    const char *v = zlibVersion();
    if (!v || !*v) return 1;
    puts(v);
    return 0;
}
"#,
            )
            .unwrap();
        consumer_manifest
    }

    #[test]
    fn builds_and_runs_downstream_consumer() {
        if !ninja_available() || !c_compiler_available() {
            skip(
                "foundation_port_zlib::builds_and_runs_downstream_consumer",
                "requires ninja + a C compiler",
            );
            return;
        }
        let tmp = TempDir::new().unwrap();
        let (archive_path, hex) = make_archive(
            &tmp.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", FAKE_ZLIB_HEADER),
                ("zlib-1.3.1/zlib.c", FAKE_ZLIB_SOURCE),
            ],
        );
        let bytes = fs::read(&archive_path).unwrap();
        let server = ArchiveServer::start(bytes);
        let archive_url = format!("{}/zlib-1.3.1.tar.gz", server.url());
        let consumer_manifest = lay_fixture(
            tmp.path(),
            &archive_url,
            &hex,
            Some("zlib-1.3.1"),
            "archive",
        );
        let build_dir = tmp.path().join("build");
        let cache_dir = tmp.path().join("cache");

        cabin()
            .args([
                "build",
                "--manifest-path",
                consumer_manifest.to_str().unwrap(),
                "--build-dir",
                build_dir.to_str().unwrap(),
                "--cache-dir",
                cache_dir.to_str().unwrap(),
            ])
            .assert()
            .success();

        // Locate and execute the built binary. The planner
        // places executables under
        // `<build_dir>/<profile>/packages/<package>/<target>`.
        let exe_name = format!("consumer{}", std::env::consts::EXE_SUFFIX);
        let candidate_dev = build_dir.join("dev/packages/consumer").join(&exe_name);
        let candidate_release = build_dir.join("release/packages/consumer").join(&exe_name);
        let exe = if candidate_dev.is_file() {
            candidate_dev
        } else if candidate_release.is_file() {
            candidate_release
        } else {
            panic!(
                "could not find consumer executable under {}; expected `{}` in `dev/packages/consumer/` or `release/packages/consumer/`",
                build_dir.display(),
                exe_name
            );
        };
        let output = std::process::Command::new(&exe)
            .output()
            .expect("run consumer");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "consumer exited non-zero: {stdout}"
        );
        assert!(
            stdout.contains("1.3.1"),
            "expected zlib version output, got {stdout:?}"
        );

        let first_count = server.request_count();
        assert!(first_count >= 1, "expected at least one archive download");

        // Re-run: the cache should satisfy preparation so the
        // HTTP server sees no additional requests.
        cabin()
            .args([
                "build",
                "--manifest-path",
                consumer_manifest.to_str().unwrap(),
                "--build-dir",
                build_dir.to_str().unwrap(),
                "--cache-dir",
                cache_dir.to_str().unwrap(),
            ])
            .assert()
            .success();
        assert_eq!(
            server.request_count(),
            first_count,
            "second cabin build should reuse the cached archive (no new HTTP requests)"
        );
    }

    #[test]
    fn checksum_mismatch_surfaces_clear_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let (archive_path, _real_hex) = make_archive(
            &tmp.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", FAKE_ZLIB_HEADER),
                ("zlib-1.3.1/zlib.c", FAKE_ZLIB_SOURCE),
            ],
        );
        let bytes = fs::read(&archive_path).unwrap();
        let server = ArchiveServer::start(bytes);
        let archive_url = format!("{}/zlib-1.3.1.tar.gz", server.url());
        let bogus = "0".repeat(64);
        let consumer_manifest = lay_fixture(
            tmp.path(),
            &archive_url,
            &bogus,
            Some("zlib-1.3.1"),
            "archive",
        );

        let assertion = cabin()
            .args([
                "build",
                "--manifest-path",
                consumer_manifest.to_str().unwrap(),
                "--build-dir",
                tmp.path().join("build").to_str().unwrap(),
                "--cache-dir",
                tmp.path().join("cache").to_str().unwrap(),
            ])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("checksum mismatch") && stderr.contains("zlib"),
            "expected a checksum-mismatch diagnostic mentioning zlib, got: {stderr}"
        );
    }

    #[test]
    fn missing_strip_prefix_surfaces_clear_diagnostic() {
        let tmp = TempDir::new().unwrap();
        // Archive's top-level directory does not match the
        // declared `strip_prefix`.
        let (archive_path, hex) = make_archive(
            &tmp.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("other-1.0/zlib.h", FAKE_ZLIB_HEADER),
                ("other-1.0/zlib.c", FAKE_ZLIB_SOURCE),
            ],
        );
        let bytes = fs::read(&archive_path).unwrap();
        let server = ArchiveServer::start(bytes);
        let archive_url = format!("{}/zlib-1.3.1.tar.gz", server.url());
        let consumer_manifest = lay_fixture(
            tmp.path(),
            &archive_url,
            &hex,
            Some("zlib-1.3.1"),
            "archive",
        );

        let assertion = cabin()
            .args([
                "build",
                "--manifest-path",
                consumer_manifest.to_str().unwrap(),
                "--build-dir",
                tmp.path().join("build").to_str().unwrap(),
                "--cache-dir",
                tmp.path().join("cache").to_str().unwrap(),
            ])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("strip_prefix") && stderr.contains("zlib-1.3.1"),
            "expected a missing-strip_prefix diagnostic, got: {stderr}"
        );
    }

    #[test]
    fn unsupported_source_type_is_rejected_before_network() {
        let tmp = TempDir::new().unwrap();
        // Use a clearly-bogus URL so a network attempt would
        // fail loudly. The parser should refuse the `git` source
        // type before any download happens.
        let consumer_manifest = lay_fixture(
            tmp.path(),
            "https://example.invalid/zlib.tar.gz",
            &"a".repeat(64),
            Some("zlib-1.3.1"),
            "git",
        );
        let assertion = cabin()
            .args([
                "build",
                "--manifest-path",
                consumer_manifest.to_str().unwrap(),
                "--build-dir",
                tmp.path().join("build").to_str().unwrap(),
            ])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("unsupported source type") || stderr.contains("`git`"),
            "expected an unsupported-source-type diagnostic, got: {stderr}"
        );
    }

    /// `cabin metadata` must be network-free: a fresh checkout
    /// that declares an HTTP-backed port whose archive has never
    /// been cached must still render metadata successfully.
    /// Provenance for the unprepared port is gracefully omitted
    /// rather than the command erroring on a download attempt.
    #[test]
    fn cabin_metadata_succeeds_against_unfetched_http_port() {
        let tmp = TempDir::new().unwrap();
        let consumer_manifest = lay_fixture(
            tmp.path(),
            "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
            &"a".repeat(64),
            Some("zlib-1.3.1"),
            "archive",
        );
        cabin()
            .env("CABIN_CACHE_DIR", tmp.path().join("cache"))
            .args([
                "metadata",
                "--manifest-path",
                consumer_manifest.to_str().unwrap(),
                "--format",
                "json",
            ])
            .assert()
            .success();
    }

    #[test]
    fn cabin_metadata_surfaces_prepared_port_provenance() {
        let tmp = TempDir::new().unwrap();
        let (archive_path, hex) = make_archive(
            &tmp.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", FAKE_ZLIB_HEADER),
                ("zlib-1.3.1/zlib.c", FAKE_ZLIB_SOURCE),
            ],
        );
        // `cabin metadata` forces `offline = true` (it is a
        // local-introspection command), so the fixture uses a
        // `file://` URL the resolver always satisfies without
        // touching the network. The metadata view should still
        // surface the prepared port's full provenance.
        let archive_url = url::Url::from_file_path(&archive_path).unwrap().to_string();
        let consumer_manifest = lay_fixture(
            tmp.path(),
            &archive_url,
            &hex,
            Some("zlib-1.3.1"),
            "archive",
        );

        // `cabin metadata` does not expose `--cache-dir`; the
        // env var is the equivalent knob for per-test cache
        // isolation now that the default lives at
        // `$HOME/.cache/cabin`.
        let assertion = cabin()
            .env("CABIN_CACHE_DIR", tmp.path().join("cache"))
            .args([
                "metadata",
                "--manifest-path",
                consumer_manifest.to_str().unwrap(),
                "--format",
                "json",
            ])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        let value: serde_json::Value =
            serde_json::from_str(&stdout).expect("metadata JSON should parse");
        let ports = value
            .get("ports")
            .and_then(serde_json::Value::as_array)
            .expect("metadata view should expose a `ports` array");
        assert_eq!(
            ports.len(),
            1,
            "expected exactly one prepared port, got {ports:?}"
        );
        let port = &ports[0];
        assert!(
            port.get("port_dir").is_none(),
            "top-level port_dir should be replaced by the origin block; got: {port:?}"
        );
        assert_eq!(port["name"].as_str(), Some("zlib"));
        assert_eq!(port["version"].as_str(), Some("1.3.1"));
        let origin = port.get("origin").expect("origin block");
        assert_eq!(origin["kind"].as_str(), Some("path"));
        let port_dir = origin["port_dir"].as_str().expect("port_dir is a string");
        assert!(
            std::path::Path::new(port_dir).is_absolute(),
            "port_dir should be absolute, got {port_dir}"
        );
        assert!(
            port_dir.ends_with("ports/zlib/1.3.1"),
            "port_dir should point at the recipe directory, got {port_dir}"
        );
        let source = port.get("source").expect("source block");
        assert_eq!(source["kind"].as_str(), Some("archive"));
        assert_eq!(source["url"].as_str(), Some(archive_url.as_str()));
        assert_eq!(
            source["sha256"].as_str(),
            Some(format!("sha256:{hex}").as_str())
        );
        assert_eq!(source["strip_prefix"].as_str(), Some("zlib-1.3.1"));
        let overlay = port["overlay_manifest"]
            .as_str()
            .expect("overlay_manifest should be a string");
        assert!(
            std::path::Path::new(overlay).is_absolute(),
            "overlay_manifest should be absolute, got {overlay}"
        );
        assert!(
            overlay.ends_with("ports/zlib/1.3.1/cabin.toml"),
            "overlay_manifest should point at the port's overlay file, got {overlay}"
        );
    }

    /// Regression for #26: port discovery must run *after* patch
    /// resolution. The root manifest declares a versioned dep on
    /// `foo`; the patched fork pulls in zlib via a `port-path`.
    /// Without the patches-before-discovery ordering, the walker
    /// never sees the patched fork's port edge and `cabin
    /// metadata` emits an empty `ports` array.
    #[test]
    fn metadata_discovers_port_introduced_by_patched_manifest() {
        let tmp = TempDir::new().unwrap();
        let (archive_path, hex) = make_archive(
            &tmp.path().join("downloads"),
            "zlib-1.3.1.tar.gz",
            &[
                ("zlib-1.3.1/zlib.h", FAKE_ZLIB_HEADER),
                ("zlib-1.3.1/zlib.c", FAKE_ZLIB_SOURCE),
            ],
        );
        let archive_url = url::Url::from_file_path(&archive_path).unwrap().to_string();
        tmp.child("ports/zlib/1.3.1/port.toml")

            .write_str(&format!(
                "[port]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[source]\ntype = \"archive\"\nurl = \"{archive_url}\"\nsha256 = \"{hex}\"\nstrip_prefix = \"zlib-1.3.1\"\n\n[overlay]\nmanifest = \"cabin.toml\"\n"
            ))

            .unwrap();
        tmp.child("ports/zlib/1.3.1/cabin.toml")
            .write_str(
                r#"[package]
name = "zlib"
version = "1.3.1"

[target.zlib]
type = "cpp_library"
sources = ["zlib.c"]
include_dirs = ["."]
"#,
            )
            .unwrap();

        let root = tmp.path().join("app");
        assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
foo = ">=0.1.0 <1.0.0"

[patch]
foo = { path = "../foo-fork" }
"#,
            )
            .unwrap();
        // The patched fork is what introduces the port edge.
        // `cabin metadata` only sees it if discovery runs against
        // the post-patch skeleton.
        tmp.child("foo-fork/cabin.toml")
            .write_str(
                r#"[package]
name = "foo"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }
"#,
            )
            .unwrap();

        let assertion = cabin()
            .env("CABIN_CACHE_DIR", tmp.path().join("cache"))
            .args([
                "metadata",
                "--manifest-path",
                root.join("cabin.toml").to_str().unwrap(),
                "--format",
                "json",
            ])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let port_names: Vec<&str> = value["ports"]
            .as_array()
            .expect("metadata view should expose a `ports` array")
            .iter()
            .filter_map(|p| p["name"].as_str())
            .collect();
        assert!(
            port_names.contains(&"zlib"),
            "zlib must enter port discovery through the patched foo manifest, got: {port_names:?}"
        );
    }

    #[test]
    fn port_toml_schema_for_real_ports_zlib_matches_published_values() {
        // Regression test that locks the on-disk port.toml in
        // ports/zlib/1.3.1/ against the typed parser. Catches
        // accidental edits without requiring any network.
        let manifest_dir =
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set during tests");
        let port_toml = PathBuf::from(manifest_dir)
            .join("../../ports/zlib/1.3.1/port.toml")
            .canonicalize()
            .expect("canonicalize ports/zlib/1.3.1/port.toml");
        let descriptor =
            cabin_port::load_port(&port_toml).expect("ports/zlib/1.3.1/port.toml should parse");
        assert_eq!(descriptor.name.as_str(), "zlib");
        assert_eq!(descriptor.version, semver::Version::new(1, 3, 1));
        match &descriptor.source {
            cabin_port::PortSource::Archive {
                url,
                sha256,
                strip_prefix,
            } => {
                assert!(
                    url.as_str().ends_with(".tar.gz"),
                    "expected a .tar.gz URL, got {url}"
                );
                assert_eq!(url.scheme(), "https");
                assert_eq!(sha256.to_hex().len(), 64);
                assert_eq!(strip_prefix.as_deref(), Some("zlib-1.3.1"));
            }
        }
        assert_eq!(
            descriptor.overlay.relative_path,
            PathBuf::from("cabin.toml")
        );
        assert_eq!(descriptor.metadata.license.as_deref(), Some("Zlib"));
    }

    #[test]
    fn port_true_resolves_against_bundled_zlib() {
        let tmp = TempDir::new().unwrap();
        let consumer = tmp.path().join("consumer");
        assert_fs::fixture::ChildPath::new(&consumer)
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(consumer.join("cabin.toml"))
            .write_str(
                r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^1.3" }
"#,
            )
            .unwrap();

        let manifest = load_manifest(consumer.join("cabin.toml")).expect("manifest parses");
        let pkg = manifest.package.expect("[package]");
        let dep = pkg
            .dependencies
            .iter()
            .find(|d| d.name.as_str() == "zlib")
            .unwrap();
        match &dep.source {
            DependencySource::Port(PortDepSource::Builtin { name, version_req }) => {
                assert_eq!(name.as_str(), "zlib");
                assert_eq!(version_req.to_string(), "^1.3");
            }
            other => panic!("expected Builtin, got {other:?}"),
        }

        // The bundled recipe is what discovery would resolve this to.
        let entry =
            cabin_port::builtin::lookup("zlib", &semver::VersionReq::parse("^1.3").unwrap())
                .expect("bundled zlib");
        let descriptor = cabin_port::parse_port_str(
            entry.port_toml,
            std::path::Path::new("<builtin:zlib>/port.toml"),
        )
        .unwrap();
        assert_eq!(descriptor.name.as_str(), "zlib");
        assert_eq!(descriptor.version.to_string(), "1.3.1");
    }

    #[test]
    fn port_true_with_unsatisfiable_version_surfaces_clear_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let consumer = tmp.path().join("consumer");
        assert_fs::fixture::ChildPath::new(&consumer)
            .create_dir_all()
            .unwrap();
        assert_fs::fixture::ChildPath::new(consumer.join("cabin.toml"))
            .write_str(
                r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^2" }
"#,
            )
            .unwrap();

        let assertion = cabin()
            .args([
                "metadata",
                "--manifest-path",
                consumer.join("cabin.toml").to_str().unwrap(),
                "--format",
                "json",
                "--offline",
            ])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("no bundled foundation port `zlib` satisfies `^2`")
                && stderr.contains("1.3.1"),
            "expected version-not-found diagnostic, got: {stderr}"
        );
    }

    /// `cabin build` does not activate `[dev-dependencies]`, so a
    /// port reachable only through a member's dev-deps must not
    /// force a download — even when its URL is unreachable. The
    /// build target itself has no port edges, so the build
    /// pipeline runs cleanly.
    #[test]
    fn build_skips_dev_only_port_preparation() {
        if !ninja_available() || !c_compiler_available() {
            skip(
                "foundation_port_zlib::build_skips_dev_only_port_preparation",
                "requires ninja + a C compiler",
            );
            return;
        }
        let tmp = TempDir::new().unwrap();
        // Lay a port + dev-only consumer; sibling `app` is what
        // we actually build.
        let _ = lay_fixture(
            tmp.path(),
            "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
            &"a".repeat(64),
            Some("zlib-1.3.1"),
            "archive",
        );
        // Rewrite consumer to reference zlib only as a dev-dep.
        tmp.child("consumer/cabin.toml")
            .write_str(
                r#"[package]
name = "consumer"
version = "0.1.0"

[dev-dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.consumer]
type = "cpp_executable"
sources = ["src/main.c"]
"#,
            )
            .unwrap();
        tmp.child("consumer/src/main.c")
            .write_str("int main(void) { return 0; }\n")
            .unwrap();
        cabin()
            .args([
                "build",
                "--manifest-path",
                tmp.path().join("consumer/cabin.toml").to_str().unwrap(),
                "--build-dir",
                tmp.path().join("build").to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    /// Port discovery must not propagate `[dev-dependencies]`
    /// through path-dep recursion: the loader's dev policy
    /// activates dev edges only on the selected test runners
    /// themselves, so a transitive path-dep's dev-only port
    /// would never become an active graph edge for this run.
    /// `cabin test` must therefore skip preparing such ports —
    /// even when the unreachable URL would otherwise stall the
    /// command on a fresh checkout.
    #[test]
    fn test_skips_transitive_path_dep_dev_only_port_preparation() {
        if !ninja_available() || !c_compiler_available() {
            skip(
                "foundation_port_zlib::test_skips_transitive_path_dep_dev_only_port_preparation",
                "requires ninja + a C compiler",
            );
            return;
        }
        let tmp = TempDir::new().unwrap();
        // A port whose URL would fail every download attempt; if
        // the walker ever decided to prep it, `cabin test` would
        // fail rather than skip.
        tmp.child("ports/zlib/1.3.1/port.toml")

            .write_str("[port]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[source]\ntype = \"archive\"\nurl = \"http://127.0.0.1:1/zlib-1.3.1.tar.gz\"\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n\n[overlay]\nmanifest = \"cabin.toml\"\n")

            .unwrap();
        tmp.child("ports/zlib/1.3.1/cabin.toml")
            .write_str("[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n")
            .unwrap();

        // The transitive path-dep `lib` is what declares the
        // dev-only port. `app`'s own dev-deps are empty.
        tmp.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
lib = { path = "../lib" }

[target.app_test]
type = "cpp_test"
sources = ["src/test.c"]
"#,
            )
            .unwrap();
        tmp.child("app/src/test.c")
            .write_str("int main(void) { return 0; }\n")
            .unwrap();
        tmp.child("lib/cabin.toml")
            .write_str(
                r#"[package]
name = "lib"
version = "0.1.0"

[dev-dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.lib]
type = "cpp_library"
sources = ["src/lib.c"]
"#,
            )
            .unwrap();
        tmp.child("lib/src/lib.c")
            .write_str("int lib_dummy(void) { return 0; }\n")
            .unwrap();

        cabin()
            .args([
                "test",
                "--manifest-path",
                tmp.path().join("app/cabin.toml").to_str().unwrap(),
                "--build-dir",
                tmp.path().join("build").to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    /// `cabin build --package <name>` must scope port
    /// preparation to `<name>`'s closure. A workspace sibling
    /// that declares an uncached HTTP-backed port must therefore
    /// not block the build of an unrelated package — the
    /// reviewer's P1 concern around selection isolation.
    #[test]
    fn build_scoped_to_package_ignores_sibling_port() {
        if !ninja_available() || !c_compiler_available() {
            skip(
                "foundation_port_zlib::build_scoped_to_package_ignores_sibling_port",
                "requires ninja + a C compiler",
            );
            return;
        }
        let tmp = TempDir::new().unwrap();
        // Lay the standard zlib consumer fixture and wrap a
        // sibling `app` (no port deps) into a workspace.
        let _ = lay_fixture(
            tmp.path(),
            "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
            &"a".repeat(64),
            Some("zlib-1.3.1"),
            "archive",
        );
        tmp.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        tmp.child("app/src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        tmp.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["consumer", "app"]
"#,
            )
            .unwrap();
        // Building only `app` must not fail on `consumer`'s
        // uncached HTTP-backed port. The sibling is outside the
        // selected closure, so port discovery never walks it.
        cabin()
            .args([
                "build",
                "--manifest-path",
                tmp.path().join("cabin.toml").to_str().unwrap(),
                "--package",
                "app",
                "--build-dir",
                tmp.path().join("build").to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    /// The flip side of `build_scoped_to_package_ignores_sibling_port`:
    /// when the SELECTED package itself has a typoed `port-path`
    /// (or a port-prep miss), the loader must still surface the
    /// typed `PortDirectoryMissing` / `PortDependencyNotPrepared`
    /// diagnostic instead of silently dropping the edge under
    /// the tolerate-missing-ports policy. Selection isolation
    /// must only relax unselected siblings.
    #[test]
    fn build_scoped_port_miss_on_selected_package_still_errors() {
        let tmp = TempDir::new().unwrap();
        // Consumer references a non-existent port-path directory.
        tmp.child("consumer/cabin.toml")
            .write_str(
                r#"[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.consumer]
type = "cpp_executable"
sources = ["src/main.c"]
"#,
            )
            .unwrap();
        tmp.child("consumer/src/main.c")
            .write_str("int main(void) { return 0; }\n")
            .unwrap();
        // No ports/ directory anywhere on disk and no workspace
        // wrapper — just the consumer with a broken port-path.
        let assertion = cabin()
            .args([
                "build",
                "--manifest-path",
                tmp.path().join("consumer/cabin.toml").to_str().unwrap(),
                "--build-dir",
                tmp.path().join("build").to_str().unwrap(),
            ])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("port") && stderr.contains("zlib"),
            "expected a port-related diagnostic naming `zlib`, got: {stderr}"
        );
    }

    /// Two consumers declaring conflicting bundled-port version
    /// requirements must surface a clear diagnostic instead of
    /// silently resolving against the first dependent's request.
    #[test]
    fn conflicting_builtin_version_requirements_surface_clear_diagnostic() {
        let tmp = TempDir::new().unwrap();
        // Workspace layout: root has two members; one accepts the
        // bundled 1.3.x recipe, the other demands ^2 which no
        // bundled recipe satisfies. The 1.3 request is declared
        // first lexicographically (`alpha` < `beta`).
        tmp.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["alpha", "beta"]
"#,
            )
            .unwrap();
        tmp.child("alpha/cabin.toml")
            .write_str(
                r#"[package]
name = "alpha"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^1.3" }
"#,
            )
            .unwrap();
        tmp.child("beta/cabin.toml")
            .write_str(
                r#"[package]
name = "beta"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^2" }
"#,
            )
            .unwrap();
        let assertion = cabin()
            .args([
                "metadata",
                "--manifest-path",
                tmp.path().join("cabin.toml").to_str().unwrap(),
                "--format",
                "json",
                "--offline",
            ])
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("zlib") && stderr.contains("^2"),
            "expected a version-not-found diagnostic naming the unsatisfied requirement, got: {stderr}"
        );
    }

    /// `cabin fmt` rewrites local source files only; it must
    /// succeed on a fresh checkout even when the workspace
    /// declares an HTTP-backed port whose archive has never been
    /// cached, because formatting needs no port content.
    #[test]
    fn fmt_succeeds_against_workspace_with_unfetched_http_port() {
        let tmp = TempDir::new().unwrap();
        let consumer_manifest = lay_fixture(
            tmp.path(),
            "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
            &"a".repeat(64),
            Some("zlib-1.3.1"),
            "archive",
        );
        let mut cmd = cabin();
        if use_fake_external_tools() {
            cmd.env("CABIN_FMT", workspace_test_bin("cabin-fmt-fake-formatter"));
        } else {
            require_external_tool("clang-format");
        }
        // We do not run `--check`: clang-format would reject the
        // fixture sources because they are not LLVM-style. What we
        // care about is that `cabin fmt` reaches the formatter at
        // all — i.e. the port-preparation step does *not* block
        // formatting on an uncached HTTP-backed port.
        cmd.args([
            "fmt",
            "--manifest-path",
            consumer_manifest.to_str().unwrap(),
        ])
        .assert()
        .success();
    }

    /// `cabin clean` only touches local build outputs, so it must
    /// succeed on a fresh checkout even when the workspace declares
    /// an HTTP-backed port whose archive has never been cached.
    /// The bogus URL would fail any actual download.
    #[test]
    fn clean_succeeds_against_workspace_with_unfetched_http_port() {
        let tmp = TempDir::new().unwrap();
        let consumer_manifest = lay_fixture(
            tmp.path(),
            "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
            &"a".repeat(64),
            Some("zlib-1.3.1"),
            "archive",
        );
        cabin()
            .args([
                "clean",
                "--manifest-path",
                consumer_manifest.to_str().unwrap(),
                "--build-dir",
                tmp.path().join("build").to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    /// `cabin publish --dry-run --package <other>` selects a
    /// single workspace member; foundation-port edges from any
    /// member should not force a download in the selection step,
    /// so a workspace with an uncached HTTP-backed port still
    /// reaches `cabin package`'s own validation (which rejects
    /// the dry-run on `cabin publish` only after the selection
    /// has succeeded).
    #[test]
    fn package_selection_does_not_force_http_port_fetch() {
        let tmp = TempDir::new().unwrap();
        // Lay out the same fixture as the build tests but wrap
        // both the consumer and a sibling, port-free `app` package
        // in a workspace root.
        let _ = lay_fixture(
            tmp.path(),
            "http://127.0.0.1:1/zlib-1.3.1.tar.gz",
            &"a".repeat(64),
            Some("zlib-1.3.1"),
            "archive",
        );
        tmp.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[target.app]
type = "cpp_executable"
sources = ["src/main.cc"]
"#,
            )
            .unwrap();
        tmp.child("app/src/main.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        tmp.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["consumer", "app"]
"#,
            )
            .unwrap();
        // `cabin package --package app` must not need the port
        // archive because `app` has no port deps. With selection
        // forced to fetch ports, this would fail with a network
        // error on the bogus URL.
        cabin()
            .args([
                "package",
                "--manifest-path",
                tmp.path().join("cabin.toml").to_str().unwrap(),
                "--package",
                "app",
                "--output-dir",
                tmp.path().join("dist").to_str().unwrap(),
            ])
            .assert()
            .success();
    }
}
