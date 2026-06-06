//! `clang-format` runner used by Cabin's developer-tools
//! commands.
//!
//! This crate keeps formatting-specific executable resolution,
//! command-line construction, and exit-status handling outside
//! `cabin`, mirroring the crate boundaries used by
//! `cabin-tidy`.
//!
//! Crate boundaries:
//!
//! - the crate owns formatter executable resolution and the
//!   `clang-format` command-line shape;
//! - it accepts typed inputs ([`FormatRequest`]) and emits
//!   typed outcomes ([`FormatReport`]);
//! - it never walks the filesystem looking for sources — that
//!   job belongs to `cabin-source-discovery`;
//! - it never reads Cabin's configuration files — the
//!   orchestration layer threads any config-derived inputs
//!   through the typed `FormatRequest`.

#![deny(missing_docs)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

/// Environment variable users can set to override the
/// `clang-format` executable Cabin invokes.
///
/// Precedence: when `CABIN_FMT` is set and non-empty, its value
/// is the absolute path / command name Cabin uses verbatim;
/// otherwise `clang-format` is resolved against `PATH` by the
/// child process spawn.
///
/// Aliased from [`cabin_env`], the single source of truth for
/// every `CABIN_*` environment-variable name.
pub(crate) use cabin_env::CABIN_FMT as CABIN_FMT_ENV;

/// Default executable name Cabin spawns when [`CABIN_FMT_ENV`]
/// is not set.
pub(crate) const DEFAULT_FORMATTER_EXECUTABLE: &str = "clang-format";

/// Operation mode the runner should perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatMode {
    /// Rewrite each file in place (`clang-format -i`).
    Write,
    /// Verify each file is already formatted (`clang-format
    /// --dry-run -Werror`).  Files are *not* modified.
    Check,
}

/// Input for [`run_formatter`].
///
/// Callers translate CLI flags into this typed shape and hand
/// it to the runner; the runner is responsible for spawning
/// the formatter and translating its exit status into a typed
/// outcome.
#[derive(Debug, Clone)]
pub struct FormatRequest {
    /// Absolute path or bare command name of the formatter
    /// executable.  Typically the value
    /// [`resolve_formatter_executable`] returns.
    pub executable: OsString,

    /// Absolute paths to the files the formatter should
    /// process.  An empty `files` list is a valid no-op: the
    /// runner returns a report with zero files processed and
    /// does not spawn a subprocess.
    pub files: Vec<PathBuf>,

    /// Operation mode.
    pub mode: FormatMode,
}

/// Per-mode outcome of [`run_formatter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatReport {
    /// Write-mode succeeded.  Every supplied file was
    /// processed by the formatter; whether or not it was
    /// actually modified is not reported (real `clang-format
    /// -i` does not advertise that distinction either).
    Wrote {
        /// Total number of files passed to the formatter.
        files_processed: usize,
    },
    /// Check-mode succeeded.  Every supplied file was already
    /// formatted.
    Clean {
        /// Total number of files inspected.
        files_inspected: usize,
    },
    /// Check-mode reported at least one file that would be
    /// reformatted.
    NeedsFormatting {
        /// Total number of files inspected.
        files_inspected: usize,
        /// Formatter stderr, trimmed of trailing whitespace.
        /// Carries the per-file `would be reformatted` lines (or
        /// any other diagnostic clang-format emitted under
        /// `--dry-run -Werror`) so the orchestration layer can
        /// pass them through to the user — matching `cargo fmt
        /// --check`, which forwards rustfmt's diff verbatim.
        stderr: String,
    },
}

/// Errors surfaced by the runner.
#[derive(Debug, Error)]
pub enum FormatError {
    /// The formatter executable was not found on the host.
    /// Surfaces the executable name the runner attempted to
    /// spawn and an actionable hint.
    #[error(
        "{executable} was not found on PATH.\n  install `clang-format` (LLVM toolchain) and re-run, or set `{env}=/path/to/clang-format` to a specific binary"
    )]
    ExecutableNotFound {
        /// Executable Cabin tried to spawn.
        executable: String,
        /// Name of the override env var.
        env: &'static str,
    },

    /// Spawning the formatter failed with an I/O error other
    /// than "not found".  Wraps the underlying error so the
    /// caller can render it verbatim.
    #[error("failed to invoke {executable}: {source}")]
    SpawnFailed {
        /// Executable Cabin tried to spawn.
        executable: String,
        /// Underlying spawn error.
        #[source]
        source: std::io::Error,
    },

    /// The formatter ran but reported a non-zero exit status
    /// outside the documented "check-mode signals a diff"
    /// contract.  Captured stderr (if any) is preserved so the
    /// CLI can show it to the user.
    #[error("{executable} exited with status {status}{}", display_stderr(stderr))]
    InvocationFailed {
        /// Executable Cabin tried to spawn.
        executable: String,
        /// The reported exit status.  `None` when the process
        /// was killed by a signal; the OS-specific code (if
        /// any) is included in the message via the formatter.
        status: ExitStatusKind,
        /// Captured stderr, trimmed of trailing whitespace.
        /// Empty when the formatter produced no stderr.
        stderr: String,
    },
}

/// Exit-status classification, shared with `cabin-tidy` so the two
/// external-tool runners report process outcomes the same way.
pub use cabin_core::ExitStatusKind;

fn display_stderr(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!("\n{stderr}")
    }
}

/// Resolve the formatter executable Cabin should spawn.
///
/// Reads `CABIN_FMT` via the supplied env lookup closure; if
/// the value is set and non-empty, it is used verbatim.
/// Otherwise `DEFAULT_FORMATTER_EXECUTABLE` is returned and
/// the spawn relies on `PATH`.
///
/// The closure interface keeps the function pure: tests pass
/// a fake env without touching the process environment.
pub fn resolve_formatter_executable<F>(env: F) -> OsString
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(value) = env(CABIN_FMT_ENV)
        && !value.is_empty()
    {
        return value;
    }
    OsString::from(DEFAULT_FORMATTER_EXECUTABLE)
}

/// Run `clang-format` over the requested files.
///
/// Returns a typed [`FormatReport`] when the formatter produced
/// a recognized outcome; otherwise returns a typed
/// [`FormatError`] that the CLI can render through its
/// diagnostic chain.
///
/// # Errors
/// Returns [`FormatError::ExecutableNotFound`] when spawning the
/// formatter fails with `ErrorKind::NotFound`, and
/// [`FormatError::SpawnFailed`] for any other spawn I/O error.
/// Returns [`FormatError::InvocationFailed`] when the formatter
/// exits unsuccessfully: in [`FormatMode::Write`] on any
/// non-success status, and in [`FormatMode::Check`] on any exit
/// status that is neither success (clean) nor code `1` (needs
/// formatting).
pub fn run_formatter(request: &FormatRequest) -> Result<FormatReport, FormatError> {
    if request.files.is_empty() {
        return Ok(match request.mode {
            FormatMode::Write => FormatReport::Wrote { files_processed: 0 },
            FormatMode::Check => FormatReport::Clean { files_inspected: 0 },
        });
    }

    // Deterministic order keeps the produced command line
    // byte-stable for very-verbose echoes and snapshot tests.
    // We dedupe by absolute path while preserving sort order.
    let files: BTreeSet<&Path> = request.files.iter().map(PathBuf::as_path).collect();
    let files: Vec<&Path> = files.into_iter().collect();

    let mut cmd = Command::new(&request.executable);
    // `clang-format` discovers `.clang-format` from the first
    // file's directory upward.  Passing `--style=file`
    // explicitly mirrors the documented behavior Cabin
    // promises so a user who points `CABIN_FMT` at a
    // wrapper sees the same discovery rule.
    cmd.arg("--style=file");
    match request.mode {
        FormatMode::Write => {
            cmd.arg("-i");
        }
        FormatMode::Check => {
            cmd.arg("--dry-run").arg("-Werror");
        }
    }
    for path in &files {
        cmd.arg(path);
    }

    let output = match cmd.output() {
        Ok(output) => output,
        Err(err) => {
            let executable = request.executable.to_string_lossy().into_owned();
            if err.kind() == std::io::ErrorKind::NotFound {
                return Err(FormatError::ExecutableNotFound {
                    executable,
                    env: CABIN_FMT_ENV,
                });
            }
            return Err(FormatError::SpawnFailed {
                executable,
                source: err,
            });
        }
    };

    let status = output.status;
    let stderr = String::from_utf8_lossy(&output.stderr)
        .trim_end()
        .to_owned();

    match request.mode {
        FormatMode::Write => {
            if status.success() {
                Ok(FormatReport::Wrote {
                    files_processed: files.len(),
                })
            } else {
                Err(FormatError::InvocationFailed {
                    executable: request.executable.to_string_lossy().into_owned(),
                    status: cabin_core::exit_status_kind(status),
                    stderr,
                })
            }
        }
        FormatMode::Check => {
            if status.success() {
                return Ok(FormatReport::Clean {
                    files_inspected: files.len(),
                });
            }
            // `clang-format --dry-run -Werror` exits with code
            // 1 when any input would be reformatted; that is
            // not an *error* for our purposes, it is the
            // documented "check failed" signal.  Anything else
            // is a hard error.
            if status.code() == Some(1) {
                return Ok(FormatReport::NeedsFormatting {
                    files_inspected: files.len(),
                    stderr,
                });
            }
            Err(FormatError::InvocationFailed {
                executable: request.executable.to_string_lossy().into_owned(),
                status: cabin_core::exit_status_kind(status),
                stderr,
            })
        }
    }
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
    fn default_executable_is_clang_format_when_env_unset() {
        let resolved = resolve_formatter_executable(|_| None);
        assert_eq!(resolved, OsString::from(DEFAULT_FORMATTER_EXECUTABLE));
    }

    #[test]
    fn env_override_wins() {
        let env = env_returning(&[(CABIN_FMT_ENV, "/opt/llvm/bin/clang-format")]);
        let resolved = resolve_formatter_executable(env);
        assert_eq!(resolved, OsString::from("/opt/llvm/bin/clang-format"));
    }

    #[test]
    fn empty_env_value_falls_back_to_default() {
        let env = env_returning(&[(CABIN_FMT_ENV, "")]);
        let resolved = resolve_formatter_executable(env);
        assert_eq!(resolved, OsString::from(DEFAULT_FORMATTER_EXECUTABLE));
    }

    #[test]
    fn empty_files_is_a_clean_no_op() {
        let req = FormatRequest {
            executable: OsString::from("this-binary-should-not-be-invoked"),
            files: Vec::new(),
            mode: FormatMode::Check,
        };
        let report = run_formatter(&req).unwrap();
        assert!(matches!(report, FormatReport::Clean { files_inspected: 0 }));

        let req = FormatRequest {
            mode: FormatMode::Write,
            ..req
        };
        let report = run_formatter(&req).unwrap();
        assert!(matches!(report, FormatReport::Wrote { files_processed: 0 }));
    }

    #[test]
    fn missing_executable_yields_actionable_error() {
        // Use a path we know does not exist.
        let req = FormatRequest {
            executable: OsString::from("/no-such/clang-format-binary"),
            files: vec![PathBuf::from("/no-such/file.cc")],
            mode: FormatMode::Write,
        };
        let err = run_formatter(&req).unwrap_err();
        match err {
            FormatError::ExecutableNotFound { executable, env } => {
                assert_eq!(executable, "/no-such/clang-format-binary");
                assert_eq!(env, CABIN_FMT_ENV);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
