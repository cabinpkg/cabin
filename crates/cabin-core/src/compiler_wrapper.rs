//! Typed compiler-cache wrapper model.
//!
//! Cabin can prefix the C++ compile driver with a *compiler cache*
//! wrapper such as `ccache` or `sccache`. The wrapper is a separate
//! concept from the compiler itself: it is layered on top, applies
//! only to compile commands (never link or archive), and is selected
//! through the same precedence ladder as the rest of the toolchain.
//!
//! This module owns *data only*: the typed enums, the manifest
//! declaration types, the resolved value, and the JSON helpers that
//! `cabin metadata` consumes. PATH lookup, env reading, and
//! subprocess version probing live in `cabin-toolchain`. CLI flag
//! handling lives in `cabin-cli`. Manifest parsing lives in
//! `cabin-manifest`.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::compiler::CompilerVersion;
use crate::condition::Condition;

/// Which compiler-cache wrapper Cabin should prefix the C++ compile
/// driver with. The "no wrapper" case is represented as the absence
/// of a [`ResolvedCompilerWrapper`] (i.e. an `Option::None` at the
/// call site), so this enum stays small and total over the wrappers
/// Cabin actually understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompilerWrapperKind {
    /// `ccache` — local compiler cache.
    Ccache,
    /// `sccache` — local-or-remote compiler cache.
    Sccache,
}

impl CompilerWrapperKind {
    /// Stable lower-case identifier used in CLI flags, manifest
    /// values, environment variables, JSON output, and error
    /// messages.
    pub const fn as_key(self) -> &'static str {
        match self {
            CompilerWrapperKind::Ccache => "ccache",
            CompilerWrapperKind::Sccache => "sccache",
        }
    }

    /// Bare command name searched on `PATH` when no explicit path
    /// is given. Today this matches [`Self::as_key`] for both
    /// supported wrappers; kept as a separate accessor so future
    /// platform-specific binaries (`sccache-dist`, …) can diverge
    /// from the manifest key without breaking existing manifests.
    pub const fn default_command(self) -> &'static str {
        match self {
            CompilerWrapperKind::Ccache => "ccache",
            CompilerWrapperKind::Sccache => "sccache",
        }
    }

    /// Every supported wrapper, in stable declaration order. Used
    /// in error messages so users see the full list of accepted
    /// values.
    pub const fn all() -> &'static [CompilerWrapperKind] {
        &[CompilerWrapperKind::Ccache, CompilerWrapperKind::Sccache]
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
/// no longer turn a wrapper back on. `Use(_)` selects a specific
/// wrapper kind. Layers that did not express any preference are
/// represented as `Option::None` at the call site, not as a variant
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum CompilerWrapperRequest {
    /// "No wrapper at all". Equivalent to the manifest value
    /// `compiler-wrapper = "none"` and the CLI flag
    /// `--no-compiler-wrapper` / `--compiler-wrapper none`.
    Disabled,
    /// Use the named wrapper. The bare command (`ccache`,
    /// `sccache`) is searched on `PATH`; missing executables are
    /// rejected by the resolver.
    Use { wrapper: CompilerWrapperKind },
}

impl CompilerWrapperRequest {
    /// Parse a manifest / CLI / env value. Accepts:
    ///
    /// - `"none"` (case-insensitive) → [`Self::Disabled`].
    /// - `"ccache"` → `Use(Ccache)`.
    /// - `"sccache"` → `Use(Sccache)`.
    ///
    /// Anything else is rejected. Path-shaped inputs are
    /// deliberately *not* accepted today: the resolver expects to
    /// do its own `PATH` search so the resulting selection stays
    /// machine-independent. A future revision may add a path
    /// variant; until then the conservative "named-only" surface
    /// is the documented contract.
    pub fn parse(raw: &str) -> Result<Self, CompilerWrapperParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(CompilerWrapperParseError::Empty);
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "none" | "off" | "disabled" => Ok(Self::Disabled),
            "ccache" => Ok(Self::Use {
                wrapper: CompilerWrapperKind::Ccache,
            }),
            "sccache" => Ok(Self::Use {
                wrapper: CompilerWrapperKind::Sccache,
            }),
            _ => Err(CompilerWrapperParseError::Unsupported {
                raw: trimmed.to_owned(),
            }),
        }
    }

    /// Stable display string. Round-trips with [`Self::parse`].
    pub const fn as_key(&self) -> &'static str {
        match self {
            CompilerWrapperRequest::Disabled => "none",
            CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Ccache,
            } => "ccache",
            CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Sccache,
            } => "sccache",
        }
    }
}

/// Errors produced by [`CompilerWrapperRequest::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompilerWrapperParseError {
    #[error("compiler-wrapper value must not be empty")]
    Empty,
    #[error(
        "compiler-wrapper value `{raw}` is not supported; expected one of: none, ccache, sccache"
    )]
    Unsupported { raw: String },
}

/// `[target.'cfg(...)'.profile.cache]` block. Same shape as the
/// general `[profile.cache]` table but tagged with the predicate
/// that gates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalCompilerWrapperDecl {
    pub condition: Condition,
    pub request: CompilerWrapperRequest,
}

/// Workspace-root manifest's compiler-wrapper declarations.
///
/// The wrapper is a single value per build invocation. To keep that
/// invariant clear, only the workspace-root manifest's
/// `[profile.cache]` / `[target.'cfg(...)'.profile.cache]`
/// declarations matter; member manifests that try to declare any
/// cache settings are rejected by the workspace loader before the
/// resolver runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerWrapperManifestSettings {
    /// Unconditional `[profile.cache].compiler-wrapper`. `None` means
    /// the manifest did not declare a general value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub general: Option<CompilerWrapperRequest>,
    /// `[target.'cfg(...)'.profile.cache]` overlays. Empty when no
    /// conditional wrapper declarations exist.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional: Vec<ConditionalCompilerWrapperDecl>,
}

impl CompilerWrapperManifestSettings {
    /// Whether the settings carry no fields at all. Used by the
    /// workspace loader to decide whether a member manifest's
    /// declaration should be rejected, and by the manifest
    /// serializer to skip emitting empty tables.
    pub fn is_empty(&self) -> bool {
        self.general.is_none() && self.conditional.is_empty()
    }
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
    /// Set by `[profile.cache]` in the user-level config file.
    UserConfig,
    /// Set by `[profile.cache]` in the workspace-level config file.
    WorkspaceConfig,
    /// Set by `[profile.cache]` in the package-local config file
    /// (non-workspace single-package projects).
    PackageConfig,
    /// Set by `[profile.cache]` in a config file pointed at by the
    /// `CABIN_CONFIG` environment variable.
    ExplicitConfig,
    /// Set by a `[target.'cfg(...)'.profile.cache]` overlay matching
    /// the host platform.
    ManifestConditional,
    /// Set by the workspace-root `[profile.cache]` table.
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
            CompilerWrapperSource::ManifestConditional => "manifest-conditional",
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
/// output. Populated by `cabin-toolchain::detect_compiler_wrapper`
/// and surfaced through metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerWrapperIdentity {
    pub kind: CompilerWrapperKind,
    /// Parsed numeric version (`Some` when the wrapper printed a
    /// recognisable version string).
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

/// Fully resolved compiler-cache wrapper, ready to prefix the C++
/// compile command.
///
/// `path` is the absolute filesystem path the resolver settled on.
/// `spec` records the original spelling (the wrapper's stable
/// `as_key()`) so metadata can show the requested name without
/// leaking machine-specific paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedCompilerWrapper {
    pub kind: CompilerWrapperKind,
    pub path: PathBuf,
    /// User-visible spelling for metadata. Today this is always
    /// the bare command name corresponding to `kind`.
    pub spec: String,
    pub source: CompilerWrapperSource,
    /// Detected identity (`Some` when version probing succeeded).
    /// Always emitted by `cabin metadata` even when `None`, so
    /// callers do not have to special-case the absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<CompilerWrapperIdentity>,
}

impl ResolvedCompilerWrapper {
    /// Compact JSON view used by `cabin metadata`. Mirrors the
    /// shape of [`crate::ResolvedTool::as_json`] so consumers see a
    /// consistent pattern.
    pub fn as_json(&self) -> serde_json::Value {
        let version = self
            .identity
            .as_ref()
            .and_then(|id| id.version.as_ref())
            .map(|v| serde_json::Value::String(v.to_display_string()))
            .unwrap_or(serde_json::Value::Null);
        let raw = self
            .identity
            .as_ref()
            .map(|id| serde_json::Value::String(id.raw_version_line.clone()))
            .unwrap_or(serde_json::Value::Null);
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerWrapperSummary {
    /// Stable wrapper key (`ccache` / `sccache`).
    pub kind: String,
    /// User-visible spec spelling.
    pub spec: String,
    /// Source label (matches [`CompilerWrapperSource::as_key`]).
    pub source: String,
    /// Detected version, when probing succeeded. Stored as a
    /// display string so the summary stays portable across
    /// `CompilerVersion` schema changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl CompilerWrapperSummary {
    /// Build a summary from a resolved wrapper.
    pub fn from_resolved(resolved: &ResolvedCompilerWrapper) -> Self {
        Self {
            kind: resolved.kind.as_key().to_owned(),
            spec: resolved.spec.clone(),
            source: resolved.source.as_key().to_owned(),
            version: resolved
                .identity
                .as_ref()
                .and_then(|id| id.version.as_ref())
                .map(|v| v.to_display_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                wrapper: CompilerWrapperKind::Ccache,
            }
        );
        assert_eq!(
            CompilerWrapperRequest::parse("sccache").unwrap(),
            CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Sccache,
            }
        );
    }

    #[test]
    fn parse_rejects_unsupported_names_with_clear_error() {
        let err = CompilerWrapperRequest::parse("fastcache").unwrap_err();
        match err {
            CompilerWrapperParseError::Unsupported { raw } => assert_eq!(raw, "fastcache"),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_paths_today() {
        // The conservative initial surface accepts only named
        // wrappers. Path-shaped inputs must error so users get a
        // clear message rather than a surprise `PATH` search.
        let err = CompilerWrapperRequest::parse("/usr/local/bin/ccache").unwrap_err();
        assert!(matches!(err, CompilerWrapperParseError::Unsupported { .. }));
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
    fn manifest_settings_is_empty_by_default() {
        assert!(CompilerWrapperManifestSettings::default().is_empty());
    }

    #[test]
    fn manifest_settings_reports_non_empty_when_general_set() {
        let settings = CompilerWrapperManifestSettings {
            general: Some(CompilerWrapperRequest::Use {
                wrapper: CompilerWrapperKind::Ccache,
            }),
            ..Default::default()
        };
        assert!(!settings.is_empty());
    }

    #[test]
    fn source_keys_are_stable() {
        for (source, key) in [
            (CompilerWrapperSource::Cli, "cli"),
            (CompilerWrapperSource::Env, "env"),
            (
                CompilerWrapperSource::ManifestConditional,
                "manifest-conditional",
            ),
            (CompilerWrapperSource::Manifest, "manifest"),
        ] {
            assert_eq!(source.as_key(), key);
        }
    }

    #[test]
    fn resolved_as_json_includes_kind_spec_source_and_optional_version() {
        let resolved = ResolvedCompilerWrapper {
            kind: CompilerWrapperKind::Ccache,
            path: PathBuf::from("/usr/local/bin/ccache"),
            spec: "ccache".into(),
            source: CompilerWrapperSource::Cli,
            identity: Some(CompilerWrapperIdentity {
                kind: CompilerWrapperKind::Ccache,
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
            kind: CompilerWrapperKind::Sccache,
            path: PathBuf::from("/usr/local/bin/sccache"),
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
            kind: CompilerWrapperKind::Ccache,
            path: PathBuf::from("/usr/local/bin/ccache"),
            spec: "ccache".into(),
            source: CompilerWrapperSource::Env,
            identity: Some(CompilerWrapperIdentity {
                kind: CompilerWrapperKind::Ccache,
                version: CompilerVersion::parse("4.10.2"),
                raw_version_line: "ccache version 4.10.2".into(),
            }),
        };
        let summary = CompilerWrapperSummary::from_resolved(&resolved);
        assert_eq!(summary.kind, "ccache");
        assert_eq!(summary.source, "env");
        assert_eq!(summary.version.as_deref(), Some("4.10.2"));
    }
}
