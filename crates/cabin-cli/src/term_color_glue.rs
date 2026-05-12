//! Glue between Cabin's CLI surface and the typed
//! [`cabin_core::ColorChoice`].
//!
//! Two pieces live here:
//! - [`CliColorChoice`] is the clap-facing enum used by
//!   `--color`. It implements [`clap::ValueEnum`] (via the
//!   derive on this side of the orphan rule) and converts
//!   into the typed core enum on demand.
//! - [`resolve_color_choice`] applies Cabin's documented
//!   precedence rule: CLI > `CABIN_TERM_COLOR` > config
//!   `term.color` > default. The function is pure: tests pass
//!   a closure for env lookup so they never depend on the host
//!   environment.

use cabin_config::{
    ConfigDiscoveryInputs, EffectiveConfig, discover_config_files, merge_loaded_files,
};
use cabin_core::{ColorChoice, ColorEnvError};

/// Discover the user-level Cabin config (no workspace context)
/// and return its `term.color` value if any. Errors are
/// swallowed: a missing or unparseable config must not block
/// the early `render_error` path. A subcommand that
/// subsequently loads its own [`EffectiveConfig`] (with the
/// proper workspace layout) will surface any parse errors
/// through its normal error chain.
///
/// This is the production input for the `config` slot of
/// [`resolve_color_choice`] before any subcommand has loaded a
/// workspace. It honours `CABIN_NO_CONFIG`, `CABIN_CONFIG`, and
/// `CABIN_CONFIG_HOME` exactly as discovery does for the rest
/// of Cabin.
pub(crate) fn discover_early_config_color() -> Option<ColorChoice> {
    let inputs = ConfigDiscoveryInputs::from_process(None);
    let discovery = discover_config_files(&inputs).ok()?;
    let effective: EffectiveConfig = merge_loaded_files(discovery.loaded_files);
    effective.term.color.map(|c| c.choice)
}

/// Clap-facing color-choice enum. Mirrors
/// [`cabin_core::ColorChoice`] one-for-one. Lives on the CLI
/// side so we can derive [`clap::ValueEnum`] without making
/// `cabin-core` depend on `clap`.
///
/// Variants are intentionally lowercase in their `to_possible_value`
/// rendering — clap's derive uses the `kebab-case` of the variant
/// name, which matches Cabin's accepted spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum CliColorChoice {
    Auto,
    Always,
    Never,
}

impl From<CliColorChoice> for ColorChoice {
    fn from(value: CliColorChoice) -> Self {
        match value {
            CliColorChoice::Auto => ColorChoice::Auto,
            CliColorChoice::Always => ColorChoice::Always,
            CliColorChoice::Never => ColorChoice::Never,
        }
    }
}

/// Apply Cabin's color-choice precedence:
/// 1. `--color` flag (`cli`),
/// 2. `CABIN_TERM_COLOR` env var (looked up via `env`),
/// 3. config `term.color` (`config`),
/// 4. default [`ColorChoice::Auto`].
///
/// The function is pure: callers pass an env lookup closure
/// so tests can drive every branch without touching the
/// process environment. An invalid env value bubbles up as a
/// [`ColorEnvError`]; the CLI surfaces that error before
/// dispatching any subcommand.
///
/// `cli` and `config` are pre-typed; only the env value goes
/// through string parsing because that is the only entry
/// point where a free-form string can reach Cabin from the
/// outside.
pub(crate) fn resolve_color_choice<F>(
    cli: Option<ColorChoice>,
    env: F,
    config: Option<ColorChoice>,
) -> Result<ColorChoice, ColorEnvError>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(choice) = cli {
        return Ok(choice);
    }
    if let Some(raw) = env(cabin_env::CABIN_TERM_COLOR) {
        // An empty `CABIN_TERM_COLOR=` is a common pattern for
        // "unset"; treat it as if the variable were absent so
        // shell scripts that clear it via `CABIN_TERM_COLOR=`
        // do not see a hard error.
        if raw.is_empty() {
            return Ok(config.unwrap_or_default());
        }
        return ColorChoice::from_env_value(&raw);
    }
    Ok(config.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn env_with<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn defaults_to_auto_with_no_inputs() {
        let resolved = resolve_color_choice(None, no_env, None).unwrap();
        assert_eq!(resolved, ColorChoice::Auto);
    }

    #[test]
    fn cli_always_overrides_env_never() {
        let resolved = resolve_color_choice(
            Some(ColorChoice::Always),
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "never")]),
            None,
        )
        .unwrap();
        assert_eq!(resolved, ColorChoice::Always);
    }

    #[test]
    fn cli_never_overrides_env_always() {
        let resolved = resolve_color_choice(
            Some(ColorChoice::Never),
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "always")]),
            None,
        )
        .unwrap();
        assert_eq!(resolved, ColorChoice::Never);
    }

    #[test]
    fn env_always_applies_when_cli_omitted() {
        let resolved = resolve_color_choice(
            None,
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "always")]),
            None,
        )
        .unwrap();
        assert_eq!(resolved, ColorChoice::Always);
    }

    #[test]
    fn env_never_applies_when_cli_omitted() {
        let resolved = resolve_color_choice(
            None,
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "never")]),
            None,
        )
        .unwrap();
        assert_eq!(resolved, ColorChoice::Never);
    }

    #[test]
    fn invalid_env_bubbles_up_as_typed_error() {
        let err = resolve_color_choice(
            None,
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "sometimes")]),
            None,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid CABIN_TERM_COLOR value 'sometimes'; expected one of: auto, always, never"
        );
    }

    #[test]
    fn cli_value_takes_precedence_over_invalid_env() {
        // `--color` parsing happens at the clap layer, so an
        // invalid `CABIN_TERM_COLOR` only fails when the CLI
        // does not already pin the choice. An explicit CLI
        // value short-circuits env validation entirely.
        let resolved = resolve_color_choice(
            Some(ColorChoice::Auto),
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "sometimes")]),
            None,
        )
        .unwrap();
        assert_eq!(resolved, ColorChoice::Auto);
    }

    #[test]
    fn empty_env_value_is_treated_as_unset() {
        let resolved =
            resolve_color_choice(None, env_with(&[(cabin_env::CABIN_TERM_COLOR, "")]), None)
                .unwrap();
        assert_eq!(resolved, ColorChoice::Auto);
    }

    #[test]
    fn config_applies_only_when_cli_and_env_silent() {
        let resolved = resolve_color_choice(None, no_env, Some(ColorChoice::Always)).unwrap();
        assert_eq!(resolved, ColorChoice::Always);
    }

    #[test]
    fn env_overrides_config() {
        let resolved = resolve_color_choice(
            None,
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "never")]),
            Some(ColorChoice::Always),
        )
        .unwrap();
        assert_eq!(resolved, ColorChoice::Never);
    }

    #[test]
    fn cli_overrides_config_too() {
        let resolved =
            resolve_color_choice(Some(ColorChoice::Always), no_env, Some(ColorChoice::Never))
                .unwrap();
        assert_eq!(resolved, ColorChoice::Always);
    }

    #[test]
    fn empty_env_falls_through_to_config() {
        // An empty `CABIN_TERM_COLOR=` should not erase a
        // config-provided `term.color` — Cabin treats the
        // empty value as "unset".
        let resolved = resolve_color_choice(
            None,
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "")]),
            Some(ColorChoice::Always),
        )
        .unwrap();
        assert_eq!(resolved, ColorChoice::Always);
    }
}
