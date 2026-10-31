//! Glue between Cabin's CLI surface and the typed
//! [`cabin_core::ColorChoice`].
//!
//! Two pieces live here:
//! - [`CliColorChoice`] is the clap-facing enum used by
//!   `--color`.  It implements [`clap::ValueEnum`] (via the
//!   derive on this side of the orphan rule) and converts
//!   into the typed core enum on demand.
//! - [`resolve_color_choice`] applies Cabin's documented
//!   precedence rule: CLI > `CABIN_TERM_COLOR` > config
//!   `term.color` > default.

use cabin_config::{
    ConfigDiscoveryInputs, EffectiveConfig, discover_config_files, merge_loaded_files,
};
use cabin_core::{ColorChoice, ColorEnvError, ConfigValueSource};

/// Discover the user-level Cabin config (no workspace context)
/// and return its `term.color` value and source, if any.  Errors are
/// swallowed: a missing or unparsable config must not block
/// the early `render_error` path.  A subcommand that
/// subsequently loads its own [`EffectiveConfig`] (with the
/// proper workspace layout) will surface any parse errors
/// through its normal error chain.
///
/// This is the production input for the `config` slot of
/// [`resolve_color_choice`] before any subcommand has loaded a
/// workspace.  It honors `CABIN_NO_CONFIG`, `CABIN_CONFIG`, and
/// `CABIN_CONFIG_HOME` exactly as discovery does for the rest
/// of Cabin.
pub(crate) fn discover_early_config_color() -> Option<(ColorChoice, ConfigValueSource)> {
    let inputs = ConfigDiscoveryInputs::from_process(None);
    let discovery = discover_config_files(&inputs).ok()?;
    let effective: EffectiveConfig = merge_loaded_files(discovery.loaded_files);
    effective
        .term
        .color
        .map(|c| (c.choice, crate::cli::config::config_value_source(c.source)))
}

/// Clap-facing color-choice enum.  Mirrors
/// [`cabin_core::ColorChoice`] one-for-one.  Lives on the CLI
/// side so we can derive [`clap::ValueEnum`] without making
/// `cabin-core` depend on `clap`.
///
/// clap's derive renders each variant as its kebab-case name
/// (`auto`, `always`, `never`), matching the `CABIN_TERM_COLOR`
/// spellings.
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
/// 4. default [`ColorChoice::Auto`],
///
/// returning the choice and the layer it came from.
///
/// The function is pure: callers pass an env lookup closure
/// so tests can drive every branch without touching the
/// process environment.  An invalid env value bubbles up as a
/// [`ColorEnvError`]; the CLI surfaces that error before
/// dispatching any subcommand.
///
/// `cli` and `config` arrive typed; only the env value is
/// parsed here.
pub(crate) fn resolve_color_choice<F>(
    cli: Option<ColorChoice>,
    env: F,
    config: Option<(ColorChoice, ConfigValueSource)>,
) -> Result<(ColorChoice, ConfigValueSource), ColorEnvError>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(choice) = cli {
        return Ok((choice, ConfigValueSource::Cli));
    }
    // An empty `CABIN_TERM_COLOR=` is a common pattern for
    // "unset"; treat it as if the variable were absent so
    // shell scripts that clear it via `CABIN_TERM_COLOR=`
    // do not see a hard error.
    if let Some(raw) = env(cabin_env::CABIN_TERM_COLOR).filter(|raw| !raw.is_empty()) {
        return Ok((ColorChoice::from_env_value(&raw)?, ConfigValueSource::Env));
    }
    Ok(config.unwrap_or((ColorChoice::default(), ConfigValueSource::BuiltinDefault)))
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
        assert_eq!(
            resolved,
            (ColorChoice::Auto, ConfigValueSource::BuiltinDefault)
        );
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
        let resolved = resolve_color_choice(
            Some(ColorChoice::Auto),
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "sometimes")]),
            None,
        )
        .unwrap();
        assert_eq!(resolved, (ColorChoice::Auto, ConfigValueSource::Cli));
    }

    #[test]
    fn env_overrides_config() {
        let resolved = resolve_color_choice(
            None,
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "never")]),
            Some((ColorChoice::Always, ConfigValueSource::UserConfig)),
        )
        .unwrap();
        assert_eq!(resolved, (ColorChoice::Never, ConfigValueSource::Env));
    }

    #[test]
    fn empty_env_falls_through_to_config() {
        let resolved = resolve_color_choice(
            None,
            env_with(&[(cabin_env::CABIN_TERM_COLOR, "")]),
            Some((ColorChoice::Always, ConfigValueSource::ExplicitConfig)),
        )
        .unwrap();
        assert_eq!(
            resolved,
            (ColorChoice::Always, ConfigValueSource::ExplicitConfig)
        );
    }
}
