//! Shared classification of a child process's exit status.
//!
//! Cabin spawns external tools (formatters, linters, the C/C++
//! toolchain) and needs to report *how* they exited without leaking
//! platform-specific `ExitStatus` details into its own error and
//! report types. [`ExitStatusKind`] is that stable, serializable-free
//! summary; [`exit_status_kind`] derives it once at the spawn site.

/// Stringified exit-status kind preserved so the orchestration layer
/// can decide whether to display an exit code or a signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitStatusKind {
    /// The process exited normally with this code.
    Code(i32),
    /// The process was terminated by a signal (Unix only).
    Signal(String),
    /// The process exited with neither a code nor a signal;
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

/// Classify a finished [`std::process::ExitStatus`] into an
/// [`ExitStatusKind`], preferring the exit code and falling back to the
/// terminating signal on Unix.
pub fn exit_status_kind(status: std::process::ExitStatus) -> ExitStatusKind {
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
