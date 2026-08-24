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
//! The original step listed a pathspec once, duplicating
//! `registry.yml`'s two trigger filters, and lost it when those
//! filters were removed. `--relevant-to <filter>` restores the
//! scoping from the shared filter file instead of a duplicated list:
//! only newer commits matching that filter's entries in
//! `.github/path-filters.yml` supersede this run. A newer commit
//! matching none of them cannot change what this run deploys or
//! publishes, so skipping for it would strand this run's work with no
//! later run redoing it.
//!
//! Failure direction: when the filter file defies the strict line
//! grammar (or lacks the named list), the guard warns on stderr and
//! falls back to the unscoped range - the pre-scoping semantics.
//! Unscoped never ships stale work over newer work; it only re-opens
//! the narrower stranding window above, which then needs the race and
//! the divergence at once. Failing the step instead would hold every
//! deploy hostage to a parser bug no rerun can clear.
//!
//! The entries reach git as `:(glob)` pathspecs, whose `**` semantics
//! match the dorny/paths-filter matcher on the shapes the lists use
//! (directory prefixes and exact filenames). `rev-list`'s default
//! history simplification under a pathspec prunes only commits whose
//! filtered changes did not survive to `origin/main`'s tip - when the
//! filtered content at the tip differs from `$GITHUB_SHA`'s, the walk
//! necessarily reports a commit that changed it - so a relevant newer
//! change cannot be hidden, the pre-squash-merge history a re-run of
//! an old run would walk included.
//!
//! Inherited properties of the port, each pinned by running the
//! original under `bash -e`, GitHub's default `run:` shell (`-e` on,
//! `-u` and `-o pipefail` off):
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

/// The shared filter file, relative to the repository root the
/// workflows run the alias from.
const FILTER_FILE: &str = ".github/path-filters.yml";

/// Answer whether `origin/main` carries a commit after `$GITHUB_SHA` -
/// scoped, when a filter name is given, to commits matching that
/// filter's paths - recording `superseded=true` in `$GITHUB_OUTPUT`
/// when it does.
///
/// # Errors
///
/// When `git fetch` fails (L1), or when `$GITHUB_OUTPUT` is unusable in
/// the positive case (L9).
pub fn run(relevant_to: Option<&str>) -> Result<()> {
    fetch_origin_main()?;

    let sha = std::env::var("GITHUB_SHA").unwrap_or_default();
    let pathspecs = relevant_to.map_or_else(Vec::new, filter_pathspecs);
    if newer_commit(&range(&sha), &pathspecs)?.is_empty() {
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

/// The named filter's entries as `:(glob)` pathspecs, or the empty
/// (unscoped) list with a warning: the module docs' failure direction.
fn filter_pathspecs(name: &str) -> Vec<String> {
    let Some(entries) = std::fs::read_to_string(FILTER_FILE)
        .ok()
        .and_then(|text| filter_entries(&text, name))
    else {
        eprintln!(
            "warning: no usable `{name}` list in {FILTER_FILE}; \
             superseding on any newer commit"
        );
        return Vec::new();
    };
    entries
        .iter()
        .map(|entry| format!(":(glob){entry}"))
        .collect()
}

/// The `name:` section's `- "entry"` lines. `None` - never a guess -
/// on anything the grammar does not cover, a duplicate section header
/// included: the caller's fallback is unscoped supersession, where a
/// silently dropped, merged, or empty entry would instead widen or
/// erase the scope invisibly.
fn filter_entries(text: &str, name: &str) -> Option<Vec<String>> {
    let mut entries = Vec::new();
    let mut sections = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(section) = line.strip_suffix(':').filter(|_| !line.starts_with(' ')) {
            if sections.contains(&section) {
                return None;
            }
            sections.push(section);
            in_section = section == name;
        } else {
            let entry = line
                .strip_prefix("  - \"")
                .and_then(|rest| rest.strip_suffix('"'))
                .filter(|entry| !entry.is_empty())?;
            if in_section {
                entries.push(entry.to_owned());
            }
        }
    }
    (!entries.is_empty()).then_some(entries)
}

/// L2. A non-zero `rev-list` yields the empty capture the shell's
/// condition context yielded, not an error. Only a failure to *spawn*
/// git surfaces - unreachable in practice, since L1 just ran it.
fn newer_commit(range: &str, pathspecs: &[String]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command.args(["rev-list", "-n", "1", range]);
    if !pathspecs.is_empty() {
        command.arg("--").args(pathspecs);
    }
    let output = command
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
    fn filter_entries_reads_only_the_named_section() {
        let text = "# comment\n\nregistry:\n  - \"registry/**\"\n  - \"Cargo.toml\"\nports:\n  - \"ports/**\"\n";
        assert_eq!(
            filter_entries(text, "ports").unwrap(),
            vec!["ports/**".to_owned()]
        );
        assert_eq!(
            filter_entries(text, "registry").unwrap(),
            vec!["registry/**".to_owned(), "Cargo.toml".to_owned()]
        );
    }

    #[test]
    fn anything_outside_the_strict_grammar_reads_as_no_list() {
        for text in [
            "registry:\n  - unquoted/**\n",
            "registry:\n  - \"\"\n",
            "registry:\n- \"registry/**\"\n",
            "ports:\n  - \"ports/**\"\n",
            "registry:\n",
            "registry:\n  - \"a/**\"\nregistry:\n  - \"b/**\"\n",
            "ports:\n  - \"p/**\"\nports:\n  - \"q/**\"\nregistry:\n  - \"r/**\"\n",
        ] {
            assert_eq!(filter_entries(text, "registry"), None, "{text:?}");
        }
    }
}
