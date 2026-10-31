//! The freshness guard: the "Skip when superseded by a newer main
//! commit" step of `.github/workflows/registry.yml` and
//! `.github/workflows/ports-publish.yml`.
//!
//! `--relevant-to <filter>` scopes the check through the shared filter
//! file instead of a duplicated list: only newer commits matching that
//! filter's entries in `.github/path-filters.yml` supersede this run.
//! A newer commit matching none of them cannot change what this run
//! deploys or publishes, so skipping for it would strand this run's
//! work with no later run redoing it.
//!
//! The entries reach git as `:(glob)` pathspecs, whose `**` semantics
//! match the dorny/paths-filter matcher on the shapes the lists use
//! (directory prefixes and exact filenames). `--full-history` walks
//! every parent of a merge, so history simplification cannot hide a
//! newer commit that changed those paths on either side of one.
//!
//! Failure direction: **every failure fails the step**, so the deploy
//! or publish it gates never runs on an unanswered question. A failed
//! fetch, a `$GITHUB_SHA` that is unset or not a full lowercase commit
//! id, a `rev-list` that exits non-zero (an unknown commit, a shallow
//! clone not containing it), and a named list that defies the strict
//! line grammar or is missing all exit 1 with the reason and leave
//! `$GITHUB_OUTPUT` untouched. Neither guessing direction is safe:
//! "not superseded" can ship an older Worker over a newer one, and an
//! unscoped "superseded" can skip a run whose newer commit's own run
//! never deploys (its change gate found nothing relevant), leaving a
//! stale Worker behind a green job. A red step is the affordance: a
//! transient failure clears on a rerun, and a broken list clears with
//! the commit that fixes it, whose own run deploys.
//!
//! Nothing reaches stdout in either answer; the negative case writes
//! nothing anywhere. Stated ceiling: git's own exit status (128 for an
//! unreachable remote) collapses to 1.

use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

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
/// When the answer cannot be established (module docs, "Failure
/// direction"), or when `$GITHUB_OUTPUT` is unusable in the positive
/// case.
pub fn run(relevant_to: Option<&str>) -> Result<()> {
    fetch_origin_main()?;

    let sha = std::env::var("GITHUB_SHA").unwrap_or_default();
    let pathspecs = match relevant_to {
        Some(name) => filter_pathspecs(name)?,
        None => Vec::new(),
    };
    if superseded(&sha, &pathspecs)? {
        record_superseded()?;
    }
    Ok(())
}

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

/// The named filter's entries as `:(glob)` pathspecs.
fn filter_pathspecs(name: &str) -> Result<Vec<String>> {
    let text =
        std::fs::read_to_string(FILTER_FILE).with_context(|| format!("reading {FILTER_FILE}"))?;
    let entries = filter_entries(&text, name).with_context(|| {
        format!("no usable `{name}` list in {FILTER_FILE}; cannot scope the freshness check")
    })?;
    Ok(entries
        .iter()
        .map(|entry| format!(":(glob){entry}"))
        .collect())
}

/// The `name:` section's `- "entry"` lines. `None` - never a guess -
/// on anything inside that section the grammar does not cover, or on a
/// duplicate section header: a silently dropped, merged, or empty
/// entry would widen or erase the scope invisibly, and so would one
/// carrying a quote or backslash, which YAML reads as a trailing
/// comment or an escape this parser would keep verbatim. Other
/// sections are dorny/paths-filter's alone, so their lines are skipped
/// unread; a header the grammar misses (`ports: # note`) reads as a
/// line of the section before it, which fails here when that section
/// is `name`'s.
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
        } else if in_section {
            let entry = line
                .strip_prefix("  - \"")
                .and_then(|rest| rest.strip_suffix('"'))
                .filter(|entry| !entry.is_empty() && !entry.contains(['"', '\\']))?;
            entries.push(entry.to_owned());
        }
    }
    (!entries.is_empty()).then_some(entries)
}

/// Whether `origin/main` carries a commit after `sha` that touches
/// `pathspecs` (any commit, when empty).
fn superseded(sha: &str, pathspecs: &[String]) -> Result<bool> {
    if !is_commit_id(sha) {
        bail!(
            "GITHUB_SHA `{sha}` is not a full commit id; cannot tell whether this run is superseded"
        );
    }
    let mut command = Command::new("git");
    command.args([
        "rev-list",
        "--full-history",
        "-n",
        "1",
        &format!("{sha}..origin/main"),
    ]);
    if !pathspecs.is_empty() {
        command.arg("--").args(pathspecs);
    }
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .context("spawning `git rev-list`")?;
    if !output.status.success() {
        bail!(
            "`git rev-list {sha}..origin/main` failed ({}); cannot tell whether this run is superseded",
            output.status
        );
    }
    Ok(!crate::substitute(output.stdout).is_empty())
}

/// A full SHA-1 or SHA-256 object name in git's lowercase hex, which
/// also keeps the `rev-list` range from ever reading as an option.
fn is_commit_id(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64)
        && sha
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Reachable only in the positive case, so an unset `GITHUB_OUTPUT`
/// fails the step there and nowhere else.
fn record_superseded() -> Result<()> {
    crate::append_github_output(OUTPUT_LINE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_full_lowercase_object_name_is_a_commit_id() {
        assert!(is_commit_id(&"a".repeat(40)));
        assert!(is_commit_id(&"0".repeat(64)));
        for sha in [
            "",
            "deadbeef",
            &"A".repeat(40),
            &"g".repeat(40),
            &"a".repeat(41),
            "--all",
            "HEAD",
        ] {
            assert!(!is_commit_id(sha), "{sha:?}");
        }
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
            "registry:\n  - \"r/**\"\nports: # note\n  - \"p/**\"\n",
            "registry:\n  - \"registry/**\" # \"worker sources\"\n",
            "registry:\n  - \"\\x72egistry/**\"\n",
        ] {
            assert_eq!(filter_entries(text, "registry"), None, "{text:?}");
        }
    }

    #[test]
    fn other_sections_are_not_held_to_the_grammar() {
        let text = "docs:\n  - 'docs/**'\n  - \"site/**\" # trailing\nports: # note\n  - p/**\nregistry:\n  - \"r/**\"\n";
        assert_eq!(
            filter_entries(text, "registry").unwrap(),
            vec!["r/**".to_owned()]
        );
    }
}
