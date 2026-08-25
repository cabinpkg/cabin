use super::*;

fn run_capture(cwd: &Path, args: &[&str]) -> (String, String) {
    let output = cabin()
        .current_dir(cwd)
        .args(args)
        .assert()
        .success()
        .get_output()
        .clone();
    (
        String::from_utf8(output.stdout).expect("stdout utf-8"),
        String::from_utf8(output.stderr).expect("stderr utf-8"),
    )
}

fn read_fake_ninja_argvs(record: &Path) -> Vec<Vec<String>> {
    fs::read_to_string(record)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.split('\u{001f}').map(str::to_owned).collect())
        .collect()
}

#[test]
fn quiet_suppresses_status_without_suppressing_the_operation() {
    let dir = TempDir::new().unwrap();
    let (stdout, _) = run_capture(dir.path(), &["init", "--name", "hello", "--quiet"]);
    assert!(!stdout.contains("Created binary"));
    assert!(dir.path().join("cabin.toml").exists());
}

#[test]
fn quiet_does_not_suppress_errors() {
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .args(["clean", "--quiet"])
        .assert()
        .failure();
    assert!(
        !assertion.get_output().stderr.is_empty(),
        "quiet suppressed the failure diagnostic"
    );
}

#[test]
fn verbose_build_reports_resolved_context() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
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
    assert!(stdout.contains("cabin: profile = "));
    assert!(stdout.contains("cabin: build dir = "));
    assert!(stdout.contains("cabin: c++ compiler = "));
}

#[test]
fn very_verbose_build_reports_the_archiver() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let build_dir = dir.path().join("build");
    let (stdout, _) = run_capture(
        dir.path(),
        &["build", "-vv", "--build-dir", build_dir.to_str().unwrap()],
    );
    assert!(stdout.contains("cabin: archiver = "));
}

#[test]
fn verbose_build_is_forwarded_to_ninja() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let record = dir.path().join("ninja.log");
    cabin()
        .current_dir(dir.path())
        .env("NINJA", workspace_test_bin("cabin-ninja-fake-ninja"))
        .env("CABIN_FAKE_NINJA_RECORD", &record)
        .args(["b", "--verbose"])
        .assert()
        .success();
    let invocations = read_fake_ninja_argvs(&record);
    assert_eq!(invocations.len(), 1);
    assert!(invocations[0].iter().any(|arg| arg == "-v"));
}

#[test]
fn verbose_progress_does_not_corrupt_json_stdout() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let manifest = dir.path().join("cabin.toml");
    let (stdout, _) = run_capture(
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
    serde_json::from_str::<serde_json::Value>(&stdout)
        .expect("resolve stdout should remain valid JSON");
}

#[test]
fn environment_can_enable_verbose_output() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
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
    assert!(stdout.contains("cabin: profile = "));
}

#[test]
fn invalid_environment_verbosity_is_rejected() {
    let dir = TempDir::new().unwrap();
    let assertion = cabin()
        .current_dir(dir.path())
        .env("CABIN_TERM_VERBOSE", "loud")
        .args(["init", "--name", "hello"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(stderr.contains("invalid CABIN_TERM_VERBOSE value 'loud'"));
}
