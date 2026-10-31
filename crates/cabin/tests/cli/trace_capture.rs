//! `CABIN_TRACE_DIR` capture: the files a traced invocation leaves
//! behind, and that tracing never changes the command's own output.

use super::*;

fn traced(project: &Path, traces: &Path) -> Command {
    let mut cmd = cabin();
    cmd.current_dir(project).env("CABIN_TRACE_DIR", traces);
    cmd
}

fn files_with_suffix(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.to_string_lossy().ends_with(suffix))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The only capture in `traces`: its parsed trace events and timings.
fn only_capture(traces: &Path) -> (Vec<serde_json::Value>, serde_json::Value) {
    let trace = files_with_suffix(traces, ".trace.json");
    let timings = files_with_suffix(traces, ".timings.json");
    assert_eq!(trace.len(), 1, "one complete trace: {trace:?}");
    assert_eq!(timings.len(), 1, "one timings summary: {timings:?}");
    let events = read_json(&trace[0]).as_array().unwrap().clone();
    (events, read_json(&timings[0]))
}

#[test]
fn traced_metadata_keeps_stdout_byte_identical() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let traces = dir.path().join("traces");

    let plain = cabin()
        .current_dir(dir.path())
        .arg("metadata")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let assertion = traced(dir.path(), &traces)
        .arg("metadata")
        .assert()
        .success();
    let output = assertion.get_output();
    assert_eq!(output.stdout, plain, "tracing must not touch stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cabin: wrote trace"), "{stderr}");

    let (events, timings) = only_capture(&traces);
    assert!(
        events
            .iter()
            .any(|e| e["ph"] == "B" && e["name"] == "cabin" && e["args"]["command"] == "metadata"),
        "the root span names the command"
    );
    assert_eq!(timings["format_version"], 1);
    assert_eq!(timings["command"], "metadata");
    assert_eq!(timings["outcome"], "success");
    assert_eq!(timings["trace_complete"], true);
    if cfg!(unix) {
        assert!(timings["process"]["self_peak_rss_bytes"].as_u64().unwrap() > 0);
    } else {
        assert!(timings["process"].is_null());
    }
    let phases = timings["phases"].as_array().unwrap();
    assert!(
        phases
            .iter()
            .any(|p| p["target"] == "cabin_workspace::loader"),
        "the workspace load is attributed: {phases:?}"
    );
}

#[test]
fn a_failing_command_keeps_its_error_and_finishes_the_capture() {
    let dir = TempDir::new().unwrap();
    let traces = dir.path().join("traces");
    let assertion = traced(dir.path(), &traces)
        .arg("metadata")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("cabin::workspace::manifest_not_found"),
        "the command's own diagnostic still renders: {stderr}"
    );

    let (_, timings) = only_capture(&traces);
    assert_eq!(timings["outcome"], "failure");
    assert_eq!(timings["trace_complete"], true);
}

#[test]
fn quiet_suppresses_the_trace_location_line() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let traces = dir.path().join("traces");
    traced(dir.path(), &traces)
        .args(["metadata", "--quiet"])
        .assert()
        .success()
        .stderr("");
    only_capture(&traces);
}

#[test]
fn malformed_filter_refuses_to_run_the_command() {
    let dir = TempDir::new().unwrap();
    let traces = dir.path().join("traces");
    traced(dir.path(), &traces)
        .env("CABIN_TRACE_FILTER", "cabin=loud")
        .args(["init", "--name", "hello"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("CABIN_TRACE_FILTER"));
    assert!(
        !dir.path().join("cabin.toml").exists(),
        "the command must not run without the requested capture"
    );
    assert!(!traces.exists(), "the filter is checked before any file");
}

#[test]
fn an_unusable_trace_dir_refuses_to_run_the_command() {
    let dir = TempDir::new().unwrap();
    let not_a_dir = dir.path().join("file");
    fs::write(&not_a_dir, "").unwrap();
    traced(dir.path(), &not_a_dir)
        .args(["init", "--name", "hello"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("CABIN_TRACE_DIR"));
    assert!(!dir.path().join("cabin.toml").exists());
}

#[test]
fn concurrent_invocations_write_separate_complete_captures() {
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    let traces = dir.path().join("traces");
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                traced(dir.path(), &traces)
                    .arg("metadata")
                    .assert()
                    .success();
            });
        }
    });
    let trace_files = files_with_suffix(&traces, ".trace.json");
    assert_eq!(trace_files.len(), 4);
    for trace in &trace_files {
        assert!(read_json(trace).is_array(), "{} parses", trace.display());
    }
    assert_eq!(files_with_suffix(&traces, ".timings.json").len(), 4);
}

/// The names on the trace's Ninja lane, checked against the summary's
/// count.
fn ninja_actions(events: &[serde_json::Value], timings: &serde_json::Value) -> Vec<String> {
    let names: Vec<String> = events
        .iter()
        .filter(|e| e["ph"] == "X" && e["pid"] == 2)
        .map(|e| e["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(timings["ninja_actions"], names.len());
    names
}

/// An executable built from `src/main.c` and `src/util.c`.
fn write_two_source_c_project(dir: &TempDir) {
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "cdemo"
version = "0.1.0"
c-standard = "c11"

[target.app]
type = "executable"
sources = ["src/main.c", "src/util.c"]
"#,
        )
        .unwrap();
    dir.child("src/main.c")
        .write_str("int util(void);\nint main(void) { return util(); }\n")
        .unwrap();
    dir.child("src/util.c")
        .write_str("int util(void) { return 0; }\n")
        .unwrap();
}

#[test]
fn traced_builds_record_only_the_ninja_actions_they_ran() {
    require_c_and_cxx_build_tools();
    let c_project = TempDir::new().unwrap();
    write_two_source_c_project(&c_project);
    let cxx_project = TempDir::new().unwrap();
    write_hello_project(cxx_project.path());

    let object = format!(".{}", host_obj_ext());
    for (project, objects) in [(c_project.path(), 2), (cxx_project.path(), 1)] {
        let clean = project.join("clean-traces");
        traced(project, &clean).arg("build").assert().success();
        let (events, timings) = only_capture(&clean);
        let actions = ninja_actions(&events, &timings);
        assert_eq!(
            actions
                .iter()
                .filter(|name| name.ends_with(&object))
                .count(),
            objects,
            "{actions:?}"
        );
        assert_eq!(actions.len(), objects + 1, "plus the link: {actions:?}");
        assert!(
            actions.iter().all(|name| name.starts_with("packages/")),
            "named relative to the build directory: {actions:?}"
        );

        let no_op = project.join("no-op-traces");
        traced(project, &no_op).arg("build").assert().success();
        let (events, timings) = only_capture(&no_op);
        assert!(
            ninja_actions(&events, &timings).is_empty(),
            "the log's earlier entries are not this build's"
        );
    }
}

#[test]
fn a_build_whose_ninja_log_is_due_for_compaction_keeps_its_actions() {
    require_c_and_cxx_build_tools();
    let c_project = TempDir::new().unwrap();
    write_two_source_c_project(&c_project);
    let cxx_project = TempDir::new().unwrap();
    write_hello_project(cxx_project.path());

    for (project, source) in [
        (c_project.path(), "src/util.c"),
        (cxx_project.path(), "src/main.cc"),
    ] {
        cabin().current_dir(project).arg("build").assert().success();
        // Copies of Ninja's own entries, well past the 100 entries and
        // three per output at which Ninja compacts its log on load.
        let log = project.join("build/dev/.ninja_log");
        let written = fs::read_to_string(&log).unwrap();
        let (header, entries) = written.split_once('\n').unwrap();
        let inflated = format!("{header}\n{}", entries.repeat(60));
        fs::write(&log, &inflated).unwrap();
        fs::File::options()
            .write(true)
            .open(project.join(source))
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();

        let traces = project.join("traces");
        traced(project, &traces).arg("build").assert().success();
        let (events, timings) = only_capture(&traces);
        let actions = ninja_actions(&events, &timings);
        assert_eq!(actions.len(), 2, "the object and the link: {actions:?}");
        assert!(
            fs::read(&log).unwrap().len() < inflated.len(),
            "the log was compacted"
        );
    }
}

#[test]
fn a_build_that_finishes_no_action_counts_zero_actions() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_hello_project(dir.path());
    // Ninja creates `.ninja_log` only once an action succeeds.
    dir.child("src/main.cc")
        .write_str("int main() { return missing; }\n")
        .unwrap();
    let traces = dir.path().join("traces");
    traced(dir.path(), &traces).arg("build").assert().failure();
    let (events, timings) = only_capture(&traces);
    assert!(ninja_actions(&events, &timings).is_empty());
}

#[test]
fn traced_test_run_records_each_test_executable() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.c_test]
type = "test"
sources = ["tests/c_test.c"]

[target.cxx_test]
type = "test"
sources = ["tests/cxx_test.cc"]
"#,
        )
        .unwrap();
    dir.child("tests/c_test.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    dir.child("tests/cxx_test.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    let traces = dir.path().join("traces");
    traced(dir.path(), &traces).arg("test").assert().success();

    let (events, _) = only_capture(&traces);
    let mut tests: Vec<&str> = events
        .iter()
        .filter(|e| e["ph"] == "X" && e["pid"] == 3)
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    tests.sort_unstable();
    assert_eq!(tests, ["demo:c_test", "demo:cxx_test"]);
}
