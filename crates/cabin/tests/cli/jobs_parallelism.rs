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
    workspace_test_bin("cabin-ninja-fake-ninja")
}

/// `cabin()` with the fake ninja, which appends each invocation's
/// argv to `record`.
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
fn test_jobs_flag_reaches_ninja_and_not_the_tests() {
    // `cabin test -j 3` limits the build like `cabin build -j 3`;
    // the same flag after `--` belongs to the test executables.
    // The fake ninja builds nothing, so the run itself fails; only
    // what reached ninja matters here.
    for (args, expected) in [
        (&["test", "-j", "3"][..], Some("-j3")),
        (&["test", "--", "-j", "3"][..], None),
    ] {
        let dir = TempDir::new().unwrap();
        let record = dir.path().join("ninja.log");
        dir.child("cabin.toml")
            .write_str(
                r#"[package]
name = "hello"
version = "0.1.0"
cxx-standard = "c++17"

[target.hello_test]
type = "test"
sources = ["tests/hello_test.cc"]
"#,
            )
            .unwrap();
        dir.child("tests/hello_test.cc")
            .write_str("int main() { return 0; }\n")
            .unwrap();
        let _ = cabin_with_fake_ninja(&record)
            .current_dir(dir.path())
            .args(args)
            .assert()
            .failure();
        let invocations = read_ninja_argvs(&record);
        assert_eq!(invocations.len(), 1, "{args:?}");
        let jobs = invocations[0].iter().find(|arg| arg.starts_with("-j"));
        assert_eq!(jobs.map(String::as_str), expected, "{args:?}");
    }
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
    // The fake ninja produces no executable, so `run` fails after
    // the ninja step; only the ninja argv is checked.
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
    // `--jobs 4` after `--` belongs to the program, not Cabin. The
    // program never runs (the fake ninja produces no executable);
    // the test pins Cabin's argument parsing only.
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
fn jobs_env_reaches_metadata_only_as_config_build_jobs() {
    // The job count never changes what is built, so it must not
    // reach build configurations or fingerprints: apart from the
    // `config.build.jobs` report, the JSON stays the same.
    let dir = TempDir::new().unwrap();
    write_minimal_project(dir.path());
    let metadata = |jobs: Option<&str>| {
        let mut cmd = cabin();
        if let Some(jobs) = jobs {
            cmd.env("CABIN_BUILD_JOBS", jobs);
        }
        let output = cmd
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .success();
        let mut value: serde_json::Value =
            serde_json::from_slice(&output.get_output().stdout).unwrap();
        let reported = value["config"]["build"]
            .as_object_mut()
            .unwrap()
            .remove("jobs")
            .unwrap();
        (value, reported)
    };
    let (baseline, unset) = metadata(None);
    let (with_jobs, set) = metadata(Some("4"));
    assert_eq!(unset, serde_json::Value::Null);
    assert_eq!(
        set,
        serde_json::json!({ "value": 4, "value_source": "env" })
    );
    assert_eq!(baseline, with_jobs);
}
