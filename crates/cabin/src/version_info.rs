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
//! Build-time metadata flows in through `option_env!` populated
//! by `build.rs`.  Runtime metadata (the OS identity) is probed
//! through the `os_info` crate, which inspects local platform
//! state without any network or filesystem access beyond a
//! `uname`-equivalent syscall.  Tests construct `VersionInfo`
//! directly through `VersionInfo::for_tests` so the formatter
//! can be exercised against controlled inputs without touching
//! the host environment.

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

/// Length of the abbreviated commit hash rendered in the header
/// line.  Matches the width cargo uses for the same field so
/// tooling that parses either banner sees the same shape.
const SHORT_COMMIT_LEN: usize = 9;

/// Typed snapshot of Cabin's version-relevant metadata.  The
/// struct is `Clone` so test helpers can compose fixtures
/// without re-deriving every field.
#[derive(Debug, Clone)]
pub(crate) struct VersionInfo {
    /// Always present — driven by the workspace's
    /// `[workspace.package] version` field.
    cabin_version: String,
    /// Full git commit hash captured at build time, or `None`
    /// when `.git` is unavailable (a published-tarball build).
    commit: Option<String>,
    /// ISO-8601 commit date (UTC), or `None`.
    commit_date: Option<String>,
    /// Host target triple (`aarch64-apple-darwin`, …), or `None`
    /// when the build script could not read `$TARGET`.
    host: Option<String>,
    /// Human-readable OS identity (`Mac OS 26.4.1 [64-bit]`,
    /// `Ubuntu 24.04 [64-bit]`, …) captured at runtime, or
    /// `None` when probing fails.
    os: Option<String>,
}

impl VersionInfo {
    /// Snapshot of the binary that is currently running.
    /// Build-time fields are captured by `build.rs`; the
    /// runtime OS string is probed once on demand.
    pub(crate) fn current() -> Self {
        Self {
            cabin_version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: option_env!("CABIN_BUILD_COMMIT").map(str::to_owned),
            commit_date: option_env!("CABIN_BUILD_COMMIT_DATE").map(str::to_owned),
            host: option_env!("CABIN_BUILD_HOST").map(str::to_owned),
            os: detect_os_string(),
        }
    }

    /// Build a [`VersionInfo`] from explicit fields.  Tests use
    /// this constructor to exercise the formatter against a
    /// controlled snapshot; production code calls
    /// [`VersionInfo::current`].
    #[cfg(test)]
    fn for_tests(
        cabin_version: &str,
        commit: Option<&str>,
        commit_date: Option<&str>,
        host: Option<&str>,
        os: Option<&str>,
    ) -> Self {
        Self {
            cabin_version: cabin_version.to_owned(),
            commit: commit.map(str::to_owned),
            commit_date: commit_date.map(str::to_owned),
            host: host.map(str::to_owned),
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

    /// Short hash prefix rendered in the verbose header.
    fn short_commit(&self) -> Option<&str> {
        self.commit
            .as_deref()
            .map(|hash| &hash[..hash.len().min(SHORT_COMMIT_LEN)])
    }

    fn format_verbose(&self) -> String {
        // Each labeled row contributes roughly `<label>:
        // <value>\n`; reserve a reasonable amount up-front to
        // keep the formatter free of intermediate allocations.
        let mut out = String::with_capacity(256);

        // Header: `cabin <ver>` plus an optional
        // `(<short-hash> <date>)` parenthetical when the build
        // captured both pieces of git metadata.  When either is
        // missing the parenthetical is omitted entirely so the
        // header stays unambiguous on tarball builds.
        match (self.short_commit(), self.commit_date.as_deref()) {
            (Some(short), Some(date)) => {
                let _ = writeln!(out, "cabin {} ({} {})", self.cabin_version, short, date);
            }
            _ => {
                let _ = writeln!(out, "cabin {}", self.cabin_version);
            }
        }

        let _ = writeln!(out, "release: {}", self.cabin_version);
        if let Some(hash) = self.commit.as_deref() {
            let _ = writeln!(out, "commit-hash: {hash}");
        }
        if let Some(date) = self.commit_date.as_deref() {
            let _ = writeln!(out, "commit-date: {date}");
        }
        if let Some(host) = self.host.as_deref() {
            let _ = writeln!(out, "host: {host}");
        }
        if let Some(os) = self.os.as_deref() {
            let _ = writeln!(out, "os: {os}");
        }
        out
    }
}

/// Probe the running OS through the `os_info` crate and format
/// the result the same way cargo formats its own `os:` line —
/// `<OS> <version> [<bitness>]`, e.g. `Mac OS 26.4.1 [64-bit]`.
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
    // `Unknown` — skip that case so the row reads cleanly on
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
        VersionInfo::for_tests(
            "0.14.0",
            Some("abc1234def56789a"),
            Some("2026-05-11"),
            Some("x86_64-unknown-linux-gnu"),
            Some("Ubuntu 24.04 [64-bit]"),
        )
    }

    fn minimal() -> VersionInfo {
        VersionInfo::for_tests("0.14.0", None, None, None, None)
    }

    #[test]
    fn concise_format_is_single_line_with_release_name() {
        let info = full();
        assert_eq!(info.format(VersionOutputMode::Concise), "cabin 0.14.0\n");
    }

    #[test]
    fn concise_format_works_with_minimal_metadata() {
        let info = minimal();
        // Concise output is independent of every optional field
        // — a published-tarball build still prints a clean line.
        assert_eq!(info.format(VersionOutputMode::Concise), "cabin 0.14.0\n");
    }

    #[test]
    fn verbose_format_header_includes_short_hash_and_commit_date() {
        let info = full();
        let out = info.format(VersionOutputMode::Verbose);
        let header = out.lines().next().expect("at least one line");
        // First nine hex chars of the captured hash plus the
        // commit date, parenthesized — matches cargo's header.
        assert_eq!(header, "cabin 0.14.0 (abc1234de 2026-05-11)");
    }

    #[test]
    fn verbose_format_drops_parenthetical_when_git_metadata_missing() {
        let info = VersionInfo::for_tests(
            "0.14.0",
            None,
            None,
            Some("x86_64-unknown-linux-gnu"),
            Some("Ubuntu 24.04 [64-bit]"),
        );
        let out = info.format(VersionOutputMode::Verbose);
        let header = out.lines().next().expect("at least one line");
        assert_eq!(header, "cabin 0.14.0");
    }

    #[test]
    fn verbose_format_emits_fields_in_cargo_order() {
        let info = full();
        let out = info.format(VersionOutputMode::Verbose);
        let expected = "\
cabin 0.14.0 (abc1234de 2026-05-11)
release: 0.14.0
commit-hash: abc1234def56789a
commit-date: 2026-05-11
host: x86_64-unknown-linux-gnu
os: Ubuntu 24.04 [64-bit]
";
        assert_eq!(out, expected);
    }

    #[test]
    fn verbose_format_omits_missing_optional_rows() {
        let info = minimal();
        let out = info.format(VersionOutputMode::Verbose);
        // Without git metadata, host, or os, only the header
        // and the `release:` line survive — there is no row to
        // print "unknown" in cargo's banner either.
        let expected = "\
cabin 0.14.0
release: 0.14.0
";
        assert_eq!(out, expected);
    }

    #[test]
    fn verbose_format_uses_short_labels_no_uppercase() {
        let info = full();
        for line in info.format(VersionOutputMode::Verbose).lines() {
            // The header line is unlabeled.  Every other line
            // has the form `<label>: <value>` with a lowercase
            // dashed label.
            if !line.contains(':') {
                continue;
            }
            let label = line.split(':').next().expect("label before colon");
            assert!(
                label.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "labels must be lowercase / dashed: got `{label}`"
            );
        }
    }

    #[test]
    fn current_uses_package_version_as_cabin_version() {
        // `VersionInfo::current` is environment-dependent, but
        // the crate version is always `env!("CARGO_PKG_VERSION")`
        // — assert that anchor without touching the optional
        // git / host / os fields.
        let info = VersionInfo::current();
        assert_eq!(info.cabin_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn verbose_format_never_leaks_local_paths() {
        // Defense-in-depth: even though no field captures a
        // path, run a structural check against the rendered
        // string so a future field cannot silently leak one in.
        let info = full();
        let out = info.format(VersionOutputMode::Verbose);
        for needle in ["/Users/", "/home/", "/private/", "/opt/", "/tmp/"] {
            assert!(
                !out.contains(needle),
                "verbose output must not leak `{needle}`: {out}"
            );
        }
    }
}
