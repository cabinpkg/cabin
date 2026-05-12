//! Glue between the CLI's `version` subcommand and the typed
//! [`crate::version_info::VersionInfo`].
//!
//! `cabin version` is the dedicated version-reporting surface;
//! `cabin --version` continues to work through clap's
//! `#[command(version)]`.  The two differ deliberately:
//!
//! - `cabin --version` — concise, clap-framework spelling.  Same
//!   wording as `cabin version` so scripts that pipe either form
//!   stay equivalent.
//! - `cabin version` — concise output by default.  Honours the
//!   global verbosity model (`-v`) for a stable key/value block.
//!
//! Output is written directly to stdout rather than through the
//! status [`crate::term_verbosity_glue::Reporter`]: a user
//! asking for `cabin version -q` still wants the version line —
//! quiet only suppresses Cabin-owned status / progress messages.

use std::io::Write as _;

use anyhow::{Context, Result};
use cabin_core::Verbosity;
use clap::Args;

use crate::version_info::{VersionInfo, VersionOutputMode};

/// Arguments accepted by `cabin version`.  The subcommand has
/// no positional or flag inputs of its own — verbose output is
/// driven entirely by the global `-v` / `--verbose` flag so the
/// surface stays small.
#[derive(Debug, Args)]
pub(crate) struct VersionArgs {}

/// Decide which output mode to emit, given Cabin's resolved
/// verbosity.  Quiet does *not* downgrade the mode — quiet
/// only suppresses status messages, not requested command output.
fn output_mode_for(verbosity: Verbosity) -> VersionOutputMode {
    if verbosity.shows_verbose() {
        VersionOutputMode::Verbose
    } else {
        VersionOutputMode::Concise
    }
}

/// Top-level entry point for `cabin version`.  The result string
/// already carries a trailing newline so the function writes
/// with `write_all` instead of `println!`.
pub(crate) fn version(_args: VersionArgs, verbosity: Verbosity) -> Result<()> {
    let info = VersionInfo::current();
    let output = info.format(output_mode_for(verbosity));
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle
        .write_all(output.as_bytes())
        .context("failed to write version output")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_mode_normal_is_concise() {
        assert_eq!(
            output_mode_for(Verbosity::Normal),
            VersionOutputMode::Concise
        );
    }

    #[test]
    fn output_mode_quiet_is_concise() {
        // `-q` does not suppress the version line.  The mode
        // stays concise so a script that runs `cabin version -q`
        // observes the same single-line output.
        assert_eq!(
            output_mode_for(Verbosity::Quiet),
            VersionOutputMode::Concise
        );
    }

    #[test]
    fn output_mode_verbose_is_verbose() {
        assert_eq!(
            output_mode_for(Verbosity::Verbose),
            VersionOutputMode::Verbose
        );
    }

    #[test]
    fn output_mode_very_verbose_is_still_verbose() {
        // The verbose key/value block is already the most
        // detailed view; `-vv` does not unlock a new mode.
        assert_eq!(
            output_mode_for(Verbosity::VeryVerbose),
            VersionOutputMode::Verbose
        );
    }
}
