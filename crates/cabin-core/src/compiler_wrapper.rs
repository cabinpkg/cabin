//! Typed compiler-wrapper model.
//!
//! Cabin can prefix C and C++ compile drivers with an executable such
//! as `ccache` or `sccache`. The wrapper is separate from the compiler:
//! it applies only to compile commands (never link or archive) and is
//! selected through the same precedence ladder as the rest of the
//! toolchain.
//!
//! This module owns *data only*: the typed enums, the manifest
//! declaration types, the resolved value, and the JSON helpers that
//! `cabin metadata` consumes.  PATH lookup, env reading, and
//! subprocess version probing live in `cabin-toolchain`.  CLI flag
//! handling lives in `cabin`.  Manifest parsing lives in
//! `cabin-manifest`.

use std::fmt;

use camino::Utf8PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::compiler::CompilerVersion;
use crate::toolchain::ToolSpec;

/// Executable-family label reported in metadata and fingerprints.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CompilerWrapperKind(String);

impl CompilerWrapperKind {
    /// Derive the wrapper family from an executable name or path.
    pub fn from_spec(spec: &ToolSpec) -> Self {
        let display = spec.display();
        let basename = camino::Utf8Path::new(&display)
            .file_name()
            .unwrap_or(&display);
        let kind = basename.strip_suffix(".exe").unwrap_or(basename);
        Self(kind.to_owned())
    }

    /// Stable identifier used in metadata, fingerprints, and errors.
    pub fn as_key(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CompilerWrapperKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

/// What the user (or a manifest layer) asked for, structurally.
///
/// `Disabled` is *explicit* opt-out: a higher-precedence layer can
/// no longer turn a wrapper back on.  `Use(_)` selects a specific
/// wrapper kind.  Layers that did not express any preference are
/// represented as `Option::None` at the call site, not as a variant
/// here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum CompilerWrapperRequest {
    /// "No wrapper at all".  Equivalent to the manifest value
    /// `compiler-wrapper = "none"` and the CLI flag
    /// `--no-compiler-wrapper` / `--compiler-wrapper none`.
    Disabled,
    /// Use an executable name searched on `PATH`, or an explicit path.
    Use { wrapper: ToolSpec },
}

impl CompilerWrapperRequest {
    /// Parse a manifest / CLI / env value.  Accepts:
    ///
    /// - `"none"` (case-insensitive) → [`Self::Disabled`].
    /// - Any other non-empty value selects that executable name or path.
    ///
    /// # Errors
    /// Returns [`CompilerWrapperParseError::Empty`] when `raw` is empty after
    /// trimming.
    pub fn parse(raw: &str) -> Result<Self, CompilerWrapperParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(CompilerWrapperParseError::Empty);
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "none" | "off" | "disabled" => Ok(Self::Disabled),
            _ => Ok(Self::Use {
                wrapper: ToolSpec::parse(trimmed.to_owned()),
            }),
        }
    }

    /// Stable display string.  Round-trips with [`Self::parse`].
    pub fn as_key(&self) -> String {
        match self {
            CompilerWrapperRequest::Disabled => "none".to_owned(),
            CompilerWrapperRequest::Use { wrapper } => wrapper.display(),
        }
    }
}

/// Errors produced by [`CompilerWrapperRequest::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompilerWrapperParseError {
    #[error("compiler-wrapper value must not be empty")]
    Empty,
}

/// Where a resolved wrapper selection ultimately came from.
/// Recorded alongside the resolved wrapper so `cabin metadata` can
/// show the precedence without re-deriving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompilerWrapperSource {
    /// Set by the `--compiler-wrapper` / `--no-compiler-wrapper`
    /// CLI flag.
    Cli,
    /// Set by the `CABIN_COMPILER_WRAPPER` environment variable.
    Env,
    /// Set by `[build]` in the user-level config file.
    UserConfig,
    /// Set by `[build]` in the workspace-level config file.
    WorkspaceConfig,
    /// Set by `[build]` in the package-local config file
    /// (non-workspace single-package projects).
    PackageConfig,
    /// Set by `[build]` in a config file pointed at by the
    /// `CABIN_CONFIG` environment variable.
    ExplicitConfig,
    /// Set by the workspace-root `[build]` table.
    Manifest,
}

impl CompilerWrapperSource {
    /// Stable lower-case label used in JSON output and error
    /// messages.
    pub const fn as_key(self) -> &'static str {
        match self {
            CompilerWrapperSource::Cli => "cli",
            CompilerWrapperSource::Env => "env",
            CompilerWrapperSource::UserConfig => "user-config",
            CompilerWrapperSource::WorkspaceConfig => "workspace-config",
            CompilerWrapperSource::PackageConfig => "package-config",
            CompilerWrapperSource::ExplicitConfig => "explicit-config",
            CompilerWrapperSource::Manifest => "manifest",
        }
    }
}

impl fmt::Display for CompilerWrapperSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

/// Identity captured from a wrapper executable's `--version`
/// output.  Populated by `cabin-toolchain::detect_compiler_wrapper`
/// and surfaced through metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerWrapperIdentity {
    pub kind: CompilerWrapperKind,
    /// Parsed numeric version (`Some` when the wrapper printed a
    /// recognizable version string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<CompilerVersion>,
    /// First non-empty line of the captured `--version` output,
    /// preserved verbatim so users can see exactly what the
    /// wrapper reported.
    pub raw_version_line: String,
}

impl CompilerWrapperIdentity {
    /// Convenience constructor for an identity whose version could
    /// not be parsed.
    pub fn unknown_version(kind: CompilerWrapperKind, raw_version_line: impl Into<String>) -> Self {
        Self {
            kind,
            version: None,
            raw_version_line: raw_version_line.into(),
        }
    }
}

/// Fully resolved compiler wrapper, ready to prefix C and C++ compile
/// commands.
///
/// `path` is the absolute filesystem path the resolver settled on.
/// `spec` records the original spelling so metadata can show the
/// requested executable without leaking the resolved machine-specific
/// path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCompilerWrapper {
    pub kind: CompilerWrapperKind,
    pub path: Utf8PathBuf,
    /// User-visible executable name or path from the selected layer.
    pub spec: String,
    pub source: CompilerWrapperSource,
    /// Detected identity (`Some` when version probing succeeded).
    /// Always emitted by `cabin metadata` even when `None`, so
    /// callers do not have to special-case the absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<CompilerWrapperIdentity>,
}

impl ResolvedCompilerWrapper {
    /// Compact JSON view used by `cabin metadata`.  Mirrors the
    /// shape of [`crate::ResolvedTool::as_json`] so consumers see a
    /// consistent pattern.
    pub fn as_json(&self) -> serde_json::Value {
        let version = self
            .identity
            .as_ref()
            .and_then(|id| id.version.as_ref())
            .map_or(serde_json::Value::Null, |v| {
                serde_json::Value::String(v.to_display_string())
            });
        let raw = self
            .identity
            .as_ref()
            .map_or(serde_json::Value::Null, |id| {
                serde_json::Value::String(id.raw_version_line.clone())
            });
        serde_json::json!({
            "kind": self.kind.as_key(),
            "spec": self.spec,
            "source": self.source.as_key(),
            "version": version,
            "raw_version_line": raw,
        })
    }
}

/// Lightweight, non-machine-specific summary of a resolved wrapper.
/// Carried inside [`crate::ToolchainSummary`] so the build
/// configuration fingerprint reflects "which wrapper did this build
/// use" without pinning the local absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerWrapperSummary {
    /// Executable-family kind derived from the selected spec.
    pub kind: CompilerWrapperKind,
    /// User-visible spec spelling.
    pub spec: String,
    /// Where the selection came from; serializes to the same
    /// kebab-case label as [`CompilerWrapperSource::as_key`].
    pub source: CompilerWrapperSource,
    /// Detected version, when probing succeeded.  Stored as a
    /// display string so the summary stays portable across
    /// `CompilerVersion` schema changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl CompilerWrapperSummary {
    /// Build a summary from a resolved wrapper.
    pub fn from_resolved(resolved: &ResolvedCompilerWrapper) -> Self {
        Self {
            kind: resolved.kind.clone(),
            spec: resolved.spec.clone(),
            source: resolved.source,
            version: resolved
                .identity
                .as_ref()
                .and_then(|id| id.version.as_ref())
                .map(super::compiler::CompilerVersion::to_display_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapper_kind(name: &str) -> CompilerWrapperKind {
        CompilerWrapperKind::from_spec(&ToolSpec::parse(name))
    }

    #[test]
    fn parse_accepts_documented_values() {
        assert_eq!(
            CompilerWrapperRequest::parse("none").unwrap(),
            CompilerWrapperRequest::Disabled
        );
        assert_eq!(
            CompilerWrapperRequest::parse("None").unwrap(),
            CompilerWrapperRequest::Disabled
        );
        assert_eq!(
            CompilerWrapperRequest::parse("ccache").unwrap(),
            CompilerWrapperRequest::Use {
                wrapper: ToolSpec::Name("ccache".into()),
            }
        );
        assert_eq!(
            CompilerWrapperRequest::parse("sccache").unwrap(),
            CompilerWrapperRequest::Use {
                wrapper: ToolSpec::Name("sccache".into()),
            }
        );
    }

    #[test]
    fn parse_accepts_any_executable_name() {
        let request = CompilerWrapperRequest::parse("icecc");
        assert!(
            request.is_ok(),
            "expected arbitrary executable name: {request:?}"
        );
    }

    #[test]
    fn parse_does_not_shell_split_executable_value() {
        assert_eq!(
            CompilerWrapperRequest::parse("wrapper with spaces").unwrap(),
            CompilerWrapperRequest::Use {
                wrapper: ToolSpec::Name("wrapper with spaces".into()),
            }
        );
    }

    #[test]
    fn parse_accepts_executable_paths() {
        let request = CompilerWrapperRequest::parse("/usr/local/bin/icecc");
        assert!(request.is_ok(), "expected executable path: {request:?}");
    }

    #[test]
    fn parse_rejects_empty() {
        assert_eq!(
            CompilerWrapperRequest::parse("").unwrap_err(),
            CompilerWrapperParseError::Empty
        );
        assert_eq!(
            CompilerWrapperRequest::parse("   ").unwrap_err(),
            CompilerWrapperParseError::Empty
        );
    }

    #[test]
    fn as_key_round_trips_through_parse() {
        for value in ["none", "ccache", "sccache"] {
            let parsed = CompilerWrapperRequest::parse(value).unwrap();
            assert_eq!(parsed.as_key(), value);
        }
    }

    #[test]
    fn kind_is_derived_from_executable_basename() {
        assert_eq!(wrapper_kind("/opt/bin/icecc").as_key(), "icecc");
        assert_eq!(wrapper_kind("sccache.exe").as_key(), "sccache");
    }

    #[test]
    fn source_keys_are_stable() {
        for (source, key) in [
            (CompilerWrapperSource::Cli, "cli"),
            (CompilerWrapperSource::Env, "env"),
            (CompilerWrapperSource::Manifest, "manifest"),
        ] {
            assert_eq!(source.as_key(), key);
        }
    }

    #[test]
    fn resolved_as_json_includes_kind_spec_source_and_optional_version() {
        let resolved = ResolvedCompilerWrapper {
            kind: wrapper_kind("ccache"),
            path: Utf8PathBuf::from("/usr/local/bin/ccache"),
            spec: "ccache".into(),
            source: CompilerWrapperSource::Cli,
            identity: Some(CompilerWrapperIdentity {
                kind: wrapper_kind("ccache"),
                version: CompilerVersion::parse("4.10.2"),
                raw_version_line: "ccache version 4.10.2".into(),
            }),
        };
        let json = resolved.as_json();
        assert_eq!(json["kind"], "ccache");
        assert_eq!(json["spec"], "ccache");
        assert_eq!(json["source"], "cli");
        assert_eq!(json["version"], "4.10.2");
        assert!(json["raw_version_line"].is_string());
    }

    #[test]
    fn resolved_as_json_emits_null_version_when_missing() {
        let resolved = ResolvedCompilerWrapper {
            kind: wrapper_kind("sccache"),
            path: Utf8PathBuf::from("/usr/local/bin/sccache"),
            spec: "sccache".into(),
            source: CompilerWrapperSource::Manifest,
            identity: None,
        };
        let json = resolved.as_json();
        assert_eq!(json["version"], serde_json::Value::Null);
        assert_eq!(json["raw_version_line"], serde_json::Value::Null);
    }

    #[test]
    fn summary_from_resolved_keeps_display_version() {
        let resolved = ResolvedCompilerWrapper {
            kind: wrapper_kind("ccache"),
            path: Utf8PathBuf::from("/usr/local/bin/ccache"),
            spec: "ccache".into(),
            source: CompilerWrapperSource::Env,
            identity: Some(CompilerWrapperIdentity {
                kind: wrapper_kind("ccache"),
                version: CompilerVersion::parse("4.10.2"),
                raw_version_line: "ccache version 4.10.2".into(),
            }),
        };
        let summary = CompilerWrapperSummary::from_resolved(&resolved);
        assert_eq!(summary.kind.as_key(), "ccache");
        assert_eq!(summary.source, CompilerWrapperSource::Env);
        assert_eq!(summary.version.as_deref(), Some("4.10.2"));
    }
}
