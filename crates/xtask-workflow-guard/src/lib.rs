//! Guards that keep an out-of-order or premature workflow run from
//! mutating a shared resource, decided from repository files, git
//! history and the GitHub Actions run context.
//!
//! This is the only crate allowed to read `GITHUB_*` context, write
//! `$GITHUB_OUTPUT` / `$GITHUB_ENV`, and call the GitHub REST API. It
//! never touches the live registry service and never performs the
//! mutation it gates - it reads committed repository state and the
//! run's own context, and only decides whether the mutation may
//! proceed. It holds no secret beyond the run's own `GITHUB_TOKEN`.

use std::fs::OpenOptions;
use std::io::Write as _;

use anyhow::{Context as _, Result, bail};

pub mod migrations_pending;
pub mod superseded;

/// The bytes `$(...)` yields for a captured command: command
/// substitution drops NUL bytes and strips every trailing newline.
pub(crate) fn substitute(mut stdout: Vec<u8>) -> Vec<u8> {
    stdout.retain(|byte| *byte != 0);
    while stdout.last() == Some(&b'\n') {
        stdout.pop();
    }
    stdout
}

/// The guards' positive answers are all one `echo "..." >> "$GITHUB_OUTPUT"`,
/// so the redirect's failure shapes live here once: an unset or empty
/// `GITHUB_OUTPUT` fails the step, exactly where the shell's `>> ""` did.
pub(crate) fn append_github_output(line: &str) -> Result<()> {
    let path = std::env::var("GITHUB_OUTPUT").unwrap_or_default();
    if path.is_empty() {
        bail!(
            "GITHUB_OUTPUT is unset; cannot record `{}`",
            line.trim_end()
        );
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {path} for append"))?;
    file.write_all(line.as_bytes())
        .with_context(|| format!("writing to {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitution_strips_every_trailing_newline() {
        assert_eq!(substitute(b"abc\n".to_vec()), b"abc");
        assert_eq!(substitute(b"abc\n\n\n".to_vec()), b"abc");
        assert_eq!(substitute(b"\n".to_vec()), b"");
        assert!(substitute(Vec::new()).is_empty());
    }

    #[test]
    fn substitution_drops_nul_bytes() {
        assert_eq!(substitute(b"a\0b\n".to_vec()), b"ab");
        // A capture of nothing but NULs is empty, so it compares
        // exactly as the shell's empty substitution did.
        assert!(substitute(b"\0\0".to_vec()).is_empty());
    }
}
