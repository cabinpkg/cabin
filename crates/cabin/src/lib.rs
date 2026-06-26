//! Library half of the `cabin` CLI binary.
//!
//! The bin (`src/main.rs`) is intentionally a thin shim that
//! calls [`run`]; the typed parser ([`Cli`]), the command
//! dispatcher, and the per-command glue modules under `cli/`
//! live here so integration tests can re-use the same surface
//! the binary does - `Cli::command()` is the single source of
//! truth for which subcommands exist and which are hidden.

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
mod cli;
mod command_list;
mod completions;
mod diagnostic_registry;
mod error_rendering;
mod help_rendering;
mod manpages;
mod port_subcommand;
mod stamp;
mod term_setup;
mod version_info;

/// Return the English plural suffix for a count: empty for one,
/// `s` for everything else.  Used across the reporter glue files
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

/// One top-level subcommand row: the canonical name first,
/// followed by each visible alias, paired with the short about
/// text.  Shared by the `--list` renderer ([`crate::command_list`])
/// and the `--help` Commands block ([`crate::help_rendering`]);
/// each keeps its own render loop (color sink vs.  ANSI string,
/// sorted vs. declaration order, all subcommands vs. visible-only),
/// but the row extraction and column-width computation are
/// identical, so they live here.
#[derive(Debug, Clone)]
pub(crate) struct SubcommandRow {
    /// The canonical name first, followed by each visible alias.
    /// Rendered joined by `, ` to match cargo's `cargo --list` style.
    pub(crate) tokens: Vec<String>,
    pub(crate) about: String,
}

/// Extract the [`SubcommandRow`] for a single clap subcommand: its
/// name plus any visible aliases, and the first line of its about
/// block (the short summary clap uses in `--help`).
pub(crate) fn row_from_subcommand(sub: &clap::Command) -> SubcommandRow {
    let mut tokens = vec![sub.get_name().to_owned()];
    for alias in sub.get_visible_aliases() {
        tokens.push(alias.to_string());
    }
    let about = sub
        .get_about()
        .map(|s| s.to_string().lines().next().unwrap_or("").trim().to_owned())
        .unwrap_or_default();
    SubcommandRow { tokens, about }
}

/// Display width of the widest row's `<name>[, <alias>…]` cell,
/// used to align the about column.  Computed from the plain-text
/// `, ` join so embedded ANSI escapes (which do not advance the
/// cursor) never inflate the width.
pub(crate) fn rows_display_width(rows: &[SubcommandRow]) -> usize {
    rows.iter()
        .map(|row| row.tokens.join(", ").len())
        .max()
        .unwrap_or(0)
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
    // `cabin stamp <FILE> -- <CMD>` is internal build plumbing (the Ninja
    // syntax-check rule's witness writer); dispatch it before clap so it
    // never enters the user-facing command surface - no `--help`,
    // `--list`, completions, or man pages, and no special-case filtering.
    let arguments: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    if let Some(code) = stamp::dispatch(&arguments) {
        return code;
    }

    let cmd = help_rendering::prepare_top_level_command();
    let matches = match cmd.try_get_matches_from(arguments) {
        Ok(m) => m,
        Err(err) => {
            // `clap::Error` already routes `--help` /
            // `--version` to stdout and real errors to stderr
            // with the correct ANSI handling.  Hand it
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
