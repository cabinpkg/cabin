//! Typed model for Cabin's terminal-output verbosity.
//!
//! Mirrors Cargo's `-q` / `-v` / `-vv` user surface as a four-state
//! enum: [`Verbosity::Quiet`], [`Verbosity::Normal`],
//! [`Verbosity::Verbose`], and [`Verbosity::VeryVerbose`].  The
//! enum lives in `cabin-core` so the CLI parser, the config
//! layer, and the status reporter share one parsing rule and one
//! error wording.
//!
//! Parsing entry points are deliberately narrow:
//! - [`Verbosity::parse_bool_env`] reads `CABIN_TERM_VERBOSE`
//!   and `CABIN_TERM_QUIET`;
//! - [`Verbosity::from_config_pair`] turns the two booleans
//!   `term.verbose` and `term.quiet` into a single typed value
//!   and rejects the both-true combination.

use std::fmt;

/// User-selected verbosity for Cabin-owned status output.
///
/// The default is [`Verbosity::Normal`], which preserves Cabin's
/// pre-existing status-message volume.  Variants are ordered from
/// quietest to loudest so callers can compare with `>=`:
///
/// ```ignore
/// if verbosity >= Verbosity::Verbose { ... }
/// ```
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verbosity {
    /// Suppress Cabin-owned status / progress / log messages.
    /// Errors and explicitly-requested output (build artifacts,
    /// JSON documents, the user program's stdout under
    /// `cabin run`) are unaffected.
    Quiet,
    /// Default volume.  Status lines such as `cabin: wrote
    /// build.ninja` are emitted; verbose-only lines are not.
    #[default]
    Normal,
    /// Adds Cabin-owned context lines such as the resolved
    /// build profile, build directory, and toolchain summary.
    Verbose,
    /// Adds further detail intended for diagnosing local builds.
    /// Output stays deterministic and never includes secrets,
    /// tokens, or environment-dependent values that Normal /
    /// Verbose would not already print.
    VeryVerbose,
}

impl Verbosity {
    /// Stable string label for this variant; matches the
    /// spelling Cabin documents.
    pub fn as_str(self) -> &'static str {
        match self {
            Verbosity::Quiet => "quiet",
            Verbosity::Normal => "normal",
            Verbosity::Verbose => "verbose",
            Verbosity::VeryVerbose => "very-verbose",
        }
    }

    /// Whether this verbosity emits Cabin-owned status messages.
    pub fn shows_status(self) -> bool {
        self >= Verbosity::Normal
    }

    /// Whether this verbosity emits verbose-only context lines.
    pub fn shows_verbose(self) -> bool {
        self >= Verbosity::Verbose
    }

    /// Whether this verbosity emits very-verbose detail lines.
    pub fn shows_very_verbose(self) -> bool {
        self >= Verbosity::VeryVerbose
    }

    /// Convert a `-v` repetition count into a verbosity.  Counts
    /// of two or more clamp to [`Verbosity::VeryVerbose`] so
    /// `-vvv` and similar keep working without erroring.
    pub fn from_verbose_count(count: u8) -> Self {
        match count {
            0 => Verbosity::Normal,
            1 => Verbosity::Verbose,
            _ => Verbosity::VeryVerbose,
        }
    }

    /// Combine the two config booleans `term.verbose` and
    /// `term.quiet` into a single verbosity.  Returns
    /// `Ok(None)` when neither is set so callers can fall through
    /// to the next layer in the precedence chain.  Returns
    /// [`InvalidVerbosityCombination`] when both are true.
    ///
    /// # Errors
    /// Returns [`InvalidVerbosityCombination`] when both `verbose` and `quiet`
    /// are `Some(true)`.
    pub fn from_config_pair(
        verbose: Option<bool>,
        quiet: Option<bool>,
    ) -> Result<Option<Self>, InvalidVerbosityCombination> {
        match (verbose, quiet) {
            (Some(true), Some(true)) => Err(InvalidVerbosityCombination),
            (Some(true), _) => Ok(Some(Verbosity::Verbose)),
            (_, Some(true)) => Ok(Some(Verbosity::Quiet)),
            _ => Ok(None),
        }
    }

    /// Parse a verbosity from a single env-var value.  Used by
    /// `CABIN_TERM_VERBOSE` and `CABIN_TERM_QUIET`: the documented
    /// truthy spellings (`1`, `true`, `yes`, `on`, case-insensitive)
    /// opt in; the falsy spellings (empty, `0`, `false`, `no`,
    /// `off`) opt out.  Other strings produce a typed error so the
    /// CLI can surface a copy-pasteable message.
    ///
    /// # Errors
    /// Returns [`VerbosityEnvError`] when `raw` matches none of the
    /// documented truthy / falsy spellings.
    pub fn parse_bool_env(variable: &'static str, raw: &str) -> Result<bool, VerbosityEnvError> {
        cabin_env::parse_bool(raw).map_err(|_| VerbosityEnvError {
            variable,
            value: raw.to_owned(),
        })
    }
}

impl fmt::Display for Verbosity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned by [`Verbosity::parse_bool_env`] when a value such as
/// `CABIN_TERM_VERBOSE=loud` does not match the documented
/// truthy / falsy spellings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerbosityEnvError {
    pub variable: &'static str,
    pub value: String,
}

impl fmt::Display for VerbosityEnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid {} value '{}'; expected one of: 1, 0, true, false, yes, no, on, off",
            self.variable, self.value
        )
    }
}

impl std::error::Error for VerbosityEnvError {}

/// Returned by [`Verbosity::from_config_pair`] when a single
/// config file sets both `term.verbose = true` and
/// `term.quiet = true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidVerbosityCombination;

impl fmt::Display for InvalidVerbosityCombination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("term.verbose and term.quiet cannot both be true")
    }
}

impl std::error::Error for InvalidVerbosityCombination {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_normal() {
        assert_eq!(Verbosity::default(), Verbosity::Normal);
    }

    #[test]
    fn ordering_matches_intuition() {
        assert!(Verbosity::Quiet < Verbosity::Normal);
        assert!(Verbosity::Normal < Verbosity::Verbose);
        assert!(Verbosity::Verbose < Verbosity::VeryVerbose);
    }

    #[test]
    fn shows_predicates_match_thresholds() {
        assert!(!Verbosity::Quiet.shows_status());
        assert!(Verbosity::Normal.shows_status());
        assert!(Verbosity::Verbose.shows_status());
        assert!(!Verbosity::Normal.shows_verbose());
        assert!(Verbosity::Verbose.shows_verbose());
        assert!(Verbosity::VeryVerbose.shows_verbose());
        assert!(!Verbosity::Verbose.shows_very_verbose());
        assert!(Verbosity::VeryVerbose.shows_very_verbose());
    }

    #[test]
    fn from_verbose_count_clamps_above_two() {
        assert_eq!(Verbosity::from_verbose_count(0), Verbosity::Normal);
        assert_eq!(Verbosity::from_verbose_count(1), Verbosity::Verbose);
        assert_eq!(Verbosity::from_verbose_count(2), Verbosity::VeryVerbose);
        assert_eq!(Verbosity::from_verbose_count(5), Verbosity::VeryVerbose);
        assert_eq!(
            Verbosity::from_verbose_count(u8::MAX),
            Verbosity::VeryVerbose
        );
    }

    #[test]
    fn from_config_pair_handles_each_combination() {
        assert_eq!(Verbosity::from_config_pair(None, None).unwrap(), None);
        assert_eq!(
            Verbosity::from_config_pair(Some(true), None).unwrap(),
            Some(Verbosity::Verbose)
        );
        assert_eq!(
            Verbosity::from_config_pair(None, Some(true)).unwrap(),
            Some(Verbosity::Quiet)
        );
        assert_eq!(
            Verbosity::from_config_pair(Some(false), Some(false)).unwrap(),
            None
        );
        assert!(Verbosity::from_config_pair(Some(true), Some(true)).is_err());
    }

    #[test]
    fn parse_bool_env_accepts_documented_values() {
        for ok in ["1", "true", "yes", "on", "TRUE", "Yes", "ON"] {
            assert!(
                Verbosity::parse_bool_env("X", ok).unwrap(),
                "truthy: {ok:?}"
            );
        }
        for falsy in ["0", "false", "no", "off", "FALSE", "No", "OFF", ""] {
            assert!(
                !Verbosity::parse_bool_env("X", falsy).unwrap(),
                "falsy: {falsy:?}"
            );
        }
    }

    #[test]
    fn parse_bool_env_rejects_unknown_value() {
        let err = Verbosity::parse_bool_env("CABIN_TERM_VERBOSE", "loud").unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid CABIN_TERM_VERBOSE value 'loud'; expected one of: 1, 0, true, false, yes, no, on, off"
        );
    }

    #[test]
    fn invalid_combination_display_is_actionable() {
        assert_eq!(
            InvalidVerbosityCombination.to_string(),
            "term.verbose and term.quiet cannot both be true"
        );
    }
}
