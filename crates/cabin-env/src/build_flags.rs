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
/// platform-specific shell behavior.
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
    shlex::split(&normalize(&value)).ok_or(EnvBuildFlagsError::Parse { name })
}

/// Normalize an env-var value before handing it to
/// [`shlex::split`].
///
/// Two of `shlex`'s defaults are appropriate for parsing a shell
/// command line but not for parsing the value of a flag-style env
/// var:
///
/// - an unquoted `#` starts a comment and silently discards the
///   rest of the input (e.g. `CFLAGS="-DFOO=1 #r1 -O2"` loses
///   `-O2`);
/// - `\r` is not a token separator, so a CRLF-contaminated value
///   carries a stray `\r` into an argument.
///
/// Neither matches how `make`, `CMake`, or autotools treat
/// their flag-style env vars.  This pre-pass escapes unquoted `#` with
/// a backslash (so `shlex` emits a literal `#`) and substitutes
/// unquoted `\r` with a space.  Bytes inside single or double
/// quotes are left untouched; the input still parses through
/// `shlex` and so behaves exactly like POSIX shell-style word
/// splitting otherwise.
fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_single = false;
    let mut in_double = false;
    let mut escape_next = false;
    for c in input.chars() {
        if escape_next {
            out.push(c);
            escape_next = false;
            continue;
        }
        if in_single {
            out.push(c);
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            out.push(c);
            if c == '\\' {
                escape_next = true;
            } else if c == '"' {
                in_double = false;
            }
            continue;
        }
        match c {
            '\\' => {
                out.push('\\');
                escape_next = true;
            }
            '\'' => {
                out.push('\'');
                in_single = true;
            }
            '"' => {
                out.push('"');
                in_double = true;
            }
            '\r' => out.push(' '),
            '#' => {
                out.push('\\');
                out.push('#');
            }
            other => out.push(other),
        }
    }
    out
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
    fn parse_env_build_flags_preserves_unquoted_hash() {
        // POSIX `sh` would treat `#bar -O2` as a comment, but
        // build-flag env vars are not shell command lines: the
        // user wrote literal characters and expects them all to
        // reach the compiler.  Make, CMake, and autotools all
        // preserve `#` verbatim in their flag env vars.
        let env = env_from(&[(CFLAGS, "-DFOO=1 #bar -O2")]);
        let flags = parse_env_build_flags(env).unwrap();
        assert_eq!(flags.cflags, vec!["-DFOO=1", "#bar", "-O2"]);
    }

    #[test]
    fn parse_env_build_flags_preserves_hash_inside_quotes() {
        let env = env_from(&[
            (CFLAGS, "-DREV='git#abc123'"),
            (CXXFLAGS, "-DOTHER=\"x#y\""),
        ]);
        let flags = parse_env_build_flags(env).unwrap();
        assert_eq!(flags.cflags, vec!["-DREV=git#abc123"]);
        assert_eq!(flags.cxxflags, vec!["-DOTHER=x#y"]);
    }

    #[test]
    fn parse_env_build_flags_treats_carriage_return_as_separator() {
        // CRLF-contaminated env vars are common when values come
        // from Windows-formatted tooling; treating `\r` as a
        // separator prevents stray `\r` from ending up in a
        // single argument.
        let env = env_from(&[(CXXFLAGS, "-O2\r-g")]);
        let flags = parse_env_build_flags(env).unwrap();
        assert_eq!(flags.cxxflags, vec!["-O2", "-g"]);
    }

    #[test]
    fn parse_env_build_flags_preserves_carriage_return_inside_quotes() {
        // `\r` inside a quoted run is part of the user's literal
        // payload and must survive into the argument.
        let env = env_from(&[(CFLAGS, "-DPAYLOAD='a\rb'")]);
        let flags = parse_env_build_flags(env).unwrap();
        assert_eq!(flags.cflags, vec!["-DPAYLOAD=a\rb"]);
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
