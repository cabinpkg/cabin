use super::*;

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

fn read_fake_ninja_argvs(record: &Path) -> Vec<Vec<String>> {
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
    write_hello_project(dir.path());
    let (stdout, _) = run_capture(dir.path(), &["clean", "--quiet"]);
    assert!(
        stdout.is_empty(),
        "quiet must suppress clean status:\n{stdout}"
    );
}

#[test]
fn quiet_short_flag_works_the_same() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
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
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
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
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
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
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
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
fn verbose_build_forwards_ninja_verbose_flag() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let record = dir.path().join("ninja.log");

    cabin()
        .current_dir(dir.path())
        .env("NINJA", workspace_test_bin("cabin-ninja-fake-ninja"))
        .env("CABIN_FAKE_NINJA_RECORD", &record)
        .args(["b", "-v"])
        .assert()
        .success();

    let invocations = read_fake_ninja_argvs(&record);
    assert_eq!(invocations.len(), 1, "expected one ninja invocation");
    assert!(
        invocations[0].iter().any(|arg| arg == "-v"),
        "verbose build must ask Ninja to print full commands: {:?}",
        invocations[0]
    );
}

#[test]
fn quiet_with_verbose_is_rejected_by_clap() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
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
    // stderr via `Reporter::aux_verbose`.  Verbose flags
    // must not reverse that split.
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
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
    write_hello_project(dir.path());
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
