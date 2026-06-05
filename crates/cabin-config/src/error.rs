//! Errors produced while loading or interpreting Cabin config.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Top-level error from the config layer.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// `CABIN_CONFIG` was set to a path Cabin could not read. The
    /// explicit-config path is treated as a hard requirement
    /// because the user opted in by name; missing or unreadable
    /// files there must surface clearly rather than silently
    /// falling back to discovery.
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
    /// to read with an I/O error. Non-existent discovered files are
    /// not an error — only files Cabin found and then could not
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

    /// A config file Cabin located lives at a path that is not valid
    /// UTF-8. Cabin's config model assumes UTF-8 paths, so an
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
    /// `https://user:pass@example.com/`). Cabin's config layer
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

    /// `[term].color` carried a value that is not one of
    /// `auto` / `always` / `never`. The wording lists the legal
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

    /// `[build.cache] compiler-wrapper` carried an unsupported
    /// value. Wraps the typed error returned by
    /// [`cabin_core::CompilerWrapperRequest::parse`].
    #[error("config key `build.cache.compiler-wrapper` is invalid: {0}")]
    InvalidCompilerWrapper(cabin_core::CompilerWrapperParseError),

    /// `build.jobs` was zero, negative, or otherwise outside
    /// the supported range.  Carries the offending value
    /// exactly as it appeared in the file so the diagnostic
    /// quotes what the user wrote.
    #[error("config key `build.jobs` is invalid: got {value}, expected a positive integer")]
    InvalidBuildJobs {
        /// Stringified offending value.
        value: String,
    },

    /// `[target.'cfg(...)']` (or any other target-conditioned
    /// table) appeared in a config file. Target-conditioned config
    /// is not supported in this version; the equivalent feature
    /// belongs in the package manifest where conditional
    /// `[target.'cfg(...)'.<...>]` tables already exist.
    #[error(
        "target-conditioned config tables are not supported; move `[target.'cfg(...)'.{table}]` to the package manifest"
    )]
    TargetConditionedNotSupported { table: String },

    /// One of the documented unsupported authentication /
    /// credential keys appeared. Cabin's config file is not a
    /// secrets store — the rejection is structural so a typo never
    /// silently smuggles a credential into a published archive.
    #[error(
        "config key `{key}` is not supported; Cabin config does not handle credentials, tokens, or registry authentication"
    )]
    UnsupportedAuthKey { key: &'static str },

    /// A top-level table the parser did not recognize. Lists the
    /// supported tables so users can see the full surface.
    #[error(
        "unknown top-level config table `{table}`; supported tables are: registry, paths, build, toolchain, term, patch, source-replacement"
    )]
    UnknownTopLevelTable { table: String },

    /// A `[patch]` package name was structurally invalid. The
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
