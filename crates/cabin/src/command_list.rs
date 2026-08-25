//! Renderer for `cabin --list`.
//!
//! `cabin --help` shows only the day-to-day commands so the
//! default view is short and easy to skim.  Advanced and
//! machine-facing commands are hidden from `--help` by a
//! `#[command(hide = true)]` annotation in [`crate::cli::Cli`].
//!
//! `cabin --list` is the full directory: it walks the canonical
//! [`clap::Command`] tree, gathers every top-level subcommand
//! (including hidden ones), sorts them alphabetically, and
//! prints a stable name + short-about block.  The output is
//! intentionally cargo-style - a `Installed Commands:` heading
//! followed by indented `<name> <about>` rows.
//!
//! The module is `pub(crate)`; integration tests run the binary
//! and assert against the printed bytes.  The pure
//! [`format_command_list`] helper is exercised by unit tests so
//! the formatter stays decoupled from the process stdout.

use anyhow::{Context, Result};
use clap::CommandFactory;
use termcolor::{Color, ColorSpec, WriteColor};

use crate::cli::Cli;
use crate::{SubcommandRow, row_from_subcommand, rows_display_width};

/// Heading printed before the indented command rows.  Stable
/// wording so integration tests can pin it.
const LIST_HEADING: &str = "Installed Commands:";

/// Indent prefix for each row.  Four spaces matches cargo's
/// `cargo --list`.
const ROW_INDENT: &str = "    ";

/// Build the deterministic command-list output for the canonical
/// [`Cli`] command tree and write it to `out`.  The writer
/// implements [`WriteColor`] so callers honor the caller-
/// resolved color choice: a `termcolor::StandardStream` built
/// from Cabin's resolved `--color` value paints the heading and
/// subcommand names in the cargo-style palette, while a
/// no-color writer (`Buffer`, redirected stdout, …) emits the
/// same content as plain bytes.
pub(crate) fn print_list<W: WriteColor>(out: &mut W) -> Result<()> {
    // `Command::build` materializes clap's auto-injected
    // `help` pseudo-subcommand so it appears in the listing.
    // Without the explicit build call `Cli::command()` only
    // carries the user-declared subcommands; cargo's
    // `cargo --list` includes `help`, and so do we.
    let mut cmd = Cli::command();
    cmd.build();
    write_command_list(out, &cmd).context("failed to write command list")
}

/// Render the command list onto a [`WriteColor`] sink, using
/// the cargo-style palette: bright green + bold heading,
/// bright cyan + bold subcommand names and aliases, plain
/// about text and plain `, ` separators.  The color
/// transitions are guarded by `set_color` / `reset` so callers
/// passing a no-color writer see the same plain text the
/// [`format_command_list`] helper produces.
fn write_command_list<W: WriteColor>(out: &mut W, cmd: &clap::Command) -> std::io::Result<()> {
    let entries = collect_entries(cmd);
    let width = rows_display_width(&entries);

    let mut heading_spec = ColorSpec::new();
    heading_spec
        .set_fg(Some(Color::Green))
        .set_intense(true)
        .set_bold(true);
    out.set_color(&heading_spec)?;
    write!(out, "{LIST_HEADING}")?;
    out.reset()?;
    writeln!(out)?;

    let mut name_spec = ColorSpec::new();
    name_spec
        .set_fg(Some(Color::Cyan))
        .set_intense(true)
        .set_bold(true);

    for entry in &entries {
        out.write_all(ROW_INDENT.as_bytes())?;
        let plain_width: usize = entry.tokens.join(", ").len();
        for (i, token) in entry.tokens.iter().enumerate() {
            if i > 0 {
                // The `, ` between name and alias stays plain
                // text - same as cargo.
                out.write_all(b", ")?;
            }
            out.set_color(&name_spec)?;
            write!(out, "{token}")?;
            out.reset()?;
        }
        if entry.about.is_empty() {
            writeln!(out)?;
        } else {
            let padding = width.saturating_sub(plain_width);
            for _ in 0..padding {
                out.write_all(b" ")?;
            }
            writeln!(out, "  {about}", about = entry.about)?;
        }
    }
    Ok(())
}

/// Test-only convenience that drives [`write_command_list`]
/// against an in-memory uncolored buffer and returns the
/// rendered text.  Wrapping the real renderer (instead of
/// duplicating its formatting code) keeps unit-test
/// expectations honest: any change to the production layout
/// shows up in both surfaces in one place.
#[cfg(test)]
fn format_command_list(cmd: &clap::Command) -> String {
    use termcolor::NoColor;
    let mut buf = NoColor::new(Vec::<u8>::new());
    write_command_list(&mut buf, cmd).expect("Vec writer never fails");
    String::from_utf8(buf.into_inner()).expect("rendered output is utf-8")
}

fn collect_entries(cmd: &clap::Command) -> Vec<SubcommandRow> {
    let mut entries: Vec<SubcommandRow> = cmd.get_subcommands().map(row_from_subcommand).collect();
    entries.sort_by(|a, b| a.tokens[0].cmp(&b.tokens[0]));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser, Subcommand};

    #[derive(Parser, Debug)]
    #[command(name = "test")]
    struct FixtureCli {
        #[command(subcommand)]
        cmd: FixtureCmd,
    }

    #[derive(Subcommand, Debug)]
    enum FixtureCmd {
        /// Build a thing.
        #[command(visible_alias = "b")]
        Build(EmptyArgs),
        /// Clean output.
        Clean(EmptyArgs),
        /// Generate completions (advanced).
        #[command(hide = true)]
        Compgen(EmptyArgs),
    }

    #[derive(Args, Debug)]
    struct EmptyArgs {}

    fn fixture_cmd() -> clap::Command {
        <FixtureCli as CommandFactory>::command()
    }

    #[test]
    fn formats_sorted_complete_rows() {
        assert_eq!(
            format_command_list(&fixture_cmd()),
            "Installed Commands:\n    build, b  Build a thing\n    clean     Clean output\n    compgen   Generate completions (advanced)\n"
        );
    }

    #[test]
    fn built_command_includes_help_pseudo_subcommand() {
        let mut cmd = fixture_cmd();
        cmd.build();
        assert!(format_command_list(&cmd).contains("\n    help"));
    }

    #[test]
    fn empty_about_does_not_emit_separator() {
        let cmd = clap::Command::new("test")
            .subcommand(clap::Command::new("alpha").about("Alpha command."))
            .subcommand(clap::Command::new("beta"));
        let out = format_command_list(&cmd);
        let beta_line = out
            .lines()
            .find(|line| line.trim_start().starts_with("beta"))
            .expect("beta row");
        assert_eq!(beta_line.trim(), "beta");
    }
}
