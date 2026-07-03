use super::*;
use std::path::PathBuf;

const VALID_C_MANIFEST: &str = r#"[package]
name = "hello"
version = "0.1.0"
cxx-standard = "c++17"

[target.hello]
type = "executable"
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
    let candidate = dir.join(format!(
        "cabin-ninja-fake-ninja{}",
        std::env::consts::EXE_SUFFIX
    ));
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
                .map(std::borrow::ToOwned::to_owned)
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
    // to ninja. `--help` after `--` reaches the user
    // program - but the fake ninja never produces an
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
    // - that catches both accidental metadata extension and
    //   any incidental status-line
    //   leak.
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
