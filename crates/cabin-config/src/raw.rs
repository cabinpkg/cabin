//! Private serde structures that mirror the on-disk
//! `.cabin/config.toml` shape. These types live behind
//! `pub(crate)` so the rest of the workspace never depends on the
//! raw layout.
//!
//! The parser in `parse.rs` immediately turns these into typed
//! values from `cabin-core` (or this crate's own typed model) so
//! downstream code only ever sees validated data.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

/// One row in a config file's `[patch]` table.
///
/// Only `path = "..."` is supported; every other key is rejected
/// by `deny_unknown_fields` as an unknown field.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfigPatch {
    #[serde(default)]
    pub(crate) path: Option<String>,
}

/// One row in a config file's `[source-replacement]` table. The
/// `original` is the table key (a URL or filesystem path); the
/// row carries exactly one of `index-path` or `index-url`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawConfigSourceReplacement {
    #[serde(default, rename = "index-path")]
    pub(crate) index_path: Option<String>,
    #[serde(default, rename = "index-url")]
    pub(crate) index_url: Option<String>,
}

// ---------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------

/// Top-level shape of a `.cabin/config.toml` file.
///
/// Unknown top-level tables surface through the catch-all
/// `extra` map and become a [`crate::ConfigParseError::UnknownTopLevelTable`]
/// during validation. Going through serde's `flatten` means a
/// helpful error message can name the offending table rather than
/// the generic "unknown field" wording `deny_unknown_fields` would
/// produce.
///
/// `target` is captured separately — and rejected — so users see
/// "target-conditioned config tables are not supported" instead of
/// "unknown table `target`", which would be misleading.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct RawConfig {
    #[serde(default)]
    pub(crate) registry: Option<RawRegistry>,
    #[serde(default)]
    pub(crate) paths: Option<RawPaths>,
    #[serde(default)]
    pub(crate) build: Option<RawBuild>,
    #[serde(default)]
    pub(crate) toolchain: Option<RawToolchain>,
    #[serde(default)]
    pub(crate) term: Option<RawTerm>,
    #[serde(default)]
    pub(crate) target: Option<BTreeMap<String, toml::Value>>,
    #[serde(default)]
    pub(crate) patch: Option<BTreeMap<String, RawConfigPatch>>,
    #[serde(default, rename = "source-replacement")]
    pub(crate) source_replacement: Option<BTreeMap<String, RawConfigSourceReplacement>>,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawRegistry {
    #[serde(default, rename = "index-path")]
    pub(crate) index_path: Option<String>,
    #[serde(default, rename = "index-url")]
    pub(crate) index_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawPaths {
    #[serde(default, rename = "cache-dir")]
    pub(crate) cache_dir: Option<PathBuf>,
    #[serde(default, rename = "build-dir")]
    pub(crate) build_dir: Option<PathBuf>,
}

/// Shape of `[build]` in a config file. Keep this *minimal* —
/// adding new keys here means adding a new piece of typed defaults
/// to [`crate::EffectiveConfig`] and a new metadata field. Anything
/// that would change package semantics (defines, include dirs,
/// extra args) belongs in the package manifest, not the config
/// file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBuild {
    #[serde(default)]
    pub(crate) profile: Option<String>,
    #[serde(default)]
    pub(crate) cache: Option<RawBuildCache>,
    /// `build.jobs` — number of parallel jobs Cabin asks the
    /// build backend to use.  Stored as `i64` so the parser
    /// can produce a clear "got 0" / "got negative" message
    /// before handing the value to the typed
    /// [`cabin_core::BuildJobs`] validator.
    #[serde(default)]
    pub(crate) jobs: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBuildCache {
    #[serde(default, rename = "compiler-wrapper")]
    pub(crate) compiler_wrapper: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawToolchain {
    #[serde(default)]
    pub(crate) cc: Option<String>,
    #[serde(default)]
    pub(crate) cxx: Option<String>,
    #[serde(default)]
    pub(crate) ar: Option<String>,
}

/// Shape of `[term]` in a config file.  Supported keys: `color`,
/// `verbose`, and `quiet`.  New keys here must travel through
/// the same raw → parsed → effective pipeline as the rest of
/// the config crate; consumers must not read raw values directly.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTerm {
    #[serde(default)]
    pub(crate) color: Option<String>,
    #[serde(default)]
    pub(crate) verbose: Option<bool>,
    #[serde(default)]
    pub(crate) quiet: Option<bool>,
}
