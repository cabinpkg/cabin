//! Errors produced while loading or interpreting Cabin config.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Top-level error from the config layer.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// `CABIN_CONFIG` was set to a path Cabin could not read.  The
    /// user named the file, so a missing or unreadable one is an
    /// error, never a fallback to discovery.
    #[error(
        "config file `{path}` was requested explicitly but could not be read: {source}",
        path = path.display(),
    )]
    ExplicitConfigRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// A file Cabin discovered (workspace, package, or user) failed
    /// to read with an I/O error.  Non-existent discovered files are
    /// not an error - only files Cabin found and then could not
    /// open surface this variant.
    #[error("failed to read config file `{path}`: {source}", path = path.display())]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// Parse / validation failure for a specific config file.
    #[error("failed to parse config file `{path}`: {source}", path = path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: ConfigParseError,
    },

    /// A workspace or package config file that is not also the user
    /// config file declared `[term]`.  Terminal
    /// settings take effect before Cabin locates a workspace, so only
    /// the user config file and `CABIN_CONFIG` can set them; accepting
    /// the table here would ignore it silently.
    #[error(
        "config file `{path}` sets `[term]`, which only the user config file or `CABIN_CONFIG` can \
         set, as terminal output is configured before Cabin reads project config; move the table \
         to the user config file, or use `CABIN_TERM_COLOR`, `CABIN_TERM_VERBOSE`, or \
         `CABIN_TERM_QUIET`",
        path = path.display(),
    )]
    TermOutsideUserConfig { path: PathBuf },

    /// A config-layer boolean env var (e.g. `CABIN_NO_CONFIG`)
    /// carried a value outside the documented truthy / falsy
    /// spellings.  Silently ignoring the value would load config
    /// files the user asked to disable.
    #[error("invalid {variable} value {value:?}: {source}")]
    InvalidBoolEnv {
        variable: &'static str,
        value: String,
        #[source]
        source: cabin_env::BoolError,
    },

    /// A config file Cabin located lives at a path that is not valid
    /// UTF-8.  Cabin's config model assumes UTF-8 paths, so an
    /// otherwise-readable file under a non-UTF-8 directory surfaces
    /// here rather than aborting the process.
    #[error("config file path `{path}` is not valid UTF-8", path = path.display())]
    NonUtf8Path { path: PathBuf },
}

/// Parse / validation errors for a single config file.
///
/// Wording is deliberately stable so integration tests can match
/// substrings.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigParseError {
    /// TOML syntax error.
    #[error("invalid TOML: {0}")]
    Toml(String),

    /// `[registry]` declared both `index-path` and `index-url` in
    /// the same file; a single config file may only declare one
    /// index source.
    #[error(
        "config key `registry.index-path` conflicts with `registry.index-url`; choose only one at the same precedence level"
    )]
    RegistryConflict,

    /// `registry.index-path` was empty / whitespace.
    #[error("config key `registry.index-path` must be a non-empty path")]
    EmptyIndexPath,

    /// `registry.index-url` was empty / whitespace.
    #[error("config key `registry.index-url` must be a non-empty URL")]
    EmptyIndexUrl,

    /// `registry.index-url` carried `userinfo` (e.g.
    /// `https://user:pass@example.com/`).  Cabin's config layer
    /// does not handle credentials; rejecting the URL up front
    /// keeps the `user:password` from flowing into the metadata
    /// view, the HTTP transport, or error output.
    #[error("config key `registry.index-url` must not contain credentials: `{url}`")]
    IndexUrlContainsCredentials { url: String },

    /// `paths.cache-dir` / `paths.build-dir` was empty.
    #[error("config key `paths.{key}` must be a non-empty path")]
    EmptyPath { key: &'static str },

    /// `build.profile` was empty.
    #[error("config key `build.profile` must be a non-empty profile name")]
    EmptyProfile,

    /// `[toolchain].cc` / `cxx` / `ar` was empty / whitespace.
    #[error("config key `toolchain.{key}` must be a non-empty tool spec")]
    EmptyToolSpec { key: &'static str },

    /// A `[runner]` or `[sysroot]` key is not a target triple.
    #[error("config table `[{table}]` key `{triple}` is not a target triple: {source}")]
    InvalidTripleKey {
        table: &'static str,
        triple: String,
        source: cabin_core::TargetTripleError,
    },

    /// A `[runner]` entry names no program.
    #[error("config key `runner.{triple}` must start with a non-empty program")]
    EmptyRunner { triple: String },

    /// A `[sysroot]` entry is empty.
    #[error("config key `sysroot.{triple}` must be a non-empty path")]
    EmptySysroot { triple: String },

    /// `[term].color` carried a value that is not one of
    /// `auto` / `always` / `never`.  The wording lists the legal
    /// set so the user can fix the file without consulting the
    /// docs.
    #[error("config key `term.color` is invalid: {0}")]
    InvalidTermColor(cabin_core::InvalidColorChoice),

    /// `[term].verbose = true` and `[term].quiet = true` were
    /// declared in the same file.  The combination is rejected
    /// at parse time so the rest of the workspace never observes
    /// a contradictory verbosity setting.
    #[error("config keys `term.verbose` and `term.quiet` cannot both be true")]
    InvalidTermVerbosityCombination,

    /// `[build] compiler-wrapper` carried an invalid value. Wraps the typed error returned by
    /// [`cabin_core::CompilerWrapperRequest::parse`].
    #[error("config key `build.compiler-wrapper` is invalid: {0}")]
    InvalidCompilerWrapper(cabin_core::CompilerWrapperParseError),

    /// `build.jobs` was zero, negative, or otherwise outside
    /// the supported range.
    #[error("config key `build.jobs` is invalid: got {value}, expected a positive integer")]
    InvalidBuildJobs {
        /// Stringified offending value.
        value: String,
    },

    /// `[target.'cfg(...)']` (or any other target-conditioned
    /// table) appeared in a config file.  Target-conditioned config
    /// is not supported; the equivalent feature
    /// belongs in the package manifest where conditional
    /// `[target.'cfg(...)'.<...>]` tables already exist.
    #[error(
        "target-conditioned config tables are not supported; move `[target.'cfg(...)'.{table}]` to the package manifest"
    )]
    TargetConditionedNotSupported { table: String },

    /// A top-level authentication / credential table appeared.  Cabin
    /// config does not store credentials, so these tables get this
    /// error rather than [`ConfigParseError::UnknownTopLevelTable`].
    #[error(
        "config key `{key}` is not supported; Cabin config does not handle credentials, tokens, or registry authentication"
    )]
    UnsupportedAuthKey { key: &'static str },

    /// `[resolver] incompatible-standards` carried a value that is
    /// not one of `allow` / `fallback`.  Wraps the typed error from
    /// [`cabin_core::IncompatibleStandards::parse`].
    #[error("config key `resolver.incompatible-standards` is invalid: {0}")]
    InvalidIncompatibleStandards(cabin_core::UnknownIncompatibleStandards),

    /// `gc.max-age` was zero, negative, or too large.
    #[error("config key `gc.max-age` is invalid: got {value}, expected a positive number of days")]
    InvalidGcMaxAge { value: i64 },

    /// `gc.max-size` did not parse as a size.
    #[error("config key `gc.max-size` is invalid: {0}")]
    InvalidGcMaxSize(cabin_core::ByteSizeParseError),

    /// A top-level table the parser did not recognize.  Lists the
    /// supported tables so users can see the full surface.
    #[error(
        "unknown top-level config table `{table}`; supported tables are: registry, paths, build, resolver, gc, toolchain, term, patch, source-replacement, runner, sysroot"
    )]
    UnknownTopLevelTable { table: String },

    /// A `[patch]` package name was structurally invalid.  The
    /// inner string carries the validator's stable wording.
    #[error("invalid patch package name: {0}")]
    InvalidPatchPackageName(String),

    /// A `[patch]` row failed validation (missing source,
    /// unsupported source kind, …).
    #[error("invalid `[patch]` entry for `{package}`: {source}")]
    InvalidPatch {
        package: String,
        #[source]
        source: cabin_core::PatchValidationError,
    },

    /// A `[source-replacement]` row failed validation
    /// (ambiguous, missing target, unsupported kind, credential
    /// in URL, …).
    #[error("invalid `[source-replacement]` entry for `{original}`: {source}")]
    InvalidSourceReplacement {
        original: String,
        #[source]
        source: cabin_core::SourceReplacementError,
    },
}
