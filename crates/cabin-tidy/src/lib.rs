//! `run-clang-tidy` runner used by Cabin's developer-tools
//! commands.
//!
//! Today the only consumer is `cabin tidy`.  The crate is split
//! out from `cabin` so the executable-resolution rule, the
//! `run-clang-tidy` command-line shape, and the typed jobs
//! forwarding can be reused without dragging in the
//! workspace-loader, build-planner, and config layers.  The
//! fix-mode safety policy (clamping jobs to one) stays in the
//! orchestration layer; the runner forwards whatever it is
//! given.
//!
//! Crate boundaries:
//!
//! - the crate owns tidy executable resolution and the
//!   `run-clang-tidy` command-line shape;
//! - it accepts typed inputs ([`TidyRequest`]) and emits typed
//!   outcomes ([`TidyReport`]);
//! - it never walks the filesystem looking for sources - that is
//!   `cabin-source-discovery`'s job;
//! - it never plans builds or generates compile databases - those
//!   are `cabin-build`'s and `cabin-ninja`'s jobs;
//! - it never reads Cabin's configuration files - the orchestration
//!   layer threads any config-derived inputs through the typed
//!   `TidyRequest`.

#![deny(missing_docs)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use thiserror::Error;

use cabin_core::BuildJobs;

/// Environment variable users can set to override the
/// `run-clang-tidy` executable Cabin invokes; see
/// [`resolve_tidy_executable`].
///
/// Aliased from [`cabin_env`], the single source of truth for
/// every `CABIN_*` environment-variable name.
pub(crate) use cabin_env::CABIN_TIDY as CABIN_TIDY_ENV;

/// Default executable name Cabin spawns when [`CABIN_TIDY_ENV`]
/// is not set. `run-clang-tidy` is the LLVM-supplied driver that
/// fans clang-tidy invocations out across a compilation database.
pub(crate) const DEFAULT_TIDY_EXECUTABLE: &str = "run-clang-tidy";

/// Operation mode the runner should perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TidyMode {
    /// Analyze only; clang-tidy emits diagnostics but does not
    /// modify any source file.
    Check,
    /// Apply clang-tidy's suggested fixes back to disk.  The CLI
    /// selects this only for an explicit `--fix` flag.
    Fix,
}

/// Verbosity hint the orchestration layer threads through to the
/// runner.  The runner uses this *only* to decide whether to pass
/// `-quiet` to `run-clang-tidy` - the actual Cabin-owned status
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
#[derive(Debug, Clone)]
pub struct TidyRequest {
    /// Absolute path or bare command name of the tidy driver
    /// executable.  Typically the value
    /// [`resolve_tidy_executable`] returns.
    pub executable: OsString,

    /// Absolute path to the directory containing the
    /// `compile_commands.json` Cabin generated for this
    /// invocation. `run-clang-tidy -p <dir>` is how
    /// `run-clang-tidy` discovers the compilation database.
    pub compile_database_dir: PathBuf,

    /// Files the tidy driver should process, each spelled exactly
    /// as the `file` of its entry in the supplied compilation
    /// database: `run-clang-tidy` selects entries by that
    /// spelling, so a path with any other spelling selects
    /// nothing.  An empty `files` list is a valid no-op: the
    /// runner returns [`TidyReport::NoFiles`] and does not spawn
    /// a subprocess.
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
    /// failure; clang-tidy's diagnostics have already reached the
    /// inherited stdout / stderr.
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

/// Exit-status classification, shared with `cabin-fmt` so the two
/// external-tool runners report process outcomes the same way.
pub use cabin_core::ExitStatusKind;

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
/// editors) already know how to consume. `cabin-tidy` does not
/// parse the output: that would duplicate clang-tidy's own
/// diagnostic format and turn every upstream change into a Cabin
/// bug.
///
/// The file list is sorted and deduplicated through a
/// [`BTreeSet`], so the argv order does not depend on the order of
/// `request.files`.
///
/// # Errors
/// Returns [`TidyError::ExecutableNotFound`] when the resolved
/// executable cannot be located (the spawn fails with
/// `ErrorKind::NotFound`); returns [`TidyError::SpawnFailed`]
/// wrapping the underlying [`std::io::Error`] for any other spawn
/// failure.  A non-zero clang-tidy exit and an empty file list are
/// reported as `Ok` (`TidyReport::TidyFailed` and
/// `TidyReport::NoFiles` respectively), not `Err`.
pub fn run_tidy(request: &TidyRequest) -> Result<TidyReport, TidyError> {
    if request.files.is_empty() {
        return Ok(TidyReport::NoFiles);
    }

    let files: BTreeSet<&Path> = request.files.iter().map(PathBuf::as_path).collect();
    let files: Vec<&Path> = files.into_iter().collect();

    let mut cmd = Command::new(&request.executable);
    // The registry credential is Cabin's input, not the linter's:
    // scrub it so the spawned tool can never read the token.
    cmd.env_remove(cabin_env::CABIN_REGISTRY_TOKEN);
    cmd.env_remove(cabin_env::ACTIONS_ID_TOKEN_REQUEST_TOKEN);
    cmd.arg("-p").arg(&request.compile_database_dir);
    if matches!(request.mode, TidyMode::Fix) {
        cmd.arg("-fix");
    }
    if matches!(request.verbosity, TidyVerbosity::Normal) {
        cmd.arg("-quiet");
    }
    if let Some(jobs) = request.jobs {
        cmd.arg("-j").arg(jobs.get().to_string());
    }
    for path in &files {
        cmd.args(file_filters(path));
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
            status: cabin_core::exit_status_kind(status),
            files_processed: files.len(),
        })
    }
}

/// The `run-clang-tidy` positional arguments that select exactly the
/// compilation-database entry whose `file` is spelled `path`.
///
/// `run-clang-tidy` joins its positional arguments with `|` into one
/// Python regular expression and keeps each entry whose path it
/// `re.search`es, so a bare path is a pattern: `.` matches any
/// character, `+` is a quantifier, and `x.c` also selects `x.cc`.
/// Each filter is the path with every Python metacharacter
/// escaped, anchored at both ends.  Before LLVM 19 the entry's path
/// is its `file` verbatim; since then it is `os.path.abspath` of it,
/// which drops `.` components, collapses doubled separators, and on
/// Windows turns `/` into `\`.  A path that normalization respells
/// therefore also gets a filter for the normalized spelling.
fn file_filters(path: &Path) -> Vec<String> {
    let normalized: PathBuf = path.components().collect();
    let mut filters = vec![literal_filter(path)];
    // `Path` equality ignores the differences normalization makes.
    if normalized.as_os_str() != path.as_os_str() {
        filters.push(literal_filter(&normalized));
    }
    filters
}

fn literal_filter(path: &Path) -> String {
    let mut filter = String::from("^");
    for c in path.to_string_lossy().chars() {
        if r"\.^$*+?{}[]|()".contains(c) {
            filter.push('\\');
        }
        filter.push(c);
    }
    filter.push('$');
    filter
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

    #[cfg(unix)]
    #[test]
    fn file_filter_matches_the_path_literally() {
        assert_eq!(
            file_filters(Path::new("/p/src/c++/x.cc")),
            [r"^/p/src/c\+\+/x\.cc$"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_filters_cover_the_normalized_spelling() {
        assert_eq!(
            file_filters(Path::new("/p/./src//x.cc")),
            [r"^/p/\./src//x\.cc$", r"^/p/src/x\.cc$"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn file_filters_cover_the_normalized_spelling() {
        assert_eq!(
            file_filters(Path::new(r"C:\p\src/c++/x.cc")),
            [r"^C:\\p\\src/c\+\+/x\.cc$", r"^C:\\p\\src\\c\+\+\\x\.cc$"]
        );
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
