//! Early-stage terminal-state resolution.
//!
//! `cabin` resolves the user's color and verbosity choice
//! before dispatching to any subcommand so even errors emitted
//! while loading a workspace honor `--color` / `--verbose`.
//! [`resolve_early_terminal_state`] applies the documented
//! precedence chains (see [`crate::cli::term_color`] and
//! [`crate::cli::term_verbosity`]) and returns the bundle the
//! dispatcher passes down: a [`ColorChoice`] and a
//! pre-configured [`Reporter`].
//!
//! This is the only place terminal settings are resolved, and
//! it runs before any workspace is located, so it observes only
//! the user-level config (or the `CABIN_CONFIG` file); config
//! discovery rejects a workspace or package `[term]` table.

use std::process::ExitCode;

use cabin_core::{ColorChoice, ConfigValueSource, Verbosity};
use termcolor::StandardStream;

use crate::cli::term_color::CliColorChoice;
use crate::cli::term_verbosity::{
    CliVerbosity, Reporter, discover_early_config_verbosity, resolve_verbosity,
};
use crate::error_rendering::write_plain_error;

/// Resolved terminal state available before any subcommand runs.
pub(crate) struct EarlyTerminalState {
    pub(crate) color: ColorChoice,
    pub(crate) reporter: Reporter,
}

/// A color choice and verbosity, each with the layer it came from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TerminalSettings {
    pub(crate) color: ColorChoice,
    pub(crate) color_source: ConfigValueSource,
    pub(crate) verbosity: Verbosity,
    pub(crate) verbosity_source: ConfigValueSource,
}

impl TerminalSettings {
    /// `cabin metadata`'s `config.term` block.
    pub(crate) fn as_json(self) -> serde_json::Value {
        serde_json::json!({
            "color": {
                "value": self.color.as_str(),
                "value_source": self.color_source.as_key(),
            },
            "verbosity": {
                "value": self.verbosity.as_str(),
                "value_source": self.verbosity_source.as_key(),
            },
        })
    }
}

/// Map a [`ColorChoice`] to a [`termcolor::ColorChoice`].
///
/// `Always` maps to `AlwaysAnsi` so test output stays
/// platform-stable: on Windows, plain `Always` would attempt to
/// drive the console API instead of emitting ANSI escape
/// sequences, which would defeat the integration tests that
/// look for `\x1b[`.
pub(crate) fn termcolor_choice(choice: ColorChoice) -> termcolor::ColorChoice {
    match choice {
        ColorChoice::Auto => termcolor::ColorChoice::Auto,
        ColorChoice::Always => termcolor::ColorChoice::AlwaysAnsi,
        ColorChoice::Never => termcolor::ColorChoice::Never,
    }
}

/// Resolve the color choice and reporter the dispatcher hands
/// to subcommands.
///
/// Color precedence: `--color` ▶ `CABIN_TERM_COLOR` ▶
/// user-level `[term] color` config ▶ `auto`.  Verbosity
/// precedence: CLI flags ▶ `CABIN_TERM_VERBOSE` /
/// `CABIN_TERM_QUIET` ▶ user-level `[term]` config ▶ default.
///
/// On an invalid env value the helper writes a plain `error:`
/// line and returns [`ExitCode::FAILURE`] so the dispatcher
/// can short-circuit.  The color-validation error uses
/// `ColorChoice::Auto` for its own styling because the
/// user-supplied color choice cannot be trusted; the
/// verbosity-validation error uses the already-resolved
/// color choice.
pub(crate) fn resolve_early_terminal_state(
    cli_color: Option<CliColorChoice>,
    verbose_count: u8,
    quiet: bool,
) -> Result<EarlyTerminalState, ExitCode> {
    let config_color = crate::cli::term_color::discover_early_config_color();
    let (color, _) = match crate::cli::term_color::resolve_color_choice(
        cli_color.map(Into::into),
        |key| std::env::var(key).ok(),
        config_color,
    ) {
        Ok(choice) => choice,
        Err(env_err) => {
            let mut stderr = StandardStream::stderr(termcolor_choice(ColorChoice::Auto));
            let _ = write_plain_error(&mut stderr, &env_err.to_string());
            return Err(ExitCode::FAILURE);
        }
    };

    let cli_verbosity = CliVerbosity {
        verbose_count,
        quiet,
    };
    let early_config_verbosity = discover_early_config_verbosity();
    let (verbosity, _) = match resolve_verbosity(
        cli_verbosity,
        |key| std::env::var(key).ok(),
        &early_config_verbosity,
    ) {
        Ok(level) => level,
        Err(env_err) => {
            let mut stderr = StandardStream::stderr(termcolor_choice(color));
            let _ = write_plain_error(&mut stderr, &env_err.to_string());
            return Err(ExitCode::FAILURE);
        }
    };
    let reporter = Reporter::with_color(verbosity, color);

    Ok(EarlyTerminalState { color, reporter })
}

/// The color choice and verbosity a command run without `--color`,
/// `-v`, or `-q` uses: [`resolve_early_terminal_state`]'s resolution
/// minus the flags.  `cabin metadata` reports these as `config.term`;
/// like `-j`, the flags configure one invocation and are left out.
pub(crate) fn configured_terminal_settings() -> anyhow::Result<TerminalSettings> {
    let env = |key: &str| std::env::var(key).ok();
    let (color, color_source) = crate::cli::term_color::resolve_color_choice(
        None,
        env,
        crate::cli::term_color::discover_early_config_color(),
    )?;
    let no_flags = CliVerbosity {
        verbose_count: 0,
        quiet: false,
    };
    let (verbosity, verbosity_source) =
        resolve_verbosity(no_flags, env, &discover_early_config_verbosity())?;
    Ok(TerminalSettings {
        color,
        color_source,
        verbosity,
        verbosity_source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_color_remains_terminal_aware() {
        assert!(matches!(
            termcolor_choice(ColorChoice::Auto),
            termcolor::ColorChoice::Auto
        ));
    }
}
