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
