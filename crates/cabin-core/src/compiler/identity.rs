//! Compiler / archiver identity, version, and taxonomy types.

use std::fmt;

use serde::{Deserialize, Serialize};

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
    /// LLVM `clang-cl`: Clang's `cl.exe`-compatible driver. Reports a
    /// `clang version …` banner like Clang, but accepts the MSVC
    /// command line (`/std:c++17`, `/showIncludes`, `/Fo…`), so it is
    /// detected by the invoked name and drives the MSVC dialect.
    ClangCl,
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
            CompilerKind::ClangCl => "clang-cl",
            CompilerKind::Gcc => "gcc",
            CompilerKind::Msvc => "msvc",
            CompilerKind::Unknown => "unknown",
        }
    }

    /// Whether this compiler is part of the Clang family. `clang-cl`
    /// is Clang under the hood, so it shares Clang's diagnostic and
    /// response-file capabilities even though it speaks the MSVC
    /// dialect.
    pub fn is_clang_like(self) -> bool {
        matches!(
            self,
            CompilerKind::Clang | CompilerKind::AppleClang | CompilerKind::ClangCl
        )
    }

    /// Whether this compiler accepts the GCC-style command line
    /// the current C++ backend emits (`-O<n>`, `-std=c++NN`,
    /// `-MMD -MF`, `-DNAME`, `-Idir`, …). Note `clang-cl` is
    /// excluded: it is Clang but parses the MSVC command line.
    pub fn supports_gcc_style_command_line(self) -> bool {
        matches!(
            self,
            CompilerKind::Clang | CompilerKind::AppleClang | CompilerKind::Gcc
        )
    }

    /// Whether this compiler drives the MSVC command-line dialect
    /// (`/std:…`, `/Fo…`, `/showIncludes`, `<name>.lib` archives):
    /// `cl.exe` and Clang's `cl`-compatible `clang-cl` driver.
    pub fn speaks_msvc_dialect(self) -> bool {
        matches!(self, CompilerKind::Msvc | CompilerKind::ClangCl)
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
