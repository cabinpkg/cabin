//! Serde structures mirroring the on-disk `.cabin/config.toml`
//! shape.  They are `pub(crate)`: `parse.rs` converts them into
//! validated `cabin-core` or crate-local types, so no other crate
//! depends on the raw layout.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;

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

/// One row in a config file's `[source-replacement]` table.  The
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
/// Unknown top-level keys land in the flattened `extra` map, so
/// validation can reject credential keys as
/// [`crate::ConfigParseError::UnsupportedAuthKey`] and everything
/// else as [`crate::ConfigParseError::UnknownTopLevelTable`], whose
/// message lists the supported tables.
///
/// `target` is captured separately and rejected as
/// [`crate::ConfigParseError::TargetConditionedNotSupported`], which
/// points users to the package manifest.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct RawConfig {
    #[serde(default)]
    pub(crate) registry: Option<RawRegistry>,
    #[serde(default)]
    pub(crate) paths: Option<RawPaths>,
    #[serde(default)]
    pub(crate) build: Option<RawBuild>,
    #[serde(default)]
    pub(crate) resolver: Option<RawResolver>,
    #[serde(default)]
    pub(crate) gc: Option<RawGc>,
    #[serde(default)]
    pub(crate) toolchain: Option<RawToolchain>,
    #[serde(default)]
    pub(crate) term: Option<RawTerm>,
    #[serde(default)]
    pub(crate) trust: Option<RawTrust>,
    #[serde(default)]
    pub(crate) target: Option<BTreeMap<String, toml::Value>>,
    #[serde(default)]
    pub(crate) patch: Option<BTreeMap<String, RawConfigPatch>>,
    #[serde(default, rename = "source-replacement")]
    pub(crate) source_replacement: Option<BTreeMap<String, RawConfigSourceReplacement>>,
    /// `[runner]`: `<triple> = ["program", "arg", ...]`.
    #[serde(default)]
    pub(crate) runner: Option<BTreeMap<String, Vec<String>>>,
    /// `[sysroot]`: `<triple> = "<dir>"`.
    #[serde(default)]
    pub(crate) sysroot: Option<BTreeMap<String, Utf8PathBuf>>,
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
    pub(crate) cache_dir: Option<Utf8PathBuf>,
    #[serde(default, rename = "build-dir")]
    pub(crate) build_dir: Option<Utf8PathBuf>,
}

/// Shape of `[build]` in a config file.  Keep this *minimal* -
/// adding new keys here means adding a new piece of typed defaults
/// to [`crate::EffectiveConfig`] and a new metadata field.  Anything
/// that would change package semantics (defines, include dirs,
/// extra args) belongs in the package manifest, not the config
/// file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBuild {
    #[serde(default)]
    pub(crate) profile: Option<String>,
    #[serde(default, rename = "compiler-wrapper")]
    pub(crate) compiler_wrapper: Option<String>,
    /// `build.jobs` - number of parallel jobs Cabin asks the
    /// build backend to use.  Stored as `i64` so the parser
    /// can produce a clear "got 0" / "got negative" message
    /// before handing the value to the typed
    /// [`cabin_core::BuildJobs`] validator.
    #[serde(default)]
    pub(crate) jobs: Option<i64>,
}

/// Shape of `[resolver]` in a config file.  Holds the standard-aware
/// version-preference knob; the value is validated into
/// [`cabin_core::IncompatibleStandards`] in `parse.rs`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawResolver {
    #[serde(default, rename = "incompatible-standards")]
    pub(crate) incompatible_standards: Option<String>,
}

/// Shape of `[gc]` in a config file: build-output garbage collection.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawGc {
    #[serde(default)]
    pub(crate) auto: Option<bool>,
    #[serde(default, rename = "max-age")]
    pub(crate) max_age: Option<i64>,
    #[serde(default, rename = "max-size")]
    pub(crate) max_size: Option<String>,
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

/// Shape of `[trust]` in a config file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawTrust {
    #[serde(default)]
    pub(crate) projects: Vec<Utf8PathBuf>,
}

/// Shape of `[term]` in a config file.
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
