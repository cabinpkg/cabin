//! The deploy freshness guard, ported one-to-one from the `run:` body of
//! the "Skip when superseded by a newer registry commit" step of
//! `.github/workflows/registry.yml`.
//!
//! ```text
//! L1  git fetch --quiet origin main
//! L2  if [ -n "$(git rev-list -n 1 "$GITHUB_SHA..origin/main" -- \
//! L3..L8    <paths>)" ]; then
//! L9    echo "superseded=true" >> "$GITHUB_OUTPUT"
//! L10 fi
//! ```
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

use std::fs::OpenOptions;
use std::io::Write as _;
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

/// L9's line, byte for byte.
const OUTPUT_LINE: &str = "superseded=true\n";

/// Answer whether `origin/main` carries a commit after `$GITHUB_SHA`
/// touching any of `paths`, recording `superseded=true` in
/// `$GITHUB_OUTPUT` when it does.
///
/// # Errors
///
/// When `git fetch` fails (L1), when `$GITHUB_OUTPUT` is unusable in the
/// positive case (L9), or when no path was given.
pub fn run(paths: &[String]) -> Result<()> {
    // Not the original's concern - the list was a literal there. An
    // empty pathspec makes `rev-list` match every commit, i.e. a guard
    // that always answers "superseded" and silently stops every deploy.
    if paths.is_empty() {
        bail!("no --path given; an empty pathspec would match every commit");
    }

    fetch_origin_main()?;

    let sha = std::env::var("GITHUB_SHA").unwrap_or_default();
    if newer_commit(&range(&sha), paths)?.is_empty() {
        return Ok(());
    }
    record_superseded()
}

/// L2's range argument. Concatenated, never resolved: see the empty-SHA
/// note in the module docs.
fn range(sha: &str) -> String {
    format!("{sha}..origin/main")
}

/// The bytes `$(...)` yields for L2: command substitution drops NUL
/// bytes and strips every trailing newline.
fn substitute(mut stdout: Vec<u8>) -> Vec<u8> {
    stdout.retain(|byte| *byte != 0);
    while stdout.last() == Some(&b'\n') {
        stdout.pop();
    }
    stdout
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
fn newer_commit(range: &str, paths: &[String]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(["rev-list", "-n", "1", range, "--"])
        .args(paths)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("spawning `git rev-list`")?
        .wait_with_output()
        .context("waiting for `git rev-list`")?;
    Ok(substitute(output.stdout))
}

/// L9, reachable only in the positive case - the redirect lives inside
/// the `if`, so an unset `GITHUB_OUTPUT` fails the step there and
/// nowhere else.
fn record_superseded() -> Result<()> {
    let path = std::env::var("GITHUB_OUTPUT").unwrap_or_default();
    if path.is_empty() {
        bail!("GITHUB_OUTPUT is unset; cannot record `superseded=true`");
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {path} for append"))?;
    file.write_all(OUTPUT_LINE.as_bytes())
        .with_context(|| format!("writing to {path}"))
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
    fn substitution_strips_every_trailing_newline() {
        assert_eq!(substitute(b"abc\n".to_vec()), b"abc");
        assert_eq!(substitute(b"abc\n\n\n".to_vec()), b"abc");
        assert_eq!(substitute(b"\n".to_vec()), b"");
        assert!(substitute(Vec::new()).is_empty());
    }

    #[test]
    fn substitution_drops_nul_bytes() {
        assert_eq!(substitute(b"a\0b\n".to_vec()), b"ab");
        // A capture of nothing but NULs is empty, so it answers "not
        // superseded" exactly as the shell's `[ -n "" ]` did.
        assert!(substitute(b"\0\0".to_vec()).is_empty());
    }

    #[test]
    fn the_recorded_line_matches_the_shells_echo() {
        assert_eq!(OUTPUT_LINE.as_bytes(), b"superseded=true\n");
    }

    #[test]
    fn an_empty_path_list_is_refused_before_any_git_call() {
        let error = run(&[]).unwrap_err().to_string();
        assert!(error.contains("no --path given"), "{error}");
    }
}
