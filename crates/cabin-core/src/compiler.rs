//! Typed compiler / tool identity and capability model.
//!
//! Cabin's build planner emits GCC/Clang-style commands. The
//! `ResolvedToolchain` (see [`crate::toolchain`]) says *which*
//! tools the user picked; this module says *what those tools are*
//! and *what they can do*. The
//! resolver in `cabin-toolchain::detect` runs harmless `--version`
//! invocations against each resolved tool, hands the output to the
//! pure parsers in this module, and assembles a typed
//! [`ToolchainDetectionReport`].
//!
//! This module is data and pure logic only. Process spawning,
//! filesystem traversal, and CLI dispatch live elsewhere.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Recognized C/C++ compiler family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompilerKind {
    /// LLVM Clang.
    Clang,
    /// Apple-shipped Clang (`Apple clang version …`). Treated as
    /// Clang-compatible for capability purposes; tracked separately
    /// for diagnostics.
    AppleClang,
    /// GNU GCC / `g++`.
    Gcc,
    /// Microsoft Visual C++ (`cl.exe`). Detected so Cabin can
    /// produce a clear unsupported-backend error; the GCC/Clang
    /// command pipeline cannot be used with this compiler.
    Msvc,
    /// Compiler whose `--version` output Cabin does not recognize.
    /// Capability detection treats this conservatively.
    Unknown,
}

impl CompilerKind {
    /// Stable lower-case identifier used in metadata output.
    pub fn as_key(self) -> &'static str {
        match self {
            CompilerKind::Clang => "clang",
            CompilerKind::AppleClang => "apple-clang",
            CompilerKind::Gcc => "gcc",
            CompilerKind::Msvc => "msvc",
            CompilerKind::Unknown => "unknown",
        }
    }

    /// Whether this compiler is part of the Clang family.
    pub fn is_clang_like(self) -> bool {
        matches!(self, CompilerKind::Clang | CompilerKind::AppleClang)
    }

    /// Whether this compiler accepts the GCC-style command line
    /// the current C++ backend emits (`-O<n>`, `-std=c++NN`,
    /// `-MMD -MF`, `-DNAME`, `-Idir`, …).
    pub fn supports_gcc_style_command_line(self) -> bool {
        matches!(
            self,
            CompilerKind::Clang | CompilerKind::AppleClang | CompilerKind::Gcc
        )
    }
}

impl fmt::Display for CompilerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

/// Recognized static-library archiver family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiverKind {
    /// GNU `ar` / BSD `ar`. Accepts the `crs` mode flags Cabin
    /// emits today.
    Ar,
    /// LLVM `llvm-ar`. Accepts the same `crs` mode flags.
    LlvmAr,
    /// Microsoft `lib.exe`. The MSVC dialect's archiver, driven as
    /// `lib /OUT:<lib> <objs>` to produce a `.lib` static library.
    Lib,
    /// Archiver whose `--version` output Cabin does not recognize.
    Unknown,
}

impl ArchiverKind {
    pub fn as_key(self) -> &'static str {
        match self {
            ArchiverKind::Ar => "ar",
            ArchiverKind::LlvmAr => "llvm-ar",
            ArchiverKind::Lib => "lib",
            ArchiverKind::Unknown => "unknown",
        }
    }

    /// Whether this archiver accepts the `crs` mode flags Cabin
    /// emits today.
    pub fn supports_ar_crs(self) -> bool {
        matches!(self, ArchiverKind::Ar | ArchiverKind::LlvmAr)
    }

    /// Whether this archiver can produce a static library in some
    /// dialect Cabin drives: GNU `ar` / `llvm-ar` via `ar crs`, or
    /// MSVC `lib.exe` via `lib /OUT:`. Distinct from
    /// [`Self::supports_ar_crs`], which is GNU-specific — `lib.exe`
    /// produces a static library but not via `crs` mode flags.
    pub fn produces_static_library(self) -> bool {
        matches!(
            self,
            ArchiverKind::Ar | ArchiverKind::LlvmAr | ArchiverKind::Lib
        )
    }
}

impl fmt::Display for ArchiverKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

/// Decomposed compiler / archiver version (`major.minor.patch`).
///
/// `major` is required; `minor` and `patch` are optional because
/// some versions only report two components. `raw` keeps the
/// original substring for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerVersion {
    pub major: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minor: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<u32>,
    pub raw: String,
}

impl CompilerVersion {
    /// Parse a `major[.minor[.patch]]` substring into a typed
    /// [`CompilerVersion`]. Returns `None` when the leading
    /// component is not a valid `u32`.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut parts = raw.split('.');
        let major: u32 = parts.next()?.parse().ok()?;
        let minor = parts.next().and_then(|s| s.parse().ok());
        let patch = parts.next().and_then(|s| s.parse().ok());
        Some(Self {
            major,
            minor,
            patch,
            raw: raw.to_owned(),
        })
    }

    /// Formatted `major.minor.patch` view, omitting unset
    /// components. Used in metadata JSON and `CABIN_*` env vars.
    pub fn to_display_string(&self) -> String {
        match (self.minor, self.patch) {
            (Some(min), Some(pat)) => format!("{}.{}.{}", self.major, min, pat),
            (Some(min), None) => format!("{}.{}", self.major, min),
            _ => self.major.to_string(),
        }
    }
}

impl fmt::Display for CompilerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_display_string())
    }
}

/// Detected identity of one C/C++ compiler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerIdentity {
    pub kind: CompilerKind,
    /// Parsed version, when the version-output line was
    /// recognized. `None` when the compiler emitted output Cabin
    /// could not parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<CompilerVersion>,
    /// Optional default target triple as the compiler reported it
    /// (the "Target: …" line from Clang, or an analogous GCC line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// First non-empty line of combined `--version` output, kept
    /// for diagnostics. Truncated to a sensible length.
    pub raw_version_line: String,
}

impl CompilerIdentity {
    /// Convenience: identity for an unknown / unparsable compiler.
    pub fn unknown(raw_version_line: impl Into<String>) -> Self {
        Self {
            kind: CompilerKind::Unknown,
            version: None,
            target: None,
            raw_version_line: raw_version_line.into(),
        }
    }

    /// Compact JSON view used by `cabin metadata`.
    pub fn as_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "kind".to_owned(),
            serde_json::Value::String(self.kind.as_key().to_owned()),
        );
        if let Some(v) = &self.version {
            obj.insert(
                "version".to_owned(),
                serde_json::Value::String(v.to_display_string()),
            );
        }
        if let Some(t) = &self.target {
            obj.insert("target".to_owned(), serde_json::Value::String(t.clone()));
        }
        obj.insert(
            "raw_version_line".to_owned(),
            serde_json::Value::String(self.raw_version_line.clone()),
        );
        serde_json::Value::Object(obj)
    }
}

/// Detected identity of a static-library archiver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiverIdentity {
    pub kind: ArchiverKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<CompilerVersion>,
    pub raw_version_line: String,
}

impl ArchiverIdentity {
    pub fn unknown(raw_version_line: impl Into<String>) -> Self {
        Self {
            kind: ArchiverKind::Unknown,
            version: None,
            raw_version_line: raw_version_line.into(),
        }
    }

    pub fn as_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "kind".to_owned(),
            serde_json::Value::String(self.kind.as_key().to_owned()),
        );
        if let Some(v) = &self.version {
            obj.insert(
                "version".to_owned(),
                serde_json::Value::String(v.to_display_string()),
            );
        }
        obj.insert(
            "raw_version_line".to_owned(),
            serde_json::Value::String(self.raw_version_line.clone()),
        );
        serde_json::Value::Object(obj)
    }
}

/// Where one capability decision came from. Recorded so
/// `cabin metadata` can show whether Cabin trusted the version
/// alone, ran a probe, or fell back to a conservative default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilitySource {
    /// Inferred from a recognized compiler kind/version.
    Version,
    /// Established by running a tightly-scoped probe command. Not
    /// currently used; reserved for a future probe-based source
    /// without changing the data model.
    Probe,
    /// Conservative default applied when the compiler kind is
    /// `Unknown` or detection failed.
    AssumedDefault,
    /// The selected tool is recognizably unable to provide this
    /// capability (e.g. MSVC asked for GCC-style flags).
    Unsupported,
}

impl CapabilitySource {
    pub fn as_key(self) -> &'static str {
        match self {
            CapabilitySource::Version => "version",
            CapabilitySource::Probe => "probe",
            CapabilitySource::AssumedDefault => "assumed-default",
            CapabilitySource::Unsupported => "unsupported",
        }
    }
}

/// One typed capability decision: whether the tool supports it,
/// and where the answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub supported: bool,
    pub source: CapabilitySource,
}

impl Capability {
    pub fn supported_from(source: CapabilitySource) -> Self {
        Self {
            supported: true,
            source,
        }
    }
    pub fn unsupported_from(source: CapabilitySource) -> Self {
        Self {
            supported: false,
            source,
        }
    }
}

/// Capability set for a C/C++ compiler. Every field is decided
/// during detection so the planner can compare its required set
/// against the resolved set without re-running parsing logic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerCapabilities {
    /// Accepts GCC-style `-O<n>`, `-DNAME`, `-Idir`, `-c`, `-o`.
    pub gcc_style_flags: Capability,
    /// Accepts MSVC-style `/O<n>`, `/DNAME`, `/I dir`. Detection-
    /// only; the current backend never emits these.
    pub msvc_style_flags: Capability,
    /// Accepts `-MMD -MF <file>` to write a make-style depfile.
    pub depfile_mmd_mf: Capability,
    /// Accepts `-std=c++NN`.
    pub std_flag: Capability,
    /// Accepts `-std=c++17` specifically (the planner's current
    /// fixed C++ standard).
    pub cxx_standard_17: Capability,
    /// Accepts `-std=c11` specifically (the planner's current fixed
    /// C standard). For MSVC this is the `/std:c11` switch, which is
    /// only available from VS2019 16.8 (`cl` 19.28) onward.
    pub c_standard_11: Capability,
    /// Accepts a color-diagnostics flag (e.g.
    /// `-fdiagnostics-color=always`). Detection-only today.
    pub color_diagnostics_flag: Capability,
    /// Accepts response-file argv (`@file`). Detection-only today.
    pub response_files: Capability,
    /// Accepts a JSON diagnostics flag (`-fdiagnostics-format=json`
    /// or equivalent). Detection-only; Cabin does not yet ask for
    /// JSON diagnostics.
    pub json_diagnostics: Capability,
    /// Accepts a SARIF diagnostics flag. Detection-only.
    pub sarif_diagnostics: Capability,
}

/// Capability set for a static-library archiver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiverCapabilities {
    /// Accepts the `crs` mode flags (the planner's archive form).
    pub ar_crs: Capability,
    /// Produces a `.a` static library archive.
    pub static_library_output: Capability,
}

/// Whole-toolchain detection report. The CLI builds one per
/// invocation that needs detection (build / metadata) and threads
/// it into the planner and the metadata view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainDetectionReport {
    pub cxx: ToolDetection<CompilerIdentity, CompilerCapabilities>,
    /// Optional because `ResolvedToolchain.cc` is itself optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<ToolDetection<CompilerIdentity, CompilerCapabilities>>,
    pub ar: ToolDetection<ArchiverIdentity, ArchiverCapabilities>,
}

impl ToolchainDetectionReport {
    /// Compact, deterministic JSON view used by `cabin metadata`
    /// and any tooling that wants to inspect detection results
    /// without re-deriving them. Each tool block carries
    /// `path` / `identity` / `capabilities`; absent tools (a
    /// missing C compiler) are omitted entirely so the JSON
    /// shape stays stable.
    pub fn as_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "cxx".to_owned(),
            serde_json::json!({
                "path": self.cxx.path.as_str().to_owned(),
                "identity": self.cxx.identity.as_json(),
                "capabilities": cxx_capabilities_as_json(&self.cxx.capabilities),
            }),
        );
        if let Some(cc) = &self.cc {
            obj.insert(
                "cc".to_owned(),
                serde_json::json!({
                    "path": cc.path.as_str().to_owned(),
                    "identity": cc.identity.as_json(),
                    "capabilities": cxx_capabilities_as_json(&cc.capabilities),
                }),
            );
        }
        obj.insert(
            "ar".to_owned(),
            serde_json::json!({
                "path": self.ar.path.as_str().to_owned(),
                "identity": self.ar.identity.as_json(),
                "capabilities": ar_capabilities_as_json(&self.ar.capabilities),
            }),
        );
        serde_json::Value::Object(obj)
    }
}

/// One tool's detection outcome plus the path it was invoked at.
/// `path` is the resolved absolute path from
/// [`crate::ResolvedToolchain`]; it is preserved here so error
/// messages can mention the exact executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDetection<I, C> {
    pub path: camino::Utf8PathBuf,
    pub identity: I,
    pub capabilities: C,
}

/// Pure parser for compiler `--version` output.
///
/// Recognizes the canonical first-line shapes Cabin cares about:
///
/// - `clang version 17.0.6 (...)`
/// - `Apple clang version 14.0.3 (clang-1403.0.22.14.1)`
/// - `g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0`
/// - `Microsoft (R) C/C++ Optimizing Compiler Version 19.39.x`
/// - any other first non-empty line → [`CompilerKind::Unknown`].
///
/// Also picks up the `Target: aarch64-apple-darwin` / similar
/// follow-up line when present so metadata can show the
/// compiler-reported target without running additional probes.
pub fn parse_cxx_version_output(text: &str) -> CompilerIdentity {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    let first_line = lines.first().copied().unwrap_or("").to_owned();

    let kind = detect_cxx_kind(&lines);
    let version = match kind {
        CompilerKind::Clang | CompilerKind::AppleClang => parse_clang_version(&lines),
        CompilerKind::Gcc => parse_gcc_version(&lines),
        CompilerKind::Msvc => parse_msvc_version(&lines),
        CompilerKind::Unknown => None,
    };
    let target = parse_target_line(&lines);

    CompilerIdentity {
        kind,
        version,
        target,
        raw_version_line: truncate(&first_line, 256),
    }
}

fn detect_cxx_kind(lines: &[&str]) -> CompilerKind {
    let joined = lines.join("\n");
    let lower = joined.to_ascii_lowercase();
    if lower.contains("apple clang") {
        return CompilerKind::AppleClang;
    }
    if lower.contains("clang version")
        || lower.contains("clang++")
        || lower.contains("openbsd clang")
    {
        return CompilerKind::Clang;
    }
    if lower.contains("microsoft (r)") || lower.contains("microsoft c/c++") {
        return CompilerKind::Msvc;
    }
    if lower.contains("free software foundation")
        || lower.starts_with("g++")
        || lower.starts_with("gcc")
        || lower.contains("gnu c++")
    {
        return CompilerKind::Gcc;
    }
    CompilerKind::Unknown
}

fn parse_clang_version(lines: &[&str]) -> Option<CompilerVersion> {
    let first = lines.first()?;
    let lower = first.to_ascii_lowercase();
    let needle = if lower.starts_with("apple clang") {
        "apple clang version "
    } else {
        "clang version "
    };
    let idx = lower.find(needle)?;
    let after = &first[idx + needle.len()..];
    let token = after
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(',');
    CompilerVersion::parse(token)
}

fn parse_gcc_version(lines: &[&str]) -> Option<CompilerVersion> {
    // GCC's first line typically looks like
    //   "g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0"
    // The version we care about is the last whitespace-delimited
    // token; some distros add a trailing copyright suffix on the
    // same line, so we accept the *last* dotted-numeric token.
    let first = lines.first()?;
    first
        .split_whitespace()
        .filter_map(|tok| {
            let trimmed = tok.trim_end_matches(',');
            CompilerVersion::parse(trimmed)
        })
        .next_back()
}

fn parse_msvc_version(lines: &[&str]) -> Option<CompilerVersion> {
    let joined = lines.join(" ");
    let lower = joined.to_ascii_lowercase();
    let idx = lower.find("version ")?;
    let after = &joined[idx + "version ".len()..];
    let token = after.split_whitespace().next().unwrap_or("");
    CompilerVersion::parse(token)
}

fn parse_target_line(lines: &[&str]) -> Option<String> {
    for line in lines {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Target:") {
            let v = rest.trim();
            if !v.is_empty() {
                return Some(v.to_owned());
            }
        }
    }
    None
}

/// Pure parser for archiver `--version` output. The recognized
/// families (`ar` and `llvm-ar`) print one line that includes the
/// family name. Anything else is classified as
/// [`ArchiverKind::Unknown`]; archivers that exit non-zero on
/// `--version` are left to the subprocess layer to surface as
/// `Unknown`.
pub fn parse_ar_version_output(text: &str) -> ArchiverIdentity {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    let first_line = lines.first().copied().unwrap_or("").to_owned();
    let lower = lines.join("\n").to_ascii_lowercase();

    let kind = if lower.contains("llvm version") || lower.contains("llvm-ar") {
        ArchiverKind::LlvmAr
    } else if lower.contains("gnu ar") || lower.contains("gnu binutils") || lower.starts_with("ar ")
    {
        ArchiverKind::Ar
    } else if lower.contains("microsoft (r) library manager") || lower.contains("lib.exe") {
        ArchiverKind::Lib
    } else {
        ArchiverKind::Unknown
    };

    let version = match kind {
        ArchiverKind::LlvmAr => parse_llvm_ar_version(&lines),
        ArchiverKind::Ar => parse_gnu_ar_version(&lines),
        ArchiverKind::Lib => parse_msvc_version(&lines),
        ArchiverKind::Unknown => None,
    };

    ArchiverIdentity {
        kind,
        version,
        raw_version_line: truncate(&first_line, 256),
    }
}

fn parse_gnu_ar_version(lines: &[&str]) -> Option<CompilerVersion> {
    // GNU ar prints e.g.
    //   "GNU ar (GNU Binutils for Debian) 2.40"
    let first = lines.first()?;
    first
        .split_whitespace()
        .filter_map(|tok| CompilerVersion::parse(tok.trim_end_matches(',')))
        .next_back()
}

fn parse_llvm_ar_version(lines: &[&str]) -> Option<CompilerVersion> {
    // llvm-ar emits multi-line output; somewhere is e.g.
    //   "LLVM version 17.0.6"
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(idx) = lower.find("llvm version ") {
            let after = &line[idx + "llvm version ".len()..];
            if let Some(token) = after.split_whitespace().next()
                && let Some(v) = CompilerVersion::parse(token)
            {
                return Some(v);
            }
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    s[..end].to_owned()
}

/// Derive a [`CompilerCapabilities`] set from the detected
/// identity. Decisions are made from the recognized compiler
/// kind, with conservative defaults for [`CompilerKind::Unknown`].
/// No probe commands are run from this function — the caller's
/// detection layer already gathered everything we need.
/// Decide a version-gated capability for an MSVC `cl` whose minimum
/// supporting version is `(min_major, min_minor)`. A parsed `cl`
/// version at or above the threshold is `supported`; below it,
/// `unsupported`. An unparsed version (`None`) is `supported` as an
/// assumed default — a real `cl` always reports a version, so a parse
/// miss must not reject an otherwise-modern compiler (mirrors the GCC
/// `cxx_standard_17` gate's `None` policy).
fn msvc_versioned_capability(
    version: Option<&CompilerVersion>,
    min_major: u32,
    min_minor: u32,
) -> Capability {
    match version.map(|v| (v.major, v.minor.unwrap_or(0))) {
        Some((major, minor)) if major > min_major || (major == min_major && minor >= min_minor) => {
            Capability::supported_from(CapabilitySource::Version)
        }
        Some(_) => Capability::unsupported_from(CapabilitySource::Version),
        None => Capability::supported_from(CapabilitySource::AssumedDefault),
    }
}

pub fn derive_cxx_capabilities(identity: &CompilerIdentity) -> CompilerCapabilities {
    let gcc_style = if identity.kind.supports_gcc_style_command_line() {
        Capability::supported_from(CapabilitySource::Version)
    } else if identity.kind == CompilerKind::Msvc {
        Capability::unsupported_from(CapabilitySource::Unsupported)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    let msvc_style = if identity.kind == CompilerKind::Msvc {
        Capability::supported_from(CapabilitySource::Version)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    let depfile_mmd_mf = if identity.kind.supports_gcc_style_command_line() {
        Capability::supported_from(CapabilitySource::Version)
    } else {
        Capability::unsupported_from(match identity.kind {
            CompilerKind::Msvc => CapabilitySource::Unsupported,
            _ => CapabilitySource::AssumedDefault,
        })
    };
    let std_flag = if identity.kind.supports_gcc_style_command_line() {
        Capability::supported_from(CapabilitySource::Version)
    } else {
        Capability::unsupported_from(match identity.kind {
            CompilerKind::Msvc => CapabilitySource::Unsupported,
            _ => CapabilitySource::AssumedDefault,
        })
    };
    // Every Clang we recognize (the version output starts with
    // `clang version` or `Apple clang version`) supports
    // `-std=c++17`. Same for any GCC modern enough to print a
    // major version: `g++ -std=c++17` was added in GCC 5.
    let cxx_standard_17 = match identity.kind {
        CompilerKind::Clang | CompilerKind::AppleClang => {
            Capability::supported_from(CapabilitySource::Version)
        }
        CompilerKind::Gcc => match identity.version.as_ref().map(|v| v.major) {
            Some(m) if m >= 5 => Capability::supported_from(CapabilitySource::Version),
            Some(_) => Capability::unsupported_from(CapabilitySource::Version),
            None => Capability::supported_from(CapabilitySource::AssumedDefault),
        },
        // `cl /std:c++17` is available from VS2017 15.3 (`cl` 19.11).
        CompilerKind::Msvc => msvc_versioned_capability(identity.version.as_ref(), 19, 11),
        CompilerKind::Unknown => Capability::unsupported_from(CapabilitySource::AssumedDefault),
    };
    // `-std=c11` has been a GCC flag since 4.7 and a Clang flag since
    // 3.1, so every recognized GCC/Clang supports it. MSVC's
    // `/std:c11` is newer: VS2019 16.8 (`cl` 19.28).
    let c_standard_11 = match identity.kind {
        CompilerKind::Clang | CompilerKind::AppleClang | CompilerKind::Gcc => {
            Capability::supported_from(CapabilitySource::Version)
        }
        CompilerKind::Msvc => msvc_versioned_capability(identity.version.as_ref(), 19, 28),
        CompilerKind::Unknown => Capability::unsupported_from(CapabilitySource::AssumedDefault),
    };
    let color = if identity.kind.is_clang_like() || identity.kind == CompilerKind::Gcc {
        Capability::supported_from(CapabilitySource::Version)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    let response_files = if identity.kind.is_clang_like() || identity.kind == CompilerKind::Gcc {
        Capability::supported_from(CapabilitySource::Version)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    let json_diagnostics = if identity.kind.is_clang_like() {
        Capability::supported_from(CapabilitySource::Version)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    // Cabin does not emit SARIF; report the capability as
    // unsupported regardless of detection so downstream tooling
    // never relies on a version-only inference here.
    let sarif_diagnostics = Capability::unsupported_from(CapabilitySource::AssumedDefault);

    CompilerCapabilities {
        gcc_style_flags: gcc_style,
        msvc_style_flags: msvc_style,
        depfile_mmd_mf,
        std_flag,
        cxx_standard_17,
        c_standard_11,
        color_diagnostics_flag: color,
        response_files,
        json_diagnostics,
        sarif_diagnostics,
    }
}

/// Derive an [`ArchiverCapabilities`] set from the detected
/// identity.
pub fn derive_ar_capabilities(identity: &ArchiverIdentity) -> ArchiverCapabilities {
    let ar_crs = if identity.kind.supports_ar_crs() {
        Capability::supported_from(CapabilitySource::Version)
    } else if identity.kind == ArchiverKind::Lib {
        Capability::unsupported_from(CapabilitySource::Unsupported)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    // Honest across both dialects: `ar` / `llvm-ar` archive via
    // `ar crs`, `lib.exe` via `lib /OUT:`. The `ar_crs` capability
    // above stays GNU-specific (`lib.exe` does not accept `crs`),
    // but both shapes do produce a static library.
    let static_library_output = if identity.kind.produces_static_library() {
        Capability::supported_from(CapabilitySource::Version)
    } else {
        Capability::unsupported_from(CapabilitySource::AssumedDefault)
    };
    ArchiverCapabilities {
        ar_crs,
        static_library_output,
    }
}

/// Errors produced while validating a detection report against
/// the current C++ backend's required capability set.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolDetectionError {
    #[error("selected C++ compiler `{spec}` cannot be matched to a supported C++ backend")]
    UnsupportedCxxBackend { spec: String },

    #[error(
        "selected C++ compiler `{spec}` could not be identified and the current backend requires GCC-style flags"
    )]
    UnknownCxxRequiresGccStyle { spec: String },

    #[error(
        "selected C++ compiler `{spec}` ({kind}) does not support the required C++17 standard flag"
    )]
    CxxLacksStdCxx17 { spec: String, kind: CompilerKind },

    #[error(
        "selected C++ compiler `{spec}` ({kind}) does not support the depfile flags required by the Ninja backend"
    )]
    CxxLacksDepfile { spec: String, kind: CompilerKind },

    #[error("selected C compiler `{spec}` cannot be matched to a supported C backend")]
    UnsupportedCBackend { spec: String },

    #[error(
        "selected C compiler `{spec}` could not be identified and the current backend requires GCC-style flags"
    )]
    UnknownCRequiresGccStyle { spec: String },

    #[error(
        "selected C compiler `{spec}` ({kind}) does not support the depfile flags required by the Ninja backend"
    )]
    CLacksDepfile { spec: String, kind: CompilerKind },

    #[error(
        "selected C compiler `{spec}` ({kind}) does not support the required C11 standard flag (MSVC `/std:c11` needs VS2019 16.8 / `cl` 19.28 or newer)"
    )]
    CLacksStdC11 { spec: String, kind: CompilerKind },

    #[error("selected archiver `{spec}` is not supported by the static-library backend")]
    UnsupportedArchiver { spec: String },

    #[error(
        "selected archiver `{spec}` could not be identified and the current backend requires `ar crs`-compatible behavior"
    )]
    UnknownArchiverRequiresArCompatible { spec: String },
}

/// Validate that the resolved C++ compiler can drive one of
/// Cabin's two C++ backends.
///
/// An MSVC compiler drives the `cl.exe` backend, which speaks the
/// MSVC command-line dialect (`/std:c++17`, `/showIncludes`,
/// `/D` / `/I` / `/c` / `/Fo`). Every other recognized compiler
/// drives the GCC/Clang backend, which requires `-std=c++17`,
/// `-MMD -MF`, and GCC-style `-D` / `-I` / `-c` / `-o`. A
/// compiler that fits neither contract is a hard error.
///
/// # Errors
/// Returns [`ToolDetectionError::UnsupportedCxxBackend`] when the compiler fits
/// no backend, [`ToolDetectionError::UnknownCxxRequiresGccStyle`] when an
/// unidentified compiler lacks GCC-style flags,
/// [`ToolDetectionError::CxxLacksDepfile`] when `-MMD -MF` is unsupported, and
/// [`ToolDetectionError::CxxLacksStdCxx17`] when `-std=c++17` is unsupported.
pub fn validate_cxx_for_backend(
    spec_display: &str,
    identity: &CompilerIdentity,
    capabilities: &CompilerCapabilities,
) -> Result<(), ToolDetectionError> {
    // MSVC drives the `cl.exe` backend. A detected `cl` always
    // reports `msvc_style_flags`, but the planner also emits
    // `/std:c++17`, which a `cl` older than VS2017 15.3 rejects, so
    // hold MSVC to the C++17 capability too rather than letting an
    // old toolset fail at the first compile.
    if identity.kind == CompilerKind::Msvc {
        if !capabilities.msvc_style_flags.supported {
            return Err(ToolDetectionError::UnsupportedCxxBackend {
                spec: spec_display.to_owned(),
            });
        }
        if !capabilities.cxx_standard_17.supported {
            return Err(ToolDetectionError::CxxLacksStdCxx17 {
                spec: spec_display.to_owned(),
                kind: identity.kind,
            });
        }
        return Ok(());
    }
    if !capabilities.gcc_style_flags.supported {
        if identity.kind == CompilerKind::Unknown {
            return Err(ToolDetectionError::UnknownCxxRequiresGccStyle {
                spec: spec_display.to_owned(),
            });
        }
        return Err(ToolDetectionError::UnsupportedCxxBackend {
            spec: spec_display.to_owned(),
        });
    }
    if !capabilities.depfile_mmd_mf.supported {
        return Err(ToolDetectionError::CxxLacksDepfile {
            spec: spec_display.to_owned(),
            kind: identity.kind,
        });
    }
    if !capabilities.cxx_standard_17.supported {
        return Err(ToolDetectionError::CxxLacksStdCxx17 {
            spec: spec_display.to_owned(),
            kind: identity.kind,
        });
    }
    Ok(())
}

/// Validate that the resolved C compiler supports the C-side
/// command shape the active backend emits. An MSVC compiler
/// drives the `cl.exe` backend; every other recognized compiler
/// drives the GCC/Clang backend, which needs GCC-style flags
/// plus `-MMD -MF` depfile generation. Unlike
/// [`validate_cxx_for_backend`], the GCC/Clang path does **not**
/// require `-std=c++17` support — a pure-C driver that lacks
/// C++ mode is acceptable when the target only carries C
/// translation units.
///
/// # Errors
/// Returns [`ToolDetectionError::UnsupportedCBackend`] when the compiler fits
/// no backend, [`ToolDetectionError::UnknownCRequiresGccStyle`] when an
/// unidentified compiler lacks GCC-style flags, and
/// [`ToolDetectionError::CLacksDepfile`] when `-MMD -MF` is unsupported.
pub fn validate_cc_for_backend(
    spec_display: &str,
    identity: &CompilerIdentity,
    capabilities: &CompilerCapabilities,
) -> Result<(), ToolDetectionError> {
    // MSVC drives the `cl.exe` backend; the GCC/Clang contract
    // below does not apply to it. The planner emits `/std:c11` for C
    // compiles, which a `cl` older than VS2019 16.8 rejects, so hold
    // MSVC to the C11 capability rather than failing at the first
    // compile.
    if identity.kind == CompilerKind::Msvc {
        if !capabilities.msvc_style_flags.supported {
            return Err(ToolDetectionError::UnsupportedCBackend {
                spec: spec_display.to_owned(),
            });
        }
        if !capabilities.c_standard_11.supported {
            return Err(ToolDetectionError::CLacksStdC11 {
                spec: spec_display.to_owned(),
                kind: identity.kind,
            });
        }
        return Ok(());
    }
    if !capabilities.gcc_style_flags.supported {
        if identity.kind == CompilerKind::Unknown {
            return Err(ToolDetectionError::UnknownCRequiresGccStyle {
                spec: spec_display.to_owned(),
            });
        }
        return Err(ToolDetectionError::UnsupportedCBackend {
            spec: spec_display.to_owned(),
        });
    }
    if !capabilities.depfile_mmd_mf.supported {
        return Err(ToolDetectionError::CLacksDepfile {
            spec: spec_display.to_owned(),
            kind: identity.kind,
        });
    }
    Ok(())
}

/// Validate that the resolved archiver can drive one of Cabin's
/// two static-library backends: `lib.exe` for MSVC
/// (`lib /OUT:<lib> <objs>`), or an `ar`-compatible archiver for
/// GCC/Clang (`ar crs <lib> <objs>`).
///
/// # Errors
/// Returns [`ToolDetectionError::UnsupportedArchiver`] when a known archiver
/// lacks `ar crs` support, and
/// [`ToolDetectionError::UnknownArchiverRequiresArCompatible`] when an
/// unidentified archiver lacks `ar crs` support.
pub fn validate_ar_for_backend(
    spec_display: &str,
    identity: &ArchiverIdentity,
    capabilities: &ArchiverCapabilities,
) -> Result<(), ToolDetectionError> {
    // `lib.exe` is the MSVC static-library backend's archiver; it
    // produces the `.lib` the `cl.exe` link step consumes.
    if identity.kind == ArchiverKind::Lib {
        return Ok(());
    }
    if !capabilities.ar_crs.supported {
        if identity.kind == ArchiverKind::Unknown {
            return Err(ToolDetectionError::UnknownArchiverRequiresArCompatible {
                spec: spec_display.to_owned(),
            });
        }
        return Err(ToolDetectionError::UnsupportedArchiver {
            spec: spec_display.to_owned(),
        });
    }
    Ok(())
}

/// Render a [`CompilerCapabilities`] as a deterministic JSON map
/// keyed by the public capability name, in alphabetical order.
pub(crate) fn cxx_capabilities_as_json(caps: &CompilerCapabilities) -> serde_json::Value {
    // Exhaustive destructure (no `..`) so adding a capability field
    // is a compile error here until it is wired into the JSON, rather
    // than being silently dropped from `cabin metadata`.
    let CompilerCapabilities {
        gcc_style_flags,
        msvc_style_flags,
        depfile_mmd_mf,
        std_flag,
        cxx_standard_17,
        c_standard_11,
        color_diagnostics_flag,
        response_files,
        json_diagnostics,
        sarif_diagnostics,
    } = caps;
    let mut entries: [(&'static str, &Capability); 10] = [
        ("gcc_style_flags", gcc_style_flags),
        ("msvc_style_flags", msvc_style_flags),
        ("depfile_mmd_mf", depfile_mmd_mf),
        ("std_flag", std_flag),
        ("cxx_standard_17", cxx_standard_17),
        ("c_standard_11", c_standard_11),
        ("color_diagnostics_flag", color_diagnostics_flag),
        ("response_files", response_files),
        ("json_diagnostics", json_diagnostics),
        ("sarif_diagnostics", sarif_diagnostics),
    ];
    capabilities_to_json(&mut entries)
}

pub(crate) fn ar_capabilities_as_json(caps: &ArchiverCapabilities) -> serde_json::Value {
    let ArchiverCapabilities {
        ar_crs,
        static_library_output,
    } = caps;
    let mut entries: [(&'static str, &Capability); 2] = [
        ("ar_crs", ar_crs),
        ("static_library_output", static_library_output),
    ];
    capabilities_to_json(&mut entries)
}

/// Render `(key, capability)` pairs into an alphabetically-keyed JSON
/// object — `{ "<key>": { "supported": <bool>, "source": <kebab> } }`.
/// Sorting here keeps the output independent of the caller's field
/// order, matching the historical BTreeSet-keyed rendering.
fn capabilities_to_json(entries: &mut [(&'static str, &Capability)]) -> serde_json::Value {
    entries.sort_by_key(|(key, _)| *key);
    let mut obj = serde_json::Map::new();
    for (key, cap) in entries {
        obj.insert(
            (*key).to_owned(),
            serde_json::json!({
                "supported": cap.supported,
                "source": cap.source.as_key(),
            }),
        );
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clang_first_line() {
        let id = parse_cxx_version_output(
            "clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\nThread model: posix\n",
        );
        assert_eq!(id.kind, CompilerKind::Clang);
        let v = id.version.expect("version parsed");
        assert_eq!(v.major, 17);
        assert_eq!(v.minor, Some(0));
        assert_eq!(v.patch, Some(6));
        assert_eq!(id.target.as_deref(), Some("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn parses_apple_clang() {
        let id = parse_cxx_version_output(
            "Apple clang version 14.0.3 (clang-1403.0.22.14.1)\nTarget: arm64-apple-darwin22.5.0\nThread model: posix\n",
        );
        assert_eq!(id.kind, CompilerKind::AppleClang);
        let v = id.version.unwrap();
        assert_eq!((v.major, v.minor, v.patch), (14, Some(0), Some(3)));
    }

    #[test]
    fn parses_gcc_with_distro_prefix() {
        let id = parse_cxx_version_output(
            "g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0\nCopyright (C) 2021 Free Software Foundation, Inc.\n",
        );
        assert_eq!(id.kind, CompilerKind::Gcc);
        let v = id.version.unwrap();
        assert_eq!((v.major, v.minor, v.patch), (11, Some(4), Some(0)));
    }

    #[test]
    fn parses_msvc_first_line() {
        let id = parse_cxx_version_output(
            "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.33523 for x64\n",
        );
        assert_eq!(id.kind, CompilerKind::Msvc);
        let v = id.version.unwrap();
        assert_eq!(v.major, 19);
    }

    #[test]
    fn unknown_when_unrecognized() {
        let id = parse_cxx_version_output("My funky compiler 0.0\n");
        assert_eq!(id.kind, CompilerKind::Unknown);
        assert!(id.version.is_none());
    }

    #[test]
    fn empty_output_is_unknown() {
        let id = parse_cxx_version_output("");
        assert_eq!(id.kind, CompilerKind::Unknown);
        assert!(id.raw_version_line.is_empty());
    }

    #[test]
    fn parses_gnu_ar() {
        let id = parse_ar_version_output(
            "GNU ar (GNU Binutils for Debian) 2.40\nCopyright (C) 2023 Free Software Foundation, Inc.\n",
        );
        assert_eq!(id.kind, ArchiverKind::Ar);
        let v = id.version.unwrap();
        assert_eq!(v.major, 2);
    }

    #[test]
    fn parses_llvm_ar_version() {
        let id = parse_ar_version_output(
            "LLVM (http://llvm.org/):\n  LLVM version 17.0.6\n  Optimized build.\n",
        );
        assert_eq!(id.kind, ArchiverKind::LlvmAr);
        let v = id.version.unwrap();
        assert_eq!(v.major, 17);
    }

    #[test]
    fn detects_lib_exe_as_unsupported() {
        let id = parse_ar_version_output(
            "Microsoft (R) Library Manager Version 14.39.33523.0\nCopyright (C) Microsoft Corporation.\n",
        );
        assert_eq!(id.kind, ArchiverKind::Lib);
    }

    #[test]
    fn unknown_archiver_classification() {
        let id = parse_ar_version_output("just-some-archiver 0.1\n");
        assert_eq!(id.kind, ArchiverKind::Unknown);
        assert!(id.version.is_none());
    }

    #[test]
    fn clang_capabilities_include_gcc_style_and_cxx17() {
        let id = CompilerIdentity {
            kind: CompilerKind::Clang,
            version: CompilerVersion::parse("17.0.6"),
            target: None,
            raw_version_line: "clang version 17.0.6".into(),
        };
        let caps = derive_cxx_capabilities(&id);
        assert!(caps.gcc_style_flags.supported);
        assert!(caps.depfile_mmd_mf.supported);
        assert!(caps.std_flag.supported);
        assert!(caps.cxx_standard_17.supported);
    }

    #[test]
    fn gcc_pre_5_does_not_claim_cxx17() {
        let id = CompilerIdentity {
            kind: CompilerKind::Gcc,
            version: CompilerVersion::parse("4.8.5"),
            target: None,
            raw_version_line: "g++ 4.8.5".into(),
        };
        let caps = derive_cxx_capabilities(&id);
        assert!(caps.gcc_style_flags.supported);
        assert!(!caps.cxx_standard_17.supported);
    }

    #[test]
    fn msvc_capabilities_reject_gcc_style() {
        let id = CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: CompilerVersion::parse("19.39.0"),
            target: None,
            raw_version_line: "Microsoft Optimizing Compiler".into(),
        };
        let caps = derive_cxx_capabilities(&id);
        assert!(!caps.gcc_style_flags.supported);
        assert_eq!(caps.gcc_style_flags.source, CapabilitySource::Unsupported);
        assert!(caps.msvc_style_flags.supported);
    }

    #[test]
    fn unknown_compiler_capabilities_are_conservative() {
        let id = CompilerIdentity::unknown("strange compiler");
        let caps = derive_cxx_capabilities(&id);
        assert!(!caps.gcc_style_flags.supported);
        assert_eq!(
            caps.gcc_style_flags.source,
            CapabilitySource::AssumedDefault
        );
        assert!(!caps.depfile_mmd_mf.supported);
    }

    #[test]
    fn ar_capabilities_recognize_gnu_ar() {
        let id = ArchiverIdentity {
            kind: ArchiverKind::Ar,
            version: CompilerVersion::parse("2.40"),
            raw_version_line: "GNU ar".into(),
        };
        let caps = derive_ar_capabilities(&id);
        assert!(caps.ar_crs.supported);
        assert!(caps.static_library_output.supported);
    }

    #[test]
    fn msvc_lib_archives_without_ar_crs() {
        // `lib.exe` does not accept GNU `crs` mode flags, but it
        // does produce a static library (`lib /OUT:`), so metadata
        // must report `static_library_output` as supported.
        let id = ArchiverIdentity {
            kind: ArchiverKind::Lib,
            version: None,
            raw_version_line: "Microsoft Library Manager".into(),
        };
        let caps = derive_ar_capabilities(&id);
        assert!(!caps.ar_crs.supported);
        assert_eq!(caps.ar_crs.source, CapabilitySource::Unsupported);
        assert!(caps.static_library_output.supported);
    }

    #[test]
    fn validate_accepts_msvc_cxx() {
        // MSVC drives the `cl.exe` backend; detection reports
        // `msvc_style_flags`, so validation must accept it.
        let id = CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: None,
            target: None,
            raw_version_line: "MSVC".into(),
        };
        let caps = derive_cxx_capabilities(&id);
        assert!(caps.msvc_style_flags.supported);
        assert!(validate_cxx_for_backend("cl.exe", &id, &caps).is_ok());
    }

    fn msvc_identity(version: &str) -> CompilerIdentity {
        CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: CompilerVersion::parse(version),
            target: None,
            raw_version_line: format!("Microsoft Optimizing Compiler {version}"),
        }
    }

    #[test]
    fn msvc_std_capabilities_are_version_gated() {
        // `/std:c++17` needs cl 19.11 (VS2017 15.3); `/std:c11` needs
        // cl 19.28 (VS2019 16.8).
        let modern = derive_cxx_capabilities(&msvc_identity("19.39.33523"));
        assert!(modern.cxx_standard_17.supported);
        assert_eq!(modern.cxx_standard_17.source, CapabilitySource::Version);
        assert!(modern.c_standard_11.supported);
        assert_eq!(modern.c_standard_11.source, CapabilitySource::Version);

        // cl 19.20 (VS2019 16.0) takes /std:c++17 but predates /std:c11.
        let mid = derive_cxx_capabilities(&msvc_identity("19.20.0"));
        assert!(mid.cxx_standard_17.supported);
        assert!(!mid.c_standard_11.supported);
        assert_eq!(mid.c_standard_11.source, CapabilitySource::Version);

        // cl 19.00 (VS2015) predates both switches.
        let old = derive_cxx_capabilities(&msvc_identity("19.00.24210"));
        assert!(!old.cxx_standard_17.supported);
        assert!(!old.c_standard_11.supported);
    }

    #[test]
    fn msvc_unparsed_version_assumes_modern_support() {
        // A real `cl` always reports a version; a parse miss
        // (`version: None`) must NOT reject an otherwise-modern
        // compiler, so the gate defaults to supported/assumed-default.
        let caps = derive_cxx_capabilities(&CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: None,
            target: None,
            raw_version_line: "Microsoft Optimizing Compiler".into(),
        });
        assert!(caps.cxx_standard_17.supported);
        assert_eq!(
            caps.cxx_standard_17.source,
            CapabilitySource::AssumedDefault
        );
        assert!(caps.c_standard_11.supported);
        assert_eq!(caps.c_standard_11.source, CapabilitySource::AssumedDefault);
    }

    #[test]
    fn gnu_c_standard_11_is_unconditional() {
        // `-std=c11` has been available far longer than `-std=c++17`,
        // so every recognized GCC/Clang reports it regardless of major.
        for id in [
            CompilerIdentity {
                kind: CompilerKind::Gcc,
                version: CompilerVersion::parse("4.8.5"),
                target: None,
                raw_version_line: "g++ 4.8.5".into(),
            },
            CompilerIdentity {
                kind: CompilerKind::Clang,
                version: CompilerVersion::parse("3.4"),
                target: None,
                raw_version_line: "clang version 3.4".into(),
            },
        ] {
            assert!(derive_cxx_capabilities(&id).c_standard_11.supported);
        }
    }

    #[test]
    fn validate_rejects_msvc_too_old_for_std_flags() {
        let old = msvc_identity("19.00.24210");
        let caps = derive_cxx_capabilities(&old);
        // C++ build: rejected for lacking /std:c++17.
        assert!(matches!(
            validate_cxx_for_backend("cl.exe", &old, &caps),
            Err(ToolDetectionError::CxxLacksStdCxx17 { .. })
        ));
        // C build: rejected for lacking /std:c11.
        assert!(matches!(
            validate_cc_for_backend("cl.exe", &old, &caps),
            Err(ToolDetectionError::CLacksStdC11 { .. })
        ));
    }

    #[test]
    fn validate_accepts_modern_and_unversioned_msvc_c() {
        for id in [
            msvc_identity("19.39.33523"),
            CompilerIdentity {
                kind: CompilerKind::Msvc,
                version: None,
                target: None,
                raw_version_line: "MSVC".into(),
            },
        ] {
            let caps = derive_cxx_capabilities(&id);
            assert!(validate_cc_for_backend("cl.exe", &id, &caps).is_ok());
        }
    }

    #[test]
    fn validate_rejects_unknown_cxx() {
        let id = CompilerIdentity::unknown("???");
        let caps = derive_cxx_capabilities(&id);
        let err = validate_cxx_for_backend("custom-cxx", &id, &caps).unwrap_err();
        assert!(matches!(
            err,
            ToolDetectionError::UnknownCxxRequiresGccStyle { .. }
        ));
    }

    #[test]
    fn validate_accepts_clang() {
        let id = CompilerIdentity {
            kind: CompilerKind::Clang,
            version: CompilerVersion::parse("17.0.6"),
            target: None,
            raw_version_line: "clang version 17.0.6".into(),
        };
        let caps = derive_cxx_capabilities(&id);
        assert!(validate_cxx_for_backend("clang++", &id, &caps).is_ok());
    }

    #[test]
    fn validate_rejects_gcc_too_old_for_cxx17() {
        let id = CompilerIdentity {
            kind: CompilerKind::Gcc,
            version: CompilerVersion::parse("4.8.5"),
            target: None,
            raw_version_line: "g++ 4.8".into(),
        };
        let caps = derive_cxx_capabilities(&id);
        let err = validate_cxx_for_backend("g++", &id, &caps).unwrap_err();
        assert!(matches!(err, ToolDetectionError::CxxLacksStdCxx17 { .. }));
    }

    #[test]
    fn validate_cc_accepts_pure_c_clang_without_cxx17_capability() {
        // The C-side validator must accept a compiler that
        // would *not* satisfy the C++ contract (no
        // `cxx_standard_17`). A bare `cc` driver on a system
        // that ships only C headers is a legitimate case; only
        // GCC-style flags + depfile are required for the C
        // backend.
        let id = CompilerIdentity {
            kind: CompilerKind::Clang,
            version: CompilerVersion::parse("17.0.6"),
            target: None,
            raw_version_line: "clang version 17.0.6".into(),
        };
        let mut caps = derive_cxx_capabilities(&id);
        // Force `cxx_standard_17` off so we can be certain the
        // C validator does not gate on it.
        caps.cxx_standard_17 = Capability {
            supported: false,
            source: CapabilitySource::Unsupported,
        };
        assert!(validate_cc_for_backend("cc", &id, &caps).is_ok());
        // Sanity: the equivalent CXX validation would now reject
        // the same compiler. Asserting both directions
        // documents the design constraint that C/C++
        // capability gating differ.
        assert!(matches!(
            validate_cxx_for_backend("cc", &id, &caps).unwrap_err(),
            ToolDetectionError::CxxLacksStdCxx17 { .. }
        ));
    }

    #[test]
    fn validate_cc_accepts_msvc() {
        let id = CompilerIdentity {
            kind: CompilerKind::Msvc,
            version: None,
            target: None,
            raw_version_line: "MSVC".into(),
        };
        let caps = derive_cxx_capabilities(&id);
        assert!(validate_cc_for_backend("cl.exe", &id, &caps).is_ok());
    }

    #[test]
    fn validate_cc_rejects_unknown_compiler_without_gcc_style() {
        // Unknown identity + missing `gcc_style_flags` capability
        // is the unrecoverable case: the planner cannot tell
        // whether the compiler accepts `-c -o` etc.
        let id = CompilerIdentity::unknown("???");
        let caps = derive_cxx_capabilities(&id);
        let err = validate_cc_for_backend("custom-cc", &id, &caps).unwrap_err();
        assert!(matches!(
            err,
            ToolDetectionError::UnknownCRequiresGccStyle { .. }
        ));
    }

    #[test]
    fn validate_cc_rejects_gcc_without_depfile_support() {
        // GCC identity but without `-MMD -MF` support — Cabin
        // emits a depfile flag for every compile so the C
        // contract requires it, even though `cxx_standard_17`
        // is not relevant.
        let id = CompilerIdentity {
            kind: CompilerKind::Gcc,
            version: CompilerVersion::parse("9.4.0"),
            target: None,
            raw_version_line: "gcc 9.4".into(),
        };
        let mut caps = derive_cxx_capabilities(&id);
        caps.depfile_mmd_mf = Capability {
            supported: false,
            source: CapabilitySource::Unsupported,
        };
        let err = validate_cc_for_backend("cc", &id, &caps).unwrap_err();
        assert!(matches!(err, ToolDetectionError::CLacksDepfile { .. }));
    }

    #[test]
    fn validate_accepts_msvc_archiver() {
        // `lib.exe` is the MSVC static-library archiver.
        let id = ArchiverIdentity {
            kind: ArchiverKind::Lib,
            version: None,
            raw_version_line: "Microsoft Library Manager".into(),
        };
        let caps = derive_ar_capabilities(&id);
        assert!(validate_ar_for_backend("lib.exe", &id, &caps).is_ok());
    }

    #[test]
    fn version_display_truncates_unset_components() {
        let v = CompilerVersion::parse("11").unwrap();
        assert_eq!(v.to_display_string(), "11");
        let v = CompilerVersion::parse("11.4").unwrap();
        assert_eq!(v.to_display_string(), "11.4");
        let v = CompilerVersion::parse("11.4.0").unwrap();
        assert_eq!(v.to_display_string(), "11.4.0");
    }

    // --------------------------------------------------------------
    // Golden / fixture tests.
    //
    // These pin the JSON shape that downstream tooling
    // (`cabin metadata`, IDE integrations) reads out of a
    // `ToolchainDetectionReport`. Any accidental change to the
    // field names or serialization order here is user-visible
    // and should be deliberate.
    // --------------------------------------------------------------

    fn pretty(value: &serde_json::Value) -> String {
        serde_json::to_string_pretty(value).unwrap()
    }

    fn cxx_identity_and_capabilities_json(version_output: &str) -> String {
        let id = parse_cxx_version_output(version_output);
        let caps = derive_cxx_capabilities(&id);
        pretty(&serde_json::json!({
            "identity": id.as_json(),
            "capabilities": cxx_capabilities_as_json(&caps),
        }))
    }

    fn ar_identity_and_capabilities_json(version_output: &str) -> String {
        let id = parse_ar_version_output(version_output);
        let caps = derive_ar_capabilities(&id);
        pretty(&serde_json::json!({
            "identity": id.as_json(),
            "capabilities": ar_capabilities_as_json(&caps),
        }))
    }

    #[test]
    fn snapshot_clang_identity_and_capabilities() {
        let actual = cxx_identity_and_capabilities_json(
            "clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\nThread model: posix\n",
        );
        let expected = r#"{
  "identity": {
    "kind": "clang",
    "version": "17.0.6",
    "target": "x86_64-unknown-linux-gnu",
    "raw_version_line": "clang version 17.0.6"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": true,
      "source": "version"
    },
    "color_diagnostics_flag": {
      "supported": true,
      "source": "version"
    },
    "cxx_standard_17": {
      "supported": true,
      "source": "version"
    },
    "depfile_mmd_mf": {
      "supported": true,
      "source": "version"
    },
    "gcc_style_flags": {
      "supported": true,
      "source": "version"
    },
    "json_diagnostics": {
      "supported": true,
      "source": "version"
    },
    "msvc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "response_files": {
      "supported": true,
      "source": "version"
    },
    "sarif_diagnostics": {
      "supported": false,
      "source": "assumed-default"
    },
    "std_flag": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
        assert_eq!(actual, expected);
    }

    #[test]
    fn snapshot_apple_clang_identity_and_capabilities() {
        let actual = cxx_identity_and_capabilities_json(
            "Apple clang version 14.0.3 (clang-1403.0.22.14.1)\nTarget: arm64-apple-darwin22.5.0\nThread model: posix\n",
        );
        let expected = r#"{
  "identity": {
    "kind": "apple-clang",
    "version": "14.0.3",
    "target": "arm64-apple-darwin22.5.0",
    "raw_version_line": "Apple clang version 14.0.3 (clang-1403.0.22.14.1)"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": true,
      "source": "version"
    },
    "color_diagnostics_flag": {
      "supported": true,
      "source": "version"
    },
    "cxx_standard_17": {
      "supported": true,
      "source": "version"
    },
    "depfile_mmd_mf": {
      "supported": true,
      "source": "version"
    },
    "gcc_style_flags": {
      "supported": true,
      "source": "version"
    },
    "json_diagnostics": {
      "supported": true,
      "source": "version"
    },
    "msvc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "response_files": {
      "supported": true,
      "source": "version"
    },
    "sarif_diagnostics": {
      "supported": false,
      "source": "assumed-default"
    },
    "std_flag": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
        assert_eq!(actual, expected);
    }

    #[test]
    fn snapshot_gcc_identity_and_capabilities() {
        let actual = cxx_identity_and_capabilities_json(
            "g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0\nCopyright (C) 2021 Free Software Foundation, Inc.\n",
        );
        let expected = r#"{
  "identity": {
    "kind": "gcc",
    "version": "11.4.0",
    "raw_version_line": "g++ (Ubuntu 11.4.0-1ubuntu1) 11.4.0"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": true,
      "source": "version"
    },
    "color_diagnostics_flag": {
      "supported": true,
      "source": "version"
    },
    "cxx_standard_17": {
      "supported": true,
      "source": "version"
    },
    "depfile_mmd_mf": {
      "supported": true,
      "source": "version"
    },
    "gcc_style_flags": {
      "supported": true,
      "source": "version"
    },
    "json_diagnostics": {
      "supported": false,
      "source": "assumed-default"
    },
    "msvc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "response_files": {
      "supported": true,
      "source": "version"
    },
    "sarif_diagnostics": {
      "supported": false,
      "source": "assumed-default"
    },
    "std_flag": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
        assert_eq!(actual, expected);
    }

    #[test]
    fn snapshot_msvc_identity_and_capabilities() {
        let actual = cxx_identity_and_capabilities_json(
            "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.33523 for x64\n",
        );
        // A modern `cl` (19.39 == VS2022 17.9) accepts the
        // `/std:c++17` and `/std:c11` switches Cabin emits, so both
        // standard capabilities are version-supported; the GCC-style
        // and depfile capabilities stay unsupported because MSVC
        // drives its own dialect.
        let expected = r#"{
  "identity": {
    "kind": "msvc",
    "version": "19.39.33523",
    "raw_version_line": "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.33523 for x64"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": true,
      "source": "version"
    },
    "color_diagnostics_flag": {
      "supported": false,
      "source": "assumed-default"
    },
    "cxx_standard_17": {
      "supported": true,
      "source": "version"
    },
    "depfile_mmd_mf": {
      "supported": false,
      "source": "unsupported"
    },
    "gcc_style_flags": {
      "supported": false,
      "source": "unsupported"
    },
    "json_diagnostics": {
      "supported": false,
      "source": "assumed-default"
    },
    "msvc_style_flags": {
      "supported": true,
      "source": "version"
    },
    "response_files": {
      "supported": false,
      "source": "assumed-default"
    },
    "sarif_diagnostics": {
      "supported": false,
      "source": "assumed-default"
    },
    "std_flag": {
      "supported": false,
      "source": "unsupported"
    }
  }
}"#;
        assert_eq!(actual, expected);
    }

    #[test]
    fn snapshot_unknown_compiler_capabilities_are_conservative() {
        let actual = cxx_identity_and_capabilities_json("My funky compiler 0.0\n");
        let expected = r#"{
  "identity": {
    "kind": "unknown",
    "raw_version_line": "My funky compiler 0.0"
  },
  "capabilities": {
    "c_standard_11": {
      "supported": false,
      "source": "assumed-default"
    },
    "color_diagnostics_flag": {
      "supported": false,
      "source": "assumed-default"
    },
    "cxx_standard_17": {
      "supported": false,
      "source": "assumed-default"
    },
    "depfile_mmd_mf": {
      "supported": false,
      "source": "assumed-default"
    },
    "gcc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "json_diagnostics": {
      "supported": false,
      "source": "assumed-default"
    },
    "msvc_style_flags": {
      "supported": false,
      "source": "assumed-default"
    },
    "response_files": {
      "supported": false,
      "source": "assumed-default"
    },
    "sarif_diagnostics": {
      "supported": false,
      "source": "assumed-default"
    },
    "std_flag": {
      "supported": false,
      "source": "assumed-default"
    }
  }
}"#;
        assert_eq!(actual, expected);
    }

    #[test]
    fn snapshot_gnu_ar_identity_and_capabilities() {
        let actual = ar_identity_and_capabilities_json(
            "GNU ar (GNU Binutils for Debian) 2.40\nCopyright (C) 2023 Free Software Foundation, Inc.\n",
        );
        let expected = r#"{
  "identity": {
    "kind": "ar",
    "version": "2.40",
    "raw_version_line": "GNU ar (GNU Binutils for Debian) 2.40"
  },
  "capabilities": {
    "ar_crs": {
      "supported": true,
      "source": "version"
    },
    "static_library_output": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
        assert_eq!(actual, expected);
    }

    #[test]
    fn snapshot_msvc_lib_archiver_produces_static_library_without_ar_crs() {
        let actual = ar_identity_and_capabilities_json(
            "Microsoft (R) Library Manager Version 14.39.33523.0\nCopyright (C) Microsoft Corporation.\n",
        );
        let expected = r#"{
  "identity": {
    "kind": "lib",
    "version": "14.39.33523",
    "raw_version_line": "Microsoft (R) Library Manager Version 14.39.33523.0"
  },
  "capabilities": {
    "ar_crs": {
      "supported": false,
      "source": "unsupported"
    },
    "static_library_output": {
      "supported": true,
      "source": "version"
    }
  }
}"#;
        assert_eq!(actual, expected);
    }

    #[test]
    fn snapshot_full_detection_report_for_clang_plus_gnu_ar() {
        // End-to-end snapshot of `ToolchainDetectionReport::as_json`
        // for a typical Linux clang + GNU ar setup. Pins the
        // top-level shape `{ cxx, [cc,] ar }` plus all nested
        // fields in their insertion order.
        let cxx_id =
            parse_cxx_version_output("clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\n");
        let cxx_caps = derive_cxx_capabilities(&cxx_id);
        let ar_id = parse_ar_version_output("GNU ar (GNU Binutils) 2.40\n");
        let ar_caps = derive_ar_capabilities(&ar_id);
        let report = ToolchainDetectionReport {
            cxx: ToolDetection {
                path: camino::Utf8PathBuf::from("/opt/llvm/bin/clang++"),
                identity: cxx_id,
                capabilities: cxx_caps,
            },
            cc: None,
            ar: ToolDetection {
                path: camino::Utf8PathBuf::from("/usr/bin/ar"),
                identity: ar_id,
                capabilities: ar_caps,
            },
        };
        let actual = pretty(&report.as_json());
        let expected = r#"{
  "cxx": {
    "path": "/opt/llvm/bin/clang++",
    "identity": {
      "kind": "clang",
      "version": "17.0.6",
      "target": "x86_64-unknown-linux-gnu",
      "raw_version_line": "clang version 17.0.6"
    },
    "capabilities": {
      "c_standard_11": {
        "supported": true,
        "source": "version"
      },
      "color_diagnostics_flag": {
        "supported": true,
        "source": "version"
      },
      "cxx_standard_17": {
        "supported": true,
        "source": "version"
      },
      "depfile_mmd_mf": {
        "supported": true,
        "source": "version"
      },
      "gcc_style_flags": {
        "supported": true,
        "source": "version"
      },
      "json_diagnostics": {
        "supported": true,
        "source": "version"
      },
      "msvc_style_flags": {
        "supported": false,
        "source": "assumed-default"
      },
      "response_files": {
        "supported": true,
        "source": "version"
      },
      "sarif_diagnostics": {
        "supported": false,
        "source": "assumed-default"
      },
      "std_flag": {
        "supported": true,
        "source": "version"
      }
    }
  },
  "ar": {
    "path": "/usr/bin/ar",
    "identity": {
      "kind": "ar",
      "version": "2.40",
      "raw_version_line": "GNU ar (GNU Binutils) 2.40"
    },
    "capabilities": {
      "ar_crs": {
        "supported": true,
        "source": "version"
      },
      "static_library_output": {
        "supported": true,
        "source": "version"
      }
    }
  }
}"#;
        assert_eq!(actual, expected);
    }
}
