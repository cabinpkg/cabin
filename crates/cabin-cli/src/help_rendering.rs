//! Customisation layer for `cabin --help`.
//!
//! Clap renders the top-level help using the `HELP_TEMPLATE`
//! declared on [`crate::cli::Cli`]; the `{after-help}` slot is
//! filled in by this module so the curated `Commands:` block
//! matches cargo's layout instead of clap's default
//! `[aliases: …]` rendering.
//!
//! [`prepare_top_level_command`] is the single entry point.
//! It mutates the clap command tree (hide the auto-injected
//! `help` row, append the cargo-style `...` hint, attach the
//! styled commands block and the `cabin help <command>`
//! trailer) and hands the result back to the dispatcher.  The
//! canonical [`crate::cli::Cli::command`] tree consumed by
//! [`crate::command_list`], [`crate::completions`], and
//! [`crate::manpages`] is never touched, so the `...` marker
//! stays out of `cabin --list`, shell completions, and man
//! pages.

use std::fmt::Write as _;

use clap::CommandFactory;

use crate::cli::Cli;

/// Marker name for the cargo-style `...` row that appears at
/// the end of the `cabin --help` Commands block.  It points
/// users at `cabin --list` without polluting the Subcommand
/// enum: the row is injected into the clap command tree only
/// for help / parsing, and the dispatcher treats it as an
/// alias for `--list`.
pub(crate) const DOTS_HINT: &str = "...";

/// About text rendered next to the [`DOTS_HINT`] row.  Matches
/// cargo's wording for the equivalent hint in `cargo --help`.
const DOTS_ABOUT: &str = "See all commands with --list";

/// Build the clap command tree used for top-level help and
/// argument parsing.
///
/// The returned command is the canonical [`Cli::command`]
/// tree with two changes:
/// - clap's auto-injected `help` pseudo-subcommand is hidden
///   so it never appears in the Commands block (`cabin help
///   <cmd>` still works);
/// - a cargo-style `...   See all commands with --list` row
///   is appended as the last visible entry; the dispatcher
///   treats `cabin ...` as a shortcut for `cabin --list`.
///
/// The styled Commands block and the `cabin help <command>`
/// trailer are then attached via `after_help` so the
/// HELP_TEMPLATE renders the same layout cargo emits.
pub(crate) fn prepare_top_level_command() -> clap::Command {
    // `Command::build` forces clap to materialise its
    // auto-injected `help` pseudo-subcommand so we can
    // address it by name.  `mut_subcommand("help", …)` then
    // hides the help row from the Commands block.
    let mut cmd = Cli::command();
    cmd.build();
    let cmd = cmd.mut_subcommand("help", |sub| sub.hide(true)).subcommand(
        clap::Command::new(DOTS_HINT)
            .about(DOTS_ABOUT)
            .disable_help_subcommand(true),
    );
    // Render the Commands block manually so visible aliases
    // appear in cargo's `name, alias` style (`build, b`).
    // Clap's `{subcommands}` placeholder uses the default
    // `[aliases: b]` rendering, which is not what cargo
    // emits.  See `format_commands_block` for the format.
    //
    // Append the cargo-style trailer that points users at
    // `cabin help <command>` for per-subcommand detail.
    let mut after_help = format_commands_block(&cmd);
    after_help.push('\n');
    after_help.push_str("See 'cabin help <command>' for more information on a specific command.\n");
    cmd.after_help(after_help)
}

/// Render the styled `Commands:` block for `cabin --help`,
/// using cargo's `name, alias` rendering instead of clap's
/// default `[aliases: alias]` form.
///
/// Embedded ANSI escapes paint:
/// - the `Commands:` heading bright green + bold (matching
///   clap's auto styling of `Usage:`);
/// - each `<name>[, <alias>]` cell bright cyan + bold;
/// - the about text stays plain.
///
/// anstream strips the escapes when the writer disables
/// colour, so `cabin --color never --help` and pipe-redirected
/// output stay clean.  Hidden subcommands are skipped because
/// `cabin --help` is the curated view; the full directory lives
/// in `cabin --list`.
fn format_commands_block(cmd: &clap::Command) -> String {
    /// One subcommand row: the canonical name plus any
    /// visible aliases, paired with the short about text.  The
    /// `tokens` list keeps each name / alias separate so the
    /// renderer can style them individually while leaving the
    /// `, ` separators unstyled — same as cargo.
    struct Row {
        tokens: Vec<String>,
        about: String,
    }

    let rows: Vec<Row> = cmd
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .map(|sub| {
            let mut tokens = vec![sub.get_name().to_owned()];
            for alias in sub.get_visible_aliases() {
                tokens.push(alias.to_string());
            }
            let about = sub
                .get_about()
                .map(|s| s.to_string().lines().next().unwrap_or("").trim().to_owned())
                .unwrap_or_default();
            Row { tokens, about }
        })
        .collect();

    // The display width of a row is the length of all tokens
    // joined by `, ` (the printed separator).  ANSI escapes
    // around each token do not add display width because they
    // do not advance the cursor, but they do add bytes — we
    // compute the visible width from the plain-text join.
    let width = rows
        .iter()
        .map(|row| row.tokens.join(", ").len())
        .max()
        .unwrap_or(0);

    // clap prepends a blank line before `{after-help}`, so
    // our block starts directly with the styled heading.
    let mut out = String::new();
    let _ = writeln!(out, "\x1b[1m\x1b[92mCommands:\x1b[0m");
    for row in &rows {
        out.push_str("  ");
        let plain_width: usize = row.tokens.join(", ").len();
        for (i, token) in row.tokens.iter().enumerate() {
            if i > 0 {
                // Cargo emits the `, ` between aliases as plain
                // text; only the name / alias tokens get the
                // bright-cyan + bold styling.
                out.push_str(", ");
            }
            let _ = write!(out, "\x1b[1m\x1b[96m{token}\x1b[0m");
        }
        if row.about.is_empty() {
            out.push('\n');
        } else {
            // Pad to the column where the about text begins.
            let padding = width.saturating_sub(plain_width);
            for _ in 0..padding {
                out.push(' ');
            }
            let _ = writeln!(out, "  {about}", about = row.about);
        }
    }
    out
}
