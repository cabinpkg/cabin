//! Library half of the `cabin` CLI binary.
//!
//! The bin (`src/main.rs`) is intentionally a thin shim that
//! calls [`run`]; the typed parser ([`Cli`]), the
//! command dispatcher, and every glue module live here so
//! integration tests can re-use the same surface the binary
//! does — `Cli::command()` is the single source of truth for
//! which subcommands exist and which are hidden.

#![allow(
    clippy::struct_excessive_bools,
    clippy::too_many_lines,
    clippy::unused_self,
    // Remaining hits are `root_settings: Default::default()` in test
    // graph fixtures, where the typed-default suggestion is MaybeIncorrect.
    clippy::default_trait_access
)]

use std::process::ExitCode;

use clap::FromArgMatches;

pub use crate::cli::Cli;
use crate::term_setup::{EarlyTerminalState, resolve_early_terminal_state};

// The clap parser stays reachable for this crate's glue modules,
// but `Cli` is re-exported at the crate root for integration
// tests and downstream command-tree generation.
mod build_prep_glue;
mod cli;
mod command_list;
mod completions;
mod config_glue;
mod diagnostic_registry;
mod env_flags_glue;
mod error_rendering;
mod explain_glue;
mod fetch_output_glue;
mod fmt_glue;
mod help_rendering;
mod manpages;
mod metadata_glue;
mod ninja_glue;
mod patch_glue;
mod port_glue;
mod port_subcommand;
mod run_glue;
mod source_tooling_glue;
mod system_deps_glue;
mod term_color_glue;
mod term_setup;
mod term_verbosity_glue;
mod test_glue;
mod tidy_glue;
mod tree_glue;
mod vendor_glue;
mod version_glue;
mod version_info;

/// Return the English plural suffix for a count: empty for one,
/// `s` for everything else. Used across the reporter glue files
/// to keep `"<n> file"` / `"<n> files"` consistent.
pub(crate) fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Serialize `value` as pretty-printed JSON and write it to stdout
/// followed by a newline. `error_context` is attached to any
/// serialization failure, matching `anyhow::Context::context`.
pub(crate) fn print_pretty_json<T>(value: &T, error_context: &'static str) -> anyhow::Result<()>
where
    T: serde::Serialize + ?Sized,
{
    use anyhow::Context;
    let body = serde_json::to_string_pretty(value).context(error_context)?;
    println!("{body}");
    Ok(())
}

/// Run the `cabin` CLI to completion using the given argv
/// iterator.  Owns parsing, color/verbosity resolution,
/// dispatch, and top-level error rendering.  The binary
/// `main` calls this with the process's own arguments.
pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cmd = help_rendering::prepare_top_level_command();
    let matches = match cmd.try_get_matches_from(args) {
        Ok(m) => m,
        Err(err) => {
            // `clap::Error` already routes `--help` /
            // `--version` to stdout and real errors to stderr
            // with the correct ANSI handling.  Just hand it
            // through.
            err.exit();
        }
    };
    // `cabin ...` is a help-row affordance that doubles as a
    // shortcut for `cabin --list`.  The unmapped subcommand
    // produces `cmd: None` after `from_arg_matches`; promote
    // it to `list = true` so the downstream dispatcher renders
    // the listing with the same color-aware code path as the
    // real flag.
    let dots_shortcut = matches.subcommand_name() == Some(help_rendering::DOTS_HINT);
    let mut parsed = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(err) => err.exit(),
    };
    if dots_shortcut {
        parsed.list = true;
    }

    let EarlyTerminalState { color, reporter } =
        match resolve_early_terminal_state(parsed.color, parsed.verbose, parsed.quiet) {
            Ok(state) => state,
            Err(exit_code) => return exit_code,
        };

    match cli::run(parsed, reporter, color) {
        Ok(code) => code,
        Err(error) => {
            error_rendering::render_error(&error, color);
            ExitCode::FAILURE
        }
    }
}
