//! Renderer for `cabin --list`.
//!
//! `cabin --help` shows only the day-to-day commands so the
//! default view is short and easy to skim. Advanced and
//! machine-facing commands are hidden from `--help` by a
//! `#[command(hide = true)]` annotation in [`crate::cli::Cli`].
//!
//! `cabin --list` is the full directory: it walks the canonical
//! [`clap::Command`] tree, gathers every top-level subcommand
//! (including hidden ones), sorts them alphabetically, and
//! prints a stable name + short-about block.  The output is
//! intentionally cargo-style — a `Installed Commands:` heading
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
    let width = entries
        .iter()
        .map(|e| e.tokens.join(", ").len())
        .max()
        .unwrap_or(0);

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
                // text — same as cargo.
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

#[derive(Debug, Clone)]
struct CommandEntry {
    /// The canonical name first, followed by each visible
    /// alias.  Rendered joined by `, ` to match cargo's
    /// `cargo --list` style.
    tokens: Vec<String>,
    about: String,
}

fn collect_entries(cmd: &clap::Command) -> Vec<CommandEntry> {
    let mut entries: Vec<CommandEntry> = cmd
        .get_subcommands()
        .map(|sub| {
            let mut tokens = vec![sub.get_name().to_owned()];
            for alias in sub.get_visible_aliases() {
                tokens.push(alias.to_string());
            }
            let about = sub
                .get_about()
                .map(|s| {
                    // First line of the about block is the
                    // short summary clap uses in `--help`; long
                    // help has a separate field we ignore.
                    s.to_string().lines().next().unwrap_or("").trim().to_owned()
                })
                .unwrap_or_default();
            CommandEntry { tokens, about }
        })
        .collect();
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
    fn header_is_first_line() {
        let out = format_command_list(&fixture_cmd());
        let first = out.lines().next().expect("non-empty output");
        assert_eq!(first, LIST_HEADING);
    }

    #[test]
    fn output_ends_with_newline() {
        let out = format_command_list(&fixture_cmd());
        assert!(out.ends_with('\n'), "expected trailing newline: {out}");
    }

    #[test]
    fn entries_are_sorted_alphabetically() {
        let out = format_command_list(&fixture_cmd());
        let names: Vec<&str> = out
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "rows must be alphabetically sorted");
    }

    #[test]
    fn hidden_commands_are_listed() {
        let out = format_command_list(&fixture_cmd());
        // `compgen` was annotated `#[command(hide = true)]`; the
        // list view still surfaces it.
        assert!(
            out.contains("compgen"),
            "hidden subcommands must still appear in `--list`: {out}"
        );
    }

    #[test]
    fn help_pseudo_subcommand_is_listed_when_built() {
        // clap auto-injects a `help` pseudo-subcommand only
        // after `Command::build`.  Once built, the row is
        // included in the listing — matching cargo's
        // `cargo --list` which also surfaces `help`.
        let mut cmd = fixture_cmd();
        cmd.build();
        let out = format_command_list(&cmd);
        let names: Vec<&str> = out
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        assert!(
            names.contains(&"help"),
            "`help` should appear in --list once the command tree is built: {names:?}"
        );
    }

    #[test]
    fn name_about_separator_is_present() {
        let out = format_command_list(&fixture_cmd());
        // Each entry has the `<name>  <about>` shape; spot-check
        // one entry rather than over-coupling to the exact
        // column width (which depends on the longest name).
        // clap strips trailing punctuation from rustdoc-derived
        // about lines, so we compare on the leading words only.
        let build_line = out
            .lines()
            .find(|line| line.trim_start().starts_with("build"))
            .expect("build row");
        assert!(
            build_line.contains("Build a thing"),
            "build row should carry its about: {build_line}"
        );
    }

    #[test]
    fn entries_align_to_longest_name() {
        let out = format_command_list(&fixture_cmd());
        // The longest visible name in the fixture is `compgen`
        // (7 chars).  Build (5 chars) gets right-padded to 7
        // before its about text, so the gap is 2 columns.  This
        // is a structural assertion: the formatter must compute
        // the width once, not per-row.
        let build_line = out
            .lines()
            .find(|line| line.trim_start().starts_with("build"))
            .expect("build row");
        let compgen_line = out
            .lines()
            .find(|line| line.trim_start().starts_with("compgen"))
            .expect("compgen row");
        // Both rows have an `about` and the about text starts
        // at the same column.
        let build_about_col = build_line.find("Build").unwrap();
        let compgen_about_col = compgen_line.find("Generate").unwrap();
        assert_eq!(
            build_about_col, compgen_about_col,
            "about columns must align across rows"
        );
    }

    #[test]
    fn visible_aliases_are_rendered_cargo_style() {
        let out = format_command_list(&fixture_cmd());
        // Cargo renders aliases comma-separated after the name
        // (`build, b`), not in clap's `[aliases: b]` form.  The
        // fixture's build subcommand has a `b` alias.
        assert!(
            out.contains("build, b"),
            "expected cargo-style `build, b` row: {out}"
        );
        assert!(
            !out.contains("[aliases:"),
            "must not use clap's default `[aliases: ...]` form: {out}"
        );
    }

    #[test]
    fn empty_about_does_not_emit_separator() {
        // Synthesize a command tree where one subcommand has no
        // about text; the formatter must still emit the row,
        // and the row must not contain the column separator
        // spaces that other rows have.
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
