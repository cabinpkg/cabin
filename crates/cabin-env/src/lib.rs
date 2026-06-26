//! `CABIN_*` environment variable name constants and the typed
//! builder for the `cabin run` / `cabin test` package-execution
//! overlay.
//!
//! Cabin is Cargo-inspired (not Cargo-compatible): the env vars
//! it reads on the *input* side and the env vars it sets on the
//! *output* side both follow Cargo's naming conventions where
//! the semantics line up, and diverge with `CABIN_*` names where
//! Cabin's C/C++ semantics differ.  This crate is the single
//! source of truth for both halves so the rest of the codebase
//! agrees on names.
//!
//! Crate boundaries:
//! - this crate must not run processes, read configuration
//!   files, or touch the filesystem;
//! - it must not depend on `cabin`, `cabin-build`, or other
//!   higher-level crates that would create cyclic dependencies;
//! - it consumes typed inputs and produces typed outputs (the
//!   orchestration layer is responsible for mapping resolved
//!   values into [`PackageEnvInputs`]).
//!
//! ## Read-side env vars
//!
//! Constants for every `CABIN_*` variable Cabin's CLI reads
//! live as `pub const ... : &str = "..."` in this crate.  The
//! orchestration layer reads each one through `std::env::var`
//! (or an injected `env_fn` for tests) and threads the value
//! through to the right resolver.
//!
//! ## Run / test overlay
//!
//! `cabin run` and `cabin test` inject exactly the same small,
//! stable set of package-execution variables built by
//! [`package_env`].  The overlay is layered on top of the
//! inherited environment; it never clears the user's `PATH`,
//! `LANG`, etc.

pub mod build_flags;

pub use build_flags::{
    CFLAGS, CPPFLAGS, CXXFLAGS, EnvBuildFlags, EnvBuildFlagsError, LDFLAGS, parse_env_build_flags,
};

use std::collections::BTreeMap;
use std::ffi::OsString;

use thiserror::Error;

// ---------------------------------------------------------------------------
// Read-side env var name constants
// ---------------------------------------------------------------------------

/// Path to a single explicit Cabin config file.  When set, no
/// other config files are loaded.
pub const CABIN_CONFIG: &str = "CABIN_CONFIG";

/// Override for the per-user config home (the directory under
/// which Cabin looks for `config.toml`).  Honored by the
/// `cabin-config` crate's discovery layer.
pub const CABIN_CONFIG_HOME: &str = "CABIN_CONFIG_HOME";

/// When truthy, Cabin loads no config files at all.  Used by the
/// integration test harness so a developer's
/// `~/.config/cabin/config.toml` cannot leak into tests.
pub const CABIN_NO_CONFIG: &str = "CABIN_NO_CONFIG";

/// Build output directory.  Honored by commands that write to,
/// read from, or deliberately exclude the build directory:
/// `cabin build`, `cabin clean`, `cabin run`, `cabin test`,
/// `cabin fmt`, and `cabin tidy`.
///
/// Precedence: CLI flag (`--build-dir`) > env var > config
/// (`[paths] build-dir`) > built-in default (`build`).
pub const CABIN_BUILD_DIR: &str = "CABIN_BUILD_DIR";

/// Override for the artifact cache directory for a single
/// invocation.  Honored by every command that resolves an
/// artifact cache.  Wins over `CABIN_CACHE_HOME` and the platform
/// fallbacks below it.
pub const CABIN_CACHE_DIR: &str = "CABIN_CACHE_DIR";

/// Override for the per-user cache home - the directory cabin's
/// global cache lives under.  Defaults to the platform user cache
/// directory with a `cabin` suffix (`$XDG_CACHE_HOME/cabin` /
/// `~/.cache/cabin` on Linux).  Mirrors the precedence shape
/// `CABIN_CONFIG_HOME` uses for the per-user config home.
///
/// Distinct from `CABIN_CACHE_DIR`: use `CABIN_CACHE_HOME` to
/// relocate the *user* cache home (every project's cache moves
/// together); use `CABIN_CACHE_DIR` to point a single invocation
/// at a specific cache directory.
pub const CABIN_CACHE_HOME: &str = "CABIN_CACHE_HOME";

/// Forbid network access for this invocation.  Equivalent to
/// passing `--offline` on the CLI.  The CLI flag still takes
/// precedence; the env var only sets the default.
pub const CABIN_NET_OFFLINE: &str = "CABIN_NET_OFFLINE";

/// Compiler-cache wrapper selector (`ccache`, `sccache`,
/// `none`).  Honored by `cabin-toolchain`'s wrapper resolver.
pub const CABIN_COMPILER_WRAPPER: &str = "CABIN_COMPILER_WRAPPER";

/// Override for the `clang-format` executable Cabin spawns
/// from `cabin fmt`.  When set and non-empty, the value is
/// used verbatim (typically an absolute path) and the `PATH`
/// lookup is skipped.  When unset, Cabin spawns `clang-format`
/// from `PATH`.
pub const CABIN_FMT: &str = "CABIN_FMT";

/// Override for the `run-clang-tidy` executable Cabin spawns
/// from `cabin tidy`.  Same shape as [`CABIN_FMT`]: when set and
/// non-empty the value is used verbatim and the `PATH` lookup is
/// skipped, otherwise Cabin resolves `run-clang-tidy` against
/// `PATH`.
pub const CABIN_TIDY: &str = "CABIN_TIDY";

/// Override for the `pkg-config` executable Cabin spawns when
/// probing `system = true` dependencies.  Same shape as
/// [`CABIN_FMT`] and the other Cabin tool overrides: when set
/// and non-empty the value is used verbatim (typically an
/// absolute path) and the `PATH` lookup is skipped; when unset,
/// Cabin spawns `pkg-config` from `PATH`.  Cabin only invokes
/// `pkg-config` when the workspace declares at least one
/// `system = true` entry.
pub const CABIN_PKG_CONFIG: &str = "CABIN_PKG_CONFIG";

/// Number of parallel jobs the build backend should use.
/// Cargo-style: positive integer, `0` is rejected.  Cabin
/// reads this env var when `--jobs` is not on the command
/// line.
///
/// Precedence: CLI `--jobs` flag > env var > `[build] jobs`
/// config setting > backend default.
pub const CABIN_BUILD_JOBS: &str = "CABIN_BUILD_JOBS";

/// Terminal-color selector (`auto`, `always`, or `never`).
/// Honored by the CLI when `--color` is not present.
pub const CABIN_TERM_COLOR: &str = "CABIN_TERM_COLOR";

/// Enable verbose Cabin-owned status output when no `-v` /
/// `--verbose` CLI flag is present.
pub const CABIN_TERM_VERBOSE: &str = "CABIN_TERM_VERBOSE";

/// Suppress Cabin-owned status output when no `-q` /
/// `--quiet` CLI flag is present.
pub const CABIN_TERM_QUIET: &str = "CABIN_TERM_QUIET";

// ---------------------------------------------------------------------------
// Package-execution env var name constants
// ---------------------------------------------------------------------------
//
// The fixed names Cabin sets on the `cabin run` / `cabin test`
// child processes.  This is the entire injected contract.

/// Absolute path to the package's manifest directory.
pub const CABIN_MANIFEST_DIR: &str = "CABIN_MANIFEST_DIR";

/// Absolute path to the package's `cabin.toml` manifest.
pub const CABIN_MANIFEST_PATH: &str = "CABIN_MANIFEST_PATH";

/// Package name in the form the manifest declares.
pub const CABIN_PACKAGE_NAME: &str = "CABIN_PACKAGE_NAME";

/// Resolved package version (`<major>.<minor>.<patch>` plus any
/// pre-release / build suffix exactly as the manifest declares).
pub const CABIN_PACKAGE_VERSION: &str = "CABIN_PACKAGE_VERSION";

/// Active profile name (`dev`, `release`, or any custom
/// profile).
pub const CABIN_PROFILE: &str = "CABIN_PROFILE";

// ---------------------------------------------------------------------------
// Package-execution env builder
// ---------------------------------------------------------------------------

/// Inputs for [`package_env`].  The orchestration layer fills
/// this in from already-resolved typed values. `cabin run` and
/// `cabin test` use the same shape - the injected overlay does
/// not depend on whether the target is a binary or a test.
#[derive(Debug, Clone)]
pub struct PackageEnvInputs<'a> {
    /// Manifest directory of the package owning the target.
    pub manifest_dir: &'a std::path::Path,
    /// `cabin.toml` path of the package owning the target.
    pub manifest_path: &'a std::path::Path,
    /// Package name as the manifest declared it.
    pub package_name: &'a str,
    /// Resolved package version.
    pub package_version: &'a str,
    /// Resolved profile name (`dev`, `release`, …).
    pub profile: &'a str,
    /// Resolved build directory.
    pub build_dir: &'a std::path::Path,
}

/// Build the `CABIN_*` overlay surfaced to a `cabin run` /
/// `cabin test` child process.  Returns a deterministic
/// `BTreeMap` so two calls with the same inputs are byte-equal.
/// Infallible: every value is copied straight from the typed
/// inputs.
#[must_use]
pub fn package_env(inputs: &PackageEnvInputs<'_>) -> BTreeMap<String, OsString> {
    let mut out = BTreeMap::new();
    out.insert(
        CABIN_MANIFEST_DIR.to_owned(),
        inputs.manifest_dir.as_os_str().to_owned(),
    );
    out.insert(
        CABIN_MANIFEST_PATH.to_owned(),
        inputs.manifest_path.as_os_str().to_owned(),
    );
    out.insert(
        CABIN_PACKAGE_NAME.to_owned(),
        OsString::from(inputs.package_name),
    );
    out.insert(
        CABIN_PACKAGE_VERSION.to_owned(),
        OsString::from(inputs.package_version),
    );
    out.insert(CABIN_PROFILE.to_owned(), OsString::from(inputs.profile));
    out.insert(
        CABIN_BUILD_DIR.to_owned(),
        inputs.build_dir.as_os_str().to_owned(),
    );
    out
}

// ---------------------------------------------------------------------------
// Truthy / boolean parsing for read-side env vars
// ---------------------------------------------------------------------------

/// Whether a raw env-var value should be treated as truthy.
/// Mirrors Cargo: any of `1`, `true`, `yes`, `on` (case-
/// insensitive) is truthy; an empty string is falsy; anything
/// else is rejected via [`BoolError::Invalid`].
///
/// # Errors
/// Returns [`BoolError::Invalid`] when `value` is non-empty and
/// matches none of the recognized truthy or falsy spellings,
/// carrying the offending input string.
pub fn parse_bool(value: &str) -> Result<bool, BoolError> {
    if value.is_empty() {
        return Ok(false);
    }
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(BoolError::Invalid(value.to_owned())),
    }
}

/// Errors produced by [`parse_bool`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BoolError {
    /// The string did not match any documented truthy / falsy
    /// spelling.
    #[error("expected one of `1`, `0`, `true`, `false`, `yes`, `no`, `on`, `off`; got `{0}`")]
    Invalid(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bool_recognizes_documented_truthy_and_falsy_spellings() {
        for v in ["1", "true", "TRUE", "yes", "On"] {
            assert!(parse_bool(v).unwrap(), "expected truthy: {v:?}");
        }
        for v in ["0", "false", "no", "off"] {
            assert!(!parse_bool(v).unwrap(), "expected falsy: {v:?}");
        }
        assert!(!parse_bool("").unwrap());
    }

    #[test]
    fn parse_bool_rejects_unknown_spellings() {
        assert!(matches!(parse_bool("maybe"), Err(BoolError::Invalid(_))));
    }

    #[test]
    fn parse_bool_error_wording_includes_raw_value() {
        let err = parse_bool("perhaps").unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("perhaps"),
            "error should echo the input: {rendered}"
        );
        // The wording must list every recognized spelling so
        // users see how to fix it.
        assert!(
            rendered.contains("true") && rendered.contains("false"),
            "{rendered}"
        );
    }

    #[test]
    fn package_env_emits_exactly_the_six_strict_keys() {
        use std::path::PathBuf;
        let manifest_dir = PathBuf::from("/abs/app");
        let manifest_path = PathBuf::from("/abs/app/cabin.toml");
        let build_dir = PathBuf::from("/abs/app/build");
        let env = package_env(&PackageEnvInputs {
            manifest_dir: &manifest_dir,
            manifest_path: &manifest_path,
            package_name: "my-pkg",
            package_version: "0.1.0",
            profile: "dev",
            build_dir: &build_dir,
        });
        let names: Vec<&str> = env.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            vec![
                CABIN_BUILD_DIR,
                CABIN_MANIFEST_DIR,
                CABIN_MANIFEST_PATH,
                CABIN_PACKAGE_NAME,
                CABIN_PACKAGE_VERSION,
                CABIN_PROFILE,
            ],
            "package_env must emit exactly the six strict keys in BTreeMap order"
        );
        assert_eq!(env.get(CABIN_PACKAGE_NAME).unwrap(), "my-pkg");
        assert_eq!(env.get(CABIN_PACKAGE_VERSION).unwrap(), "0.1.0");
        assert_eq!(env.get(CABIN_PROFILE).unwrap(), "dev");
        assert_eq!(env.get(CABIN_BUILD_DIR).unwrap(), "/abs/app/build");
    }

    #[test]
    fn package_env_does_not_emit_any_removed_variable() {
        use std::path::PathBuf;
        let dir = PathBuf::from("/abs/app");
        let path = PathBuf::from("/abs/app/cabin.toml");
        let env = package_env(&PackageEnvInputs {
            manifest_dir: &dir,
            manifest_path: &path,
            package_name: "demo",
            package_version: "0.1.0",
            profile: "release",
            build_dir: &dir,
        });
        for removed in [
            "CABIN",
            "CABIN_PACKAGE_NAME_CANONICAL",
            "CABIN_BIN_NAME",
            "CABIN_BIN_NAME_CANONICAL",
            "CABIN_TEST_NAME",
            "CABIN_TEST_NAME_CANONICAL",
            "CABIN_TARGET_KIND",
            "CABIN_TARGET_TRIPLE",
            "CABIN_HOST_TRIPLE",
            "CABIN_BUILD_CONFIGURATION_FINGERPRINT",
        ] {
            assert!(
                !env.contains_key(removed),
                "removed variable `{removed}` must not be injected"
            );
        }
    }

    #[test]
    fn package_env_is_byte_stable_for_equal_inputs() {
        use std::path::PathBuf;
        let dir = PathBuf::from("/abs/app");
        let path = PathBuf::from("/abs/app/cabin.toml");
        let mk = || {
            package_env(&PackageEnvInputs {
                manifest_dir: &dir,
                manifest_path: &path,
                package_name: "demo",
                package_version: "0.1.0",
                profile: "dev",
                build_dir: &dir,
            })
        };
        assert_eq!(mk(), mk());
    }
}
