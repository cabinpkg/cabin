//! Read-side ingestion of the conventional C / C++ build-flag
//! environment variables: `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, and
//! `LDFLAGS`.
//!
//! Cabin reads each variable at command start, parses its value
//! into argv tokens using POSIX shell-style word splitting via
//! the [`shlex`] crate, and surfaces the result as a typed
//! [`EnvBuildFlags`].  The orchestration layer is responsible for
//! merging the parsed flags into the per-package
//! `cabin_core::ResolvedProfileFlags` map; this module owns the
//! parsing, error wording, and variable attribution only.
//!
//! Crate boundaries (matching the rest of `cabin-env`):
//! - this module never invokes a shell, reads files, or touches
//!   the filesystem;
//! - it never depends on `cabin-build`, `cabin-core`, or any
//!   higher-level crate;
//! - it consumes a `Fn(&str) -> Option<OsString>` env-lookup
//!   closure so tests can pump fixture values through without
//!   mutating the process environment.

use std::ffi::OsString;

use thiserror::Error;

/// `CPPFLAGS` — preprocessor flags applied to **both** C and C++
/// compile commands.  Cabin appends parsed tokens to each
/// primary package's language-neutral `extra_compile_args`
/// bucket, after profile / manifest / dependency / pkg-config
/// flags.
pub const CPPFLAGS: &str = "CPPFLAGS";

/// `CFLAGS` — flags applied **only** to C compile commands.
/// Appended to each primary package's `cflags`
/// bucket.  Never reaches a C++ compile line.
pub const CFLAGS: &str = "CFLAGS";

/// `CXXFLAGS` — flags applied **only** to C++ compile commands.
/// Appended to each primary package's `cxxflags`
/// bucket.  Never reaches a C compile line.
pub const CXXFLAGS: &str = "CXXFLAGS";

/// `LDFLAGS` — flags applied **only** to link commands.
/// Appended to each primary package's `ldflags` bucket.
/// Never reaches a compile command.
pub const LDFLAGS: &str = "LDFLAGS";

/// Typed view of the four conventional C / C++ build-flag
/// environment variables, already shell-split into argv tokens.
///
/// Each field preserves the order the user wrote.  Empty,
/// missing, and whitespace-only variables produce an empty
/// vector; the planner-side merge then has nothing to append
/// for that bucket.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvBuildFlags {
    /// Parsed `CPPFLAGS` tokens.  Applied to both C and C++
    /// compile commands.
    pub cppflags: Vec<String>,
    /// Parsed `CFLAGS` tokens.  Applied only to C compile
    /// commands.
    pub cflags: Vec<String>,
    /// Parsed `CXXFLAGS` tokens.  Applied only to C++ compile
    /// commands.
    pub cxxflags: Vec<String>,
    /// Parsed `LDFLAGS` tokens.  Applied only to link commands.
    pub ldflags: Vec<String>,
}

impl EnvBuildFlags {
    /// Whether every bucket is empty.  Callers use this to skip
    /// the merge step entirely when no environment flag is
    /// active.
    pub fn is_empty(&self) -> bool {
        self.cppflags.is_empty()
            && self.cflags.is_empty()
            && self.cxxflags.is_empty()
            && self.ldflags.is_empty()
    }
}

/// Read `CPPFLAGS`, `CFLAGS`, `CXXFLAGS`, and `LDFLAGS` through
/// the supplied env-lookup closure, parse each value using
/// POSIX shell-style word splitting (via [`shlex::split`]), and
/// return the typed view.
///
/// Empty and whitespace-only values yield an empty vector for
/// that bucket.  Malformed shell input is surfaced as an
/// [`EnvBuildFlagsError`] that names the offending environment
/// variable so the diagnostic is actionable.
///
/// The function never invokes a shell and never depends on
/// platform-specific shell behaviour.
pub fn parse_env_build_flags<F>(env: F) -> Result<EnvBuildFlags, EnvBuildFlagsError>
where
    F: Fn(&str) -> Option<OsString>,
{
    let cppflags = parse_one(&env, CPPFLAGS)?;
    let cflags = parse_one(&env, CFLAGS)?;
    let cxxflags = parse_one(&env, CXXFLAGS)?;
    let ldflags = parse_one(&env, LDFLAGS)?;
    Ok(EnvBuildFlags {
        cppflags,
        cflags,
        cxxflags,
        ldflags,
    })
}

fn parse_one<F>(env: &F, name: &'static str) -> Result<Vec<String>, EnvBuildFlagsError>
where
    F: Fn(&str) -> Option<OsString>,
{
    let Some(raw) = env(name) else {
        return Ok(Vec::new());
    };
    let value = raw
        .into_string()
        .map_err(|_| EnvBuildFlagsError::NonUtf8 { name })?;
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    shlex::split(&value).ok_or(EnvBuildFlagsError::Parse { name })
}

/// Reason a [`parse_env_build_flags`] call failed.  Both
/// variants name the offending variable so the diagnostic is
/// actionable.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EnvBuildFlagsError {
    /// The environment variable held a non-UTF-8 byte sequence
    /// that Cabin cannot interpret as shell-like text.  Variable
    /// values are expected to be UTF-8; non-UTF-8 input is
    /// reported here rather than silently dropped.
    #[error("invalid {name}: value is not valid UTF-8")]
    NonUtf8 { name: &'static str },
    /// The value was UTF-8 but POSIX shell-style word splitting
    /// rejected it (for example, an unterminated quote or a
    /// trailing escape).
    #[error("invalid {name}: could not parse shell words")]
    Parse { name: &'static str },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(*v))
        }
    }

    #[test]
    fn parse_env_build_flags_picks_each_variable_into_its_bucket() {
        let env = env_from(&[
            (CPPFLAGS, "-DFOO=1"),
            (CFLAGS, "-std=c11 -Wall"),
            (CXXFLAGS, "-std=c++17 -fno-rtti"),
            (LDFLAGS, "-L/opt/lib -lthing"),
        ]);
        let flags = parse_env_build_flags(env).unwrap();
        assert_eq!(flags.cppflags, vec!["-DFOO=1"]);
        assert_eq!(flags.cflags, vec!["-std=c11", "-Wall"]);
        assert_eq!(flags.cxxflags, vec!["-std=c++17", "-fno-rtti"]);
        assert_eq!(flags.ldflags, vec!["-L/opt/lib", "-lthing"]);
    }

    #[test]
    fn parse_env_build_flags_ignores_empty_and_whitespace_values() {
        let env = env_from(&[
            (CPPFLAGS, ""),
            (CFLAGS, "   "),
            (CXXFLAGS, "\t\n  "),
            // LDFLAGS omitted: covered by the absent-variable
            // branch.
        ]);
        let flags = parse_env_build_flags(env).unwrap();
        assert!(flags.is_empty(), "{flags:?}");
    }

    #[test]
    fn parse_env_build_flags_unterminated_quote_names_variable() {
        let env = env_from(&[(CXXFLAGS, "'oops")]);
        let err = parse_env_build_flags(env).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("CXXFLAGS"), "{msg}");
        assert!(msg.contains("shell"), "{msg}");
    }

    #[test]
    fn parse_env_build_flags_trailing_escape_names_variable() {
        let env = env_from(&[(LDFLAGS, r"-L/lib\")]);
        let err = parse_env_build_flags(env).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("LDFLAGS"), "{msg}");
        assert!(msg.contains("shell"), "{msg}");
    }

    #[test]
    fn parse_env_build_flags_preserves_define_with_quoted_value() {
        let env = env_from(&[(CXXFLAGS, "-DNAME=\"hello world\"")]);
        let flags = parse_env_build_flags(env).unwrap();
        assert_eq!(flags.cxxflags, vec!["-DNAME=hello world"]);
    }

    #[test]
    fn parse_env_build_flags_supports_single_quoted_value_with_space() {
        let env = env_from(&[(CPPFLAGS, "-DNAME='hello world'")]);
        let flags = parse_env_build_flags(env).unwrap();
        assert_eq!(flags.cppflags, vec!["-DNAME=hello world"]);
    }

    #[test]
    fn parse_env_build_flags_supports_escaped_space() {
        let env = env_from(&[(CFLAGS, r"-DPATH=foo\ bar")]);
        let flags = parse_env_build_flags(env).unwrap();
        assert_eq!(flags.cflags, vec!["-DPATH=foo bar"]);
    }

    #[test]
    fn parse_env_build_flags_empty_is_default() {
        let env = |_: &str| None;
        let flags = parse_env_build_flags(env).unwrap();
        assert_eq!(flags, EnvBuildFlags::default());
        assert!(flags.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn parse_env_build_flags_rejects_non_utf8_value() {
        use std::os::unix::ffi::OsStringExt;
        // 0xFE / 0xFF are illegal UTF-8 lead bytes.
        let bad = OsString::from_vec(vec![b'-', b'D', b'A', b'=', 0xFE, 0xFF]);
        let env = move |key: &str| {
            if key == CFLAGS {
                Some(bad.clone())
            } else {
                None
            }
        };
        let err = parse_env_build_flags(env).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("CFLAGS"), "{msg}");
        assert!(msg.contains("UTF-8"), "{msg}");
    }
}
