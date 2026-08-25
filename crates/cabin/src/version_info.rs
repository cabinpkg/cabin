//! Typed model and deterministic formatter for `cabin version`.
//!
//! `cabin --version` is the clap-style framework spelling and
//! prints the concise `cabin <semver>` line; `cabin version`
//! is the dedicated subcommand:
//!
//! - the concise form (`cabin version`) prints `cabin <semver>`;
//! - the verbose form (`cabin version -v`, or the global
//!   `cabin -v version`) prints a cargo-style key/value block.
//!
//! Runtime metadata (the OS identity) is probed through the
//! `os_info` crate, which inspects local platform state without
//! any network or filesystem access beyond a `uname`-equivalent
//! syscall.  Tests construct `VersionInfo` directly through
//! `VersionInfo::for_tests` so the formatter can be exercised
//! against controlled inputs without touching the host
//! environment.

use std::fmt::Write as _;

/// Output mode requested by the CLI caller.  The mapping from
/// global verbosity to mode happens in the dispatcher so this
/// module stays decoupled from `cabin_core::Verbosity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VersionOutputMode {
    /// Concise single-line release-name + semver.
    Concise,
    /// Cargo-style verbose block, headed by the release line
    /// and followed by labeled key/value rows.
    Verbose,
}

/// Typed snapshot of Cabin's version-relevant metadata.  The
/// struct is `Clone` so test helpers can compose fixtures
/// without re-deriving every field.
#[derive(Debug, Clone)]
pub(crate) struct VersionInfo {
    /// Always present - driven by the workspace's
    /// `[workspace.package] version` field.
    cabin_version: String,
    /// Human-readable OS identity (`Mac OS 26.4.1 [64-bit]`,
    /// `Ubuntu 24.04 [64-bit]`, …) captured at runtime, or
    /// `None` when probing fails.
    os: Option<String>,
}

impl VersionInfo {
    /// Snapshot of the binary that is currently running.
    /// The runtime OS string is probed once on demand.
    pub(crate) fn current() -> Self {
        Self {
            cabin_version: env!("CARGO_PKG_VERSION").to_owned(),
            os: detect_os_string(),
        }
    }

    /// Build a [`VersionInfo`] from explicit fields.  Tests use
    /// this constructor to exercise the formatter against a
    /// controlled snapshot; production code calls
    /// [`VersionInfo::current`].
    #[cfg(test)]
    fn for_tests(cabin_version: &str, os: Option<&str>) -> Self {
        Self {
            cabin_version: cabin_version.to_owned(),
            os: os.map(str::to_owned),
        }
    }

    /// Render the requested output mode into a fresh `String`.
    /// Trailing newline is included for both modes so a CLI
    /// caller can write the result directly with `print!`.
    pub(crate) fn format(&self, mode: VersionOutputMode) -> String {
        match mode {
            VersionOutputMode::Concise => format!("cabin {}\n", self.cabin_version),
            VersionOutputMode::Verbose => self.format_verbose(),
        }
    }

    fn format_verbose(&self) -> String {
        // Each labeled row contributes roughly `<label>:
        // <value>\n`; reserve a reasonable amount up-front to
        // keep the formatter free of intermediate allocations.
        let mut out = String::with_capacity(256);

        let _ = writeln!(out, "cabin {}", self.cabin_version);
        let _ = writeln!(out, "release: {}", self.cabin_version);
        if let Some(os) = self.os.as_deref() {
            let _ = writeln!(out, "os: {os}");
        }
        out
    }
}

/// Probe the running OS through the `os_info` crate and format
/// the result the same way cargo formats its own `os:` line -
/// `<OS> <version> [<bitness>]`, e.g.  `Mac OS 26.4.1 [64-bit]`.
/// Returns `None` only if every component reports as `Unknown`
/// so the formatter can skip the row entirely.
fn detect_os_string() -> Option<String> {
    let info = os_info::get();

    let os_type = info.os_type();
    let version = info.version();
    let bitness = info.bitness();

    let mut buf = String::new();
    let _ = write!(buf, "{os_type}");

    // `os_info::Version::Unknown` renders as the literal
    // `Unknown` - skip that case so the row reads cleanly on
    // platforms where a version is unavailable.
    if !matches!(version, os_info::Version::Unknown) {
        let _ = write!(buf, " {version}");
    }

    if !matches!(bitness, os_info::Bitness::Unknown) {
        let _ = write!(buf, " [{bitness}]");
    }

    let buf = buf.trim().to_owned();
    if buf.is_empty() { None } else { Some(buf) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> VersionInfo {
        VersionInfo::for_tests("x.y.z", Some("Ubuntu 24.04 [64-bit]"))
    }

    fn minimal() -> VersionInfo {
        VersionInfo::for_tests("x.y.z", None)
    }

    #[test]
    fn concise_format_is_single_line_with_release_name() {
        let info = full();
        assert_eq!(info.format(VersionOutputMode::Concise), "cabin x.y.z\n");
    }

    #[test]
    fn verbose_format_emits_release_and_os() {
        let info = full();
        let out = info.format(VersionOutputMode::Verbose);
        let expected = "\
cabin x.y.z
release: x.y.z
os: Ubuntu 24.04 [64-bit]
";
        assert_eq!(out, expected);
    }

    #[test]
    fn verbose_format_omits_missing_optional_rows() {
        let info = minimal();
        let out = info.format(VersionOutputMode::Verbose);
        // Without OS metadata, only the header and the
        // `release:` line survive.
        let expected = "\
cabin x.y.z
release: x.y.z
";
        assert_eq!(out, expected);
    }
}
