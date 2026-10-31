//! Opt-in performance trace capture (`CABIN_TRACE_DIR`).
//!
//! Domain crates emit `tracing` spans and never install a subscriber;
//! this module is the only place one is built.  With
//! [`cabin_env::CABIN_TRACE_DIR`] unset nothing here runs: no
//! subscriber, no writer thread, no files, and every span macro in the
//! workspace stays a disabled no-op.
//!
//! When set, one invocation writes two files into that directory under
//! a stem unique to the process (`<command>-<unix-ms>-<pid>-<seq>`):
//!
//! - `<stem>.trace.json`: a Chrome trace-event file (Perfetto,
//!   `chrome://tracing`).  Span names, fields, and nesting are
//!   developer detail and change with the code.  It is written as
//!   `<stem>.trace.json.partial` and renamed only once complete, so an
//!   interrupted or failed capture never looks finished.
//! - `<stem>.timings.json`: a versioned summary (see [`Timings`]) whose
//!   field set and units are stable within a `format_version`.
//!
//! Child processes that run concurrently cannot be spans on Cabin's
//! one thread, so the trace shows them as lanes of their own: Ninja's
//! actions ([`NinjaLog`]) and the test executables ([`TestLane`]).
//!
//! The subscriber is scoped to the invocation's thread with
//! [`tracing::dispatcher::with_default`], never installed globally, so
//! repeated or concurrent in-process calls to [`crate::run`] each get
//! their own capture.  A thread spawned inside the command records
//! spans only if it is handed the dispatcher.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read as _, Seek as _, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr as _;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tracing::field::{Field, Visit};
use tracing::span;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;

use crate::cli::term_verbosity::Reporter;

/// The `*.timings.json` schema version.  Bump it when a field changes
/// meaning or is removed; adding a field keeps the version.
const TIMINGS_FORMAT_VERSION: u32 = 1;

/// The span levels recorded when `CABIN_TRACE_FILTER` is unset: the
/// command's phases, without per-package and per-request detail.
const DEFAULT_FILTER: &str = "info";

/// Distinguishes captures of repeated in-process invocations that land
/// in the same millisecond.
static SEQUENCE: AtomicU32 = AtomicU32::new(0);

type Phases = Arc<Mutex<BTreeMap<(&'static str, &'static str), Phase>>>;
type WriteError = Arc<Mutex<Option<io::Error>>>;
type SharedTimeline = Arc<Mutex<Timeline>>;

/// The trace processes Cabin adds beside `tracing-chrome`'s pid 1, as
/// `(pid, name)`.
const NINJA_LANE: (u32, &str) = (2, "ninja");
const TESTS_LANE: (u32, &str) = (3, "tests");

/// Ninja's log of finished actions, in the directory it builds in.
const NINJA_LOG: &str = ".ninja_log";

/// One invocation's capture: the scoped subscriber plus the files it
/// writes.
pub(crate) struct TraceSession {
    started: Instant,
    command: String,
    partial_path: PathBuf,
    /// A second handle on the partial file, for appending after the
    /// writer closes its own: reopening by path could follow an entry
    /// swapped in a shared directory.
    partial: File,
    trace_path: PathBuf,
    timings_path: PathBuf,
    dispatch: tracing::Dispatch,
    guard: tracing_chrome::FlushGuard,
    write_error: WriteError,
    phases: Phases,
    timeline: SharedTimeline,
}

impl TraceSession {
    /// Start a capture when `CABIN_TRACE_DIR` asks for one.  `started`
    /// is when [`crate::run`] began, so the summary covers argument
    /// parsing that precedes the subscriber.
    ///
    /// # Errors
    /// Returns a message when `CABIN_TRACE_FILTER` does not parse or
    /// the trace file cannot be created; the command must not run with
    /// a capture the user asked for silently missing.
    pub(crate) fn from_env(command: &str, started: Instant) -> Result<Option<Self>, String> {
        let dir = match std::env::var_os(cabin_env::CABIN_TRACE_DIR) {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => return Ok(None),
        };
        let filter = std::env::var(cabin_env::CABIN_TRACE_FILTER)
            .ok()
            .filter(|value| !value.trim().is_empty());
        Self::start(&dir, filter.as_deref(), command, started).map(Some)
    }

    fn start(
        dir: &Path,
        filter: Option<&str>,
        command: &str,
        started: Instant,
    ) -> Result<Self, String> {
        let filter = filter.unwrap_or(DEFAULT_FILTER);
        let targets = Targets::from_str(filter).map_err(|err| {
            format!(
                "invalid {} value `{filter}`: {err}",
                cabin_env::CABIN_TRACE_FILTER
            )
        })?;

        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        let create_error = |path: &Path, err: io::Error| {
            format!(
                "cannot create trace file {} ({} is set): {err}",
                path.display(),
                cabin_env::CABIN_TRACE_DIR
            )
        };
        std::fs::create_dir_all(dir).map_err(|err| create_error(dir, err))?;
        let (stem, partial_path, file) = loop {
            let stem = format!(
                "{command}-{unix_ms}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            );
            let partial_path = dir.join(format!("{stem}.trace.json.partial"));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&partial_path)
            {
                // Another PID namespace sharing the directory took this
                // name in the same millisecond.
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                result => {
                    let file = result.map_err(|err| create_error(&partial_path, err))?;
                    break (stem, partial_path, file);
                }
            }
        };
        let trace_path = dir.join(format!("{stem}.trace.json"));
        let timings_path = dir.join(format!("{stem}.timings.json"));

        let partial = file
            .try_clone()
            .map_err(|err| create_error(&partial_path, err))?;
        let write_error = WriteError::default();
        let (chrome, guard) = tracing_chrome::ChromeLayerBuilder::new()
            .writer(CheckedWriter {
                file,
                error: Arc::clone(&write_error),
            })
            .include_args(true)
            // A `cargo install` build's callsite paths are absolute
            // and name the user's home directory.
            .include_locations(false)
            .build();
        // `tracing-chrome` timestamps count from when its layer was built.
        let timeline = Arc::new(Mutex::new(Timeline {
            origin: Instant::now(),
            ninja_actions: None,
            intervals: Vec::new(),
        }));
        let phases = Phases::default();
        let subscriber = tracing_subscriber::registry()
            .with(targets)
            .with(chrome)
            .with(PhaseLayer {
                phases: Arc::clone(&phases),
                timeline: Arc::clone(&timeline),
            });
        Ok(Self {
            started,
            command: command.to_owned(),
            partial_path,
            partial,
            trace_path,
            timings_path,
            dispatch: tracing::Dispatch::new(subscriber),
            guard,
            write_error,
            phases,
            timeline,
        })
    }

    /// Run `command` under the capture, then finish both files.  The
    /// command's exit code is returned unchanged: a capture failure is
    /// reported as a warning, never as the command's failure.
    pub(crate) fn run(self, reporter: Reporter, command: impl FnOnce() -> ExitCode) -> ExitCode {
        let startup = self.started.elapsed();
        let code = tracing::dispatcher::with_default(&self.dispatch, || {
            let _root = tracing::info_span!(
                target: "cabin",
                "cabin",
                command = %self.command,
                startup_us = duration_us(startup),
            )
            .entered();
            command()
        });
        let wall = self.started.elapsed();
        self.finish(code == ExitCode::SUCCESS, startup, wall, reporter);
        code
    }

    fn finish(self, success: bool, startup: Duration, wall: Duration, reporter: Reporter) {
        let Self {
            started: _,
            command,
            partial_path,
            partial,
            trace_path,
            timings_path,
            dispatch,
            guard,
            write_error,
            phases,
            timeline,
        } = self;
        // Dropping the guard closes the JSON array and joins the
        // writer thread, so every event is on disk before the rename.
        drop(dispatch);
        drop(guard);
        let mut failure = write_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let mut timeline = timeline.lock().unwrap_or_else(PoisonError::into_inner);
        if failure.is_none() {
            let mut events = vec![serde_json::json!({
                "ph": "M",
                "pid": 1,
                "name": "process_name",
                "args": { "name": format!("cabin {command}") },
            })];
            events.extend(complete_events(std::mem::take(&mut timeline.intervals)));
            failure = append_events(partial, &events)
                .and_then(|()| std::fs::rename(&partial_path, &trace_path))
                .err();
        }
        let trace_complete = failure.is_none();

        let phases = std::mem::take(&mut *phases.lock().unwrap_or_else(PoisonError::into_inner));
        let timings = Timings {
            format_version: TIMINGS_FORMAT_VERSION,
            cabin_version: env!("CARGO_PKG_VERSION"),
            command: &command,
            outcome: if success { "success" } else { "failure" },
            trace_complete,
            wall_time_us: duration_us(wall),
            startup_us: duration_us(startup),
            ninja_actions: timeline.ninja_actions,
            phases: phases
                .into_iter()
                .map(|((target, name), phase)| PhaseReport {
                    target,
                    name,
                    count: phase.count,
                    total_us: duration_us(phase.total),
                    max_us: duration_us(phase.max),
                    fields: phase.fields,
                })
                .collect(),
            process: process_usage(),
        };
        let timings_written = serde_json::to_vec_pretty(&timings)
            .map_err(io::Error::other)
            .and_then(|mut body| {
                body.push(b'\n');
                // Rename for atomicity only: a diagnostic needs no fsync,
                // which costs milliseconds per file on some platforms.
                // `create_new` refuses a planted symlink in a shared dir.
                let partial = timings_path.with_extension("json.partial");
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&partial)?
                    .write_all(&body)?;
                std::fs::rename(&partial, &timings_path)
            });

        match failure {
            None => {
                if reporter.verbosity().shows_status() {
                    // Like the reporter, never fail a command over stderr.
                    let _ = writeln!(io::stderr(), "cabin: wrote trace {}", trace_path.display());
                }
            }
            Some(err) => reporter.warning(format_args!(
                "trace capture failed, {} is incomplete: {err}",
                partial_path.display()
            )),
        }
        if let Err(err) = timings_written {
            reporter.warning(format_args!(
                "cannot write trace timings {}: {err}",
                timings_path.display()
            ));
        }
    }
}

/// The trace file writer.  `tracing-chrome` unwraps every write on its
/// writer thread, so a full disk would panic onto the user's stderr;
/// instead the first error is kept for [`TraceSession::finish`] and
/// later writes are dropped.
struct CheckedWriter {
    file: File,
    error: WriteError,
}

impl CheckedWriter {
    fn attempt(&mut self, op: impl FnOnce(&mut File) -> io::Result<()>) {
        let mut error = self.error.lock().unwrap_or_else(PoisonError::into_inner);
        if error.is_none()
            && let Err(err) = op(&mut self.file)
        {
            *error = Some(err);
        }
    }
}

impl Write for CheckedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.attempt(|file| file.write_all(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.attempt(Write::flush);
        Ok(())
    }
}

/// Append trace events to a finished `tracing-chrome` file, which ends
/// its JSON array with `\n]` (`[\n\n]` when it holds no event).
fn append_events(mut file: File, events: &[serde_json::Value]) -> io::Result<()> {
    const EMPTY: u64 = b"[\n\n]".len() as u64;
    let len = file.seek(SeekFrom::End(0))?;
    let mut tail = [0_u8; 2];
    file.seek(SeekFrom::Start(len.saturating_sub(2)))?;
    file.read_exact(&mut tail)?;
    if len < EMPTY || &tail != b"\n]" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "trace file does not end its event array",
        ));
    }
    file.seek(SeekFrom::Start(len - 2))?;
    let mut out = BufWriter::new(file);
    let mut needs_comma = len > EMPTY;
    for event in events {
        if needs_comma {
            out.write_all(b",\n")?;
        }
        serde_json::to_writer(&mut out, event)?;
        needs_comma = true;
    }
    out.write_all(b"\n]")?;
    out.flush()
}

/// Child-process runs recorded beside the spans, and the Ninja action
/// count for the summary.
struct Timeline {
    /// The trace clock's zero.
    origin: Instant,
    ninja_actions: Option<usize>,
    intervals: Vec<Interval>,
}

/// One child-process run, in microseconds on the trace clock.
struct Interval {
    lane: (u32, &'static str),
    name: String,
    start_us: u64,
    dur_us: u64,
}

/// The capture running on this thread, found through its dispatcher so
/// it follows the same scoping as the spans.
fn current_timeline() -> Option<SharedTimeline> {
    tracing::dispatcher::get_default(|dispatch| {
        dispatch
            .downcast_ref::<PhaseLayer>()
            .map(|layer| Arc::clone(&layer.timeline))
    })
}

/// `.ninja_log` before a Ninja run.  The log keeps earlier builds'
/// entries, so the run's own are those appended after this snapshot.
pub(crate) struct NinjaLog {
    timeline: SharedTimeline,
    build_root: PathBuf,
    before: Vec<u8>,
}

impl NinjaLog {
    /// Snapshot `<build_root>/.ninja_log` when a capture is running.
    pub(crate) fn snapshot(build_root: &Path) -> Option<Self> {
        let timeline = current_timeline()?;
        let before = read_ninja_log(build_root)?;
        Some(Self {
            timeline,
            build_root: build_root.to_owned(),
            before,
        })
    }

    /// Add the actions Ninja logged since [`Self::snapshot`] to the
    /// ninja lane.  Ninja times them from its own start, which
    /// `spawned` (when Cabin started Ninja) places on the trace clock.
    pub(crate) fn record(self, spawned: Instant) {
        let Some(after) = read_ninja_log(&self.build_root) else {
            return;
        };
        let Some(actions) = appended_actions(&self.before, &after) else {
            return;
        };
        let mut timeline = self.timeline.lock().unwrap_or_else(PoisonError::into_inner);
        let spawned_us = duration_us(spawned.saturating_duration_since(timeline.origin));
        timeline.ninja_actions = Some(actions.len());
        let build_root = lexically_normal(&self.build_root);
        for (start_ms, end_ms, output, _) in actions {
            // Ninja logs the absolute paths Cabin wrote; relative to
            // the build root they cannot name the user's home.
            let output = Path::new(output);
            let name = output
                .strip_prefix(&build_root)
                .unwrap_or_else(|_| Path::new(output.file_name().unwrap_or_default()));
            timeline.intervals.push(Interval {
                lane: NINJA_LANE,
                name: name.to_string_lossy().into_owned(),
                start_us: spawned_us + u64::from(start_ms) * 1000,
                dur_us: u64::from(end_ms.saturating_sub(start_ms)) * 1000,
            });
        }
    }
}

/// `<build_root>/.ninja_log`, empty until Ninja first logs a finished
/// action.
fn read_ninja_log(build_root: &Path) -> Option<Vec<u8>> {
    match std::fs::read(build_root.join(NINJA_LOG)) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
    }
}

/// `path` with each `dir/..` collapsed, as Ninja collapses the paths
/// it logs: `--build-dir ../build` keeps its `..` in the build root.
fn lexically_normal(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir
                if matches!(normal.components().next_back(), Some(Component::Normal(_))) =>
            {
                normal.pop();
            }
            component => normal.push(component),
        }
    }
    normal
}

/// The actions in `after` past its prefix `before`, as `(start_ms,
/// end_ms, output, command_hash)`.  `None` when `after` does not start
/// with `before`: Ninja rewrote the log (it recompacts it at startup),
/// so this run's entries cannot be told from earlier ones.
fn appended_actions<'a>(
    before: &[u8],
    after: &'a [u8],
) -> Option<Vec<(u32, u32, &'a str, &'a str)>> {
    let appended = std::str::from_utf8(after.strip_prefix(before)?).ok()?;
    // `# ninja log vN`, then `start \t end \t mtime \t output \t hash`.
    let mut actions: Vec<_> = appended
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let [start, end, _, output, hash] = line.split('\t').collect::<Vec<_>>()[..] else {
                return None;
            };
            Some((start.parse().ok()?, end.parse().ok()?, output, hash))
        })
        .collect();
    // A command with several outputs logs a line per output, each with
    // the command's start, end, and hash.
    actions.dedup_by_key(|&mut (start, end, _, hash)| (start, end, hash));
    Some(actions)
}

/// A [`cabin_test::TestOutputSink`] that also adds each finished test
/// executable to the tests lane while a capture runs.
pub(crate) struct TestLane<S>(pub(crate) S);

impl<S: cabin_test::TestOutputSink> cabin_test::TestOutputSink for TestLane<S> {
    fn write_stdout(
        &mut self,
        executable: &cabin_test::TestExecutable,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.0.write_stdout(executable, bytes)
    }

    fn write_stderr(
        &mut self,
        executable: &cabin_test::TestExecutable,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.0.write_stderr(executable, bytes)
    }

    fn test_finished(&mut self, result: &cabin_test::TestRunResult) -> io::Result<()> {
        if let Some(timeline) = current_timeline() {
            let mut timeline = timeline.lock().unwrap_or_else(PoisonError::into_inner);
            // The runner reports a test once it has ended.
            let end_us = duration_us(timeline.origin.elapsed());
            let dur_us = duration_us(result.duration);
            timeline.intervals.push(Interval {
                lane: TESTS_LANE,
                name: format!("{}:{}", result.executable.package, result.executable.target),
                start_us: end_us.saturating_sub(dur_us),
                dur_us,
            });
        }
        self.0.test_finished(result)
    }
}

/// Chrome complete events for `intervals`, each lane's after its
/// `process_name`.  An interval takes the lowest thread id free at its
/// start, so no two on one thread overlap.
fn complete_events(mut intervals: Vec<Interval>) -> Vec<serde_json::Value> {
    intervals.sort_by_key(|interval| (interval.lane.0, interval.start_us));
    let mut events = Vec::new();
    let mut pid = None;
    let mut busy_until: Vec<u64> = Vec::new();
    for interval in intervals {
        if pid != Some(interval.lane.0) {
            pid = Some(interval.lane.0);
            busy_until.clear();
            events.push(serde_json::json!({
                "ph": "M",
                "pid": interval.lane.0,
                "name": "process_name",
                "args": { "name": interval.lane.1 },
            }));
        }
        let tid = busy_until
            .iter()
            .position(|&busy| busy <= interval.start_us)
            .unwrap_or_else(|| {
                busy_until.push(0);
                busy_until.len() - 1
            });
        busy_until[tid] = interval.start_us + interval.dur_us;
        events.push(serde_json::json!({
            "ph": "X",
            "pid": interval.lane.0,
            "tid": tid,
            "ts": interval.start_us,
            "dur": interval.dur_us,
            "name": interval.name,
        }));
    }
    events
}

/// The `*.timings.json` document.
///
/// Stable within a `format_version`: every field's name, unit, and
/// meaning below.  Not stable: which phases appear and their
/// `target` / `name` / `fields` keys, which follow the code's spans.
#[derive(Serialize)]
struct Timings<'a> {
    format_version: u32,
    cabin_version: &'static str,
    /// The subcommand name, or `cabin` when none was given.
    command: &'a str,
    /// `success` when the command exited 0, else `failure`.
    outcome: &'static str,
    /// Whether `<stem>.trace.json` was written completely.
    trace_complete: bool,
    /// From [`crate::run`] entry to the end of the command.  Process
    /// creation and loading before `main` are not included.
    wall_time_us: u64,
    /// From [`crate::run`] entry to the start of the capture (argument
    /// parsing and terminal setup), which no span covers.
    startup_us: u64,
    /// The actions Ninja ran in this invocation, from its `.ninja_log`;
    /// `null` when Cabin ran no Ninja or Ninja rewrote its log, so its
    /// actions are unavailable.
    ninja_actions: Option<usize>,
    /// Every recorded span, aggregated per (target, name).
    phases: Vec<PhaseReport<'a>>,
    /// Resource usage at exit; `null` where unsupported (Windows).
    process: Option<ProcessUsage>,
}

/// One span name's aggregate.  Times are each occurrence's inclusive
/// lifetime from creation to close, so they include child spans and
/// waits on I/O or child processes; `total_us` sums occurrences and
/// exceeds elapsed time when occurrences overlap.
#[derive(Serialize)]
struct PhaseReport<'a> {
    target: &'a str,
    name: &'a str,
    count: u64,
    total_us: u64,
    max_us: u64,
    /// Sum, over occurrences, of each non-negative integer field.
    fields: BTreeMap<&'a str, u64>,
}

#[derive(Default)]
struct Phase {
    count: u64,
    total: Duration,
    max: Duration,
    fields: BTreeMap<&'static str, u64>,
}

/// `getrusage` at exit.  `self_*` is the Cabin process alone.
/// `children_*` covers every child process Cabin waited for (and their
/// waited descendants): CPU times are summed, while the peak is the
/// single largest child's high-water mark - children's peaks are not
/// simultaneous, so they are never added up.
#[derive(Serialize)]
struct ProcessUsage {
    self_peak_rss_bytes: u64,
    self_user_us: u64,
    self_system_us: u64,
    children_largest_peak_rss_bytes: u64,
    children_user_us: u64,
    children_system_us: u64,
}

#[cfg(unix)]
fn process_usage() -> Option<ProcessUsage> {
    fn rusage(who: libc::c_int) -> Option<libc::rusage> {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: `getrusage` only writes the struct it is handed.
        let rc = unsafe { libc::getrusage(who, usage.as_mut_ptr()) };
        // SAFETY: a zero return means the kernel filled the struct.
        (rc == 0).then(|| unsafe { usage.assume_init() })
    }
    fn micros(time: libc::timeval) -> u64 {
        u64::try_from(time.tv_sec).unwrap_or(0) * 1_000_000
            + u64::try_from(time.tv_usec).unwrap_or(0)
    }
    // `ru_maxrss` is bytes on Apple platforms and KiB elsewhere.
    let rss_unit: u64 = if cfg!(target_vendor = "apple") {
        1
    } else {
        1024
    };
    let own = rusage(libc::RUSAGE_SELF)?;
    let children = rusage(libc::RUSAGE_CHILDREN)?;
    Some(ProcessUsage {
        self_peak_rss_bytes: u64::try_from(own.ru_maxrss).unwrap_or(0) * rss_unit,
        self_user_us: micros(own.ru_utime),
        self_system_us: micros(own.ru_stime),
        children_largest_peak_rss_bytes: u64::try_from(children.ru_maxrss).unwrap_or(0) * rss_unit,
        children_user_us: micros(children.ru_utime),
        children_system_us: micros(children.ru_stime),
    })
}

#[cfg(not(unix))]
fn process_usage() -> Option<ProcessUsage> {
    None
}

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// Aggregates span lifetimes per (target, name) for the summary, and
/// holds the capture's [`Timeline`] where [`current_timeline`] finds it.
struct PhaseLayer {
    phases: Phases,
    timeline: SharedTimeline,
}

/// A span's open time and its integer fields so far.
struct SpanRecord {
    opened: Instant,
    fields: Vec<(&'static str, u64)>,
}

struct IntegerFields<'a>(&'a mut Vec<(&'static str, u64)>);

impl IntegerFields<'_> {
    fn set(&mut self, field: &Field, value: u64) {
        match self.0.iter_mut().find(|(name, _)| *name == field.name()) {
            Some(slot) => slot.1 = value,
            None => self.0.push((field.name(), value)),
        }
    }
}

impl Visit for IntegerFields<'_> {
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.set(field, value);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if let Ok(value) = u64::try_from(value) {
            self.set(field, value);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl<S> tracing_subscriber::Layer<S> for PhaseLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut fields = Vec::new();
        attrs.record(&mut IntegerFields(&mut fields));
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanRecord {
                opened: Instant::now(),
                fields,
            });
        }
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(record) = span.extensions_mut().get_mut::<SpanRecord>()
        {
            values.record(&mut IntegerFields(&mut record.fields));
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let extensions = span.extensions();
        let Some(record) = extensions.get::<SpanRecord>() else {
            return;
        };
        let elapsed = record.opened.elapsed();
        let metadata = span.metadata();
        let mut phases = self.phases.lock().unwrap_or_else(PoisonError::into_inner);
        let phase = phases
            .entry((metadata.target(), metadata.name()))
            .or_default();
        phase.count += 1;
        phase.total += elapsed;
        phase.max = phase.max.max(elapsed);
        for &(name, value) in &record.fields {
            *phase.fields.entry(name).or_default() += value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace_file(body: &str) -> (assert_fs::TempDir, PathBuf) {
        let dir = assert_fs::TempDir::new().unwrap();
        let path = dir.path().join("t.json");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    fn open(path: &Path) -> File {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap()
    }

    fn appended(body: &str) -> serde_json::Value {
        let (_dir, path) = trace_file(body);
        append_events(
            open(&path),
            &[serde_json::json!({"n": 1}), serde_json::json!({"n": 2})],
        )
        .unwrap();
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
    }

    #[test]
    fn appending_keeps_the_event_array_valid() {
        assert_eq!(appended("[\n\n]"), serde_json::json!([{"n": 1}, {"n": 2}]));
        assert_eq!(
            appended("[\n{\"n\":0}\n]"),
            serde_json::json!([{"n": 0}, {"n": 1}, {"n": 2}])
        );
    }

    fn quiet() -> Reporter {
        Reporter::with_color(cabin_core::Verbosity::Quiet, cabin_core::ColorChoice::Never)
    }

    fn files(dir: &Path, suffix: &str) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.to_string_lossy().ends_with(suffix))
            .collect();
        found.sort();
        found
    }

    fn timings(path: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn repeated_sessions_in_one_thread_each_capture_their_own_command() {
        let dir = assert_fs::TempDir::new().unwrap();
        for (command, code) in [("first", ExitCode::SUCCESS), ("second", ExitCode::FAILURE)] {
            let session = TraceSession::start(dir.path(), None, command, Instant::now()).unwrap();
            let returned = session.run(quiet(), || {
                let _phase = tracing::info_span!("phase", items = 3_u64).entered();
                code
            });
            assert_eq!(returned, code, "the command's exit code passes through");
        }
        let traces = files(dir.path(), ".trace.json");
        assert_eq!(traces.len(), 2);
        for trace in &traces {
            let events: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(trace).unwrap()).unwrap();
            let opened = events.iter().filter(|e| e["ph"] == "B").count();
            assert_eq!(opened, 2, "root and phase span, each exactly once");
        }
        let summaries: Vec<_> = files(dir.path(), ".timings.json")
            .iter()
            .map(|path| timings(path))
            .collect();
        assert_eq!(summaries[0]["command"], "first");
        assert_eq!(summaries[0]["outcome"], "success");
        assert_eq!(summaries[1]["command"], "second");
        assert_eq!(summaries[1]["outcome"], "failure");
        let phase = summaries[0]["phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "phase")
            .unwrap();
        assert_eq!(phase["count"], 1);
        assert_eq!(phase["fields"]["items"], 3);
    }

    #[test]
    fn a_write_failure_leaves_a_partial_trace_and_says_so() {
        let dir = assert_fs::TempDir::new().unwrap();
        let session = TraceSession::start(dir.path(), None, "build", Instant::now()).unwrap();
        *session.write_error.lock().unwrap() = Some(io::Error::other("disk full"));
        let code = session.run(quiet(), || ExitCode::SUCCESS);
        assert_eq!(
            code,
            ExitCode::SUCCESS,
            "a capture failure never fails the command"
        );
        assert!(files(dir.path(), ".trace.json").is_empty());
        assert_eq!(files(dir.path(), ".trace.json.partial").len(), 1);
        let summary = timings(&files(dir.path(), ".timings.json")[0]);
        assert_eq!(summary["trace_complete"], false);
    }

    #[test]
    fn checked_writer_keeps_the_first_error_and_swallows_the_rest() {
        let dir = assert_fs::TempDir::new().unwrap();
        let path = dir.path().join("read-only");
        std::fs::write(&path, b"").unwrap();
        let error = WriteError::default();
        let mut writer = CheckedWriter {
            file: File::open(&path).unwrap(),
            error: Arc::clone(&error),
        };
        assert_eq!(writer.write(b"abc").unwrap(), 3);
        assert_eq!(writer.write(b"def").unwrap(), 3);
        assert!(error.lock().unwrap().is_some());
    }

    #[test]
    fn an_invalid_filter_is_refused_before_any_file_exists() {
        let dir = assert_fs::TempDir::new().unwrap();
        let err = TraceSession::start(dir.path(), Some("cabin=loud"), "build", Instant::now())
            .err()
            .unwrap();
        assert!(err.contains(cabin_env::CABIN_TRACE_FILTER), "{err}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    const LOG: &str = "# ninja log v7\n0\t9\t1\t/b/old.o\taa\n";

    #[test]
    fn only_the_entries_appended_after_the_snapshot_are_actions() {
        let after = format!(
            "{LOG}5\t20\t2\t/b/x.o\tbb\n# ninja log v7\n20\t30\t3\t/b/app\tcc\n20\t30\t3\t/b/app.map\tcc\n"
        );
        assert_eq!(
            appended_actions(LOG.as_bytes(), after.as_bytes()),
            Some(vec![(5, 20, "/b/x.o", "bb"), (20, 30, "/b/app", "cc")]),
            "a header line is skipped and a second output of one command is the same action"
        );
        assert_eq!(
            appended_actions(LOG.as_bytes(), LOG.as_bytes()),
            Some(vec![])
        );
        assert_eq!(
            appended_actions(b"", LOG.as_bytes()),
            Some(vec![(0, 9, "/b/old.o", "aa")]),
            "a first build's log is all new"
        );
    }

    #[test]
    fn a_parent_relative_build_root_collapses_as_ninja_logs_it() {
        let root = Path::new("/work/app/../build/./dev");
        assert_eq!(lexically_normal(root), Path::new("/work/build/dev"));
    }

    #[test]
    fn a_rewritten_log_has_no_actions_rather_than_old_ones() {
        let recompacted = "# ninja log v7\n5\t20\t2\t/b/x.o\tbb\n";
        assert_eq!(
            appended_actions(LOG.as_bytes(), recompacted.as_bytes()),
            None
        );
    }

    fn interval(lane: (u32, &'static str), name: &str, start_us: u64, dur_us: u64) -> Interval {
        Interval {
            lane,
            name: name.to_owned(),
            start_us,
            dur_us,
        }
    }

    #[test]
    fn overlapping_intervals_never_share_a_thread() {
        let events = complete_events(vec![
            interval(TESTS_LANE, "t", 0, 10),
            interval(NINJA_LANE, "c", 10, 5),
            interval(NINJA_LANE, "b", 2, 8),
            interval(NINJA_LANE, "a", 0, 10),
        ]);
        let placed: Vec<(u64, u64, &str)> = events
            .iter()
            .filter(|e| e["ph"] == "X")
            .map(|e| {
                (
                    e["pid"].as_u64().unwrap(),
                    e["tid"].as_u64().unwrap(),
                    e["name"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            placed,
            [(2, 0, "a"), (2, 1, "b"), (2, 0, "c"), (3, 0, "t")],
            "`b` overlaps `a`; `c` starts as `a` ends; each lane counts its own threads"
        );
        let processes: Vec<&str> = events
            .iter()
            .filter(|e| e["ph"] == "M")
            .map(|e| e["args"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(processes, ["ninja", "tests"]);
    }

    #[test]
    fn appending_refuses_an_unterminated_trace() {
        let (_dir, path) = trace_file("[\n{\"n\":0}");
        assert!(append_events(open(&path), &[serde_json::json!({})]).is_err());
    }
}
