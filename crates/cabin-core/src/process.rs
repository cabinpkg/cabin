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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_renders_each_variant() {
        assert_eq!(ExitStatusKind::Code(0).to_string(), "0");
        assert_eq!(ExitStatusKind::Code(-1).to_string(), "-1");
        assert_eq!(
            ExitStatusKind::Signal("11".to_owned()).to_string(),
            "signal 11"
        );
        assert_eq!(ExitStatusKind::Unknown.to_string(), "<unknown>");
    }

    #[cfg(unix)]
    #[test]
    fn classifies_unix_wait_statuses() {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;
        // POSIX wait status encoding: the exit code lives in the
        // high byte, a terminating signal in the low bits.
        assert_eq!(
            exit_status_kind(ExitStatus::from_raw(0)),
            ExitStatusKind::Code(0)
        );
        assert_eq!(
            exit_status_kind(ExitStatus::from_raw(2 << 8)),
            ExitStatusKind::Code(2)
        );
        assert_eq!(
            exit_status_kind(ExitStatus::from_raw(9)),
            ExitStatusKind::Signal("9".to_owned())
        );
        // A stopped status (low byte 0x7f) carries neither an exit
        // code nor a terminating signal: the fallback kicks in.
        assert_eq!(
            exit_status_kind(ExitStatus::from_raw(0x7f)),
            ExitStatusKind::Unknown
        );
    }

    #[cfg(windows)]
    #[test]
    fn classifies_windows_exit_codes() {
        use std::os::windows::process::ExitStatusExt;
        use std::process::ExitStatus;
        assert_eq!(
            exit_status_kind(ExitStatus::from_raw(0)),
            ExitStatusKind::Code(0)
        );
        assert_eq!(
            exit_status_kind(ExitStatus::from_raw(2)),
            ExitStatusKind::Code(2)
        );
    }
}
