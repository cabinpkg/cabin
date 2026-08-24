//! Shell-completion generation for `cabin compgen`.
//!
//! Completions are derived from the canonical [`clap::Command`]
//! produced by the top-level CLI (`Cli::command()`).  The clap
//! definition in [`crate::cli`] is the single source of truth - this
//! module never reaches into command names or argument metadata
//! directly.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory};
use clap_complete::Shell;

use crate::cli::Cli;

/// Binary name to embed in generated completion scripts.
const BIN_NAME: &str = "cabin";

/// Every shell `cabin compgen --all` writes a script for.  Other
/// `clap_complete::Shell` variants are still accepted as a positional
/// arg so users can ask for them explicitly.
const ALL_SHELLS: &[Shell] = &[
    Shell::Bash,
    Shell::Zsh,
    Shell::Fish,
    Shell::PowerShell,
    Shell::Elvish,
];

/// Arguments accepted by `cabin compgen`.
#[derive(Debug, Args)]
pub(crate) struct CompgenArgs {
    /// Target shell.  Required unless `--all` is given.
    #[arg(value_enum, conflicts_with = "all", required_unless_present = "all")]
    shell: Option<Shell>,

    /// Generate completions for every supported shell.  Requires
    /// `--output-dir`; multiple files cannot be written to stdout
    /// cleanly.
    #[arg(long)]
    all: bool,

    /// Directory to write the completion file(s) into.  Created if it
    /// does not already exist; existing files are overwritten.
    /// Without this flag a single shell's completion is written to
    /// stdout.
    #[arg(long, value_name = "PATH")]
    output_dir: Option<std::path::PathBuf>,
}

/// Top-level entry point for `cabin compgen`.
pub(crate) fn run(args: &CompgenArgs) -> Result<()> {
    if args.all && args.output_dir.is_none() {
        bail!(
            "`--all` requires `--output-dir`; multiple completion files cannot be written to stdout cleanly"
        );
    }

    let mut cmd = Cli::command();

    if args.all {
        let dir = args.output_dir.as_ref().expect("checked above");
        write_all(&mut cmd, dir)?;
        return Ok(());
    }

    let shell = args.shell.expect("clap enforces required-unless-all");
    match args.output_dir.as_ref() {
        Some(dir) => write_one_to_dir(&mut cmd, shell, dir)?,
        None => write_one_to_stdout(&mut cmd, shell),
    }
    Ok(())
}

fn write_all(cmd: &mut clap::Command, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create completion output dir {}", dir.display()))?;
    for shell in ALL_SHELLS {
        write_one_to_dir(cmd, *shell, dir)?;
    }
    Ok(())
}

fn write_one_to_dir(cmd: &mut clap::Command, shell: Shell, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create completion output dir {}", dir.display()))?;
    let path = dir.join(filename_for(shell));
    let mut file =
        fs::File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;
    clap_complete::generate(shell, cmd, BIN_NAME, &mut file);
    Ok(())
}

fn write_one_to_stdout(cmd: &mut clap::Command, shell: Shell) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    clap_complete::generate(shell, cmd, BIN_NAME, &mut handle);
}

/// Filename `cabin compgen --output-dir <dir>` writes for each
/// `Shell`.  The names match what package managers usually expect on
/// disk (`cabin.bash`, `_cabin`, `cabin.fish`, …); deviations from
/// `clap_complete`'s default filenames are intentional and stable.
fn filename_for(shell: Shell) -> String {
    match shell {
        Shell::Bash => "cabin.bash".to_owned(),
        Shell::Zsh => "_cabin".to_owned(),
        Shell::Fish => "cabin.fish".to_owned(),
        Shell::PowerShell => "cabin.ps1".to_owned(),
        Shell::Elvish => "cabin.elv".to_owned(),
        // `clap_complete::Shell` is `#[non_exhaustive]`; future
        // variants get a generic, deterministic filename.
        other => format!("cabin.{}", other.to_string().to_lowercase()),
    }
}
