//! The deploy freshness guard, ported from the `run:` body of the
//! "Skip when superseded by a newer main commit" step of
//! `.github/workflows/registry.yml`.
//!
//! ```text
//! L1  git fetch --quiet origin main
//! L2  if [ -n "$(git rev-list -n 1 "$GITHUB_SHA..origin/main")" ]; then
//! L9    echo "superseded=true" >> "$GITHUB_OUTPUT"
//! L10 fi
//! ```
//!
//! The step listed a pathspec once, duplicating `registry.yml`'s two
//! trigger filters. The filters are gone - the workflow runs on every
//! main push - so any newer commit supersedes this run.
//!
//! Inherited properties, preserved rather than fixed - a port is not a
//! place to change behavior. Each was pinned by running the original
//! under `bash -e`, GitHub's default `run:` shell (`-e` on, `-u` and
//! `-o pipefail` off):
//!
//! - **L2 fails open.** The substitution sits in an `if` condition,
//!   where `set -e` is suppressed and `[` sees only the captured text,
//!   never `rev-list`'s status. A `rev-list` that errors - an unknown
//!   `$GITHUB_SHA`, a shallow clone not containing it - therefore
//!   captures nothing, answers "not superseded" and exits 0 with
//!   `$GITHUB_OUTPUT` untouched. Only L1, a standalone command, is
//!   fail-safe.
//! - **An unset `GITHUB_SHA` reads as empty**, so the range degrades to
//!   `..origin/main`, which git resolves as `HEAD..origin/main`. Hence
//!   the literal concatenation in `range` rather than resolving the SHA
//!   first.
//! - **Nothing reaches stdout** in either answer. The negative case
//!   writes nothing anywhere.
//!
//! Stated ceiling: L1's exit status collapses to 1 here, where the shell
//! propagated git's own (128 for an unreachable remote), matching the
//! precedent the `registry-verify` port set.

use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

/// L9's line, byte for byte.
const OUTPUT_LINE: &str = "superseded=true\n";

/// Answer whether `origin/main` carries a commit after `$GITHUB_SHA`,
/// recording `superseded=true` in `$GITHUB_OUTPUT` when it does.
///
/// # Errors
///
/// When `git fetch` fails (L1), or when `$GITHUB_OUTPUT` is unusable in
/// the positive case (L9).
pub fn run() -> Result<()> {
    fetch_origin_main()?;

    let sha = std::env::var("GITHUB_SHA").unwrap_or_default();
    if newer_commit(&range(&sha))?.is_empty() {
        return Ok(());
    }
    record_superseded()
}

/// L2's range argument. Concatenated, never resolved: see the empty-SHA
/// note in the module docs.
fn range(sha: &str) -> String {
    format!("{sha}..origin/main")
}

/// L1. The one fail-safe step: a standalone command under `set -e`.
fn fetch_origin_main() -> Result<()> {
    let status = Command::new("git")
        .args(["fetch", "--quiet", "origin", "main"])
        .status()
        .context("spawning `git fetch --quiet origin main`")?;
    if !status.success() {
        bail!("`git fetch --quiet origin main` failed: {status}");
    }
    Ok(())
}

/// L2. A non-zero `rev-list` yields the empty capture the shell's
/// condition context yielded, not an error. Only a failure to *spawn*
/// git surfaces - unreachable in practice, since L1 just ran it.
fn newer_commit(range: &str) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(["rev-list", "-n", "1", range])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawning `git rev-list`")?
        .wait_with_output()
        .context("waiting for `git rev-list`")?;
    Ok(crate::substitute(output.stdout))
}

/// L9, reachable only in the positive case - the redirect lives inside
/// the `if`, so an unset `GITHUB_OUTPUT` fails the step there and
/// nowhere else.
fn record_superseded() -> Result<()> {
    crate::append_github_output(OUTPUT_LINE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_sha_leaves_git_reading_the_range_as_head() {
        assert_eq!(range(""), "..origin/main");
        assert_eq!(range("deadbeef"), "deadbeef..origin/main");
    }

    #[test]
    fn the_recorded_line_matches_the_shells_echo() {
        assert_eq!(OUTPUT_LINE.as_bytes(), b"superseded=true\n");
    }
}
