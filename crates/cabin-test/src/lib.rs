//! Test plan + test runner for Cabin's `test` targets.
//!
//! `cabin test` is a thin layer on top of the build pipeline:
//!
//! 1. The CLI builds the selected `test` targets through the
//!    ordinary `cabin-build` planner.
//! 2. This crate turns the resulting [`cabin_build::BuildGraph`]
//!    into a deterministic [`TestPlan`].
//! 3. [`run_tests`] executes the plan, running up to
//!    [`TestRunOptions::jobs`] executables at once with the user's
//!    arguments, captures each one's stdout / stderr, and produces a
//!    [`TestSummary`] describing what passed and what failed.
//!
//! Crate boundary: this crate does not parse manifests, build
//! dependency graphs, generate Ninja, or know about config /
//! patches.  The CLI orchestrates those layers and hands a
//! finished `BuildGraph` and its `PackageGraph` to [`plan_tests`],
//! then the plan to [`run_tests`].

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use cabin_build::BuildGraph;
use cabin_core::{ExitStatusKind, TargetKind, TargetPlatform, exit_status_kind};
use cabin_workspace::{PackageGraph, WorkspacePackage};
use quick_junit::{NonSuccessKind, Report, TestCase, TestCaseStatus, TestSuite};
use shared_child::SharedChild;
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
    /// via `cabin_env::package_env`.  Empty as built by
    /// [`plan_tests`].  The runner removes the registry credentials
    /// from the environment whatever the overlay holds.
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

impl TestPlan {
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
/// The plan holds every `test` target whose linked executable
/// appears in `build_graph.default_outputs`.  Callers select which
/// test targets to build (typically through the planner's
/// manifest-target selector list); a `test` target the planner did
/// not build is absent from the plan.
///
/// If `selected_packages` is `Some`, the plan is restricted to
/// those package indices; passing `None` walks the graph's
/// primary set, matching the planner's default selection.
///
/// Ordering is `(package_name, target_name)` ascending - the
/// same order `cabin metadata` and the planner emit, so plans
/// are deterministic across runs.
#[tracing::instrument(level = "info", skip_all)]
pub fn plan_tests(
    package_graph: &PackageGraph,
    build_graph: &BuildGraph,
    platform: &TargetPlatform,
    selected_packages: Option<&[usize]>,
) -> TestPlan {
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
            let exe_name = build_graph
                .dialect
                .executable_name(target.name.as_str(), platform);
            let Some(exe) = expected_executable(package, &exe_name, &outputs) else {
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
    exe_name: &str,
    outputs: &BTreeSet<&'a Path>,
) -> Option<&'a Path> {
    // The planner names every `test` executable
    // `<build_dir>/<profile>/packages/<pkg-components>/<target>`
    // (one directory per `path_components` element, so a scoped
    // package nests under its scope dir) using the dialect's
    // executable spelling - `<target>.exe` for Windows, bare
    // elsewhere.  Only that tail, which the caller spells with the
    // planner's own dialect from the graph, is matched against
    // `default_outputs`, so the planner stays the single source of
    // truth for the rest of the path.
    let needle_tail: PathBuf = std::iter::once("packages")
        .chain(package.package.name.path_components())
        .chain(std::iter::once(exe_name))
        .collect();
    outputs.iter().copied().find(|p| p.ends_with(&needle_tail))
}

/// Result of running one test executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRunResult {
    /// The executable that was run.
    pub executable: TestExecutable,
    /// Outcome classification.
    pub status: TestRunStatus,
    /// Time from starting the test until it exited or was killed;
    /// zero for a test that could not start.
    pub duration: Duration,
    /// Captured stdout, in order of arrival.
    pub stdout: Vec<u8>,
    /// Captured stderr, in order of arrival.
    pub stderr: Vec<u8>,
    /// `true` when a process the test started still held its output
    /// open a second after the test exited, so the runner stopped
    /// reading: anything written after that is missing above.
    pub output_cut_off: bool,
}

/// Outcome of one test executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestRunStatus {
    /// Process exited with status `0`.
    Passed,
    /// Process exited unsuccessfully: a non-zero exit code or, on
    /// Unix, a terminating signal.
    Failed(ExitStatusKind),
    /// Process was still running when the per-test timeout (carried
    /// here) passed, so the runner killed it.
    TimedOut(Duration),
    /// The OS could not start the executable; carries the error.
    SpawnFailed(String),
}

impl TestRunStatus {
    /// `true` for successful outcomes only.
    pub const fn is_success(&self) -> bool {
        matches!(self, TestRunStatus::Passed)
    }
}

/// Aggregate summary of one `cabin test` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSummary {
    /// Per-executable results in plan order.
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

    /// Number of executables that did not pass: failed, timed out, or
    /// could not start.
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
/// stderr to this sink (see [`run_tests`] for when), and also keeps
/// a full captured copy in [`TestRunResult`].
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

    /// Called exactly once per executable, when it has finished and
    /// after its last output reached this sink.  Streaming sinks
    /// render the per-test result line here so multi-test runs
    /// report progress the way `cargo test` does.  The default
    /// implementation does nothing.
    ///
    /// # Errors
    /// Returns the implementor's [`io::Error`] if the sink fails to
    /// write the result line.
    fn test_finished(&mut self, result: &TestRunResult) -> io::Result<()> {
        let _ = result;
        Ok(())
    }
}

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

/// How [`run_tests`] runs a plan.
#[derive(Debug, Clone)]
pub struct TestRunOptions {
    /// Arguments passed to every executable, one `OsString` per
    /// argument.
    pub args: Vec<OsString>,
    /// Maximum number of executables running at once.
    pub jobs: NonZeroUsize,
    /// Wall-clock limit per executable; `None` waits indefinitely.
    pub timeout: Option<Duration>,
    /// A program and its arguments that run each executable, which
    /// they receive after `runner[1..]`; empty runs it directly.
    pub runner: Vec<String>,
}

/// Run every executable in `plan`, starting them in the order
/// produced by [`plan_tests`] with at most `options.jobs` running at
/// once.  Every executable runs, whatever the earlier ones did.  The
/// returned [`TestSummary`] keeps the plan's order; `sink` sees each
/// result as its executable finishes.
///
/// Every executable receives `options.args` as its argument list, one
/// `OsString` per argument, through [`Command::args`]: no shell
/// re-splits or quotes them, so empty arguments, spaces, and a
/// literal `--` reach the test framework unchanged.  Its stdin is
/// closed, since concurrent tests cannot share a terminal's input.
///
/// With one job, stdout / stderr chunks reach `sink` while the
/// process runs.  With more, each executable's captured output
/// reaches `sink` in one piece when it finishes, so concurrent tests
/// never interleave.  Either way [`TestRunResult`] keeps a full copy.
///
/// A test still running after `options.timeout` is killed and
/// reported as [`TestRunStatus::TimedOut`]; only the test process is
/// killed, not processes it started.  Once a test exits, the runner
/// waits at most a second for its output pipes to close, so a
/// leftover process holding them cannot stall the run (see
/// [`TestRunResult::output_cut_off`]).  A test that cannot start is
/// reported as [`TestRunStatus::SpawnFailed`].
///
/// # Panics
///
/// Panics if a spawned child process does not expose stdout or
/// stderr after the runner configured both streams as piped.
///
/// # Errors
/// Returns [`TestRunError`] when the runner itself fails: `Wait` or
/// `Kill` if waiting for or killing a running child fails, `OutputIo` if
/// reading a child's stdout/stderr fails, and `SinkIo` if forwarding
/// output or a result to `sink` fails.  Every test still running is
/// killed before the error returns.
#[tracing::instrument(level = "info", skip_all, fields(executables = plan.len()))]
pub fn run_tests<S: TestOutputSink>(
    plan: &TestPlan,
    options: &TestRunOptions,
    sink: &mut S,
) -> Result<TestSummary, TestRunError> {
    let started = Instant::now();
    let live = options.jobs.get() == 1;
    let (events, rx) = mpsc::channel();
    let mut queue = plan.executables.iter().enumerate();
    let mut running: Vec<RunningTest<'_>> = Vec::new();
    let mut results: Vec<Option<TestRunResult>> = vec![None; plan.executables.len()];
    loop {
        while running.len() < options.jobs.get() {
            let Some((index, executable)) = queue.next() else {
                break;
            };
            match RunningTest::spawn(index, executable, options, &events) {
                Ok(test) => running.push(test),
                Err(source) => {
                    let result = TestRunResult {
                        executable: executable.clone(),
                        status: TestRunStatus::SpawnFailed(source.to_string()),
                        duration: Duration::ZERO,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                        output_cut_off: false,
                    };
                    sink.test_finished(&result).map_err(TestRunError::SinkIo)?;
                    results[index] = Some(result);
                }
            }
        }
        if running.is_empty() {
            break;
        }
        // One event per pass, then a look at every test, so a test
        // flooding its pipes cannot hold off the others' timeouts.
        // Without a deadline the wait cannot hang: every running test
        // still owes an exit or end-of-file event.  The runner keeps a
        // sender, so the only error is a timeout.
        let next = running
            .iter()
            .filter_map(|test| test.deadline(options.timeout))
            .min();
        let event = match next {
            Some(deadline) => rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .ok(),
            None => rx.recv().ok(),
        };
        if let Some(event) = event {
            // Readers a cut-off test left behind may still deliver
            // events after it finished; they are dropped.
            if let Some(test) = running.iter_mut().find(|test| test.index == event.index) {
                test.record(event.kind, live, sink)?;
            }
        }
        // Sink writes block this loop, so a deadline is enforced late;
        // a test that exited before its kill keeps its own status.
        let now = Instant::now();
        let mut i = 0;
        while i < running.len() {
            running[i].enforce_timeout(options.timeout, now)?;
            if let Some((exited_at, status)) = running[i].finished(now).cloned() {
                let test = running.swap_remove(i);
                let index = test.index;
                results[index] = Some(test.finish(status, exited_at, live, sink)?);
            } else {
                i += 1;
            }
        }
    }
    Ok(TestSummary {
        results: results.into_iter().flatten().collect(),
        elapsed: started.elapsed(),
    })
}

/// How long the runner keeps reading a test's output after the test
/// exits.  A process the test started inherits its pipes and may
/// hold them open indefinitely.
const OUTPUT_GRACE: Duration = Duration::from_secs(1);

/// A test process the runner started and has not reported yet.
/// Dropping it before the child exits kills the child, so an
/// early error return or a panic never leaves a test running behind
/// Cabin.
struct RunningTest<'a> {
    index: usize,
    executable: &'a TestExecutable,
    /// Shared with a thread that waits for the exit, so the runner
    /// can kill the child meanwhile without racing PID reuse.
    child: Arc<SharedChild>,
    started: Instant,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    /// Pipes whose end of file has not been handled yet.
    open_streams: u8,
    /// Pipes whose reader has reached end of file.  This runs ahead of
    /// `open_streams` while the pipe's last events wait in the channel.
    pipes_closed: Arc<AtomicU8>,
    /// The timeout, once it passed and the child was killed.
    timed_out: Option<Duration>,
    /// When the runner saw the child exit, and how it ended.
    exited: Option<(Instant, TestRunStatus)>,
}

impl<'a> RunningTest<'a> {
    fn spawn(
        index: usize,
        executable: &'a TestExecutable,
        options: &TestRunOptions,
        events: &mpsc::Sender<TestEvent>,
    ) -> io::Result<Self> {
        let mut command = match options.runner.split_first() {
            Some((program, runner_args)) => {
                let mut command = Command::new(program);
                command.args(runner_args).arg(&executable.executable);
                command
            }
            None => Command::new(&executable.executable),
        };
        command
            .args(&options.args)
            .current_dir(&executable.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &executable.env {
            command.env(key, value);
        }
        // The registry credentials are Cabin's input, not the test's:
        // scrub them so arbitrary test code can never read them from
        // its environment.
        command.env_remove(cabin_env::CABIN_REGISTRY_TOKEN);
        command.env_remove(cabin_env::ACTIONS_ID_TOKEN_REQUEST_TOKEN);
        // Retry on `ETXTBSY`: Linux refuses to exec a file that any
        // process still holds open for writing, such as the linker of a
        // concurrent build that has not yet closed this executable.
        let child = retry_on_etxtbsy(SPAWN_RETRY_ATTEMPTS, SPAWN_RETRY_BASE_DELAY, || {
            SharedChild::spawn(&mut command)
        })
        .map_err(|error| match options.runner.first() {
            // The result line names the test, not the runner that failed.
            Some(program) => io::Error::new(error.kind(), format!("runner `{program}`: {error}")),
            None => error,
        })?;
        let stdout = child
            .take_stdout()
            .expect("stdout is piped before child spawn");
        let stderr = child
            .take_stderr()
            .expect("stderr is piped before child spawn");
        let test = Self {
            index,
            executable,
            child: Arc::new(child),
            started: Instant::now(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            open_streams: 2,
            pipes_closed: Arc::new(AtomicU8::new(0)),
            timed_out: None,
            exited: None,
        };
        let closed = &test.pipes_closed;
        spawn_output_reader(
            index,
            OutputStream::Stdout,
            stdout,
            events.clone(),
            closed.clone(),
        );
        spawn_output_reader(
            index,
            OutputStream::Stderr,
            stderr,
            events.clone(),
            closed.clone(),
        );
        let child = Arc::clone(&test.child);
        let events = events.clone();
        thread::spawn(move || {
            let kind = TestEventKind::Exited(child.wait());
            let _ = events.send(TestEvent { index, kind });
        });
        Ok(test)
    }

    fn record<S: TestOutputSink>(
        &mut self,
        event: TestEventKind,
        live: bool,
        sink: &mut S,
    ) -> Result<(), TestRunError> {
        match event {
            TestEventKind::Output(OutputStream::Stdout, bytes) => {
                if live {
                    sink.write_stdout(self.executable, &bytes)
                        .map_err(TestRunError::SinkIo)?;
                }
                self.stdout.extend_from_slice(&bytes);
            }
            TestEventKind::Output(OutputStream::Stderr, bytes) => {
                if live {
                    sink.write_stderr(self.executable, &bytes)
                        .map_err(TestRunError::SinkIo)?;
                }
                self.stderr.extend_from_slice(&bytes);
            }
            TestEventKind::Closed => self.open_streams -= 1,
            TestEventKind::ReadFailed(source) => return Err(TestRunError::OutputIo(source)),
            TestEventKind::Exited(status) => self.record_exit(status)?,
        }
        Ok(())
    }

    /// Record the exit as seen now, which starts the output grace.
    /// The deadline check may have seen it first; the waiter's report
    /// then changes nothing.
    fn record_exit(&mut self, status: io::Result<ExitStatus>) -> Result<(), TestRunError> {
        if self.exited.is_some() {
            return Ok(());
        }
        let status = status.map_err(|source| TestRunError::Wait {
            package: self.executable.package.clone(),
            target: self.executable.target.clone(),
            executable: self.executable.executable.clone(),
            source,
        })?;
        let status = match self.timed_out {
            Some(limit) => TestRunStatus::TimedOut(limit),
            None if status.success() => TestRunStatus::Passed,
            None => TestRunStatus::Failed(exit_status_kind(status)),
        };
        self.exited = Some((Instant::now(), status));
        Ok(())
    }

    /// Kill the child once `timeout` has passed.  The kill does not
    /// wait: the exit arrives as an event like any other, so one stuck
    /// kill cannot stall the other tests.
    fn enforce_timeout(
        &mut self,
        timeout: Option<Duration>,
        now: Instant,
    ) -> Result<(), TestRunError> {
        if self.exited.is_some() || self.timed_out.is_some() {
            return Ok(());
        }
        let Some(limit) = timeout.filter(|&limit| now.duration_since(self.started) >= limit) else {
            return Ok(());
        };
        // An exit still queued behind the sink's writes keeps its own
        // status.
        if let Some(status) = self.child.try_wait().transpose() {
            return self.record_exit(status);
        }
        self.child.kill().map_err(|source| TestRunError::Kill {
            package: self.executable.package.clone(),
            target: self.executable.target.clone(),
            executable: self.executable.executable.clone(),
            source,
        })?;
        self.timed_out = Some(limit);
        Ok(())
    }

    /// When the runner must look at this test without an event: at
    /// its timeout while it runs, or once its output grace ends.
    /// `None` when only an event can move it on.
    fn deadline(&self, timeout: Option<Duration>) -> Option<Instant> {
        match &self.exited {
            None if self.timed_out.is_none() => self.started.checked_add(timeout?),
            Some((exited_at, _)) if self.only_held() => exited_at.checked_add(OUTPUT_GRACE),
            // Killed and awaiting its exit, or a pipe's end of file is queued.
            _ => None,
        }
    }

    /// Every pipe at end of file has been handled; the rest are held.
    fn only_held(&self) -> bool {
        self.open_streams == 2 - self.pipes_closed.load(Ordering::Relaxed)
    }

    /// The test's status once it has exited and its output has been
    /// handled to the end of both pipes, or once [`OUTPUT_GRACE`] has
    /// passed since the exit with a pipe still open.  A pipe that
    /// closed is always handled to its end, however slow the sink.
    fn finished(&self, now: Instant) -> Option<&(Instant, TestRunStatus)> {
        let exited = self.exited.as_ref()?;
        let done = self.open_streams == 0
            || (self.only_held() && now.duration_since(exited.0) >= OUTPUT_GRACE);
        done.then_some(exited)
    }

    fn finish<S: TestOutputSink>(
        mut self,
        status: TestRunStatus,
        exited_at: Instant,
        live: bool,
        sink: &mut S,
    ) -> Result<TestRunResult, TestRunError> {
        if !live {
            sink.write_stdout(self.executable, &self.stdout)
                .map_err(TestRunError::SinkIo)?;
            sink.write_stderr(self.executable, &self.stderr)
                .map_err(TestRunError::SinkIo)?;
        }
        let result = TestRunResult {
            executable: self.executable.clone(),
            status,
            duration: exited_at.duration_since(self.started),
            stdout: std::mem::take(&mut self.stdout),
            stderr: std::mem::take(&mut self.stderr),
            output_cut_off: self.open_streams > 0,
        };
        sink.test_finished(&result).map_err(TestRunError::SinkIo)?;
        Ok(result)
    }
}

impl Drop for RunningTest<'_> {
    fn drop(&mut self) {
        if self.exited.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
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

/// What a reader thread observed on one of a test's pipes, or the
/// waiter thread on its process.
struct TestEvent {
    /// The test's position in the plan.
    index: usize,
    kind: TestEventKind,
}

enum TestEventKind {
    Output(OutputStream, Vec<u8>),
    /// The pipe reached end of file.
    Closed,
    ReadFailed(io::Error),
    /// The process exited, or waiting for it failed.
    Exited(io::Result<ExitStatus>),
}

/// Forward `reader` to the runner until end of file.  The thread is
/// detached: a process the test started can keep the pipe open after
/// the runner stops waiting, and a blocking read cannot be cancelled.
/// It exits on its next read once the runner has returned.
fn spawn_output_reader<R: Read + Send + 'static>(
    index: usize,
    stream: OutputStream,
    mut reader: R,
    events: mpsc::Sender<TestEvent>,
    pipes_closed: Arc<AtomicU8>,
) {
    thread::spawn(move || {
        let mut buf = [0_u8; 8192];
        loop {
            let (kind, last) = match reader.read(&mut buf) {
                Ok(0) => {
                    pipes_closed.fetch_add(1, Ordering::Relaxed);
                    (TestEventKind::Closed, true)
                }
                Ok(n) => (TestEventKind::Output(stream, buf[..n].to_vec()), false),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => (TestEventKind::ReadFailed(err), true),
            };
            if events.send(TestEvent { index, kind }).is_err() || last {
                break;
            }
        }
    });
}

/// Format the `cargo test`-shaped one-line summary:
/// `test result: ok. P passed; F failed; 0 ignored; 0 measured;
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

/// Why a test failed, or `None` when it passed.
fn failure_reason(status: &TestRunStatus) -> Option<String> {
    Some(match status {
        TestRunStatus::Passed => return None,
        TestRunStatus::Failed(ExitStatusKind::Code(code)) => format!("exit {code}"),
        TestRunStatus::Failed(ExitStatusKind::Signal(signal)) => {
            format!("terminated by signal {signal}")
        }
        TestRunStatus::Failed(ExitStatusKind::Unknown) => "unknown exit status".to_owned(),
        TestRunStatus::TimedOut(limit) => format!("timed out after {}s", limit.as_secs_f64()),
        TestRunStatus::SpawnFailed(error) => format!("could not start: {error}"),
    })
}

/// Render the per-test result line emitted after each executable
/// finishes.
pub fn render_result_line(result: &TestRunResult) -> String {
    let label = failure_reason(&result.status)
        .map_or_else(|| "ok".to_owned(), |reason| format!("FAILED ({reason})"));
    let cut_off = if result.output_cut_off {
        " (output cut off)"
    } else {
        ""
    };
    format!(
        "test {}:{} ... {label}{cut_off}",
        result.executable.package, result.executable.target
    )
}

/// Build the `--junit` report of `summary`: one `testsuite` per package
/// and one `testcase` per test executable, since Cabin observes
/// executables, not the test cases inside them.  A test that could
/// not start is an `error`, every other failure a `failure`.  A test
/// whose output was cut off has the property `output-cut-off` set to
/// `true`.
pub fn junit_report(summary: &TestSummary) -> Report {
    let mut report = Report::new("cabin test");
    report.set_time(summary.elapsed);
    // Plans are sorted by package, so each package's results are adjacent.
    for results in summary
        .results
        .chunk_by(|a, b| a.executable.package == b.executable.package)
    {
        let mut suite = TestSuite::new(results[0].executable.package.as_str());
        for result in results {
            let status = match failure_reason(&result.status) {
                None => TestCaseStatus::success(),
                Some(reason) => {
                    let kind = if matches!(result.status, TestRunStatus::SpawnFailed(_)) {
                        NonSuccessKind::Error
                    } else {
                        NonSuccessKind::Failure
                    };
                    let mut status = TestCaseStatus::non_success(kind);
                    status.set_message(reason);
                    status
                }
            };
            let mut case = TestCase::new(result.executable.target.as_str(), status);
            case.set_classname(result.executable.package.as_str())
                .set_time(result.duration)
                .set_system_out(xml_text(&result.stdout))
                .set_system_err(xml_text(&result.stderr));
            if result.output_cut_off {
                case.add_property(("output-cut-off", "true"));
            }
            suite.add_test_case(case);
        }
        report.add_test_suite(suite);
    }
    report
}

/// Test output as XML text, decoded lossily.  `quick-junit` drops
/// ANSI escapes and control characters but keeps U+FFFE and U+FFFF,
/// which XML also forbids, so they become replacement characters.
fn xml_text(output: &[u8]) -> String {
    String::from_utf8_lossy(output).replace(['\u{fffe}', '\u{ffff}'], "\u{fffd}")
}

/// Errors that stop the runner itself.  A test that fails, times
/// out, or cannot start is a [`TestRunStatus`], not an error.
#[derive(Debug, Error)]
pub enum TestRunError {
    /// Waiting for a running test executable to exit failed.
    #[error("failed to wait for test target `{package}:{target}` ({}): {source}", .executable.display())]
    Wait {
        package: String,
        target: String,
        executable: PathBuf,
        #[source]
        source: io::Error,
    },
    /// Killing a test executable that ran past its timeout failed.
    #[error("failed to kill timed-out test target `{package}:{target}` ({}): {source}", .executable.display())]
    Kill {
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
    use assert_fs::TempDir;
    // The prelude is only used by the Unix-only fixture tests below.
    #[cfg(unix)]
    use assert_fs::prelude::*;

    // The fixture-based tests below run a fake test executable.  On Unix
    // that fixture is a `#!/bin/sh` script marked executable; Windows
    // has no equivalent that `Command::new` can spawn directly (a
    // `.bat` needs `cmd`, a real `.exe` needs a compiler), so those
    // tests are Unix-only.  The production `cabin test` path is covered
    // on Windows by the `library-with-tests` example end-to-end test,
    // which runs real compiled `.exe` test targets.
    //
    // Only the `sh` child opens the fixture for writing: a child that
    // another test's thread forks while this process holds it open keeps
    // the descriptor until its own exec, and Linux refuses to exec a file
    // open for writing (ETXTBSY).
    #[cfg(unix)]
    fn write_executable(file: &assert_fs::fixture::ChildPath, body: &str) {
        let status = std::process::Command::new("sh")
            .args(["-c", r#"printf %s "$2" >"$1" && chmod 755 "$1""#, "sh"])
            .arg(file.path())
            .arg(body)
            .status()
            .unwrap();
        assert!(status.success());
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
                    duration: Duration::ZERO,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    output_cut_off: false,
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
        };
        let scoped_exe = Path::new("/b/dev/packages/fmtlib/fmt/unit_test");
        let outputs: BTreeSet<&Path> = [scoped_exe].into_iter().collect();
        assert_eq!(
            expected_executable(&make("fmtlib/fmt"), "unit_test", &outputs),
            Some(scoped_exe)
        );
        assert_eq!(
            expected_executable(&make("fmt"), "unit_test", &outputs),
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
            duration: Duration::ZERO,
            stdout: Vec::new(),
            stderr: Vec::new(),
            output_cut_off: false,
        }
    }

    #[test]
    fn junit_report_has_one_case_per_executable_grouped_by_package() {
        let mut passed = result_for("pass_test", TestRunStatus::Passed);
        passed.executable.package = "alpha".into();
        passed.duration = Duration::from_millis(1500);
        // A byte that is not UTF-8, then U+FFFE and U+FFFF.
        passed.stdout = b"<&> \xff \xef\xbf\xbe\xef\xbf\xbf".to_vec();
        passed.output_cut_off = true;
        let mut exited = result_for("exit_test", TestRunStatus::Failed(ExitStatusKind::Code(1)));
        exited.stderr = b"boom".to_vec();
        let summary = TestSummary {
            results: vec![
                passed,
                exited,
                result_for(
                    "signal_test",
                    TestRunStatus::Failed(ExitStatusKind::Signal("9".to_owned())),
                ),
                result_for("odd_test", TestRunStatus::Failed(ExitStatusKind::Unknown)),
                result_for("hang_test", TestRunStatus::TimedOut(Duration::from_secs(5))),
                result_for("gone_test", TestRunStatus::SpawnFailed("not found".into())),
            ],
            elapsed: Duration::from_secs(2),
        };

        let xml = junit_report(&summary).to_string().unwrap();
        let report = Report::deserialize_from_str(&xml).unwrap();

        let [alpha, demo] = &report.test_suites[..] else {
            panic!("expected two suites: {xml}");
        };
        assert_eq!((alpha.name.as_str(), alpha.tests), ("alpha", 1));
        assert_eq!(
            (demo.name.as_str(), demo.tests, demo.failures, demo.errors),
            ("demo", 5, 4, 1)
        );
        let pass = &alpha.test_cases[0];
        assert_eq!(pass.classname.as_ref().unwrap().as_str(), "alpha");
        assert_eq!(pass.time, Some(Duration::from_millis(1500)));
        assert_eq!(
            pass.system_out.as_ref().unwrap().as_str(),
            "<&> \u{fffd} \u{fffd}\u{fffd}"
        );
        let property = &pass.properties[0];
        assert_eq!(
            (property.name.as_str(), property.value.as_str()),
            ("output-cut-off", "true")
        );
        let exit = &demo.test_cases[0];
        assert_eq!(exit.system_err.as_ref().unwrap().as_str(), "boom");
        let outcomes: Vec<_> = demo
            .test_cases
            .iter()
            .map(|case| match &case.status {
                TestCaseStatus::NonSuccess { kind, message, .. } => {
                    (*kind, message.as_ref().unwrap().as_str())
                }
                status => panic!("unexpected {status:?}"),
            })
            .collect();
        assert_eq!(
            outcomes,
            [
                (NonSuccessKind::Failure, "exit 1"),
                (NonSuccessKind::Failure, "terminated by signal 9"),
                (NonSuccessKind::Failure, "unknown exit status"),
                (NonSuccessKind::Failure, "timed out after 5s"),
                (NonSuccessKind::Error, "could not start: not found"),
            ]
        );
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
                result_for("fail_test", TestRunStatus::Failed(ExitStatusKind::Code(1))),
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
        let summary = run_tests(&plan, &options(1, None), &mut sink).unwrap();
        assert_eq!(summary.total(), 2);
        assert_eq!(summary.passed(), 1);
        assert_eq!(summary.failed(), 1);
        assert!(!summary.all_passed());
        // execution order matches the plan's input order
        // (run_tests does not re-sort; that is plan_tests's job).
        assert_eq!(summary.results[0].executable.target, "fail_test");
        assert_eq!(
            summary.results[0].status,
            TestRunStatus::Failed(ExitStatusKind::Code(1))
        );
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
        let summary = run_tests(&plan, &options(1, None), &mut sink).unwrap();

        assert!(summary.all_passed(), "{summary:?}");
        assert_eq!(summary.results[0].stdout, b"ready\n");
    }

    fn options(jobs: usize, timeout: Option<Duration>) -> TestRunOptions {
        TestRunOptions {
            args: Vec::new(),
            jobs: NonZeroUsize::new(jobs).unwrap(),
            timeout,
            runner: Vec::new(),
        }
    }

    /// Records every sink call, in order, as `(target, call, bytes)`.
    #[derive(Default)]
    struct Recorder {
        calls: Vec<(String, &'static str, Vec<u8>)>,
    }

    impl Recorder {
        fn finished(&self) -> Vec<&str> {
            self.calls
                .iter()
                .filter(|(_, call, _)| *call == "finished")
                .map(|(target, ..)| target.as_str())
                .collect()
        }
    }

    impl TestOutputSink for Recorder {
        fn write_stdout(&mut self, e: &TestExecutable, b: &[u8]) -> io::Result<()> {
            self.calls.push((e.target.clone(), "stdout", b.to_vec()));
            Ok(())
        }
        fn write_stderr(&mut self, e: &TestExecutable, b: &[u8]) -> io::Result<()> {
            self.calls.push((e.target.clone(), "stderr", b.to_vec()));
            Ok(())
        }
        fn test_finished(&mut self, result: &TestRunResult) -> io::Result<()> {
            self.calls
                .push((result.executable.target.clone(), "finished", Vec::new()));
            Ok(())
        }
    }

    /// A `#!/bin/sh` test executable named `name`, run in `dir`.
    #[cfg(unix)]
    fn script(dir: &TempDir, name: &str, body: &str) -> TestExecutable {
        let file = dir.child(name);
        write_executable(&file, &format!("#!/bin/sh\n{body}"));
        TestExecutable {
            package: "demo".into(),
            target: name.into(),
            executable: file.to_path_buf(),
            working_dir: dir.path().to_path_buf(),
            env: BTreeMap::new(),
        }
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_runs_executables_concurrently() {
        let dir = TempDir::new().unwrap();
        // Each waits up to ten seconds for the other to start, so
        // both pass only when they run at the same time.
        let rendezvous = |me: &str, other: &str| {
            script(
                &dir,
                &format!("{me}_test"),
                &format!(
                    "touch {me}.started\ni=0\nwhile [ ! -f {other}.started ]; do\n  i=$((i + 1))\n  [ \"$i\" -gt 200 ] && exit 3\n  sleep 0.05\ndone\n"
                ),
            )
        };
        let plan = TestPlan {
            executables: vec![
                rendezvous("a", "b"),
                rendezvous("b", "a"),
                script(&dir, "fail_test", "exit 7\n"),
            ],
        };

        let summary = run_tests(&plan, &options(2, None), &mut Recorder::default()).unwrap();

        let statuses: Vec<(&str, &TestRunStatus)> = summary
            .results
            .iter()
            .map(|r| (r.executable.target.as_str(), &r.status))
            .collect();
        assert_eq!(
            statuses,
            [
                ("a_test", &TestRunStatus::Passed),
                ("b_test", &TestRunStatus::Passed),
                ("fail_test", &TestRunStatus::Failed(ExitStatusKind::Code(7))),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_kills_a_test_past_its_timeout() {
        let dir = TempDir::new().unwrap();
        let plan = TestPlan {
            executables: vec![script(&dir, "hang_test", "exec sleep 30\n")],
        };
        let limit = Duration::from_millis(200);
        let started = Instant::now();

        let summary = run_tests(&plan, &options(1, Some(limit)), &mut Recorder::default()).unwrap();

        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(summary.results[0].status, TestRunStatus::TimedOut(limit));
        assert!(summary.results[0].duration >= limit);
        assert!(!summary.results[0].output_cut_off);
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_credits_each_silent_exit_to_its_own_test() {
        let dir = TempDir::new().unwrap();
        // Silent tests have nothing but their exit to tell apart, and
        // distinct exit codes show an exit credited to the wrong test.
        let plan = TestPlan {
            executables: (0..12)
                .map(|code| script(&dir, &format!("t{code:02}_test"), &format!("exit {code}\n")))
                .collect(),
        };

        let summary = run_tests(&plan, &options(4, None), &mut Recorder::default()).unwrap();

        let statuses: Vec<TestRunStatus> = summary.results.into_iter().map(|r| r.status).collect();
        let expected: Vec<TestRunStatus> = (0..12)
            .map(|code| match code {
                0 => TestRunStatus::Passed,
                code => TestRunStatus::Failed(ExitStatusKind::Code(code)),
            })
            .collect();
        assert_eq!(statuses, expected);
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_keeps_the_status_of_a_test_that_exited_before_its_kill() {
        // Long enough for the test to start and print on a loaded host.
        const LIMIT: Duration = Duration::from_secs(2);
        // Holds the runner until the test has exited and its timeout has
        // passed, so the runner kills a test whose exit is still queued
        // behind its output.
        struct StallingSink(PathBuf);
        impl TestOutputSink for StallingSink {
            fn write_stdout(&mut self, _e: &TestExecutable, _b: &[u8]) -> io::Result<()> {
                std::fs::write(self.0.join("seen"), b"")?;
                let pid = std::fs::read_to_string(self.0.join("test.pid"))?;
                let started = Instant::now();
                while Command::new("kill")
                    .args(["-0", pid.trim()])
                    .stderr(Stdio::null())
                    .status()?
                    .success()
                {
                    assert!(
                        started.elapsed() < Duration::from_secs(10),
                        "test never exited"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                thread::sleep(LIMIT);
                Ok(())
            }
            fn write_stderr(&mut self, _e: &TestExecutable, _b: &[u8]) -> io::Result<()> {
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        let plan = TestPlan {
            executables: vec![script(
                &dir,
                "quick_test",
                "echo $$ > test.pid\necho hi\nwhile [ ! -f seen ]; do sleep 0.01; done\nexit 3\n",
            )],
        };
        let mut sink = StallingSink(dir.path().to_path_buf());

        let summary = run_tests(&plan, &options(1, Some(LIMIT)), &mut sink).unwrap();

        assert_eq!(
            summary.results[0].status,
            TestRunStatus::Failed(ExitStatusKind::Code(3))
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_stops_reading_output_a_leftover_process_holds() {
        let dir = TempDir::new().unwrap();
        // The background `sleep` inherits the test's stdout and
        // stderr and outlives it.
        let plan = TestPlan {
            executables: vec![script(&dir, "leak_test", "echo before\nsleep 30 &\n")],
        };
        let started = Instant::now();

        let summary = run_tests(&plan, &options(1, None), &mut Recorder::default()).unwrap();

        assert!(started.elapsed() < Duration::from_secs(10));
        let result = &summary.results[0];
        assert_eq!(result.status, TestRunStatus::Passed);
        assert!(result.output_cut_off);
        assert_eq!(result.stdout, b"before\n");
        assert_eq!(
            render_result_line(result),
            "test demo:leak_test ... ok (output cut off)"
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_starts_the_output_grace_when_it_sees_the_exit() {
        // Holds the runner past the test's exit and its grace second.
        // The test waits for `seen`, so it exits only once the runner
        // is stalled here.
        struct StallingSink(PathBuf);
        impl TestOutputSink for StallingSink {
            fn write_stdout(&mut self, _e: &TestExecutable, b: &[u8]) -> io::Result<()> {
                if b.starts_with(b"before") {
                    std::fs::write(self.0.join("seen"), b"")?;
                    std::thread::sleep(Duration::from_secs(2));
                }
                Ok(())
            }
            fn write_stderr(&mut self, _e: &TestExecutable, _b: &[u8]) -> io::Result<()> {
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        // `after` is written within a second of the exit, by a leftover
        // that keeps stdout open.
        let plan = TestPlan {
            executables: vec![script(
                &dir,
                "late_test",
                "echo before\nwhile [ ! -f seen ]; do sleep 0.01; done\n\
                 (sleep 0.3; echo after; sleep 30) &\n",
            )],
        };
        let mut sink = StallingSink(dir.path().to_path_buf());

        let summary = run_tests(&plan, &options(1, None), &mut sink).unwrap();

        let result = &summary.results[0];
        assert!(result.output_cut_off);
        assert_eq!(result.stdout, b"before\nafter\n");
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_handles_all_output_a_slow_sink_still_holds() {
        struct SlowSink(Vec<u8>);
        impl TestOutputSink for SlowSink {
            fn write_stdout(&mut self, _e: &TestExecutable, b: &[u8]) -> io::Result<()> {
                std::thread::sleep(Duration::from_millis(400));
                self.0.extend_from_slice(b);
                Ok(())
            }
            fn write_stderr(&mut self, _e: &TestExecutable, _b: &[u8]) -> io::Result<()> {
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        // Five separate writes, all made long before the sink has
        // taken them, and a leftover holding only stderr: stdout
        // closes while most writes still wait, and must not be cut.
        let plan = TestPlan {
            executables: vec![script(
                &dir,
                "chatty_test",
                "sleep 30 >/dev/null &\nfor i in 1 2 3 4 5; do echo \"$i\"; sleep 0.02; done\n",
            )],
        };
        let mut sink = SlowSink(Vec::new());

        let summary = run_tests(&plan, &options(1, None), &mut sink).unwrap();

        assert!(summary.results[0].output_cut_off);
        assert_eq!(summary.results[0].stdout, b"1\n2\n3\n4\n5\n");
        assert_eq!(sink.0, b"1\n2\n3\n4\n5\n");
    }

    #[test]
    fn run_tests_reports_spawn_failures_and_keeps_going() {
        let dir = TempDir::new().unwrap();
        let missing = |name: &str| TestExecutable {
            package: "demo".into(),
            target: name.into(),
            executable: dir.path().join(name),
            working_dir: dir.path().to_path_buf(),
            env: BTreeMap::new(),
        };
        let plan = TestPlan {
            executables: vec![missing("a_test"), missing("b_test")],
        };
        let mut sink = Recorder::default();

        let summary = run_tests(&plan, &options(1, None), &mut sink).unwrap();

        for result in &summary.results {
            assert!(
                matches!(result.status, TestRunStatus::SpawnFailed(_)),
                "{result:?}"
            );
        }
        assert_eq!(summary.failed(), 2);
        assert_eq!(sink.finished(), ["a_test", "b_test"]);
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_hands_each_executable_to_the_runner() {
        let dir = TempDir::new().unwrap();
        let runner = dir.child("runner");
        write_executable(&runner, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n");
        // Not executable: only the runner can start it.
        let test = dir.child("cross_test");
        test.write_str("").unwrap();
        let plan = TestPlan {
            executables: vec![TestExecutable {
                package: "demo".into(),
                target: "cross_test".into(),
                executable: test.to_path_buf(),
                working_dir: dir.path().to_path_buf(),
                env: BTreeMap::new(),
            }],
        };
        let options = TestRunOptions {
            args: vec!["--filter".into()],
            runner: vec![runner.to_str().unwrap().to_owned(), "-L".to_owned()],
            ..options(1, None)
        };

        let summary = run_tests(&plan, &options, &mut Recorder::default()).unwrap();

        assert!(summary.all_passed(), "{summary:?}");
        assert_eq!(
            String::from_utf8_lossy(&summary.results[0].stdout),
            format!("-L\n{}\n--filter\n", test.display())
        );
    }

    #[test]
    fn run_tests_names_a_runner_that_cannot_start() {
        let dir = TempDir::new().unwrap();
        let plan = TestPlan {
            executables: vec![TestExecutable {
                package: "demo".into(),
                target: "cross_test".into(),
                executable: dir.path().join("cross_test"),
                working_dir: dir.path().to_path_buf(),
                env: BTreeMap::new(),
            }],
        };
        let missing = dir.path().join("no-such-runner");
        let options = TestRunOptions {
            runner: vec![missing.to_str().unwrap().to_owned()],
            ..options(1, None)
        };

        let summary = run_tests(&plan, &options, &mut Recorder::default()).unwrap();

        let TestRunStatus::SpawnFailed(error) = &summary.results[0].status else {
            panic!("{summary:?}");
        };
        assert!(
            error.contains(&format!("runner `{}`", missing.display())),
            "{error}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_hands_over_concurrent_output_whole() {
        const MIB: usize = 1 << 20;
        let dir = TempDir::new().unwrap();
        // A mebibyte per stream is far past a pipe's buffer, so a
        // runner that stopped draining would deadlock; `\377` is not
        // UTF-8.
        let body = |byte: char| {
            format!(
                "head -c {MIB} /dev/zero | tr '\\0' '{byte}'\nprintf '\\377'\nhead -c {MIB} /dev/zero | tr '\\0' '{byte}' >&2\n"
            )
        };
        let plan = TestPlan {
            executables: vec![
                script(&dir, "a_test", &body('a')),
                script(&dir, "b_test", &body('b')),
            ],
        };
        let mut sink = Recorder::default();

        let summary = run_tests(&plan, &options(2, None), &mut sink).unwrap();

        for (result, byte) in summary.results.iter().zip(*b"ab") {
            let mut stdout = vec![byte; MIB];
            stdout.push(0xff);
            assert!(
                result.stdout == stdout,
                "{} stdout differs",
                result.executable.target
            );
            assert!(
                result.stderr == vec![byte; MIB],
                "{} stderr differs",
                result.executable.target
            );
        }
        // Each test's output reaches the sink in one piece per
        // stream, right before its result.
        let mut calls: Vec<(&str, &str, usize)> = sink
            .calls
            .iter()
            .map(|(target, call, bytes)| (target.as_str(), *call, bytes.len()))
            .collect();
        calls.sort_by_key(|&(target, ..)| target);
        assert_eq!(
            calls,
            [
                ("a_test", "stdout", MIB + 1),
                ("a_test", "stderr", MIB),
                ("a_test", "finished", 0),
                ("b_test", "stdout", MIB + 1),
                ("b_test", "stderr", MIB),
                ("b_test", "finished", 0),
            ]
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_tests_kills_running_tests_when_the_sink_fails() {
        struct FailingSink;
        impl TestOutputSink for FailingSink {
            fn write_stdout(&mut self, _e: &TestExecutable, _b: &[u8]) -> io::Result<()> {
                Err(io::Error::other("sink closed"))
            }
            fn write_stderr(&mut self, _e: &TestExecutable, _b: &[u8]) -> io::Result<()> {
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        let plan = TestPlan {
            executables: vec![
                script(&dir, "hang_test", "echo $$ > hang.pid\nexec sleep 30\n"),
                script(
                    &dir,
                    "quick_test",
                    "while [ ! -s hang.pid ]; do sleep 0.01; done\necho done\n",
                ),
            ],
        };
        let started = Instant::now();

        let err = run_tests(&plan, &options(2, None), &mut FailingSink).unwrap_err();

        assert!(matches!(err, TestRunError::SinkIo(_)), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
        let pid = std::fs::read_to_string(dir.child("hang.pid").path()).unwrap();
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the still-running test must be killed");
    }

    #[test]
    fn render_result_line_describes_each_outcome() {
        for (status, label) in [
            (TestRunStatus::Passed, "ok"),
            (
                TestRunStatus::Failed(ExitStatusKind::Code(42)),
                "FAILED (exit 42)",
            ),
            (
                TestRunStatus::Failed(ExitStatusKind::Signal("9".to_owned())),
                "FAILED (terminated by signal 9)",
            ),
            (
                TestRunStatus::TimedOut(Duration::from_secs(30)),
                "FAILED (timed out after 30s)",
            ),
            (
                TestRunStatus::SpawnFailed("permission denied".to_owned()),
                "FAILED (could not start: permission denied)",
            ),
        ] {
            assert_eq!(
                render_result_line(&result_for("fail_test", status)),
                format!("test demo:fail_test ... {label}")
            );
        }
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
            duration: Duration::ZERO,
            stdout: Vec::new(),
            stderr: Vec::new(),
            output_cut_off: false,
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
