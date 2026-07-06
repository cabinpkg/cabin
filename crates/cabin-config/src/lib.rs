//! Typed configuration files for Cabin.
//!
//! Cabin loads a small, deterministic set of TOML files -
//! optionally a per-user file, optionally a workspace-level or
//! package-local file - and merges them into an
//! [`EffectiveConfig`] that the rest of the CLI consumes as
//! defaults.
//!
//! Crate boundaries:
//! - this crate must not depend on `clap`, `cabin-toolchain`,
//!   `cabin-build`, or any other planning / resolver / artifact
//!   crate;
//! - manifest parsing lives in `cabin-manifest` - config files are
//!   *local policy*, not package source spec, and the two grammars
//!   stay separate;
//! - CLI orchestration (passing the effective config to
//!   resolvers, paths, and the metadata view) lives in
//!   `cabin`.
//!
//! Network access: none.  Discovery walks the filesystem and reads
//! local files.  The resulting [`EffectiveConfig`] may carry an
//! index URL but Cabin only contacts that URL when a command
//! already needs the index.
//!
//! Determinism: discovery returns a stable, ordered list of
//! [`LoadedConfigFile`]s; merge is field-wise with explicit
//! precedence (lower-priority files come first, higher-priority
//! files override); every value carries a [`ConfigSource`] so
//! `cabin metadata` can show why each effective value was picked.

mod discovery;
mod effective;
mod error;
mod parse;
mod source;

mod raw;

pub use discovery::{
    CABIN_CONFIG_ENV, CABIN_CONFIG_HOME_ENV, CABIN_NO_CONFIG_ENV, ConfigDiscovery,
    ConfigDiscoveryInputs, EnvLookup, WorkspaceLayout, discover_config_files,
};
pub use effective::{
    EffectiveBuild, EffectiveBuildJobs, EffectiveColor, EffectiveCompilerWrapper, EffectiveConfig,
    EffectivePatch, EffectivePathSetting, EffectivePaths, EffectiveProfile, EffectiveRegistry,
    EffectiveRegistrySource, EffectiveResolver, EffectiveTerm, EffectiveTool, EffectiveToolchain,
    EffectiveVerbosity, merge_loaded_files,
};
pub use error::{ConfigError, ConfigParseError};
pub use parse::{
    ParsedConfig, ParsedSourceReplacement, ParsedTerm, redact_userinfo, url_contains_credentials,
};
pub use source::{ConfigSource, LoadedConfigFile, SourcedValue};
