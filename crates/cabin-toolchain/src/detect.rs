//! Compiler / tool detection on top of a [`ResolvedToolchain`].
//!
//! Detection runs three short-lived subprocesses in the worst
//! case (`cxx --version`, `cc --version`, `ar --version`),
//! captures their output, and hands it to the pure parsers in
//! `cabin_core::compiler`. The result is a typed
//! [`ToolchainDetectionReport`] that downstream crates consume
//! without re-running anything.
//!
//! No network access. No probe compilations in this step. The
//! [`ToolRunner`] trait makes the subprocess layer injectable so
//! tests can exercise every code path without touching the
//! filesystem or PATH.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use cabin_core::{
    ArchiverCapabilities, ArchiverIdentity, CompilerCapabilities, CompilerIdentity, ResolvedTool,
    ResolvedToolchain, ToolDetection, ToolKind, ToolchainDetectionReport, derive_ar_capabilities,
    derive_cxx_capabilities, parse_ar_version_output, parse_cxx_version_output,
};
use thiserror::Error;

/// Run a tool with `args` and capture its merged output.
///
/// The trait abstracts over `std::process::Command` so the unit
/// tests in this module can drive every detection branch without
/// real binaries on PATH. The default production runner is
/// [`ProcessRunner`].
pub trait ToolRunner {
    /// Spawn `path` with `args` and capture its stdout/stderr.
    /// The runner must not hang on hostile binaries: a deadline
    /// or fast subprocess form is the implementation's
    /// responsibility.
    ///
    /// # Errors
    /// Returns [`RunError`]: `Spawn` if `path` cannot be launched,
    /// `Read` if capturing the child's output fails, and `Timeout`
    /// if the runner's deadline elapses before the process exits.
    fn run(&self, path: &Path, args: &[&str]) -> Result<RunOutput, RunError>;
}

/// Captured output of one [`ToolRunner::run`] invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutput {
    /// Combined `stdout + stderr` string used by the parsers.
    /// Some toolchains print version info on stderr (notably
    /// older GCCs); concatenating both lets us recognize them
    /// without per-tool branching.
    pub fn combined(&self) -> String {
        let mut s = self.stdout.clone();
        if !self.stderr.is_empty() {
            if !s.is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&self.stderr);
        }
        s
    }
}

/// Errors produced by [`ToolRunner::run`].
#[derive(Debug, Error)]
pub enum RunError {
    #[error("failed to spawn {path}: {source}", path = path.display())]
    Spawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read output from {path}: {source}", path = path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "timed out after {seconds:.1}s running {path}",
        path = path.display(),
        seconds = timeout.as_secs_f64()
    )]
    Timeout { path: PathBuf, timeout: Duration },
}

/// Production [`ToolRunner`] backed by `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessRunner;

/// Production [`ToolRunner`] with an explicit deadline.
#[derive(Debug, Clone, Copy)]
pub struct TimedProcessRunner {
    timeout: Duration,
}

impl ProcessRunner {
    /// Default deadline for one `tool --version` probe.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

    /// Build a production runner with a caller-supplied deadline.
    pub fn with_timeout(timeout: Duration) -> TimedProcessRunner {
        TimedProcessRunner { timeout }
    }
}

impl ToolRunner for ProcessRunner {
    fn run(&self, path: &Path, args: &[&str]) -> Result<RunOutput, RunError> {
        run_process_with_timeout(path, args, Self::DEFAULT_TIMEOUT)
    }
}

impl ToolRunner for TimedProcessRunner {
    fn run(&self, path: &Path, args: &[&str]) -> Result<RunOutput, RunError> {
        run_process_with_timeout(path, args, self.timeout)
    }
}

fn run_process_with_timeout(
    path: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<RunOutput, RunError> {
    let mut child = Command::new(path)
        .args(args)
        // Give the subprocess a clean stdin so detectors that
        // read from stdin (some old GCC wrappers do) cannot wait
        // for user input.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| RunError::Spawn {
            path: path.to_path_buf(),
            source,
        })?;

    let stdout = child.stdout.take().expect("stdout pipe requested");
    let stderr = child.stderr.take().expect("stderr pipe requested");
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|source| RunError::Read {
            path: path.to_path_buf(),
            source,
        })? {
            break status;
        }
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Timeout {
                path: path.to_path_buf(),
                timeout,
            });
        }
        thread::sleep(
            deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(10)),
        );
    };

    let stdout = collect_pipe(stdout_reader, path)?;
    let stderr = collect_pipe(stderr_reader, path)?;
    Ok(RunOutput {
        status: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    pipe.read_to_end(&mut out)?;
    Ok(out)
}

fn collect_pipe(
    handle: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    path: &Path,
) -> Result<Vec<u8>, RunError> {
    handle
        .join()
        .map_err(|_| RunError::Read {
            path: path.to_path_buf(),
            source: std::io::Error::other("output reader thread panicked"),
        })?
        .map_err(|source| RunError::Read {
            path: path.to_path_buf(),
            source,
        })
}

/// Errors produced by [`detect_toolchain`].
#[derive(Debug, Error)]
pub enum DetectionError {
    #[error(
        "failed to run {label} `{spec}` for version detection: {source}",
        label = kind_label(*kind),
    )]
    SubprocessFailed {
        kind: ToolKind,
        spec: String,
        #[source]
        source: RunError,
    },
}

fn kind_label(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::CCompiler => "C compiler",
        ToolKind::CxxCompiler => "C++ compiler",
        ToolKind::Archiver => "archiver",
    }
}

/// Detect identity and capabilities for every tool in `toolchain`.
///
/// `runner` is responsible for spawning each tool. Production
/// callers pass [`ProcessRunner`]; tests inject a fake. The
/// returned [`ToolchainDetectionReport`] is consumed by the build
/// planner (which validates that the resolved compiler / archiver
/// can run the commands the planner emits) and by the
/// `cabin metadata` view.
///
/// # Errors
/// Returns [`DetectionError::SubprocessFailed`] when `runner` fails
/// to spawn or capture output from a tool's `--version` probe; the
/// underlying [`RunError`] is propagated as its source. A non-zero
/// exit status is not an error (the tool is recorded as unknown).
pub fn detect_toolchain(
    toolchain: &ResolvedToolchain,
    runner: &dyn ToolRunner,
) -> Result<ToolchainDetectionReport, DetectionError> {
    let cxx = detect_cxx(&toolchain.cxx, runner)?;
    let cc = match toolchain.cc.as_ref() {
        Some(tool) => Some(detect_cxx(tool, runner)?),
        None => None,
    };
    let ar = detect_ar(&toolchain.ar, runner)?;
    Ok(ToolchainDetectionReport { cxx, cc, ar })
}

fn detect_cxx(
    tool: &ResolvedTool,
    runner: &dyn ToolRunner,
) -> Result<ToolDetection<CompilerIdentity, CompilerCapabilities>, DetectionError> {
    let output = runner
        .run(tool.path().as_std_path(), &["--version"])
        .map_err(|source| DetectionError::SubprocessFailed {
            kind: tool.kind,
            spec: tool.spec.display(),
            source,
        })?;
    let combined = output.combined();
    let identity = if output.status == 0 {
        parse_cxx_version_output(&combined)
    } else {
        // A non-zero exit is treated as an unidentifiable
        // compiler. We keep the captured first line so metadata
        // and errors can still tell the user *what* misbehaved.
        CompilerIdentity::unknown(first_non_empty_line(&combined))
    };
    let capabilities = derive_cxx_capabilities(&identity);
    Ok(ToolDetection {
        path: tool.path.clone(),
        identity,
        capabilities,
    })
}

fn detect_ar(
    tool: &ResolvedTool,
    runner: &dyn ToolRunner,
) -> Result<ToolDetection<ArchiverIdentity, ArchiverCapabilities>, DetectionError> {
    let output = runner
        .run(tool.path().as_std_path(), &["--version"])
        .map_err(|source| DetectionError::SubprocessFailed {
            kind: tool.kind,
            spec: tool.spec.display(),
            source,
        })?;
    // Try to parse the captured output first. BSD `ar` (notably
    // Apple's) does not accept `--version` and prints a usage
    // banner instead, so identity-by-output is unreliable for
    // archivers. When parsing yields `Unknown`, fall back to a
    // conservative name-based classification — archivers that are
    // *named* `ar` or `llvm-ar` (or `lib.exe`) reliably behave as
    // their family does. The strict name check is acceptable here
    // because, unlike compilers, we do not have a portable
    // `--version` invocation to rely on.
    let combined = output.combined();
    let mut identity = parse_ar_version_output(&combined);
    if matches!(identity.kind, cabin_core::ArchiverKind::Unknown)
        && let Some(by_name) = classify_ar_by_basename(tool)
    {
        identity.kind = by_name;
    }
    let capabilities = derive_ar_capabilities(&identity);
    Ok(ToolDetection {
        path: tool.path.clone(),
        identity,
        capabilities,
    })
}

/// Conservative basename-based classification used as a fallback
/// for archivers that do not implement `--version`. Recognizes
/// only the families Cabin already supports plus the unsupported
/// `lib.exe`. Anything else stays [`cabin_core::ArchiverKind::Unknown`].
fn classify_ar_by_basename(tool: &ResolvedTool) -> Option<cabin_core::ArchiverKind> {
    let basename = tool.path.file_name().unwrap_or("").to_ascii_lowercase();
    let stem = basename.strip_suffix(".exe").unwrap_or(&basename);
    if stem == "lib" {
        return Some(cabin_core::ArchiverKind::Lib);
    }
    if stem == "llvm-ar" || stem.starts_with("llvm-ar-") {
        return Some(cabin_core::ArchiverKind::LlvmAr);
    }
    if stem == "ar" || stem.starts_with("ar-") {
        return Some(cabin_core::ArchiverKind::Ar);
    }
    None
}

pub(crate) fn first_non_empty_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{
        ArchiverKind, CompilerKind, ResolvedTool, ResolvedToolchain, ToolKind, ToolSource, ToolSpec,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    use camino::Utf8PathBuf;

    /// In-memory `ToolRunner`: maps `(absolute path, args)` to a
    /// fixed `RunOutput`. Anything not in the map returns a spawn
    /// error so the test surfaces the missing fixture instead of
    /// silently picking up a real binary on PATH.
    struct FakeRunner {
        outputs: HashMap<(PathBuf, Vec<String>), RunOutput>,
    }

    impl FakeRunner {
        fn new() -> Self {
            Self {
                outputs: HashMap::new(),
            }
        }

        fn with(
            mut self,
            path: impl Into<PathBuf>,
            args: &[&str],
            stdout: &str,
            stderr: &str,
            status: i32,
        ) -> Self {
            let key = (
                path.into(),
                args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            );
            self.outputs.insert(
                key,
                RunOutput {
                    status,
                    stdout: stdout.to_owned(),
                    stderr: stderr.to_owned(),
                },
            );
            self
        }
    }

    impl ToolRunner for FakeRunner {
        fn run(&self, path: &Path, args: &[&str]) -> Result<RunOutput, RunError> {
            let key = (
                path.to_path_buf(),
                args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
            );
            self.outputs
                .get(&key)
                .cloned()
                .ok_or_else(|| RunError::Spawn {
                    path: path.to_path_buf(),
                    source: std::io::Error::new(std::io::ErrorKind::NotFound, "fake-runner-miss"),
                })
        }
    }

    fn tool(kind: ToolKind, path: &str, spec: &str) -> ResolvedTool {
        ResolvedTool {
            kind,
            path: Utf8PathBuf::from(path),
            spec: ToolSpec::Name(spec.into()),
            source: ToolSource::Default,
        }
    }

    fn toolchain_with(cxx: ResolvedTool, ar: ResolvedTool) -> ResolvedToolchain {
        ResolvedToolchain { cxx, ar, cc: None }
    }

    #[test]
    fn detects_clang_and_gnu_ar() {
        let cxx = tool(ToolKind::CxxCompiler, "/bin/clang++", "clang++");
        let ar = tool(ToolKind::Archiver, "/bin/ar", "ar");
        let runner = FakeRunner::new()
            .with(
                "/bin/clang++",
                &["--version"],
                "clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\n",
                "",
                0,
            )
            .with(
                "/bin/ar",
                &["--version"],
                "GNU ar (GNU Binutils for Debian) 2.40\n",
                "",
                0,
            );
        let report = detect_toolchain(&toolchain_with(cxx, ar), &runner).unwrap();
        assert_eq!(report.cxx.identity.kind, CompilerKind::Clang);
        assert!(report.cxx.capabilities.gcc_style_flags.supported);
        assert_eq!(report.ar.identity.kind, ArchiverKind::Ar);
        assert!(report.ar.capabilities.ar_crs.supported);
    }

    #[test]
    fn detects_apple_clang_and_falls_back_to_ar_by_name() {
        // BSD `ar` (notably Apple's) does not accept `--version`
        // and prints usage to stderr. The basename-based fallback
        // recognizes it as an `ar`-family archiver so that
        // building on macOS without GNU binutils still works.
        let cxx = tool(ToolKind::CxxCompiler, "/usr/bin/c++", "c++");
        let ar = tool(ToolKind::Archiver, "/usr/bin/ar", "ar");
        let runner = FakeRunner::new()
            .with(
                "/usr/bin/c++",
                &["--version"],
                "Apple clang version 14.0.3 (clang-1403.0.22.14.1)\nTarget: arm64-apple-darwin22.5.0\n",
                "",
                0,
            )
            .with(
                "/usr/bin/ar",
                &["--version"],
                "",
                "usage: ar [-cdmpqrstx] [...]\n",
                1,
            );
        let report = detect_toolchain(&toolchain_with(cxx, ar), &runner).unwrap();
        assert_eq!(report.cxx.identity.kind, CompilerKind::AppleClang);
        // Name-based fallback classifies BSD `ar` as the GNU/BSD
        // family for capability purposes.
        assert_eq!(report.ar.identity.kind, ArchiverKind::Ar);
        assert!(report.ar.capabilities.ar_crs.supported);
    }

    #[test]
    fn unknown_archiver_with_non_ar_basename_stays_unknown() {
        // Basename-based fallback is intentionally narrow:
        // `funky-archiver` does not match `ar` / `llvm-ar` /
        // `lib`, so it remains `Unknown`.
        let cxx = tool(ToolKind::CxxCompiler, "/bin/clang++", "clang++");
        let ar = tool(ToolKind::Archiver, "/bin/funky-archiver", "funky-archiver");
        let runner = FakeRunner::new()
            .with(
                "/bin/clang++",
                &["--version"],
                "clang version 17.0.6\n",
                "",
                0,
            )
            .with(
                "/bin/funky-archiver",
                &["--version"],
                "weird banner\n",
                "",
                0,
            );
        let report = detect_toolchain(&toolchain_with(cxx, ar), &runner).unwrap();
        assert_eq!(report.ar.identity.kind, ArchiverKind::Unknown);
        assert!(!report.ar.capabilities.ar_crs.supported);
    }

    #[test]
    fn nonzero_compiler_exit_yields_unknown() {
        let cxx = tool(ToolKind::CxxCompiler, "/bin/funky-cxx", "funky-cxx");
        let ar = tool(ToolKind::Archiver, "/bin/ar", "ar");
        let runner = FakeRunner::new()
            .with("/bin/funky-cxx", &["--version"], "", "boom\n", 1)
            .with("/bin/ar", &["--version"], "GNU ar 2.40\n", "", 0);
        let report = detect_toolchain(&toolchain_with(cxx, ar), &runner).unwrap();
        assert_eq!(report.cxx.identity.kind, CompilerKind::Unknown);
        assert!(!report.cxx.capabilities.gcc_style_flags.supported);
    }

    #[test]
    fn detects_msvc_compiler() {
        let cxx = tool(ToolKind::CxxCompiler, "/bin/cl", "cl");
        let ar = tool(ToolKind::Archiver, "/bin/lib", "lib");
        let runner = FakeRunner::new()
            .with(
                "/bin/cl",
                &["--version"],
                "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.0 for x64\n",
                "",
                0,
            )
            .with(
                "/bin/lib",
                &["--version"],
                "Microsoft (R) Library Manager Version 14.39.0\n",
                "",
                0,
            );
        let report = detect_toolchain(&toolchain_with(cxx, ar), &runner).unwrap();
        assert_eq!(report.cxx.identity.kind, CompilerKind::Msvc);
        assert!(!report.cxx.capabilities.gcc_style_flags.supported);
        assert_eq!(report.ar.identity.kind, ArchiverKind::Lib);
        assert!(!report.ar.capabilities.ar_crs.supported);
    }

    #[test]
    fn subprocess_spawn_failure_surfaces_typed_error() {
        let cxx = tool(ToolKind::CxxCompiler, "/nonexistent/cxx", "cxx");
        let ar = tool(ToolKind::Archiver, "/nonexistent/ar", "ar");
        let runner = FakeRunner::new();
        let err = detect_toolchain(&toolchain_with(cxx, ar), &runner).unwrap_err();
        match err {
            DetectionError::SubprocessFailed { kind, spec, .. } => {
                assert_eq!(kind, ToolKind::CxxCompiler);
                assert_eq!(spec, "cxx");
            }
        }
    }

    #[test]
    fn ar_with_nonzero_status_falls_back_to_name_based_kind() {
        // `--version` returns nonzero with no recognizable banner,
        // but the executable is named `ar`, so the name-based
        // fallback classifies it as the GNU/BSD `ar` family.
        let cxx = tool(ToolKind::CxxCompiler, "/bin/clang++", "clang++");
        let ar = tool(ToolKind::Archiver, "/usr/bin/ar", "ar");
        let runner = FakeRunner::new()
            .with(
                "/bin/clang++",
                &["--version"],
                "clang version 17.0.6\n",
                "",
                0,
            )
            .with("/usr/bin/ar", &["--version"], "", "boom\n", 2);
        let report = detect_toolchain(&toolchain_with(cxx, ar), &runner).unwrap();
        assert_eq!(report.ar.identity.kind, ArchiverKind::Ar);
    }

    #[test]
    fn cc_is_detected_when_present() {
        let cxx = tool(ToolKind::CxxCompiler, "/bin/clang++", "clang++");
        let cc = tool(ToolKind::CCompiler, "/bin/clang", "clang");
        let ar = tool(ToolKind::Archiver, "/bin/ar", "ar");
        let runner = FakeRunner::new()
            .with(
                "/bin/clang++",
                &["--version"],
                "clang version 17.0.6\n",
                "",
                0,
            )
            .with(
                "/bin/clang",
                &["--version"],
                "clang version 17.0.6\n",
                "",
                0,
            )
            .with("/bin/ar", &["--version"], "GNU ar 2.40\n", "", 0);
        let toolchain = ResolvedToolchain {
            cxx,
            ar,
            cc: Some(cc),
        };
        let report = detect_toolchain(&toolchain, &runner).unwrap();
        let cc_detection = report.cc.expect("cc detected");
        assert_eq!(cc_detection.identity.kind, CompilerKind::Clang);
    }

    #[test]
    fn process_runner_times_out_hanging_tool() {
        let runner = ProcessRunner::with_timeout(Duration::from_millis(25));
        let exe = std::env::current_exe().expect("current test executable");

        let err = runner
            .run(
                &exe,
                &[
                    "--ignored",
                    "--exact",
                    "detect::tests::process_runner_timeout_helper",
                ],
            )
            .expect_err("sleeping helper should exceed runner deadline");

        assert!(
            matches!(err, RunError::Timeout { .. }),
            "expected timeout error, got {err:?}"
        );
    }

    #[ignore = "spawned by process_runner_times_out_hanging_tool"]
    #[test]
    fn process_runner_timeout_helper() {
        thread::sleep(Duration::from_secs(30));
    }
}
