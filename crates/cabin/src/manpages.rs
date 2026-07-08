//! Man-page generation for `cabin mangen`.
//!
//! Like `compgen`, this module derives every byte of output from the
//! canonical [`clap::Command`] tree exposed by `Cli::command()`.
//! Every top-level subcommand - including ones hidden from
//! `cabin --help` such as `compgen` and `mangen` - gets its own
//! `cabin-<sub>.1` page so downstream packagers ship a complete
//! manual set.  The root `cabin.1` page mirrors `cabin --help` and
//! therefore omits hidden subcommands from its SUBCOMMANDS section;
//! the per-subcommand pages cover them.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use clap::{Args, CommandFactory};
use clap_mangen::Man;

use crate::cli::Cli;

/// Arguments accepted by `cabin mangen`.
#[derive(Debug, Args)]
pub(crate) struct MangenArgs {
    /// Directory to write man pages into.  Created if it does not
    /// already exist; existing files are overwritten.  Without this
    /// flag the root `cabin(1)` man page is written to stdout.
    #[arg(long, value_name = "PATH")]
    output_dir: Option<std::path::PathBuf>,
}

/// Top-level entry point for `cabin mangen`.
pub(crate) fn run(args: &MangenArgs) -> Result<()> {
    let cmd = Cli::command();
    match args.output_dir.as_deref() {
        None => write_root_to_stdout(&cmd)?,
        Some(dir) => write_to_dir(&cmd, dir)?,
    }
    Ok(())
}

fn write_root_to_stdout(cmd: &clap::Command) -> Result<()> {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    Man::new(cmd.clone())
        .render(&mut handle)
        .context("failed to render cabin(1) man page")?;
    Ok(())
}

fn write_to_dir(cmd: &clap::Command, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create man-page output dir {}", dir.display()))?;

    // Root page first: `cabin.1`.
    let root_path = dir.join(filename_for_root());
    let mut file = fs::File::create(&root_path)
        .with_context(|| format!("failed to create {}", root_path.display()))?;
    Man::new(cmd.clone())
        .render(&mut file)
        .with_context(|| format!("failed to render {}", root_path.display()))?;

    // One page per top-level subcommand, including ones hidden
    // from `cabin --help`.  Hidden commands stay shipped through
    // `cabin --list`, shell completions, and these per-command
    // pages.  Renaming the subcommand to `cabin-<sub>` yields a
    // man page whose `.TH` and SYNOPSIS show the conventional
    // `cabin-build(1)` form; the subcommand's own arguments
    // still render correctly because clap_mangen reads them
    // from the same Command.
    for sub in cmd.get_subcommands() {
        // Skip clap's auto-injected `help` pseudo-subcommand: the root
        // page already documents `--help`.
        if sub.get_name() == "help" {
            continue;
        }
        // clap's `Command::name` requires `&'static str`; leak the
        // freshly-built display name once per subcommand.  The CLI
        // process exits right after `mangen` returns, so the leak is
        // bounded.
        let display_name: &'static str =
            Box::leak(format!("cabin-{}", sub.get_name()).into_boxed_str());
        // Clear the hidden flag for the per-page render so the
        // page includes the command's arguments and options; the
        // root `cabin(1)` page still observes the original hidden
        // status and omits the command from its SUBCOMMANDS list
        // to match `cabin --help`.
        let renamed = sub.clone().name(display_name).hide(false);
        let path = dir.join(filename_for_subcommand(display_name));
        let mut file = fs::File::create(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
        Man::new(renamed)
            .render(&mut file)
            .with_context(|| format!("failed to render {}", path.display()))?;
    }
    Ok(())
}

/// Filename for the root `cabin(1)` man page.
fn filename_for_root() -> String {
    "cabin.1".to_owned()
}

/// Filename for the per-subcommand `cabin-<sub>(1)` man page,
/// given the already-prefixed display name.
fn filename_for_subcommand(display_name: &str) -> String {
    format!("{display_name}.1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_filename_is_cabin_dot_one() {
        assert_eq!(filename_for_root(), "cabin.1");
    }

    #[test]
    fn subcommand_filename_uses_dashed_form() {
        assert_eq!(filename_for_subcommand("cabin-build"), "cabin-build.1");
    }

    #[test]
    fn hidden_subcommands_are_known_and_curated() {
        // `cabin --help` curates the day-to-day surface matching
        // cargo's `--help` pattern: inspection (`metadata`,
        // `tree`, `explain`), low-level (`resolve`), offline /
        // networking (`fetch`, `vendor`), pre-publish
        // (`package`), and distribution helpers (`compgen`,
        // `mangen`) are hidden from the curated view but still
        // ship per-command man pages and appear in `cabin
        // --list`.  This test pins the hidden set so a new
        // hidden subcommand is reviewed intentionally rather
        // than slipping in by accident.
        use std::collections::BTreeSet;
        let cmd = Cli::command();
        let hidden: BTreeSet<&str> = cmd
            .get_subcommands()
            .filter(|s| s.is_hide_set())
            .map(clap::Command::get_name)
            .collect();
        // `login` / `logout` are hidden while the remote-registry
        // client they belong to stays behind `-Z remote-registry`.
        let expected: BTreeSet<&str> = [
            "compgen", "explain", "fetch", "login", "logout", "mangen", "metadata", "package",
            "resolve", "tree", "vendor",
        ]
        .iter()
        .copied()
        .collect();
        assert_eq!(
            hidden, expected,
            "hidden subcommand set drifted; update tests and review --help surface"
        );
    }
}
