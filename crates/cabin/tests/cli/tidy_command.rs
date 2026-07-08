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
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A stand-in compiler that exists on every host (the bundled fake
/// ninja binary).  The tidy planner resolves a toolchain only to
/// thread the compiler path into `compile_commands.json`; it never
/// runs it as a compiler. `/bin/sh` would serve on Unix but is
/// absent on Windows, so a built binary is used instead.
fn tidy_dummy_compiler() -> PathBuf {
    workspace_test_bin("cabin-ninja-fake-ninja")
}

/// Build the integration-test command with `CABIN_TIDY`
/// pointing at the bundled fake tidy.  Also sets `CXX` / `CC` /
/// `AR` to a binary that exists on every host (see
/// [`tidy_dummy_compiler`]) so the tidy planner's toolchain
/// resolver does not fail when the developer's PATH lacks
/// `c++` / `clang++` / `g++`.
fn cabin_with_fake_tidy() -> Command {
    let mut cmd = cabin();
    cmd.env("CABIN_TIDY", fake_tidy_path());
    let dummy = tidy_dummy_compiler();
    cmd.env("CXX", &dummy);
    cmd.env("CC", &dummy);
    cmd.env("AR", &dummy);
    // Poison the environment with a registry credential: the fake
    // tidy hard-fails when it sees the variable, so every test in
    // this module enforces the child-env scrub.
    cmd.env("CABIN_REGISTRY_TOKEN", "cabin_secretToken1234");
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
    let candidate = dir.join(format!(
        "cabin-tidy-fake-tidy{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        candidate.is_file(),
        "expected fake tidy at {}; build cabin-tidy with `--features test-fake-tidy`",
        candidate.display()
    );
    candidate
}

/// Collect the raw record lines the fake tidy appended.
fn read_record(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Normalize a tidy record (or one of its fields) to forward
/// slashes so path assertions read the same on every host.  The
/// fake tidy escapes each `\` as `\\` in the argv / files fields
/// but leaves the compile-db field raw, so collapse the doubled
/// form first, then any lone separator.
fn normalize(s: &str) -> String {
    s.replace("\\\\", "/").replace('\\', "/")
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
        .stdout(predicate::str::contains("Checked 1 file"));
}

/// `cabin tidy` must analyze `test` and `example` sources, plus
/// default-buildable ones. `cabin fmt` already formats
/// those files; tidy must match its surface.
#[test]
fn tidy_analyses_test_and_example_sources() {
    let _guard = tidy_record_lock();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[target.demo]
type = "library"
sources = ["src/lib.cc"]

[target.demo_test]
type = "test"
sources = ["tests/lib_test.cc"]
deps = ["demo"]

[target.hello_example]
type = "example"
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
        .stdout(predicate::str::contains("Checked 3 files"));
}

#[test]
fn tidy_skips_gated_target_but_errors_when_everything_is_gated() {
    let _guard = tidy_record_lock();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[features]
ssl = []

[target.demo]
type = "library"
sources = ["src/lib.cc"]

[target.tls]
type = "library"
sources = ["src/tls.cc"]
required-features = ["ssl"]
"#,
        )
        .unwrap();
    dir.child("src/lib.cc")
        .write_str("int demo() { return 1; }\n")
        .unwrap();
    dir.child("src/tls.cc")
        .write_str("int tls() { return 2; }\n")
        .unwrap();
    // The gated target's source stays out of the analyzed set.
    cabin_with_fake_tidy()
        .current_dir(dir.path())
        .arg("tidy")
        .assert()
        .success()
        .stdout(predicate::str::contains("Checked 1 file"));

    // With *every* target gated, tidy must fail loudly instead of
    // reporting a clean empty run.
    let all_gated = TempDir::new().unwrap();
    all_gated
        .child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[features]
ssl = []

[target.tls]
type = "library"
sources = ["src/tls.cc"]
required-features = ["ssl"]
"#,
        )
        .unwrap();
    all_gated
        .child("src/tls.cc")
        .write_str("int tls() { return 2; }\n")
        .unwrap();
    cabin_with_fake_tidy()
        .current_dir(all_gated.path())
        .arg("tidy")
        .assert()
        .failure()
        .stderr(predicate::str::contains("demo:tls"))
        .stderr(predicate::str::contains("[features].default"));
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
        normalize(cdb).ends_with("build/dev"),
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
        normalize(cdb).ends_with("out/dev"),
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
        .stdout(predicate::str::contains("no C/C++ source files"));
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

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.hello]\ntype = \"executable\"\nsources = [\"src/main.cc\", \"src/extra.cc\"]\n")

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

    let body = normalize(&std::fs::read_to_string(&record).unwrap());
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

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.hello]\ntype = \"executable\"\nsources = [\"src/main.cc\", \"src/a.cc\", \"src/b.cc\"]\n")

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

    let body = normalize(&std::fs::read_to_string(&record).unwrap());
    assert!(body.contains("src/main.cc"));
    assert!(!body.contains("src/a.cc"));
    assert!(!body.contains("src/b.cc"));
}

#[test]
fn vcs_ignored_files_are_skipped_by_default() {
    let _guard = tidy_record_lock();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.hello]\ntype = \"executable\"\nsources = [\"src/main.cc\", \"src/generated.cc\"]\n")

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

    let body = normalize(&std::fs::read_to_string(&record).unwrap());
    assert!(body.contains("src/main.cc"));
    assert!(!body.contains("src/generated.cc"));
}

#[test]
fn no_ignore_vcs_includes_gitignored_files() {
    let _guard = tidy_record_lock();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.hello]\ntype = \"executable\"\nsources = [\"src/main.cc\", \"src/generated.cc\"]\n")

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

    let body = normalize(&std::fs::read_to_string(&record).unwrap());
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

    let body = normalize(&std::fs::read_to_string(&record).unwrap());
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

    let body = normalize(&std::fs::read_to_string(&record).unwrap());
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

    let dummy = tidy_dummy_compiler();
    let assertion = cabin()
        .current_dir(dir.path())
        .env("CABIN_TIDY", "/no-such/run-clang-tidy-binary")
        .env("CXX", &dummy)
        .env("CC", &dummy)
        .env("AR", &dummy)
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
    let dummy = tidy_dummy_compiler();
    cabin()
        .current_dir(dir.path())
        .env("CABIN_TIDY", fake_tidy_path())
        .env("CABIN_FAKE_TIDY_RECORD", &record)
        .env("CXX", &dummy)
        .env("CC", &dummy)
        .env("AR", &dummy)
        .arg("tidy")
        .assert()
        .success();
    let body = normalize(&std::fs::read_to_string(&record).unwrap());
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
cxx-standard = "c++17"

[target.outer]
type = "executable"
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
cxx-standard = "c++17"

[target.nested]
type = "executable"
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

    let body = normalize(&std::fs::read_to_string(&record).unwrap());
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
cxx-standard = "c++17"

[target.clean]
type = "executable"
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

    let body = normalize(&std::fs::read_to_string(&record).unwrap());
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

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.hello]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n\n[dependencies]\nfmt = \"1.0\"\n")

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
