//! Typed patch / override model.
//!
//! A *patch* replaces a registry-resolved package candidate with
//! a local source for the duration of one Cabin invocation. The
//! patch is local development policy — it is never serialized
//! into published package metadata, never affects the resolver
//! for downstream consumers, and never triggers network access.
//!
//! Public syntax lives in two places:
//!
//! - the workspace-root `cabin.toml`'s `[patch]` table
//!   (root/workspace policy);
//! - any `.cabin/config.toml`'s `[patch]` table (user / workspace
//!   / package / explicit policy from the config layer).
//!
//! Both forms produce the same typed model. The orchestration
//! layer in `cabin` merges the two and the workspace loader
//! stitches the patched packages into the package graph.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ConfigValueSource;

/// Kind of source a patch points at. Today only local paths are
/// supported. The enum is closed: adding new source kinds is a
/// deliberate change that requires matching parser, validator,
/// and resolver work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatchSourceKind {
    /// A local filesystem path that contains a Cabin package
    /// (a directory with a `cabin.toml` file).
    Path,
}

impl PatchSourceKind {
    /// Stable lower-case label used in JSON output and error
    /// messages.
    pub const fn as_key(self) -> &'static str {
        match self {
            PatchSourceKind::Path => "path",
        }
    }
}

impl fmt::Display for PatchSourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_key())
    }
}

/// What a patch redirects the patched package to. Each variant
/// pairs the [`PatchSourceKind`] with its concrete data.
///
/// Rationale: keeping the variant data closed (instead of a
/// stringly-typed "spec" string) means the resolver, fetch
/// pipeline, and metadata view all agree on what each patch
/// actually points at. Future kinds (artifact archive, local
/// index reference) extend this enum explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PatchSource {
    /// `path = "../fmt"`. Carries the path *as written* in the
    /// declaring file. Resolution against the file's directory
    /// happens one layer up in the orchestration code so this
    /// type stays free of filesystem context.
    Path { path: PathBuf },
}

impl PatchSource {
    /// Stable kind label, useful for metadata / lockfile output.
    pub fn kind(&self) -> PatchSourceKind {
        match self {
            PatchSource::Path { .. } => PatchSourceKind::Path,
        }
    }

    /// Build a [`PatchSource`] from a `[patch]` row's `path` field —
    /// the only supported patch grammar today. Requires the path,
    /// trims it, and rejects empty / whitespace, surfacing
    /// [`PatchValidationError::MissingSource`] on failure. Shared by
    /// the manifest and config parsers so the path→source rule lives
    /// next to the type; each caller keeps its own outer error
    /// wrapping and its own package-name validation.
    pub fn from_path_field(
        package: &str,
        raw_path: Option<String>,
    ) -> Result<PatchSource, PatchValidationError> {
        match raw_path {
            Some(path) if !path.trim().is_empty() => Ok(PatchSource::Path {
                path: PathBuf::from(path.trim()),
            }),
            _ => Err(PatchValidationError::MissingSource {
                package: package.to_owned(),
            }),
        }
    }
}

/// Provenance label for a patch entry. Mirrors the precedence
/// ladder Cabin walks for patch resolution and is surfaced
/// verbatim in `cabin metadata` so users can audit which file
/// supplied each active patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatchProvenance {
    /// The workspace-root `cabin.toml`'s `[patch]` table.
    Manifest,
    /// A `.cabin/config.toml`'s `[patch]` table. The inner
    /// [`ConfigValueSource`] identifies which config file
    /// supplied the value.
    Config(ConfigValueSource),
}

impl PatchProvenance {
    /// Stable lower-case label used in JSON output, matching the
    /// `value_source` keys from the config layer.
    pub fn as_key(self) -> String {
        match self {
            PatchProvenance::Manifest => "manifest".to_owned(),
            PatchProvenance::Config(source) => source.as_key().to_owned(),
        }
    }
}

impl fmt::Display for PatchProvenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_key())
    }
}

/// One patch entry as declared in a single source file. Carries
/// the relative `source` value plus the absolute path of the file
/// that declared it so the orchestration layer can resolve any
/// relative paths against the right base directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredPatch {
    pub source: PatchSource,
    /// Absolute path of the file that declared this patch
    /// (`cabin.toml` for manifest patches, `.cabin/config.toml`
    /// for config patches). Used as the base for resolving
    /// relative `path` values.
    pub declared_in: PathBuf,
    pub provenance: PatchProvenance,
}

/// Workspace-root manifest's `[patch]` declarations. Member
/// manifests cannot declare patches — the workspace loader
/// rejects them — so reading off the root is sufficient.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchManifestSettings {
    /// `(package name -> source)`. Iteration is deterministic
    /// via [`BTreeMap`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entries: BTreeMap<crate::PackageName, PatchSource>,
}

impl PatchManifestSettings {
    /// Whether the table carries no entries. Mirrors the
    /// `is_empty` helpers on the other workspace-root-only
    /// settings types so the workspace loader can reject member
    /// manifests with a uniform check.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Errors produced while validating patch declarations. Wording
/// is intentionally stable so integration tests can match
/// substrings.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PatchValidationError {
    /// A patch table did not declare any source. The expected
    /// shape is `{ path = "..." }`; the parser surfaces this when
    /// no recognized key was supplied.
    #[error("patch for package `{package}` is missing a source; expected `path = \"...\"`")]
    MissingSource { package: String },

    /// The patched package directory does not contain a
    /// `cabin.toml`. Cabin prefers a clear early error to the
    /// later confusing "manifest not found" failure.
    #[error(
        "patch for package `{package}` points to `{path}`, but that path does not contain a cabin.toml"
    )]
    MissingManifest { package: String, path: String },

    /// The patched package's manifest declares a different
    /// `[package].name` than the patch table key.
    #[error(
        "patch for package `{package}` points to package `{actual}`; patch package name must match `{package}`"
    )]
    PackageNameMismatch { package: String, actual: String },

    /// The patched package's version does not satisfy the
    /// version requirement of an active dependency on it.
    #[error(
        "patch package `{package}` has version `{version}`, which does not satisfy dependency requirement `{requirement}`"
    )]
    VersionMismatch {
        package: String,
        version: String,
        requirement: String,
    },

    /// The same package name appears in two patch declarations
    /// at the same precedence level. Across precedence levels
    /// the higher level overrides; *within* a level, duplicates
    /// are rejected so two co-equal config files cannot silently
    /// disagree about a patch.
    #[error(
        "multiple patches for package `{package}` are active at the same precedence level; remove one patch declaration"
    )]
    DuplicateAtSameLevel { package: String },
}
