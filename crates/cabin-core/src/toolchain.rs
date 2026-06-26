//! Typed C/C++ toolchain selection model.
//!
//! Cabin builds C/C++ packages with three external tools - a C
//! compiler, a C++ compiler, and a static-library archiver.  The
//! selection is explicit, deterministic, and auditable: every
//! component owns a typed model in this module, and the resolver
//! in `cabin-toolchain` produces one [`ResolvedToolchain`] per
//! build.
//!
//! This module owns *data only*.  PATH lookup, env reading, and
//! filesystem checks live in `cabin-toolchain`.  Manifest parsing
//! lives in `cabin-manifest`.  CLI flag handling lives in
//! `cabin`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use camino::{Utf8Path, Utf8PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::condition::Condition;

/// Which kind of tool a selection refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolKind {
    /// C compiler (driver, e.g. `cc`, `clang`, `gcc`).
    CCompiler,
    /// C++ compiler (driver, e.g. `c++`, `clang++`, `g++`).  Also
    /// drives linking in the current backend.
    CxxCompiler,
    /// Static-library archiver (e.g. `ar`, `llvm-ar`).
    Archiver,
}

impl ToolKind {
    /// Stable, lower-case identifier used in CLI flags, manifest
    /// keys, JSON serialization, and error messages.
    pub fn as_key(self) -> &'static str {
        match self {
            ToolKind::CCompiler => "cc",
            ToolKind::CxxCompiler => "cxx",
            ToolKind::Archiver => "ar",
        }
    }

    /// Human-readable label used in error messages so users can map
    /// the failure back to the tool they were thinking about.
    pub fn human_label(self) -> &'static str {
        match self {
            ToolKind::CCompiler => "C compiler",
            ToolKind::CxxCompiler => "C++ compiler",
            ToolKind::Archiver => "archiver",
        }
    }
}

impl fmt::Display for ToolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

/// Where a tool selection ultimately came from.  Recorded alongside
/// the resolved tool so `cabin metadata` can show the precedence
/// without re-deriving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolSource {
    /// Set by a CLI flag (`--cc`, `--cxx`, `--ar`).
    Cli,
    /// Set by an environment variable (`CC`, `CXX`, `AR`).
    Env,
    /// Set by `[toolchain]` in the user-level config file.
    UserConfig,
    /// Set by `[toolchain]` in the workspace-level config file.
    WorkspaceConfig,
    /// Set by `[toolchain]` in the package-local config file
    /// (non-workspace single-package projects).
    PackageConfig,
    /// Set by `[toolchain]` in a config file pointed at by the
    /// `CABIN_CONFIG` environment variable.
    ExplicitConfig,
    /// Set by a `[target.'cfg(...)'.toolchain]` table that matches
    /// the host platform.
    ManifestConditional,
    /// Set by the workspace-root `[toolchain]` table.
    Manifest,
    /// Auto-detected from PATH using Cabin's documented fallback
    /// list (`c++` / `clang++` / `g++` for the C++ compiler, `cc` /
    /// `clang` / `gcc` for the C compiler, `ar` for the archiver).
    Default,
}

/// Either a bare command name (resolved against `PATH`) or an
/// explicit filesystem path.  The resolver turns either form into a
/// concrete [`Utf8PathBuf`] when it builds a [`ResolvedTool`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolSpec {
    /// Filesystem path.  Absolute paths are validated as-is;
    /// relative paths are resolved against the current working
    /// directory at build time.
    Path(Utf8PathBuf),
    /// Bare command name searched on `PATH`.
    Name(String),
}

impl ToolSpec {
    /// Parse a user-supplied string into the matching variant.
    /// Anything that contains a path separator (`/`, or `\` on
    /// Windows) is treated as a path; otherwise the value is a
    /// bare name.
    pub fn parse(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        if looks_like_path(&raw) {
            ToolSpec::Path(Utf8PathBuf::from(raw))
        } else {
            ToolSpec::Name(raw)
        }
    }

    /// Parse a `[toolchain]` cc/cxx/ar value, treating an empty or
    /// whitespace-only string as absent: returns `None` so each
    /// caller can map that to its own "empty tool spec" diagnostic;
    /// otherwise trims and delegates to [`ToolSpec::parse`].  Shared by
    /// the manifest and config parsers.
    pub fn parse_non_empty(raw: &str) -> Option<ToolSpec> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(ToolSpec::parse(trimmed.to_owned()))
        }
    }

    /// Human-readable form used in errors and metadata.
    pub fn display(&self) -> String {
        match self {
            ToolSpec::Path(p) => p.as_str().to_owned(),
            ToolSpec::Name(n) => n.clone(),
        }
    }

    /// View as a borrowed `Utf8Path` regardless of variant.  Used by
    /// the resolver when probing for executables.
    pub fn as_path(&self) -> &Utf8Path {
        match self {
            ToolSpec::Path(p) => p.as_path(),
            ToolSpec::Name(n) => Utf8Path::new(n),
        }
    }
}

fn looks_like_path(raw: &str) -> bool {
    if raw.contains('/') {
        return true;
    }
    if cfg!(windows) && raw.contains('\\') {
        return true;
    }
    false
}

/// CLI / orchestration-supplied request for one tool.
///
/// `ToolSelection::default()` is "no preference"; the resolver
/// then consults environment variables, manifest tables, and
/// finally the built-in default list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolSelection {
    /// Set when the user passed a CLI flag for this tool.  Highest
    /// precedence.
    pub cli: Option<ToolSpec>,
}

/// Aggregate of [`ToolSelection`]s, one per [`ToolKind`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolchainSelection {
    pub cc: ToolSelection,
    pub cxx: ToolSelection,
    pub ar: ToolSelection,
}

impl ToolchainSelection {
    /// Empty selection: every tool is "no preference".
    pub fn empty() -> Self {
        Self::default()
    }

    /// Helper for tests / programmatic construction.
    #[must_use]
    pub fn with_cli(mut self, kind: ToolKind, spec: ToolSpec) -> Self {
        let slot = match kind {
            ToolKind::CCompiler => &mut self.cc,
            ToolKind::CxxCompiler => &mut self.cxx,
            ToolKind::Archiver => &mut self.ar,
        };
        slot.cli = Some(spec);
        self
    }
}

/// Manifest-shape declaration for tool selection.
///
/// Used by `cabin-manifest` to expose `[toolchain]` and
/// `[target.'cfg(...)'.toolchain]` parse output as typed values.
/// Every field is optional so omission means "no preference at
/// this layer".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainDecl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<ToolSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cxx: Option<ToolSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ar: Option<ToolSpec>,
}

impl ToolchainDecl {
    /// Whether the declaration carries no fields.  Used to skip
    /// emitting empty tables in serialized metadata.
    pub fn is_empty(&self) -> bool {
        self.cc.is_none() && self.cxx.is_none() && self.ar.is_none()
    }

    /// Look up the preference for one tool kind.
    pub fn get(&self, kind: ToolKind) -> Option<&ToolSpec> {
        match kind {
            ToolKind::CCompiler => self.cc.as_ref(),
            ToolKind::CxxCompiler => self.cxx.as_ref(),
            ToolKind::Archiver => self.ar.as_ref(),
        }
    }
}

/// Conditional `[target.'cfg(...)'.toolchain]` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalToolchainDecl {
    pub condition: Condition,
    #[serde(default, skip_serializing_if = "ToolchainDecl::is_empty", flatten)]
    pub toolchain: ToolchainDecl,
}

/// Workspace-root toolchain settings derived from the manifest.
/// Holds both the unconditional `[toolchain]` table and any
/// `[target.'cfg(...)'.toolchain]` overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainSettings {
    #[serde(default, skip_serializing_if = "ToolchainDecl::is_empty")]
    pub general: ToolchainDecl,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional: Vec<ConditionalToolchainDecl>,
}

impl ToolchainSettings {
    /// Whether the settings carry no fields at all.
    pub fn is_empty(&self) -> bool {
        self.general.is_empty() && self.conditional.is_empty()
    }
}

/// One concrete tool, ready to be invoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedTool {
    pub kind: ToolKind,
    /// Absolute filesystem path the tool was resolved to.  Always
    /// pointed at an existing file by the time a `ResolvedTool`
    /// is built.
    pub path: Utf8PathBuf,
    /// What the user (or default) asked for.  Stored separately
    /// from `path` so metadata can show the original spelling
    /// (`clang++`) without leaking the absolute resolved path.
    pub spec: ToolSpec,
    /// Where the selection ultimately came from.
    pub source: ToolSource,
}

impl ResolvedTool {
    /// Path the build planner uses when constructing compile / link
    /// / archive commands.
    pub fn path(&self) -> &Utf8Path {
        &self.path
    }

    /// Compact JSON view used by `cabin metadata`.  Reports the
    /// requested spec and the source; omits the absolute resolved
    /// path because that is machine-specific.
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.kind.as_key(),
            "spec": self.spec.display(),
            "source": tool_source_label(self.source),
        })
    }
}

/// Stable lower-case label for a [`ToolSource`].  Used by the
/// `cabin metadata` JSON view and the build-configuration
/// fingerprint summary so callers do not have to redefine the
/// label in two places.
pub(crate) fn tool_source_label(source: ToolSource) -> &'static str {
    match source {
        ToolSource::Cli => "cli",
        ToolSource::Env => "env",
        ToolSource::UserConfig => "user-config",
        ToolSource::WorkspaceConfig => "workspace-config",
        ToolSource::PackageConfig => "package-config",
        ToolSource::ExplicitConfig => "explicit-config",
        ToolSource::ManifestConditional => "manifest-conditional",
        ToolSource::Manifest => "manifest",
        ToolSource::Default => "default",
    }
}

/// Fully-resolved C/C++ toolchain.
///
/// The build planner reads `cxx`, `cc`, and `ar` directly.  Build
/// scripts get every entry exposed through `CABIN_*` environment
/// variables. `cabin metadata` reports the same struct serialized
/// to JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedToolchain {
    /// C++ compiler.  Always populated.  Used for `.cc` / `.cpp` /
    /// `.cxx` / `.c++` / `.C` compiles and for linking any target
    /// whose object set contains a C++ translation unit.
    pub cxx: ResolvedTool,
    /// Static-library archiver.  Always populated.
    pub ar: ResolvedTool,
    /// C compiler.  Used for `.c` compiles and as the link driver
    /// for targets whose objects are pure C.  Optional: the resolver
    /// also probes the documented fallback list (`cc`, `clang`,
    /// `gcc`) so any standard system populates this without an
    /// explicit selection.  Only `None` when no candidate exists on
    /// `PATH`; the planner then errors with `MissingCCompiler` if a
    /// `.c` source is encountered.
    pub cc: Option<ResolvedTool>,
}

impl ResolvedToolchain {
    /// Iterator over every populated tool, sorted by [`ToolKind`].
    pub fn iter(&self) -> impl Iterator<Item = &ResolvedTool> {
        let mut entries: Vec<&ResolvedTool> = Vec::with_capacity(3);
        if let Some(cc) = &self.cc {
            entries.push(cc);
        }
        entries.push(&self.cxx);
        entries.push(&self.ar);
        entries.sort_by_key(|t| t.kind);
        entries.into_iter()
    }

    /// Compact JSON view used by `cabin metadata` and
    /// `CABIN_BUILD_CONFIGURATION_JSON`.
    pub fn as_json(&self) -> serde_json::Value {
        let entries: BTreeMap<String, serde_json::Value> = self
            .iter()
            .map(|t| (t.kind.as_key().to_owned(), t.as_json()))
            .collect();
        serde_json::Value::Object(entries.into_iter().collect())
    }
}

/// Errors produced while resolving a toolchain.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolchainResolutionError {
    /// The user asked for a specific tool but Cabin could not find
    /// an executable that matches.
    #[error(
        "{label} `{spec}` was requested by {source_label} but could not be found",
        label = kind.human_label(),
        source_label = source_label(*selected_from)
    )]
    ToolNotFound {
        kind: ToolKind,
        spec: String,
        selected_from: ToolSource,
    },
    /// No tool was specified and the built-in fallback list also
    /// failed.
    #[error("no usable {label} found on PATH; set {env_var} or add `{key} = ...` under [toolchain]",
        label = kind.human_label(),
        env_var = env_var_for(*kind),
        key = kind.as_key()
    )]
    NoDefault { kind: ToolKind },
    /// Selected compiler is recognizably unsupported (e.g.  MSVC
    /// `cl.exe`).
    #[error(
        "selected {label} `{spec}` is not supported by the current C++ backend; use a GCC- or Clang-like compiler driver",
        label = kind.human_label()
    )]
    UnsupportedCompiler { kind: ToolKind, spec: String },
    /// A tool was located on `PATH` but the resolved path is not
    /// valid UTF-8.  Cabin's toolchain model assumes UTF-8 paths, so
    /// an executable under a non-UTF-8 directory is surfaced here
    /// rather than aborting the process.
    #[error(
        "resolved {label} path `{path}` is not valid UTF-8",
        label = kind.human_label(),
        path = path.display(),
    )]
    NonUtf8Path { kind: ToolKind, path: PathBuf },
}

fn env_var_for(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::CCompiler => "CC",
        ToolKind::CxxCompiler => "CXX",
        ToolKind::Archiver => "AR",
    }
}

fn source_label(source: ToolSource) -> &'static str {
    match source {
        ToolSource::Cli => "--cli",
        ToolSource::Env => "an environment variable",
        ToolSource::UserConfig => "the user `[toolchain]` config table",
        ToolSource::WorkspaceConfig => "the workspace `[toolchain]` config table",
        ToolSource::PackageConfig => "the package `[toolchain]` config table",
        ToolSource::ExplicitConfig => "the `CABIN_CONFIG` `[toolchain]` table",
        ToolSource::ManifestConditional => "[target.'cfg(...)'.toolchain]",
        ToolSource::Manifest => "[toolchain]",
        ToolSource::Default => "the built-in default list",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_kind_keys_are_stable() {
        assert_eq!(ToolKind::CCompiler.as_key(), "cc");
        assert_eq!(ToolKind::CxxCompiler.as_key(), "cxx");
        assert_eq!(ToolKind::Archiver.as_key(), "ar");
    }

    #[test]
    fn tool_spec_parse_distinguishes_paths_and_names() {
        match ToolSpec::parse("clang++") {
            ToolSpec::Name(n) => assert_eq!(n, "clang++"),
            ToolSpec::Path(p) => panic!("expected name, got {p:?}"),
        }
        match ToolSpec::parse("/usr/bin/clang++") {
            ToolSpec::Path(p) => assert_eq!(p, Utf8PathBuf::from("/usr/bin/clang++")),
            ToolSpec::Name(n) => panic!("expected path, got {n:?}"),
        }
        match ToolSpec::parse("./bin/clang++") {
            ToolSpec::Path(p) => assert_eq!(p, Utf8PathBuf::from("./bin/clang++")),
            ToolSpec::Name(n) => panic!("expected path, got {n:?}"),
        }
    }

    #[test]
    fn toolchain_decl_is_empty_when_unset() {
        assert!(ToolchainDecl::default().is_empty());
        let d = ToolchainDecl {
            cxx: Some(ToolSpec::Name("clang++".into())),
            ..Default::default()
        };
        assert!(!d.is_empty());
        assert_eq!(
            d.get(ToolKind::CxxCompiler).map(ToolSpec::display),
            Some("clang++".to_owned())
        );
        assert!(d.get(ToolKind::CCompiler).is_none());
    }

    #[test]
    fn resolved_toolchain_iter_is_sorted_and_skips_missing_cc() {
        let cxx = ResolvedTool {
            kind: ToolKind::CxxCompiler,
            path: Utf8PathBuf::from("/usr/bin/c++"),
            spec: ToolSpec::Name("c++".into()),
            source: ToolSource::Default,
        };
        let ar = ResolvedTool {
            kind: ToolKind::Archiver,
            path: Utf8PathBuf::from("/usr/bin/ar"),
            spec: ToolSpec::Name("ar".into()),
            source: ToolSource::Default,
        };
        let resolved = ResolvedToolchain { cxx, ar, cc: None };
        let kinds: Vec<ToolKind> = resolved.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, vec![ToolKind::CxxCompiler, ToolKind::Archiver]);
    }
}
