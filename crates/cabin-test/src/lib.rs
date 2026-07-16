//! Test plan + sequential test runner for Cabin's `test`
//! targets.
//!
//! `cabin test` is intentionally a thin layer on top of the
//! existing build pipeline:
//!
//! 1. The CLI builds the selected `test` targets through the
//!    ordinary `cabin-build` planner - no test-specific build
//!    machinery is invented here.
//! 2. This crate turns the resulting [`cabin_build::BuildGraph`]
//!    into a deterministic [`TestPlan`].
//! 3. [`run_tests`] executes the plan sequentially, captures
//!    stdout / stderr from each test executable, and produces a
//!    [`TestSummary`] describing what passed and what failed.
//!
//! Crate boundary: this crate does not parse manifests, build
//! dependency graphs, generate Ninja, or know about config /
//! patches.  The CLI orchestrates those layers and hands a
//! finished `BuildGraph` plus the per-package CWD policy to
//! [`plan_tests`] / [`run_tests`].

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cabin_build::{BuildGraph, Dialect};
use cabin_core::TargetKind;
use cabin_workspace::{PackageGraph, WorkspacePackage};
use thiserror::Error;

/// One executable in a [`TestPlan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestExecutable {
    /// Workspace package the test belongs to.  Used both for
    /// summary output and for the executable's working directory.
    pub package: String,
    /// Manifest-declared target name (without any path / extension).
    pub target: String,
    /// Filesystem path of the linked test executable.
    pub executable: PathBuf,
    /// Manifest directory of the producing package.  Used as the
    /// working directory when the executable runs so tests can
    /// reach repository-relative fixture data deterministically.
    pub working_dir: PathBuf,
    /// Deterministic env overlay applied on top of the
    /// inherited environment when the executable runs.  Intended
    /// for `CABIN_*` keys produced by the orchestration layer
    /// via `cabin_env::package_env`.  Empty by default; callers
    /// that do not populate the overlay see the inherited
    /// environment unchanged.
    pub env: BTreeMap<String, OsString>,
}

/// A finalized, ordered list of `test` executables to run.
///
/// Ordering is deterministic: by package name, then by target
/// name.  Build it with [`plan_tests`] and consume it with
/// [`run_tests`].  Empty plans are allowed; the CLI decides
/// whether an empty plan is an error or a clean no-op.
#[derive(Debug, Clone, Default)]
pub struct TestPlan {
    executables: Vec<TestExecutable>,
}

impl<'a> IntoIterator for &'a TestPlan {
    type Item = &'a TestExecutable;
    type IntoIter = std::slice::Iter<'a, TestExecutable>;

    fn into_iter(self) -> Self::IntoIter {
        self.executables.iter()
    }
}

impl TestPlan {
    /// Iterate executables in the plan's documented order.
    pub fn iter(&self) -> std::slice::Iter<'_, TestExecutable> {
        self.executables.iter()
    }

    /// Number of executables to run.
    pub fn len(&self) -> usize {
        self.executables.len()
    }

    /// `true` if the plan has no executables.
    pub fn is_empty(&self) -> bool {
        self.executables.is_empty()
    }

    /// Apply `f` to every executable in the plan.  Used by the
    /// orchestration layer to attach a `CABIN_*` env overlay
    /// after planning without exposing the executables vec
    /// directly.
    pub fn for_each_executable_mut(&mut self, mut f: impl FnMut(&mut TestExecutable)) {
        for exe in &mut self.executables {
            f(exe);
        }
    }
}

/// Build a [`TestPlan`] from a finished [`BuildGraph`] plus the
/// originating [`PackageGraph`].
///
/// The plan picks every `test` target whose linked
/// executable appears in `graph.default_outputs` (i.e. every
/// `test` the build was asked to produce). `test`
/// targets that the planner did *not* build are absent from the
/// plan - that is the contract: callers select which test targets
/// to build (typically through the planner's manifest-target
/// selector list), and `plan_tests` runs exactly the ones whose
/// executable exists in the graph.
///
/// If `selected_packages` is `Some`, the plan is restricted to
/// those package indices; passing `None` walks the graph's
/// primary set, matching the planner's default selection.
///
/// Ordering is `(package_name, target_name)` ascending - the
/// same order `cabin metadata` and the planner emit, so plans
/// are deterministic across runs.
pub fn plan_tests(
    package_graph: &PackageGraph,
    build_graph: &BuildGraph,
    selected_packages: Option<&[usize]>,
) -> TestPlan {
    // `default_outputs` are UTF-8 build-graph paths; borrow each as a
    // native `&Path` for the filesystem comparison below.
    let outputs: BTreeSet<&Path> = build_graph
        .default_outputs
        .iter()
        .map(|p| p.as_std_path())
        .collect();

    let pkg_indices: Vec<usize> = match selected_packages {
        Some(s) => s.to_vec(),
        None => package_graph.primary_packages.clone(),
    };

    let mut entries: Vec<TestExecutable> = Vec::new();
    for idx in pkg_indices {
        let package = &package_graph.packages[idx];
        for target in &package.package.targets {
            if target.kind != TargetKind::Test {
                continue;
            }
            // Skip tests the planner was not asked to build.
            // Callers that pass a narrower manifest-target
            // selector list rely on this to drop targets that did
            // not make it into the graph.
            let Some(exe) =
                expected_executable(package, target.name.as_str(), build_graph.dialect, &outputs)
            else {
                continue;
            };
            entries.push(TestExecutable {
                package: package.package.name.to_string(),
                target: target.name.to_string(),
                executable: exe.to_path_buf(),
                working_dir: package.manifest_dir.clone(),
                env: BTreeMap::new(),
            });
        }
    }

    entries.sort_by(|a, b| {
        a.package
            .cmp(&b.package)
            .then_with(|| a.target.cmp(&b.target))
    });
    TestPlan {
        executables: entries,
    }
}

fn expected_executable<'a>(
    package: &WorkspacePackage,
    target_name: &str,
    dialect: Dialect,
    outputs: &BTreeSet<&'a Path>,
) -> Option<&'a Path> {
    // The planner names every `test` executable
    // `<build_dir>/<profile>/packages/<pkg-components>/<target>`
    // (one directory per `path_components` element, so a scoped
    // package nests under its scope dir) using the dialect's
    // executable spelling - bare on GNU/Clang, `<target>.exe`
    // under MSVC.  Build the tail with that same spelling (the dialect is
    // the planner's own, carried on the graph) and scan
    // `default_outputs` for it, rather than re-deriving the full path
    // here, so the planner stays the single source of truth for output
    // paths - and so Windows `.exe` test binaries are matched, not
    // silently skipped.
    let exe_name = dialect.executable_name(target_name);
    let needle_tail: PathBuf = std::iter::once("packages")
        .chain(package.package.name.path_components())
        .chain(std::iter::once(exe_name.as_str()))
        .collect();
    outputs.iter().copied().find(|p| p.ends_with(&needle_tail))
}

/// Result of running one test executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRunResult {
    /// The executable that was run.
    pub executable: TestExecutable,
    /// Outcome classification (passed / failed).
    pub status: TestRunStatus,
    /// Captured stdout, in order of arrival.
    pub stdout: Vec<u8>,
    /// Captured stderr, in order of arrival.
    pub stderr: Vec<u8>,
}

/// Outcome of one test executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRunStatus {
    /// Process exited with status `0`.
    Passed,
    /// Process exited with a non-zero status.  The exit status is
    /// included so callers can render `(exit code N)`.
    Failed { code: Option<i32> },
}

impl TestRunStatus {
    /// `true` for successful outcomes only.
    pub const fn is_success(self) -> bool {
        matches!(self, TestRunStatus::Passed)
    }

    fn from_status(status: ExitStatus) -> Self {
        if status.success() {
            Self::Passed
        } else {
            Self::Failed {
                code: status.code(),
            }
        }
    }
}

/// Aggregate summary of one `cabin test` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSummary {
    /// Per-executable results in execution order.
    pub results: Vec<TestRunResult>,
    /// Wall-clock time the whole run took, measured by
    /// [`run_tests`] across every executable in the plan.
    pub elapsed: Duration,
}

impl TestSummary {
    /// Total number of executables run.
    pub fn total(&self) -> usize {
        self.results.len()
    }

    /// Number of executables that exited with status `0`.
    pub fn passed(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.status.is_success())
            .count()
    }

    /// Number of executables that exited non-zero.
    pub fn failed(&self) -> usize {
        self.results
            .iter()
            .filter(|r| !r.status.is_success())
            .count()
    }

    /// `true` if every executable in the summary passed.
    pub fn all_passed(&self) -> bool {
        self.results.iter().all(|r| r.status.is_success())
    }
}

/// Sink for test executable output.  The runner forwards stdout /
/// stderr chunks to this sink while each process is still
/// running, and also keeps a full captured copy in
/// [`TestRunResult`].  Tests in this crate use [`null_sink`] to
/// discard output.
pub trait TestOutputSink {
    /// Called zero or more times per executable with stdout bytes.
    ///
    /// # Errors
    /// Returns the implementor's [`io::Error`] if the sink fails to
    /// write the supplied stdout bytes.
    fn write_stdout(&mut self, executable: &TestExecutable, bytes: &[u8]) -> io::Result<()>;
    /// Called zero or more times per executable with stderr bytes.
    ///
    /// # Errors
    /// Returns the implementor's [`io::Error`] if the sink fails to
    /// write the supplied stderr bytes.
    fn write_stderr(&mut self, executable: &TestExecutable, bytes: &[u8]) -> io::Result<()>;

    /// Called exactly once per executable, immediately after it
    /// exits and before the next executable starts.  Streaming
    /// sinks render the per-test result line here so multi-test
    /// runs report progress the way `cargo test` does.  The
    /// default implementation does nothing.
    ///
    /// # Errors
    /// Returns the implementor's [`io::Error`] if the sink fails to
    /// write the result line.
    fn test_finished(&mut self, result: &TestRunResult) -> io::Result<()> {
        let _ = result;
        Ok(())
    }
}

impl TestOutputSink for () {
    fn write_stdout(&mut self, _executable: &TestExecutable, _bytes: &[u8]) -> io::Result<()> {
        Ok(())
    }
    fn write_stderr(&mut self, _executable: &TestExecutable, _bytes: &[u8]) -> io::Result<()> {
        Ok(())
    }
}

/// A `TestOutputSink` that discards all bytes - useful for unit
/// tests of the runner itself.
pub fn null_sink() -> impl TestOutputSink {}

/// A `TestOutputSink` that streams bytes to the supplied
/// stdout/stderr writers.  Each non-empty write prepends a header
/// so the user can tell which executable is speaking.
pub struct StreamingSink<W1, W2> {
    /// Writer for captured stdout (typically the parent process's
    /// stdout).
    pub stdout: W1,
    /// Writer for captured stderr (typically the parent process's
    /// stderr).
    pub stderr: W2,
}

/// Stream `bytes` to `writer` under a `---- <label>: <pkg>:<target> ----`
/// header, appending a trailing newline when the payload lacks one.  A
/// no-op for empty `bytes`.
fn write_labeled<W: Write>(
    writer: &mut W,
    executable: &TestExecutable,
    bytes: &[u8],
    label: &str,
) -> io::Result<()> {
    if !bytes.is_empty() {
        writeln!(
            writer,
            "---- {label}: {}:{} ----",
            executable.package, executable.target
        )?;
        writer.write_all(bytes)?;
        if !bytes.ends_with(b"\n") {
            writer.write_all(b"\n")?;
        }
    }
    Ok(())
}

impl<W1: Write, W2: Write> TestOutputSink for StreamingSink<W1, W2> {
    fn write_stdout(&mut self, executable: &TestExecutable, bytes: &[u8]) -> io::Result<()> {
        write_labeled(&mut self.stdout, executable, bytes, "stdout")
    }
    fn write_stderr(&mut self, executable: &TestExecutable, bytes: &[u8]) -> io::Result<()> {
        write_labeled(&mut self.stderr, executable, bytes, "stderr")
    }
    fn test_finished(&mut self, result: &TestRunResult) -> io::Result<()> {
        writeln!(self.stdout, "{}", render_result_line(result))
    }
}

/// Run every executable in `plan` sequentially in the order
/// produced by [`plan_tests`].  Each test runs to completion
/// before the next starts; the runner does not introduce
/// parallelism in this release.  The returned [`TestSummary`]
/// preserves the plan's order so output stays deterministic.
///
/// A test executable's stdout / stderr are forwarded to `sink`
/// while the process is running and also captured to memory for
/// the returned summary.  Streaming sinks (see [`StreamingSink`])
/// write a header for each non-empty output chunk so multi-test
/// runs are easy to read.
///
/// # Panics
///
/// Panics if a spawned child process does not expose stdout or
/// stderr after the runner configured both streams as piped.
///
/// # Errors
/// Returns [`TestRunError`]: `Spawn` if a test executable cannot be
/// started, `Wait` if waiting on a running child fails, `OutputIo`
/// if reading the child's stdout/stderr fails, and `SinkIo` if
/// forwarding captured output to `sink` fails (propagated from the
/// sink's `write_stdout` / `write_stderr`).
pub fn run_tests<S: TestOutputSink>(
    plan: &TestPlan,
    sink: &mut S,
) -> Result<TestSummary, TestRunError> {
    let started = Instant::now();
    let mut results: Vec<TestRunResult> = Vec::with_capacity(plan.executables.len());
    for executable in &plan.executables {
        let mut command = Command::new(&executable.executable);
        command.current_dir(&executable.working_dir);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        // Tests inherit the user's PATH plus whatever Cabin's
        // own caller has set, with the per-executable env
        // overlay applied on top so the orchestration layer can
        // surface deterministic CABIN_* values without forcing
        // every test fixture to re-derive them.
        for (key, value) in &executable.env {
            command.env(key, value);
        }
        // The registry credential is Cabin's input, not the test's:
        // scrub it so arbitrary test code can never read the token.
        command.env_remove(cabin_env::CABIN_REGISTRY_TOKEN);
        // Retry on `ETXTBSY`: a sibling thread that forks while we
        // are mid-`write`/`chmod` of another executable can leave a
        // writable fd to this file briefly inherited in its
        // not-yet-`execve`d child, which makes our own `execve`
        // race-fail.  The window clears within milliseconds.
        let mut child = retry_on_etxtbsy(SPAWN_RETRY_ATTEMPTS, SPAWN_RETRY_BASE_DELAY, || {
            command.spawn()
        })
        .map_err(|source| TestRunError::Spawn {
            package: executable.package.clone(),
            target: executable.target.clone(),
            executable: executable.executable.clone(),
            source,
        })?;

        let stdout = child
            .stdout
            .take()
            .expect("stdout is piped before child spawn");
        let stderr = child
            .stderr
            .take()
            .expect("stderr is piped before child spawn");
        let (tx, rx) = mpsc::channel();
        let stdout_thread = spawn_output_reader(OutputStream::Stdout, stdout, tx.clone());
        let stderr_thread = spawn_output_reader(OutputStream::Stderr, stderr, tx);

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let output_result = forward_output_events(executable, sink, rx, &mut stdout, &mut stderr);
        if let Err(err) = output_result {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            return Err(err);
        }
        let status = child.wait().map_err(|source| TestRunError::Wait {
            package: executable.package.clone(),
            target: executable.target.clone(),
            executable: executable.executable.clone(),
            source,
        })?;
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();

        let result = TestRunResult {
            executable: executable.clone(),
            status: TestRunStatus::from_status(status),
            stdout,
            stderr,
        };
        sink.test_finished(&result).map_err(TestRunError::SinkIo)?;
        results.push(result);
    }
    Ok(TestSummary {
        results,
        elapsed: started.elapsed(),
    })
}

/// Total spawn attempts before an `ETXTBSY` failure is surfaced.
const SPAWN_RETRY_ATTEMPTS: u32 = 8;
/// Backoff before the first spawn retry; doubles on each retry, so
/// eight attempts wait up to ~127ms in total before giving up.
const SPAWN_RETRY_BASE_DELAY: Duration = Duration::from_millis(1);

/// Call `attempt`, retrying with exponential backoff while it fails
/// with [`io::ErrorKind::ExecutableFileBusy`] (`ETXTBSY`).  Any other
/// outcome - success or a different error - returns immediately, and
/// the final attempt's result is returned even if still busy.  Always
/// calls `attempt` at least once.
fn retry_on_etxtbsy<T>(
    max_attempts: u32,
    base_delay: Duration,
    mut attempt: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let mut delay = base_delay;
    let mut result = attempt();
    for _ in 1..max_attempts {
        match &result {
            Err(err) if err.kind() == io::ErrorKind::ExecutableFileBusy => {}
            _ => return result,
        }
        thread::sleep(delay);
        delay = delay.saturating_mul(2);
        result = attempt();
    }
    result
}

#[derive(Debug, Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

struct OutputEvent {
    stream: OutputStream,
    bytes: Vec<u8>,
}

fn spawn_output_reader<R: Read + Send + 'static>(
    stream: OutputStream,
    mut reader: R,
    tx: mpsc::Sender<Result<OutputEvent, io::Error>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0_u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx
                        .send(Ok(OutputEvent {
                            stream,
                            bytes: buf[..n].to_vec(),
                        }))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(source) => {
                    let _ = tx.send(Err(source));
                    break;
                }
            }
        }
    })
}

fn forward_output_events<S: TestOutputSink>(
    executable: &TestExecutable,
    sink: &mut S,
    rx: mpsc::Receiver<Result<OutputEvent, io::Error>>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) -> Result<(), TestRunError> {
    for event in rx {
        let event = event.map_err(TestRunError::OutputIo)?;
        match event.stream {
            OutputStream::Stdout => {
                sink.write_stdout(executable, &event.bytes)
                    .map_err(TestRunError::SinkIo)?;
                stdout.extend_from_slice(&event.bytes);
            }
            OutputStream::Stderr => {
                sink.write_stderr(executable, &event.bytes)
                    .map_err(TestRunError::SinkIo)?;
                stderr.extend_from_slice(&event.bytes);
            }
        }
    }
    Ok(())
}

/// Format the `cargo test`-shaped one-line summary:
/// `test result: ok.  P passed; F failed; 0 ignored; 0 measured;
/// FO filtered out; finished in T.TTs`.  Centralized here so the
/// CLI does not invent its own format.
///
/// Cabin has no ignore or benchmark mechanism, so the `ignored`
/// and `measured` fields render as constant zeros to keep the
/// line shaped exactly like `cargo test`'s. `filtered_out` is
/// the number of `test` targets the invocation deselected (via
/// `--test <NAME>`), supplied by the orchestration layer.
pub fn render_summary_line(summary: &TestSummary, filtered_out: usize) -> String {
    let passed = summary.passed();
    let failed = summary.failed();
    let outcome = if failed == 0 { "ok" } else { "FAILED" };
    let elapsed = summary.elapsed.as_secs_f64();
    format!(
        "test result: {outcome}. {passed} passed; {failed} failed; 0 ignored; 0 measured; {filtered_out} filtered out; finished in {elapsed:.2}s"
    )
}

/// Render everything the CLI prints after the last per-test
/// result line: the `cargo test`-shaped `failures:` name recap
/// (only when something failed) followed by the summary line,
/// each preceded by a blank line.  The returned string carries no
/// trailing newline; callers terminate it themselves.
pub fn render_epilogue(summary: &TestSummary, filtered_out: usize) -> String {
    // Writing into a `String` is infallible, so the `writeln!`
    // results below are safely discarded.
    use std::fmt::Write as _;

    let mut out = String::new();
    let failed: Vec<&TestRunResult> = summary
        .results
        .iter()
        .filter(|r| !r.status.is_success())
        .collect();
    if !failed.is_empty() {
        out.push_str("\nfailures:\n");
        for result in failed {
            let _ = writeln!(
                out,
                "    {}:{}",
                result.executable.package, result.executable.target
            );
        }
    }
    out.push('\n');
    out.push_str(&render_summary_line(summary, filtered_out));
    out
}

/// Render the per-test result line emitted after each executable
/// finishes.
pub fn render_result_line(result: &TestRunResult) -> String {
    let label = match result.status {
        TestRunStatus::Passed => "ok".to_owned(),
        TestRunStatus::Failed { code: Some(c) } => format!("FAILED (exit {c})"),
        TestRunStatus::Failed { code: None } => "FAILED (terminated by signal)".to_owned(),
    };
    format!(
        "test {}:{} ... {label}",
        result.executable.package, result.executable.target
    )
}

/// Errors produced while running tests.
#[derive(Debug, Error)]
pub enum TestRunError {
    /// The OS could not start the test executable.
    #[error("failed to start test target `{package}:{target}` ({}): {source}", .executable.display())]
    Spawn {
        package: String,
        target: String,
        executable: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The OS started the test executable, but waiting for it to
    /// finish failed.
    #[error("failed to wait for test target `{package}:{target}` ({}): {source}", .executable.display())]
    Wait {
        package: String,
        target: String,
        executable: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Reading stdout / stderr from the child process failed.
    #[error("failed to read captured test output: {0}")]
    OutputIo(#[source] io::Error),
    /// Writing captured stdout / stderr to the sink failed.  The
    /// runner stops at the first failure rather than continuing
    /// silently.
    #[error("failed to write captured test output: {0}")]
    SinkIo(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    // `assert_fs` is only used by the Unix-only fixture tests below.
    #[cfg(unix)]
    use assert_fs::TempDir;
    #[cfg(unix)]
    use assert_fs::prelude::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    // The fixture-based tests below run a fake test executable.  On Unix
    // that fixture is a `#!/bin/sh` script marked executable; Windows
    // has no equivalent that `Command::new` can spawn directly (a
    // `.bat` needs `cmd`, a real `.exe` needs a compiler), so those
    // tests are Unix-only.  The production `cabin test` path is covered
    // on Windows by the `library-with-tests` example end-to-end test,
    // which runs real compiled `.exe` test targets.
    #[cfg(unix)]
    fn write_executable(file: &assert_fs::fixture::ChildPath, body: &str) {
        file.write_str(body).unwrap();
        let mut perms = std::fs::metadata(file.path()).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(file.path(), perms).unwrap();
    }

    #[test]
    fn plan_orders_executables_by_package_then_target() {
        let plan = TestPlan {
            executables: vec![
                TestExecutable {
                    package: "alpha".into(),
                    target: "z_test".into(),
                    executable: PathBuf::from("/tmp/x"),
                    working_dir: PathBuf::from("/tmp"),
                    env: BTreeMap::new(),
                },
                TestExecutable {
                    package: "alpha".into(),
                    target: "a_test".into(),
                    executable: PathBuf::from("/tmp/x"),
                    working_dir: PathBuf::from("/tmp"),
                    env: BTreeMap::new(),
                },
            ],
        };
        // sanity: TestPlan does not reorder; ordering is the
        // plan_tests() job.  We test here that summary_line is
        // stable for a known shape.
        let summary = TestSummary {
            results: plan
                .executables
                .iter()
                .map(|e| TestRunResult {
                    executable: e.clone(),
                    status: TestRunStatus::Passed,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                })
                .collect(),
            elapsed: Duration::ZERO,
        };
        assert_eq!(summary.total(), 2);
        assert_eq!(summary.passed(), 2);
        assert!(summary.all_passed());
        assert_eq!(
            render_summary_line(&summary, 0),
            "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s"
        );
    }

    /// A scoped package's test executable lives under
    /// `packages/<scope>/<name>/<target>`; the discovery needle must
    /// nest per name component, and a bare needle must not match
    /// across the scope boundary.
    #[test]
    fn expected_executable_matches_scoped_package_dirs() {
        use cabin_core::{Package, PackageName};
        use cabin_workspace::{PackageKind, WorkspacePackage};
        let make = |name: &str| WorkspacePackage {
            package: Package::new(
                PackageName::new(name).unwrap(),
                "0.1.0".parse().unwrap(),
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
            manifest_path: PathBuf::from("/w/cabin.toml"),
            manifest_dir: PathBuf::from("/w"),
            deps: Vec::new(),
            kind: PackageKind::Local,
            is_port: false,
        };
        let scoped_exe = Path::new("/b/dev/packages/fmtlib/fmt/unit_test");
        let outputs: BTreeSet<&Path> = [scoped_exe].into_iter().collect();
        assert_eq!(
            expected_executable(&make("fmtlib/fmt"), "unit_test", Dialect::GnuLike, &outputs),
            Some(scoped_exe)
        );
        assert_eq!(
            expected_executable(&make("fmt"), "unit_test", Dialect::GnuLike, &outputs),
            None
        );
    }

    fn result_for(target: &str, status: TestRunStatus) -> TestRunResult {
        TestRunResult {
            executable: TestExecutable {
                package: "demo".into(),
                target: target.into(),
                executable: PathBuf::from("/tmp/x"),
                working_dir: PathBuf::from("/tmp"),
                env: BTreeMap::new(),
            },
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn epilogue_is_blank_line_plus_summary_when_everything_passes() {
        let summary = TestSummary {
            results: vec![result_for("pass_test", TestRunStatus::Passed)],
            elapsed: Duration::ZERO,
        };
        assert_eq!(
            render_epilogue(&summary, 2),
            "\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s"
        );
    }

    #[test]
    fn epilogue_recaps_failed_test_names_before_the_summary() {
        let summary = TestSummary {
            results: vec![
                result_for("fail_test", TestRunStatus::Failed { code: Some(1) }),
                result_for("pass_test", TestRunStatus::Passed),
            ],
            elapsed: Duration::ZERO,
        };
        assert_eq!(
            render_epilogue(&summary, 0),
            "\nfailures:\n    demo:fail_test\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s"
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_reports_pass_and_fail_in_summary() {
        struct RecordingSink {
            finished: Vec<String>,
        }
        impl TestOutputSink for RecordingSink {
            fn write_stdout(&mut self, _e: &TestExecutable, _b: &[u8]) -> io::Result<()> {
                Ok(())
            }
            fn write_stderr(&mut self, _e: &TestExecutable, _b: &[u8]) -> io::Result<()> {
                Ok(())
            }
            fn test_finished(&mut self, result: &TestRunResult) -> io::Result<()> {
                self.finished.push(result.executable.target.clone());
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        let pass = dir.child("pass_test");
        let fail = dir.child("fail_test");
        write_executable(&pass, "#!/bin/sh\nexit 0\n");
        write_executable(&fail, "#!/bin/sh\nexit 1\n");
        let plan = TestPlan {
            executables: vec![
                TestExecutable {
                    package: "demo".into(),
                    target: "fail_test".into(),
                    executable: fail.to_path_buf(),
                    working_dir: dir.path().to_path_buf(),
                    env: BTreeMap::new(),
                },
                TestExecutable {
                    package: "demo".into(),
                    target: "pass_test".into(),
                    executable: pass.to_path_buf(),
                    working_dir: dir.path().to_path_buf(),
                    env: BTreeMap::new(),
                },
            ],
        };
        let mut sink = RecordingSink {
            finished: Vec::new(),
        };
        let summary = run_tests(&plan, &mut sink).unwrap();
        assert_eq!(summary.total(), 2);
        assert_eq!(summary.passed(), 1);
        assert_eq!(summary.failed(), 1);
        assert!(!summary.all_passed());
        // execution order matches the plan's input order
        // (run_tests does not re-sort; that is plan_tests's job).
        assert_eq!(summary.results[0].executable.target, "fail_test");
        assert!(matches!(
            summary.results[0].status,
            TestRunStatus::Failed { code: Some(1) }
        ));
        assert_eq!(summary.results[1].executable.target, "pass_test");
        assert!(summary.results[1].status.is_success());
        // the sink saw each completion as it happened, in order.
        assert_eq!(sink.finished, vec!["fail_test", "pass_test"]);
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_forwards_output_before_process_exits() {
        struct MarkerSink {
            marker: PathBuf,
        }

        impl TestOutputSink for MarkerSink {
            fn write_stdout(
                &mut self,
                _executable: &TestExecutable,
                bytes: &[u8],
            ) -> io::Result<()> {
                if bytes
                    .windows("ready".len())
                    .any(|window| window == b"ready")
                {
                    std::fs::write(&self.marker, b"seen")?;
                }
                Ok(())
            }

            fn write_stderr(
                &mut self,
                _executable: &TestExecutable,
                _bytes: &[u8],
            ) -> io::Result<()> {
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        let marker = dir.child("sink-saw-output");
        let script = dir.child("streaming_test");
        write_executable(
            &script,
            r#"#!/bin/sh
printf 'ready\n'
i=0
while [ "$i" -lt 40 ]; do
  if [ -f "$MARKER" ]; then
    exit 0
  fi
  i=$((i + 1))
  sleep 0.05
done
exit 42
"#,
        );
        let plan = TestPlan {
            executables: vec![TestExecutable {
                package: "demo".into(),
                target: "streaming_test".into(),
                executable: script.to_path_buf(),
                working_dir: dir.path().to_path_buf(),
                env: BTreeMap::from([("MARKER".to_owned(), marker.path().as_os_str().to_owned())]),
            }],
        };
        let mut sink = MarkerSink {
            marker: marker.to_path_buf(),
        };
        let summary = run_tests(&plan, &mut sink).unwrap();

        assert!(summary.all_passed(), "{summary:?}");
        assert_eq!(summary.results[0].stdout, b"ready\n");
    }

    #[test]
    fn render_result_line_includes_exit_code_for_failures() {
        let exe = TestExecutable {
            package: "demo".into(),
            target: "fail_test".into(),
            executable: PathBuf::from("/tmp/x"),
            working_dir: PathBuf::from("/tmp"),
            env: BTreeMap::new(),
        };
        let result = TestRunResult {
            executable: exe.clone(),
            status: TestRunStatus::Failed { code: Some(42) },
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert_eq!(
            render_result_line(&result),
            "test demo:fail_test ... FAILED (exit 42)"
        );
        let result = TestRunResult {
            executable: exe,
            status: TestRunStatus::Passed,
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert_eq!(render_result_line(&result), "test demo:fail_test ... ok");
    }

    #[test]
    fn streaming_sink_skips_empty_output() {
        let mut sink = StreamingSink {
            stdout: Vec::<u8>::new(),
            stderr: Vec::<u8>::new(),
        };
        let exe = TestExecutable {
            package: "demo".into(),
            target: "x".into(),
            executable: PathBuf::from("/tmp/x"),
            working_dir: PathBuf::from("/tmp"),
            env: BTreeMap::new(),
        };
        sink.write_stdout(&exe, &[]).unwrap();
        sink.write_stderr(&exe, &[]).unwrap();
        assert!(sink.stdout.is_empty());
        assert!(sink.stderr.is_empty());
        sink.write_stdout(&exe, b"hello").unwrap();
        sink.test_finished(&TestRunResult {
            executable: exe,
            status: TestRunStatus::Passed,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
        .unwrap();
        let out = String::from_utf8(sink.stdout).unwrap();
        assert!(out.contains("---- stdout: demo:x ----"));
        assert!(out.contains("hello"));
        assert!(out.contains("test demo:x ... ok\n"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn retry_on_etxtbsy_retries_until_spawn_succeeds() {
        let mut calls = 0;
        let result = retry_on_etxtbsy(8, Duration::ZERO, || {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from(io::ErrorKind::ExecutableFileBusy))
            } else {
                Ok(99)
            }
        });
        assert_eq!(result.unwrap(), 99);
        assert_eq!(calls, 3);
    }

    #[test]
    fn retry_on_etxtbsy_gives_up_after_max_attempts() {
        let mut calls = 0;
        let result: io::Result<()> = retry_on_etxtbsy(4, Duration::ZERO, || {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::ExecutableFileBusy))
        });
        assert_eq!(
            result.unwrap_err().kind(),
            io::ErrorKind::ExecutableFileBusy
        );
        assert_eq!(calls, 4);
    }

    #[test]
    fn retry_on_etxtbsy_does_not_retry_other_errors() {
        let mut calls = 0;
        let result: io::Result<()> = retry_on_etxtbsy(8, Duration::ZERO, || {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(calls, 1);
    }
}
