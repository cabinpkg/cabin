//! `run-clang-tidy` runner used by Cabin's developer-tools
//! commands.
//!
//! Today the only consumer is `cabin tidy`.  The crate is split
//! out from `cabin-cli` so the executable-resolution rule, the
//! `run-clang-tidy` command-line shape, the typed jobs forwarding,
//! and the fix-mode safety policy can be reused without dragging
//! in the workspace-loader, build-planner, and config layers.  It
//! mirrors the shape of `cabin-fmt` so a developer reading the
//! two crates side-by-side sees the same pattern twice.
//!
//! Crate boundaries:
//!
//! - the crate owns tidy executable resolution and the
//!   `run-clang-tidy` command-line shape;
//! - it accepts typed inputs ([`TidyRequest`]) and emits typed
//!   outcomes ([`TidyReport`]);
//! - it never walks the filesystem looking for sources — that is
//!   `cabin-source-discovery`'s job;
//! - it never plans builds or generates compile databases — those
//!   are `cabin-build`'s and `cabin-ninja`'s jobs;
//! - it never reads Cabin's configuration files — the orchestration
//!   layer threads any config-derived inputs through the typed
//!   `TidyRequest`.

#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

use cabin_core::BuildJobs;

/// Environment variable users can set to override the
/// `run-clang-tidy` executable Cabin invokes.
///
/// Precedence: when `CABIN_TIDY` is set and non-empty its value
/// is the absolute path / command name Cabin uses verbatim;
/// otherwise [`DEFAULT_TIDY_EXECUTABLE`] is returned and the
/// child-process spawn relies on `PATH`.
pub(crate) const CABIN_TIDY_ENV: &str = "CABIN_TIDY";

/// Default executable name Cabin spawns when [`CABIN_TIDY_ENV`]
/// is not set.  `run-clang-tidy` is the LLVM-supplied driver that
/// fans clang-tidy invocations out across a compilation database;
/// it ships with every modern LLVM install and is the standard
/// way to drive clang-tidy at package scale.
pub(crate) const DEFAULT_TIDY_EXECUTABLE: &str = "run-clang-tidy";

/// Operation mode the runner should perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TidyMode {
    /// Analyze only; clang-tidy emits diagnostics but does not
    /// modify any source file.
    Check,
    /// Apply clang-tidy's suggested fixes back to disk.  Mutually
    /// exclusive with [`TidyMode::Check`] at the API surface; the
    /// CLI surface gates this on an explicit `--fix` flag.
    Fix,
}

/// Verbosity hint the orchestration layer threads through to the
/// runner.  The runner uses this *only* to decide whether to pass
/// `-quiet` to `run-clang-tidy` — the actual Cabin-owned status
/// output is the orchestration layer's responsibility.
///
/// `Normal` includes the `-quiet` flag so users see clang-tidy
/// diagnostics without the per-file progress chatter.  `Verbose`
/// omits `-quiet` so users opting into more output see
/// `run-clang-tidy`'s own progress lines too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TidyVerbosity {
    /// Cabin's quiet or normal verbosity.  Pass `-quiet` to
    /// `run-clang-tidy`.
    Normal,
    /// Cabin's verbose or very-verbose verbosity.  Do not pass
    /// `-quiet`.
    Verbose,
}

/// Input for [`run_tidy`].
///
/// Callers translate CLI flags into this typed shape and hand it
/// to the runner.  The runner spawns `run-clang-tidy` and reports
/// the typed outcome.
#[derive(Debug, Clone)]
pub struct TidyRequest {
    /// Absolute path or bare command name of the tidy driver
    /// executable.  Typically the value
    /// [`resolve_tidy_executable`] returns.
    pub executable: OsString,

    /// Absolute path to the directory containing the
    /// `compile_commands.json` Cabin generated for this
    /// invocation.  `run-clang-tidy -p <dir>` is how
    /// `run-clang-tidy` discovers the compilation database.
    pub compile_database_dir: PathBuf,

    /// Absolute paths to the files the tidy driver should
    /// process.  An empty `files` list is a valid no-op: the
    /// runner returns [`TidyReport::NoFiles`] and does not spawn
    /// a subprocess.  Callers are expected to filter the list to
    /// translation units that actually appear in the supplied
    /// compilation database; `run-clang-tidy` ignores files that
    /// have no compile entry but emitting them anyway makes the
    /// command line longer without changing behavior.
    pub files: Vec<PathBuf>,

    /// Operation mode.
    pub mode: TidyMode,

    /// Number of parallel `clang-tidy` instances `run-clang-tidy`
    /// should run.  `None` means "let `run-clang-tidy` pick its
    /// default" (which today is the host's CPU count).  When the
    /// CLI selects [`TidyMode::Fix`] the orchestration layer is
    /// expected to clamp this to `Some(1)` for safety; the runner
    /// passes whatever value it receives, so the policy lives in
    /// one place.
    pub jobs: Option<BuildJobs>,

    /// Cabin's verbosity hint.  Controls only whether Cabin
    /// passes `-quiet` to `run-clang-tidy`.
    pub verbosity: TidyVerbosity,
}

/// Typed outcome of [`run_tidy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TidyReport {
    /// `run-clang-tidy` exited cleanly over the supplied file
    /// list.
    Tidied {
        /// Number of files passed to `run-clang-tidy`.
        files_processed: usize,
    },
    /// The file list was empty, so `run-clang-tidy` was not
    /// invoked.
    NoFiles,
    /// `run-clang-tidy` exited with a non-zero status.  The
    /// orchestration layer surfaces this as a Cabin command
    /// failure; clang-tidy's diagnostics have already been
    /// emitted on stderr because the runner inherits stderr.
    TidyFailed {
        /// Reported exit status of the tidy driver.
        status: ExitStatusKind,
        /// Number of files in the failing invocation.
        files_processed: usize,
    },
}

/// Errors surfaced by the runner.
#[derive(Debug, Error)]
pub enum TidyError {
    /// The tidy driver executable was not found on the host.
    /// Surfaces the executable name the runner attempted to spawn
    /// and an actionable hint that names both the install path
    /// and the override env var.
    #[error(
        "{executable} was not found on PATH.\n  install `clang-tidy` (LLVM toolchain) and re-run, or set `{env}=/path/to/run-clang-tidy` to a specific binary"
    )]
    ExecutableNotFound {
        /// Executable Cabin tried to spawn.
        executable: String,
        /// Name of the override env var.
        env: &'static str,
    },

    /// Spawning the tidy driver failed with an I/O error other
    /// than "not found".
    #[error("failed to invoke {executable}: {source}")]
    SpawnFailed {
        /// Executable Cabin tried to spawn.
        executable: String,
        /// Underlying spawn error.
        #[source]
        source: std::io::Error,
    },
}

/// Stringified exit-status kind preserved so the orchestration
/// layer can decide whether to display a code or a signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitStatusKind {
    /// The driver exited normally with this code.
    Code(i32),
    /// The driver was terminated by a signal (Unix only).
    Signal(String),
    /// The driver exited with neither a code nor a signal;
    /// preserved as a fallback only.
    Unknown,
}

impl std::fmt::Display for ExitStatusKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExitStatusKind::Code(c) => write!(f, "{c}"),
            ExitStatusKind::Signal(s) => write!(f, "signal {s}"),
            ExitStatusKind::Unknown => write!(f, "<unknown>"),
        }
    }
}

/// Resolve the tidy driver executable Cabin should spawn.
///
/// Reads `CABIN_TIDY` via the supplied env-lookup closure; if
/// the value is set and non-empty, it is used verbatim.
/// Otherwise `DEFAULT_TIDY_EXECUTABLE` is returned and the
/// spawn relies on `PATH`.
///
/// The closure interface keeps the function pure: tests pass a
/// fake env without touching the process environment.
pub fn resolve_tidy_executable<F>(env: F) -> OsString
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(value) = env(CABIN_TIDY_ENV)
        && !value.is_empty()
    {
        return value;
    }
    OsString::from(DEFAULT_TIDY_EXECUTABLE)
}

/// Run `run-clang-tidy` over the requested files.
///
/// The runner inherits the caller's stdout and stderr so
/// clang-tidy diagnostics reach the user verbatim, preserving the
/// `<file>:<line>:<col>: <category> [<check>]` shape users (and
/// editors) already know how to consume.  `cabin-tidy` does not
/// parse the output: that would duplicate clang-tidy's own
/// diagnostic format and turn every upstream change into a Cabin
/// bug.
///
/// Argument ordering is byte-stable across platforms: the file
/// list is deduplicated through a [`BTreeSet`] so a caller that
/// hands the runner an out-of-order list still produces a
/// command line that very-verbose echoes (and snapshot tests) can
/// rely on.
pub fn run_tidy(request: &TidyRequest) -> Result<TidyReport, TidyError> {
    if request.files.is_empty() {
        return Ok(TidyReport::NoFiles);
    }

    // Deduplicate by absolute path while preserving sort order.
    // The orchestration layer is expected to hand us an
    // already-sorted list, but the explicit `BTreeSet` keeps the
    // contract honest for direct API consumers.
    let files: BTreeSet<&Path> = request.files.iter().map(PathBuf::as_path).collect();
    let files: Vec<&Path> = files.into_iter().collect();

    let mut cmd = Command::new(&request.executable);
    // `-p <dir>` is how `run-clang-tidy` (and `clang-tidy`
    // directly) discovers a `compile_commands.json` in `<dir>`.
    cmd.arg("-p").arg(&request.compile_database_dir);
    if matches!(request.mode, TidyMode::Fix) {
        // `-fix` tells `run-clang-tidy` to apply the fixes
        // clang-tidy emits.  Cabin never enables this implicitly;
        // the orchestration layer requires an explicit `--fix`
        // CLI flag.
        cmd.arg("-fix");
    }
    if matches!(request.verbosity, TidyVerbosity::Normal) {
        // `-quiet` is `run-clang-tidy`'s "no progress chatter"
        // toggle; clang-tidy diagnostics still appear.
        cmd.arg("-quiet");
    }
    if let Some(jobs) = request.jobs {
        // `run-clang-tidy` accepts `-j N` (a positional
        // followed by the count).  We render it as two
        // arguments because that is the spelling LLVM's own
        // `--help` uses; the fused `-jN` form is not documented
        // and historical versions have rejected it.
        cmd.arg("-j").arg(jobs.get().to_string());
    }
    for path in &files {
        cmd.arg(path);
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = match cmd.status() {
        Ok(status) => status,
        Err(err) => {
            let executable = request.executable.to_string_lossy().into_owned();
            if err.kind() == std::io::ErrorKind::NotFound {
                return Err(TidyError::ExecutableNotFound {
                    executable,
                    env: CABIN_TIDY_ENV,
                });
            }
            return Err(TidyError::SpawnFailed {
                executable,
                source: err,
            });
        }
    };

    if status.success() {
        Ok(TidyReport::Tidied {
            files_processed: files.len(),
        })
    } else {
        Ok(TidyReport::TidyFailed {
            status: exit_status_kind(status),
            files_processed: files.len(),
        })
    }
}

fn exit_status_kind(status: std::process::ExitStatus) -> ExitStatusKind {
    if let Some(code) = status.code() {
        return ExitStatusKind::Code(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return ExitStatusKind::Signal(sig.to_string());
        }
    }
    ExitStatusKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_returning<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(*v))
        }
    }

    #[test]
    fn default_executable_is_run_clang_tidy_when_env_unset() {
        let resolved = resolve_tidy_executable(|_| None);
        assert_eq!(resolved, OsString::from(DEFAULT_TIDY_EXECUTABLE));
    }

    #[test]
    fn env_override_wins() {
        let env = env_returning(&[(CABIN_TIDY_ENV, "/opt/llvm/bin/run-clang-tidy")]);
        let resolved = resolve_tidy_executable(env);
        assert_eq!(resolved, OsString::from("/opt/llvm/bin/run-clang-tidy"));
    }

    #[test]
    fn empty_env_value_falls_back_to_default() {
        let env = env_returning(&[(CABIN_TIDY_ENV, "")]);
        let resolved = resolve_tidy_executable(env);
        assert_eq!(resolved, OsString::from(DEFAULT_TIDY_EXECUTABLE));
    }

    #[test]
    fn empty_files_returns_no_files_without_spawning() {
        let req = TidyRequest {
            executable: OsString::from("this-binary-should-not-be-invoked"),
            compile_database_dir: PathBuf::from("/no-such/build"),
            files: Vec::new(),
            mode: TidyMode::Check,
            jobs: None,
            verbosity: TidyVerbosity::Normal,
        };
        let report = run_tidy(&req).unwrap();
        assert_eq!(report, TidyReport::NoFiles);
    }

    #[test]
    fn missing_executable_yields_actionable_error() {
        let req = TidyRequest {
            executable: OsString::from("/no-such/run-clang-tidy-binary"),
            compile_database_dir: PathBuf::from("/no-such/build"),
            files: vec![PathBuf::from("/no-such/file.cc")],
            mode: TidyMode::Check,
            jobs: None,
            verbosity: TidyVerbosity::Normal,
        };
        let err = run_tidy(&req).unwrap_err();
        match err {
            TidyError::ExecutableNotFound { executable, env } => {
                assert_eq!(executable, "/no-such/run-clang-tidy-binary");
                assert_eq!(env, CABIN_TIDY_ENV);
            }
            TidyError::SpawnFailed { executable, source } => {
                panic!("unexpected spawn failure for {executable}: {source}");
            }
        }
    }
}
