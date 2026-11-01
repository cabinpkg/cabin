//! Typed model and deterministic formatter for `cabin version`.
//!
//! `cabin version` and `cabin --version` share this formatter:
//!
//! - the concise form (`cabin version`) prints `cabin <semver>`;
//! - the verbose form (`cabin version -v`, or the global
//!   `cabin -v version`) prints a cargo-style key/value block.
//!
//! Runtime metadata (the OS identity) is probed through the
//! `os_info` crate, which reads OS release files and runs
//! platform tools (`sw_vers`, `lsb_release`, `getconf`, …) but
//! never touches the network.

use std::fmt::Write as _;

/// Output mode requested by the CLI caller.  `cli::version` maps
/// global verbosity to a mode, so this module stays decoupled from
/// `cabin_core::Verbosity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VersionOutputMode {
    /// Concise single-line release-name + semver.
    Concise,
    /// Cargo-style verbose block, headed by the release line
    /// and followed by labeled key/value rows.
    Verbose,
}

/// Typed snapshot of Cabin's version-relevant metadata.
#[derive(Debug, Clone)]
pub(crate) struct VersionInfo {
    /// Always present - driven by the workspace's
    /// `[workspace.package] version` field.
    cabin_version: String,
    /// Human-readable OS identity (`Mac OS 26.4.1 [64-bit]`,
    /// `Ubuntu 24.04 [64-bit]`, …) captured at runtime; `Unknown`
    /// when `os_info` cannot identify the OS.
    os: String,
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
    fn for_tests(cabin_version: &str, os: &str) -> Self {
        Self {
            cabin_version: cabin_version.to_owned(),
            os: os.to_owned(),
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
        let mut out = String::with_capacity(256);

        let _ = writeln!(out, "cabin {}", self.cabin_version);
        let _ = writeln!(out, "release: {}", self.cabin_version);
        let _ = writeln!(out, "os: {}", self.os);
        out
    }
}

/// Probe the running OS through the `os_info` crate and format
/// the result the same way cargo formats its own `os:` line -
/// `<OS> <version> [<bitness>]`, e.g.  `Mac OS 26.4.1 [64-bit]`.
fn detect_os_string() -> String {
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
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> VersionInfo {
        VersionInfo::for_tests("x.y.z", "Ubuntu 24.04 [64-bit]")
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
}
