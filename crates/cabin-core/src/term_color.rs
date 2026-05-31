//! Typed model for Cabin's terminal-color choice.
//!
//! This is a single-purpose enum mirroring Cargo's `--color`
//! tri-state (`auto` / `always` / `never`). It lives in
//! `cabin-core` so the CLI parser, the config layer, and the
//! diagnostic renderer all share one parsing rule and one error
//! wording.
//!
//! Parsing is implemented for `&str` inputs that come from two
//! places:
//! - the `CABIN_TERM_COLOR` environment variable, parsed via
//!   [`ColorChoice::from_env_value`];
//! - config files (and any other typed-string source), parsed via
//!   [`ColorChoice::from_config_value`].
//!
//! The CLI uses clap's `ValueEnum` derive directly, which keeps
//! the help text "[possible values: auto, always, never]" wired
//! to the same set of variants.

use std::fmt;

/// User-selected terminal-color mode. The default is
/// [`ColorChoice::Auto`], which lets the renderer fall back to
/// terminal detection (TTY check + `NO_COLOR` honoring).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColorChoice {
    /// Emit colors only when stderr is connected to a terminal
    /// and the environment does not opt out (e.g. `NO_COLOR`).
    /// This is Cabin's default and Cargo's default.
    #[default]
    Auto,
    /// Always emit colors, even when output is redirected.
    Always,
    /// Never emit colors, regardless of terminal detection.
    Never,
}

impl ColorChoice {
    /// Stable string label for this variant. Round-trips through
    /// [`ColorChoice::from_config_value`] and serializes to TOML
    /// without surprises.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorChoice::Auto => "auto",
            ColorChoice::Always => "always",
            ColorChoice::Never => "never",
        }
    }

    /// Parse a value coming from `CABIN_TERM_COLOR`. The
    /// returned error names the offending variable so the CLI
    /// can render a single, copy-pasteable message.
    pub fn from_env_value(raw: &str) -> Result<Self, ColorEnvError> {
        Self::from_str_inner(raw).ok_or_else(|| ColorEnvError {
            variable: "CABIN_TERM_COLOR",
            value: raw.to_owned(),
        })
    }

    /// Parse a value coming from a config file. The error type
    /// is bare so the config crate can attach its own location
    /// information.
    pub fn from_config_value(raw: &str) -> Result<Self, InvalidColorChoice> {
        Self::from_str_inner(raw).ok_or_else(|| InvalidColorChoice {
            value: raw.to_owned(),
        })
    }

    fn from_str_inner(raw: &str) -> Option<Self> {
        // The accepted spelling list is exactly what `--color`
        // accepts; we deliberately do not lowercase or trim
        // surrounding whitespace because both env variables and
        // config values use exact-match parsing elsewhere.
        match raw {
            "auto" => Some(ColorChoice::Auto),
            "always" => Some(ColorChoice::Always),
            "never" => Some(ColorChoice::Never),
            _ => None,
        }
    }
}

impl fmt::Display for ColorChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when the value of `CABIN_TERM_COLOR` does not
/// match `auto` / `always` / `never`.
///
/// The `Display` impl is the user-visible wording the CLI
/// surfaces:
///
/// ```text
/// invalid CABIN_TERM_COLOR value 'sometimes'; expected one of: auto, always, never
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorEnvError {
    /// Name of the environment variable whose value failed to
    /// parse. Always `"CABIN_TERM_COLOR"` today; carrying it
    /// keeps the message generation in one spot if a future
    /// alias variable is ever recognized.
    pub variable: &'static str,
    /// The raw, invalid value as the user provided it.
    pub value: String,
}

impl fmt::Display for ColorEnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid {} value '{}'; expected one of: auto, always, never",
            self.variable, self.value
        )
    }
}

impl std::error::Error for ColorEnvError {}

/// Error returned by [`ColorChoice::from_config_value`]. The
/// caller decorates this with file location information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidColorChoice {
    /// The raw, invalid value as the user provided it.
    pub value: String,
}

impl fmt::Display for InvalidColorChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid color value '{}'; expected one of: auto, always, never",
            self.value
        )
    }
}

impl std::error::Error for InvalidColorChoice {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_auto() {
        assert_eq!(ColorChoice::default(), ColorChoice::Auto);
    }

    #[test]
    fn as_str_round_trips_through_config_parser() {
        for choice in [ColorChoice::Auto, ColorChoice::Always, ColorChoice::Never] {
            assert_eq!(
                ColorChoice::from_config_value(choice.as_str()).unwrap(),
                choice,
                "{choice:?} did not round-trip"
            );
        }
    }

    #[test]
    fn from_env_value_accepts_documented_values() {
        assert_eq!(
            ColorChoice::from_env_value("auto").unwrap(),
            ColorChoice::Auto
        );
        assert_eq!(
            ColorChoice::from_env_value("always").unwrap(),
            ColorChoice::Always
        );
        assert_eq!(
            ColorChoice::from_env_value("never").unwrap(),
            ColorChoice::Never
        );
    }

    #[test]
    fn from_env_value_rejects_unknown_value_with_documented_wording() {
        let err = ColorChoice::from_env_value("sometimes").unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid CABIN_TERM_COLOR value 'sometimes'; expected one of: auto, always, never"
        );
    }

    #[test]
    fn from_env_value_does_not_normalize_case() {
        // Mirrors Cargo's behavior: `Always` is rejected. The
        // documented spellings are lowercase; accepting a
        // mixed-case value here would create an inconsistent
        // grammar between CLI and env parsing.
        assert!(ColorChoice::from_env_value("Always").is_err());
        assert!(ColorChoice::from_env_value("ALWAYS").is_err());
    }

    #[test]
    fn from_config_value_rejects_empty() {
        let err = ColorChoice::from_config_value("").unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid color value ''; expected one of: auto, always, never"
        );
    }

    #[test]
    fn display_matches_as_str() {
        assert_eq!(ColorChoice::Auto.to_string(), "auto");
        assert_eq!(ColorChoice::Always.to_string(), "always");
        assert_eq!(ColorChoice::Never.to_string(), "never");
    }
}
