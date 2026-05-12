//! CABIN_* environment variable name constants, package /
//! target name canonicalization, and typed env builders.
//!
//! Cabin is Cargo-inspired (not Cargo-compatible): the env vars
//! it reads on the *input* side and the env vars it sets on the
//! *output* side both follow Cargo's naming conventions where
//! the semantics line up, and diverge with `CABIN_*` names where
//! Cabin's C/C++ semantics differ. This crate is the single
//! source of truth for both halves so the rest of the codebase
//! agrees on names, capitalization, and canonicalization rules.
//!
//! Crate boundaries:
//! - this crate must not run processes, read configuration
//!   files, or touch the filesystem;
//! - it must not depend on `cabin-cli`, `cabin-build`, or other
//!   higher-level crates that would create cyclic dependencies;
//! - it consumes typed inputs and produces typed outputs (the
//!   orchestration layer is responsible for mapping resolved
//!   values into [`RunEnvInputs`] / [`TestEnvInputs`]).
//!
//! ## Read-side env vars
//!
//! Constants for every `CABIN_*` variable Cabin's CLI reads
//! live as `pub const ... : &str = "..."` in this crate. The
//! orchestration layer reads each one through `std::env::var`
//! (or an injected `env_fn` for tests) and threads the value
//! through to the right resolver.
//!
//! ## Canonicalization
//!
//! Package, feature, option, variant, and target names enter
//! the manifest in arbitrary case (`fmt`, `OpenSSL`,
//! `my-pkg.tools`). They cannot be embedded in env-var names
//! verbatim because env-var names traditionally use
//! `[A-Z0-9_]+`. [`canonicalize_name`] is the single rule:
//!
//! 1. uppercase ASCII letters;
//! 2. replace any byte that is *not* `A-Z`, `0-9`, or `_` with
//!    a single underscore;
//! 3. preserve runs (do not collapse), so `foo--bar` becomes
//!    `FOO__BAR` and round-trips uniquely;
//! 4. reject empty names eagerly so the canonicalised form is
//!    never the empty string.
//!
//! [`detect_collisions`] checks that a set of names produces a
//! set of distinct canonical forms; collisions are reported as
//! [`CanonicalCollision`] so the orchestration layer can render
//! a deterministic diagnostic that lists every offending name.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod build_flags;

pub use build_flags::{
    CFLAGS, CPPFLAGS, CXXFLAGS, EnvBuildFlags, EnvBuildFlagsError, LDFLAGS, ShellSplitError,
    parse_env_build_flags, shell_split,
};

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use thiserror::Error;

// ---------------------------------------------------------------------------
// Read-side env var name constants
// ---------------------------------------------------------------------------

/// Path to a single explicit Cabin config file. When set, no
/// other config files are loaded.
pub const CABIN_CONFIG: &str = "CABIN_CONFIG";

/// Override for the per-user config home (the directory under
/// which Cabin looks for `config.toml`). Honoured by the
/// `cabin-config` crate's discovery layer.
pub const CABIN_CONFIG_HOME: &str = "CABIN_CONFIG_HOME";

/// When truthy, Cabin loads no config files at all. Used by the
/// integration test harness so a developer's
/// `~/.config/cabin/config.toml` cannot leak into tests.
pub const CABIN_NO_CONFIG: &str = "CABIN_NO_CONFIG";

/// Build output directory. Honoured by commands that write to,
/// read from, or deliberately exclude the build directory:
/// `cabin build`, `cabin clean`, `cabin run`, `cabin test`,
/// `cabin fmt`, `cabin lint`, and `cabin tidy`.
///
/// Precedence: CLI flag (`--build-dir`) > env var > config
/// (`[paths] build-dir`) > built-in default (`build`).
pub const CABIN_BUILD_DIR: &str = "CABIN_BUILD_DIR";

/// Override for the artifact cache directory. Honoured by every
/// command that resolves an artifact cache.
pub const CABIN_CACHE_DIR: &str = "CABIN_CACHE_DIR";

/// Forbid network access for this invocation. Equivalent to
/// passing `--offline` on the CLI. The CLI flag still takes
/// precedence; the env var only sets the default.
pub const CABIN_NET_OFFLINE: &str = "CABIN_NET_OFFLINE";

/// Compiler-cache wrapper selector (`ccache`, `sccache`,
/// `none`). Honoured by `cabin-toolchain`'s wrapper resolver.
pub const CABIN_COMPILER_WRAPPER: &str = "CABIN_COMPILER_WRAPPER";

/// Override for the `clang-format` executable Cabin spawns
/// from `cabin fmt`.  When set and non-empty, the value is
/// used verbatim (typically an absolute path) and the `PATH`
/// lookup is skipped.  When unset, Cabin spawns `clang-format`
/// from `PATH`.
pub const CABIN_FMT: &str = "CABIN_FMT";

/// Override for the `cpplint` executable Cabin spawns from
/// `cabin lint`.  Same shape as [`CABIN_FMT`]: a non-empty
/// value is used verbatim and the `PATH` lookup is skipped.
pub const CABIN_CPPLINT: &str = "CABIN_CPPLINT";

/// Override for the `run-clang-tidy` executable Cabin spawns
/// from `cabin tidy`.  Same shape as [`CABIN_FMT`] and
/// [`CABIN_CPPLINT`]: when set and non-empty the value is used
/// verbatim and the `PATH` lookup is skipped, otherwise Cabin
/// resolves `run-clang-tidy` against `PATH`.
pub const CABIN_TIDY: &str = "CABIN_TIDY";

/// Override for the `pkg-config` executable Cabin spawns when
/// probing `system = true` dependencies. Same shape as
/// [`CABIN_FMT`] and the other Cabin tool overrides: when set
/// and non-empty the value is used verbatim (typically an
/// absolute path) and the `PATH` lookup is skipped; when unset,
/// Cabin spawns `pkg-config` from `PATH`. Cabin only invokes
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
/// Honoured by the CLI when `--color` is not present.
pub const CABIN_TERM_COLOR: &str = "CABIN_TERM_COLOR";

/// Enable verbose Cabin-owned status output when no `-v` /
/// `--verbose` CLI flag is present.
pub const CABIN_TERM_VERBOSE: &str = "CABIN_TERM_VERBOSE";

/// Suppress Cabin-owned status output when no `-q` /
/// `--quiet` CLI flag is present.
pub const CABIN_TERM_QUIET: &str = "CABIN_TERM_QUIET";

// ---------------------------------------------------------------------------
// Process env var name constants
// ---------------------------------------------------------------------------
//
// These constants cover the fixed names Cabin sets on the
// `cabin run` / `cabin test` child processes. Dynamic
// per-feature / per-option / per-variant names are derived with
// [`canonical_env_name`].

/// `cabin-cli`'s own absolute path. Set by the `cabin run` /
/// `cabin test` overlays when the CLI can determine it.
pub const CABIN_BIN: &str = "CABIN";

/// Absolute path to the package's manifest directory.
pub const CABIN_MANIFEST_DIR: &str = "CABIN_MANIFEST_DIR";

/// Absolute path to the package's `cabin.toml` manifest.
pub const CABIN_MANIFEST_PATH: &str = "CABIN_MANIFEST_PATH";

/// Package name in the form the manifest declares.
pub const CABIN_PACKAGE_NAME: &str = "CABIN_PACKAGE_NAME";

/// Canonicalized form of [`CABIN_PACKAGE_NAME`] —
/// `[A-Z0-9_]` only, suitable for embedding in another env-var
/// name.
pub const CABIN_PACKAGE_NAME_CANONICAL: &str = "CABIN_PACKAGE_NAME_CANONICAL";

/// Resolved package version (`<major>.<minor>.<patch>` plus any
/// pre-release / build suffix exactly as the manifest declares).
pub const CABIN_PACKAGE_VERSION: &str = "CABIN_PACKAGE_VERSION";

/// Active profile name (`dev`, `release`, or any custom
/// profile).
pub const CABIN_PROFILE: &str = "CABIN_PROFILE";

/// Build-target triple (`x86_64-unknown-linux-gnu`, etc.). Set
/// to the host triple when no cross-compilation flag is given.
pub const CABIN_TARGET_TRIPLE: &str = "CABIN_TARGET_TRIPLE";

/// Host triple — the triple Cabin itself was invoked on.
pub const CABIN_HOST_TRIPLE: &str = "CABIN_HOST_TRIPLE";

/// SHA-256 fingerprint of the resolved Cabin build
/// configuration. Exposed for run/test processes, metadata,
/// and future cache keying; Cabin's current C/C++ build outputs
/// are not discarded solely because this value changes.
pub const CABIN_BUILD_CONFIGURATION_FINGERPRINT: &str = "CABIN_BUILD_CONFIGURATION_FINGERPRINT";

// ---------------------------------------------------------------------------
// Run / test env var name constants
// ---------------------------------------------------------------------------

/// Build target name surfaced to the spawned binary. Set by
/// `cabin run` for `bin` and `example` targets, and by
/// `cabin test` for `cpp_test` targets.
pub const CABIN_BIN_NAME: &str = "CABIN_BIN_NAME";

/// Canonicalized form of [`CABIN_BIN_NAME`].
pub const CABIN_BIN_NAME_CANONICAL: &str = "CABIN_BIN_NAME_CANONICAL";

/// Test target name surfaced to the spawned test binary.
pub const CABIN_TEST_NAME: &str = "CABIN_TEST_NAME";

/// Canonicalized form of [`CABIN_TEST_NAME`].
pub const CABIN_TEST_NAME_CANONICAL: &str = "CABIN_TEST_NAME_CANONICAL";

/// Manifest-declared kind of the running target
/// (`cpp_executable`, `cpp_example`, `cpp_test`, …).
pub const CABIN_TARGET_KIND: &str = "CABIN_TARGET_KIND";

// ---------------------------------------------------------------------------
// Canonicalization
// ---------------------------------------------------------------------------

/// Canonicalise an arbitrary identifier into the form Cabin uses
/// when embedding it in an env-var name suffix.
///
/// Returns [`CanonicalError::EmptyInput`] for the empty string;
/// otherwise returns a `String` in which every byte is
/// `[A-Z0-9_]`. See the crate-level docs for the exact rule.
///
/// Examples:
/// - `"fmt"` → `"FMT"`
/// - `"my-pkg"` → `"MY_PKG"`
/// - `"my.pkg.v2"` → `"MY_PKG_V2"`
/// - `"OpenSSL"` → `"OPENSSL"`
pub fn canonicalize_name(name: &str) -> Result<String, CanonicalError> {
    if name.is_empty() {
        return Err(CanonicalError::EmptyInput);
    }
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() {
            out.push(b.to_ascii_uppercase() as char);
        } else {
            // Non-alphanumeric bytes (including existing `_`)
            // all map to a single `_`. Runs are preserved
            // deliberately so `foo--bar` -> `FOO__BAR`
            // round-trips uniquely.
            out.push('_');
        }
    }
    Ok(out)
}

/// Convenience: canonicalise and prepend a stable prefix. Used
/// when a feature / option / variant / cfg name has to land in
/// an env-var like `CABIN_FEATURE_<NAME>`.
///
/// Returns the same error variants as [`canonicalize_name`].
pub fn canonical_env_name(prefix: &str, raw: &str) -> Result<String, CanonicalError> {
    if prefix.is_empty() {
        return Err(CanonicalError::EmptyInput);
    }
    let canonical = canonicalize_name(raw)?;
    Ok(format!("{prefix}_{canonical}"))
}

/// Detect collisions in a set of raw names. The returned error
/// lists every group of two or more raw names that canonicalise
/// to the same string, sorted by canonical form for stable
/// output.
pub fn detect_collisions<I, S>(names: I) -> Result<(), CanonicalCollision>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    // BTreeMap keeps the diagnostic deterministic.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for raw in names {
        let raw_owned = raw.as_ref().to_owned();
        match canonicalize_name(&raw_owned) {
            Ok(c) => groups.entry(c).or_default().push(raw_owned),
            Err(_) => {
                // Empty-string entries are reported as their
                // own canonical bucket so the diagnostic is
                // actionable.
                groups.entry(String::new()).or_default().push(raw_owned);
            }
        }
    }
    let conflicts: Vec<(String, Vec<String>)> = groups
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(k, mut v)| {
            v.sort();
            v.dedup();
            (k, v)
        })
        .filter(|(_, v)| v.len() > 1)
        .collect();
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(CanonicalCollision { groups: conflicts })
    }
}

/// Errors produced by [`canonicalize_name`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CanonicalError {
    /// The input was empty. The caller is responsible for
    /// rejecting empty manifest names earlier in the pipeline;
    /// surfaced here so a regression cannot silently turn into
    /// a `CABIN__` env var.
    #[error("cannot canonicalise an empty identifier")]
    EmptyInput,
}

/// Two or more raw names produced the same canonical form.
#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "canonicalised env-var name collision: {}",
    render_collision(.groups)
)]
pub struct CanonicalCollision {
    /// `(canonical → [raw names])`, sorted by canonical form.
    pub groups: Vec<(String, Vec<String>)>,
}

fn render_collision(groups: &[(String, Vec<String>)]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (canonical, raws) in groups {
        parts.push(format!("{canonical} <- {}", raws.join(", ")));
    }
    parts.join("; ")
}

// ---------------------------------------------------------------------------
// Run / test env builders
// ---------------------------------------------------------------------------

/// Inputs for [`run_env`]. The orchestration layer fills this
/// in from already-resolved typed values.
#[derive(Debug, Clone)]
pub struct RunEnvInputs<'a> {
    /// Absolute path of `cabin-cli` itself, surfaced as
    /// [`CABIN_BIN`]. `None` to skip when the running CLI
    /// cannot determine its own path.
    pub cabin_bin: Option<&'a std::path::Path>,
    /// Manifest directory of the package owning the binary.
    pub manifest_dir: &'a std::path::Path,
    /// `cabin.toml` path of the package owning the binary.
    pub manifest_path: &'a std::path::Path,
    /// Package name as the manifest declared it.
    pub package_name: &'a str,
    /// Resolved package version.
    pub package_version: &'a str,
    /// Build target name (e.g. `app`, `tool`, `example1`).
    pub bin_name: &'a str,
    /// Manifest target kind, surfaced as
    /// [`CABIN_TARGET_KIND`].
    pub target_kind: &'a str,
    /// Resolved profile name (`dev`, `release`, …).
    pub profile: &'a str,
    /// Resolved build directory.
    pub build_dir: &'a std::path::Path,
    /// Resolved build-target triple (host triple when no cross
    /// flag was given).
    pub target_triple: Option<&'a str>,
    /// Host triple.
    pub host_triple: &'a str,
    /// `BuildConfiguration::fingerprint`.
    pub fingerprint: &'a str,
}

/// Build the `CABIN_*` environment surfaced to a `cabin run`
/// child process. Returns a deterministic `BTreeMap` so two
/// calls with the same inputs are byte-equal.
pub fn run_env(inputs: &RunEnvInputs<'_>) -> Result<BTreeMap<String, OsString>, CanonicalError> {
    let mut out = BTreeMap::new();
    if let Some(p) = inputs.cabin_bin {
        out.insert(CABIN_BIN.to_owned(), p.as_os_str().to_owned());
    }
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
        CABIN_PACKAGE_NAME_CANONICAL.to_owned(),
        OsString::from(canonicalize_name(inputs.package_name)?),
    );
    out.insert(
        CABIN_PACKAGE_VERSION.to_owned(),
        OsString::from(inputs.package_version),
    );
    out.insert(CABIN_BIN_NAME.to_owned(), OsString::from(inputs.bin_name));
    out.insert(
        CABIN_BIN_NAME_CANONICAL.to_owned(),
        OsString::from(canonicalize_name(inputs.bin_name)?),
    );
    out.insert(
        CABIN_TARGET_KIND.to_owned(),
        OsString::from(inputs.target_kind),
    );
    out.insert(CABIN_PROFILE.to_owned(), OsString::from(inputs.profile));
    out.insert(
        CABIN_BUILD_DIR.to_owned(),
        inputs.build_dir.as_os_str().to_owned(),
    );
    if let Some(t) = inputs.target_triple {
        out.insert(CABIN_TARGET_TRIPLE.to_owned(), OsString::from(t));
    }
    out.insert(
        CABIN_HOST_TRIPLE.to_owned(),
        OsString::from(inputs.host_triple),
    );
    out.insert(
        CABIN_BUILD_CONFIGURATION_FINGERPRINT.to_owned(),
        OsString::from(inputs.fingerprint),
    );
    Ok(out)
}

/// Inputs for [`test_env`]. Mirrors [`RunEnvInputs`] plus the
/// per-test `CABIN_TEST_NAME` / `CABIN_TARGET_TMPDIR` keys.
#[derive(Debug, Clone)]
pub struct TestEnvInputs<'a> {
    pub cabin_bin: Option<&'a std::path::Path>,
    pub manifest_dir: &'a std::path::Path,
    pub manifest_path: &'a std::path::Path,
    pub package_name: &'a str,
    pub package_version: &'a str,
    /// Test target name (the `cpp_test`'s manifest name).
    pub test_name: &'a str,
    pub target_kind: &'a str,
    pub profile: &'a str,
    pub build_dir: &'a std::path::Path,
    pub target_triple: Option<&'a str>,
    pub host_triple: &'a str,
    pub fingerprint: &'a str,
    /// Optional per-test scratch directory. When set the
    /// test inherits `CABIN_TARGET_TMPDIR=<absolute path>`.
    pub target_tmpdir: Option<PathBuf>,
}

/// Build the `CABIN_*` environment surfaced to a `cabin test`
/// executable.
pub fn test_env(inputs: &TestEnvInputs<'_>) -> Result<BTreeMap<String, OsString>, CanonicalError> {
    let mut out = BTreeMap::new();
    if let Some(p) = inputs.cabin_bin {
        out.insert(CABIN_BIN.to_owned(), p.as_os_str().to_owned());
    }
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
        CABIN_PACKAGE_NAME_CANONICAL.to_owned(),
        OsString::from(canonicalize_name(inputs.package_name)?),
    );
    out.insert(
        CABIN_PACKAGE_VERSION.to_owned(),
        OsString::from(inputs.package_version),
    );
    out.insert(CABIN_TEST_NAME.to_owned(), OsString::from(inputs.test_name));
    out.insert(
        CABIN_TEST_NAME_CANONICAL.to_owned(),
        OsString::from(canonicalize_name(inputs.test_name)?),
    );
    out.insert(
        CABIN_TARGET_KIND.to_owned(),
        OsString::from(inputs.target_kind),
    );
    out.insert(CABIN_PROFILE.to_owned(), OsString::from(inputs.profile));
    out.insert(
        CABIN_BUILD_DIR.to_owned(),
        inputs.build_dir.as_os_str().to_owned(),
    );
    if let Some(t) = inputs.target_triple {
        out.insert(CABIN_TARGET_TRIPLE.to_owned(), OsString::from(t));
    }
    out.insert(
        CABIN_HOST_TRIPLE.to_owned(),
        OsString::from(inputs.host_triple),
    );
    out.insert(
        CABIN_BUILD_CONFIGURATION_FINGERPRINT.to_owned(),
        OsString::from(inputs.fingerprint),
    );
    if let Some(p) = &inputs.target_tmpdir {
        out.insert("CABIN_TARGET_TMPDIR".to_owned(), p.as_os_str().to_owned());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Truthy / boolean parsing for read-side env vars
// ---------------------------------------------------------------------------

/// Whether a raw env-var value should be treated as truthy.
/// Mirrors Cargo: any of `1`, `true`, `yes`, `on` (case-
/// insensitive) is truthy; an empty string is falsy; anything
/// else is rejected via [`BoolError::Invalid`].
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
    fn canonicalize_uppercases_ascii_letters() {
        assert_eq!(canonicalize_name("fmt").unwrap(), "FMT");
        assert_eq!(canonicalize_name("OpenSSL").unwrap(), "OPENSSL");
    }

    #[test]
    fn canonicalize_replaces_non_alphanum_with_underscore() {
        assert_eq!(canonicalize_name("my-pkg").unwrap(), "MY_PKG");
        assert_eq!(canonicalize_name("my.pkg.v2").unwrap(), "MY_PKG_V2");
        assert_eq!(canonicalize_name("a/b\\c d").unwrap(), "A_B_C_D");
    }

    #[test]
    fn canonicalize_preserves_existing_underscores() {
        assert_eq!(canonicalize_name("my_pkg").unwrap(), "MY_PKG");
        assert_eq!(canonicalize_name("foo__bar").unwrap(), "FOO__BAR");
    }

    #[test]
    fn canonicalize_rejects_empty_input() {
        assert_eq!(canonicalize_name(""), Err(CanonicalError::EmptyInput));
    }

    #[test]
    fn canonical_env_name_combines_prefix_and_canonical_form() {
        assert_eq!(
            canonical_env_name("CABIN_FEATURE", "ssl-enabled").unwrap(),
            "CABIN_FEATURE_SSL_ENABLED"
        );
    }

    #[test]
    fn detect_collisions_returns_groups_with_two_or_more_raw_names() {
        let err = detect_collisions(["foo-bar", "foo.bar"]).unwrap_err();
        assert_eq!(err.groups.len(), 1);
        let (canonical, raws) = &err.groups[0];
        assert_eq!(canonical, "FOO_BAR");
        assert_eq!(raws, &vec!["foo-bar".to_owned(), "foo.bar".to_owned()]);
    }

    #[test]
    fn detect_collisions_ignores_unique_canonical_forms() {
        assert!(detect_collisions(["foo", "bar", "baz"]).is_ok());
    }

    #[test]
    fn detect_collisions_treats_repeated_identical_inputs_as_no_op() {
        // Same raw name appearing twice should not be reported
        // as a collision — the *value* is identical, so no
        // ambiguity exists.
        assert!(detect_collisions(["foo", "foo"]).is_ok());
    }

    #[test]
    fn parse_bool_recognises_documented_truthy_and_falsy_spellings() {
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
    fn detect_collisions_diagnostic_lists_every_offending_name_in_sorted_order() {
        let err = detect_collisions(["foo.bar", "foo-bar", "foo bar"]).unwrap_err();
        let rendered = err.to_string();
        // Sort order is by raw name within each canonical group;
        // the rendered diagnostic must be deterministic so future
        // tests can match on its substring.
        assert!(
            rendered.contains("FOO_BAR <- foo bar, foo-bar, foo.bar"),
            "diagnostic should list raws sorted: {rendered}"
        );
    }

    #[test]
    fn detect_collisions_groups_multiple_buckets_in_canonical_order() {
        let err = detect_collisions(["b-1", "b.1", "a-1", "a.1"]).unwrap_err();
        // Two buckets: A_1 then B_1 — the BTreeMap key order is
        // canonical-form ascending.
        let canonicals: Vec<&str> = err.groups.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(canonicals, vec!["A_1", "B_1"]);
    }

    #[test]
    fn parse_bool_error_wording_includes_raw_value() {
        let err = parse_bool("perhaps").unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("perhaps"),
            "error should echo the input: {rendered}"
        );
        // The wording must list every recognised spelling so
        // users see how to fix it.
        assert!(
            rendered.contains("true") && rendered.contains("false"),
            "{rendered}"
        );
    }

    #[test]
    fn canonical_env_name_rejects_empty_prefix() {
        assert!(matches!(
            canonical_env_name("", "feature"),
            Err(CanonicalError::EmptyInput)
        ));
    }

    #[test]
    fn run_env_omits_target_triple_when_not_provided() {
        use std::path::PathBuf;
        let dir = PathBuf::from("/abs/app");
        let path = PathBuf::from("/abs/app/cabin.toml");
        let env = run_env(&RunEnvInputs {
            cabin_bin: None,
            manifest_dir: &dir,
            manifest_path: &path,
            package_name: "p",
            package_version: "0.0.0",
            bin_name: "p",
            target_kind: "cpp_executable",
            profile: "dev",
            build_dir: &dir,
            target_triple: None,
            host_triple: "x86_64-unknown-linux-gnu",
            fingerprint: "",
        })
        .unwrap();
        assert!(!env.contains_key(CABIN_TARGET_TRIPLE));
        assert_eq!(
            env.get(CABIN_HOST_TRIPLE).unwrap(),
            "x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn test_env_emits_test_name_canonical_and_optional_tmpdir() {
        use std::path::PathBuf;
        let dir = PathBuf::from("/abs/app");
        let path = PathBuf::from("/abs/app/cabin.toml");
        let tmp = PathBuf::from("/abs/app/build/tmp");
        let env = test_env(&TestEnvInputs {
            cabin_bin: None,
            manifest_dir: &dir,
            manifest_path: &path,
            package_name: "demo",
            package_version: "0.1.0",
            test_name: "smoke-test",
            target_kind: "cpp_test",
            profile: "release",
            build_dir: &dir,
            target_triple: None,
            host_triple: "x86_64-apple-darwin",
            fingerprint: "sha256:abc",
            target_tmpdir: Some(tmp.clone()),
        })
        .unwrap();
        assert_eq!(env.get(CABIN_TEST_NAME).unwrap(), "smoke-test");
        assert_eq!(env.get(CABIN_TEST_NAME_CANONICAL).unwrap(), "SMOKE_TEST");
        assert_eq!(env.get(CABIN_TARGET_KIND).unwrap(), "cpp_test");
        assert_eq!(env.get(CABIN_PROFILE).unwrap(), "release");
        assert_eq!(env.get("CABIN_TARGET_TMPDIR").unwrap(), tmp.as_os_str());
    }

    #[test]
    fn run_env_keys_are_emitted_in_btreemap_order() {
        use std::path::PathBuf;
        let dir = PathBuf::from("/abs/app");
        let path = PathBuf::from("/abs/app/cabin.toml");
        let env = run_env(&RunEnvInputs {
            cabin_bin: None,
            manifest_dir: &dir,
            manifest_path: &path,
            package_name: "demo",
            package_version: "0.1.0",
            bin_name: "demo",
            target_kind: "cpp_executable",
            profile: "dev",
            build_dir: &dir,
            target_triple: None,
            host_triple: "x86_64-unknown-linux-gnu",
            fingerprint: "",
        })
        .unwrap();
        let names: Vec<&str> = env.keys().map(String::as_str).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "run_env must iterate in canonical (BTreeMap) order"
        );
    }

    #[test]
    fn run_env_emits_documented_keys_and_canonicalised_names() {
        use std::path::PathBuf;
        let manifest_dir = PathBuf::from("/abs/app");
        let manifest_path = PathBuf::from("/abs/app/cabin.toml");
        let build_dir = PathBuf::from("/abs/app/build");
        let env = run_env(&RunEnvInputs {
            cabin_bin: None,
            manifest_dir: &manifest_dir,
            manifest_path: &manifest_path,
            package_name: "my-pkg",
            package_version: "0.1.0",
            bin_name: "tool",
            target_kind: "cpp_executable",
            profile: "dev",
            build_dir: &build_dir,
            target_triple: Some("x86_64-unknown-linux-gnu"),
            host_triple: "x86_64-unknown-linux-gnu",
            fingerprint: "sha256:abc",
        })
        .unwrap();
        assert_eq!(env.get(CABIN_PACKAGE_NAME).unwrap(), "my-pkg");
        assert_eq!(env.get(CABIN_PACKAGE_NAME_CANONICAL).unwrap(), "MY_PKG");
        assert_eq!(env.get(CABIN_BIN_NAME).unwrap(), "tool");
        assert_eq!(env.get(CABIN_BIN_NAME_CANONICAL).unwrap(), "TOOL");
        assert_eq!(env.get(CABIN_PROFILE).unwrap(), "dev");
        assert_eq!(env.get(CABIN_BUILD_DIR).unwrap(), "/abs/app/build");
    }
}
