//! Early-stage terminal-state resolution.
//!
//! `cabin` resolves the user's color and verbosity choice
//! before dispatching to any subcommand so even errors emitted
//! while loading a workspace honor `--color` / `--verbose`.
//! [`resolve_early_terminal_state`] applies the documented
//! precedence chains (see [`crate::term_color_glue`] and
//! [`crate::term_verbosity_glue`]) and returns the bundle the
//! dispatcher passes down: a [`ColorChoice`] and a
//! pre-configured [`Reporter`].
//!
//! Workspace-level overrides are intentionally *not* resolved
//! here — subcommands that load their own [`EffectiveConfig`]
//! see them through their own loop.  The early resolve only
//! observes the user-level config, which is the right shape
//! when no workspace context is available yet.

use std::process::ExitCode;

use cabin_core::ColorChoice;
use termcolor::StandardStream;

use crate::error_rendering::write_plain_error;
use crate::term_color_glue::{self, CliColorChoice};
use crate::term_verbosity_glue::{
    CliVerbosity, Reporter, discover_early_config_verbosity, resolve_verbosity,
};

/// Resolved terminal state available before any subcommand runs.
pub(crate) struct EarlyTerminalState {
    pub(crate) color: ColorChoice,
    pub(crate) reporter: Reporter,
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
    let config_color = term_color_glue::discover_early_config_color();
    let color = match term_color_glue::resolve_color_choice(
        cli_color.map(Into::into),
        |key| std::env::var(key).ok(),
        config_color,
    ) {
        Ok(choice) => choice,
        Err(env_err) => {
            // Use `Auto` for the styling of the error itself —
            // we cannot trust the value the user gave us.
            let mut stderr =
                StandardStream::stderr(cabin_diagnostics::termcolor_choice(ColorChoice::Auto));
            let _ = write_plain_error(&mut stderr, &env_err.to_string());
            return Err(ExitCode::FAILURE);
        }
    };

    let cli_verbosity = CliVerbosity {
        verbose_count,
        quiet,
    };
    let early_config_verbosity = discover_early_config_verbosity();
    let verbosity = match resolve_verbosity(
        cli_verbosity,
        |key| std::env::var(key).ok(),
        &early_config_verbosity,
    ) {
        Ok(level) => level,
        Err(env_err) => {
            let mut stderr = StandardStream::stderr(cabin_diagnostics::termcolor_choice(color));
            let _ = write_plain_error(&mut stderr, &env_err.to_string());
            return Err(ExitCode::FAILURE);
        }
    };
    let reporter = Reporter::with_color(verbosity, color);

    Ok(EarlyTerminalState { color, reporter })
}
