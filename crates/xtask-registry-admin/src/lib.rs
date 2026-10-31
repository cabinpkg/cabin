//! Operator commands against the hosted registry Worker
//! (`registry/`), run by hand from the repository root - or, for
//! [`verify`], by the `registry-verify` workflow, and for
//! [`confirm_deploy`], by the registry workflow's deploy job.
//!
//! Crate boundaries: these hold the operator's own credentials and talk
//! to the live service, which is what separates them from the static
//! guards in `xtask-registry-guard` - those read committed sources,
//! take no credentials and mutate nothing.
//!
//! The disclosure rule the commands keep (`docs/architecture.md`,
//! "`xtask-registry-admin`") is written for an operator reading their
//! own terminal, and [`verify`] and [`confirm_deploy`] are the commands
//! whose audience is neither an operator nor a private one: they run
//! unattended and their output is a public CI log.  `verify` prints
//! package names and versions, which the admin API already discloses to
//! any verify-scope holder, plus the verdicts and reason codes its own
//! verifier run computes - and never the token, the corpus, or archive
//! bytes; `confirm_deploy` prints Worker version ids and the run's tag.
//!
//! D1 and the R2 object commands go through the pinned `wrangler`
//! ([`wrangler`]), the only supported path to them from outside the
//! Worker.  Listing a bucket is the exception: wrangler exposes no
//! command for it, and no bulk mode for deleting one - so the audit, the
//! governor's evidence checks and the wipe's sweep call Cloudflare's R2
//! REST API directly with the operator's `CLOUDFLARE_API_TOKEN`.

pub mod audit;
pub mod backfill;
pub mod confirm_deploy;
pub mod diagnose;
pub mod governor;
pub mod launch_guard;
pub mod migrate;
pub mod restore_drill;
pub mod verify;
pub mod wipe;

#[cfg(test)]
mod r2_mock;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// The BACKUP bucket, which more than one command reaches: the
/// backfill writes to it, the audit, the governor's evidence checks and
/// the restore drill read it, and the pre-launch wipe empties it.
/// Outside that wipe its `blobs/` namespace is append-only
/// (`registry/docs/runbook.md`, "Disaster recovery"), so nothing else
/// here deletes from it.
pub const BACKUP_BUCKET: &str = "cabin-registry-backup";

/// The PRIMARY blob bucket, which more than one command reaches: the
/// governor's evidence checks list it, the wipe sweeps it, and the
/// backfill copies out of it.  The deploy guard
/// (`xtask-registry-guard`) keeps its own literal on purpose - it
/// asserts `wrangler.jsonc` declares this binding, and a guard
/// importing the value it checks would check it against itself.
pub const BLOBS_BUCKET: &str = "cabin-registry-blobs";

/// The repository this tool was built from.
///
/// Resolved from the crate's own manifest directory rather than the
/// working directory: the Cargo aliases are run from the repository
/// root, but nothing here should depend on that.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `registry/` directory the commands operate on.  Every wrangler
/// invocation runs here, because `wrangler.jsonc` is what binds D1 and
/// R2 to the commands' arguments.
#[must_use]
pub fn registry_dir() -> PathBuf {
    repo_root().join("registry")
}

/// The registry root a command reads, writes and runs wrangler in.
/// `CABIN_REGISTRY_DIR` overrides [`registry_dir`], which is how tests
/// point a command at a synthetic registry root.
///
/// The launch guard needs it as much as its callers do: a guard
/// resolving its `wrangler.jsonc` or its wrangler working directory any
/// other way reads a different tree than the wipe it is guarding.
#[must_use]
pub fn registry_root() -> PathBuf {
    std::env::var_os(cabin_env::CABIN_REGISTRY_DIR)
        .filter(|value| !value.is_empty())
        .map_or_else(registry_dir, PathBuf::from)
}

/// The Cloudflare account the registry is deployed to, read from
/// `wrangler.jsonc` for the REST calls wrangler exposes no command
/// for.
///
/// # Errors
///
/// If `wrangler.jsonc` cannot be read, or declares no such id.
pub fn account_id() -> Result<String> {
    let path = registry_dir().join("wrangler.jsonc");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    declared_account_id(&text).context("CF_ACCOUNT_ID not found in wrangler.jsonc")
}

/// The account id a wrangler config declares: the first
/// `"CF_ACCOUNT_ID":` followed by 32 lower-case hex digits in quotes.
/// A config that binds the id some other way (an expression, a separate
/// vars file) is not one this can read, and saying so beats calling the
/// API with a guess.
#[must_use]
pub fn declared_account_id(text: &str) -> Option<String> {
    declared(text, "CF_ACCOUNT_ID", 32, |byte| {
        byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
    })
}

/// The D1 database id a wrangler config binds: the first
/// `"database_id":` followed by 36 characters of lower-case hex or `-`
/// in quotes.
///
/// Ceiling: this checks the alphabet and the width, never the
/// 8-4-4-4-12 shape, so 36 hyphens satisfy it - and it reads a
/// commented-out binding as live, because a `//` line is still text.
/// The guard compares the answer against the account's own id, and two
/// ids that agree are what it needs; a malformed one agrees with
/// nothing.
#[must_use]
pub fn declared_database_id(text: &str) -> Option<String> {
    declared(text, "database_id", 36, |byte| {
        byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-'
    })
}

/// The first `"<key>":` in `text` followed by exactly `width`
/// characters of `alphabet` in quotes, as an unanchored regex found
/// it: a candidate that does not fit the shape is skipped, not fatal,
/// so a later occurrence still wins.
fn declared(text: &str, key: &str, width: usize, alphabet: fn(u8) -> bool) -> Option<String> {
    let needle = format!("\"{key}\":");
    text.match_indices(&needle).find_map(|(index, matched)| {
        let rest = text[index + matched.len()..]
            .trim_start_matches(js_whitespace)
            .strip_prefix('"')?;
        let value = rest.get(..width)?;
        (rest.get(width..width + 1) == Some("\"") && value.bytes().all(alphabet))
            .then(|| value.to_owned())
    })
}

/// JavaScript's `\s`, which `str::trim_start` is not: `char::
/// is_whitespace` trims U+0085 where the regex does not, and the regex
/// consumes U+FEFF where `is_whitespace` does not.
fn js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `wrangler` as `npm ci` installed it from `registry/package-lock.json`,
/// run from `registry/`.  The entry point is named by path, not resolved
/// through `npx`, which falls back to any other wrangler on the machine
/// when that install is missing; so no command can reach an unpinned CLI.
pub fn wrangler(arguments: &[&str]) -> Command {
    let mut command = Command::new("node");
    command
        .arg(registry_dir().join("node_modules/wrangler/bin/wrangler.js"))
        .args(arguments)
        // Wrangler's diagnostics go to the operator's terminal; a
        // caller that wants them quiet says so itself.
        .stderr(Stdio::inherit())
        // `Command::output` would give the child no stdin at all;
        // wrangler runs attached to the operator's terminal.
        .stdin(Stdio::inherit())
        // A registry credential the operator's shell may export for
        // the governor commands; no wrangler invocation needs it.
        .env_remove(cabin_env::CABIN_REGISTRY_TOKEN)
        .current_dir(registry_dir());
    command
}

/// The captured stdout of `command`, or an error naming it.
///
/// # Errors
///
/// If the program cannot be spawned, or exits non-zero.
pub fn output(command: &mut Command) -> Result<String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.output().with_context(|| format!("run {program}"))?;
    if !output.status.success() {
        bail!("{program} failed: {}", output.status);
    }
    String::from_utf8(output.stdout).with_context(|| format!("{program} wrote invalid UTF-8"))
}

/// [`output`]'s uncaptured twin: the exit status alone is read, and
/// the command's configured stdio is left as the caller set it -
/// inherited to the operator's terminal or nulled, but never captured.
///
/// # Errors
///
/// If the program cannot be spawned, or exits non-zero.
pub fn status(command: &mut Command) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command.status().with_context(|| format!("run {program}"))?;
    if !status.success() {
        bail!("{program} failed: {status}");
    }
    Ok(())
}

/// One step banner: `==> <label>` on stdout.
pub fn step(message: &str) {
    println!("==> {message}");
}

/// A D1 value as JavaScript's `String()` renders it: a JSON string keeps
/// its own text, an array joins its elements with `,` (which is how D1
/// hands back a BLOB column), anything else takes its JSON form, so
/// `null` prints as `null`.
///
/// Ceiling: a JSON object prints as JSON where JavaScript prints
/// `[object Object]`. D1 returns no column that way.
#[must_use]
pub fn display(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            // `Array.prototype.join` renders null as empty, not "null".
            .map(|item| match item {
                serde_json::Value::Null => String::new(),
                other => display(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
    }
}

/// The `key` and `value` of one row of a key/value read, rendered as
/// the bundle prints them.
///
/// # Errors
///
/// If the row lacks `key` or `value`: a row that is not the shape the
/// query asked for means the answer is not the one asked for, and this
/// crate reports that rather than showing the operator a line they
/// cannot act on.
pub fn key_value(row: &serde_json::Map<String, serde_json::Value>) -> Result<(String, String)> {
    let (Some(key), Some(value)) = (row.get("key"), row.get("value")) else {
        bail!("a key/value row carries no `key` and `value` pair");
    };
    Ok((display(key), display(value)))
}

/// What [`column_text`] prints for a column that is JSON `null` or
/// absent.
#[derive(Clone, Copy)]
pub enum Nullish {
    /// `null` and a missing key print themselves, as `null` and
    /// `undefined`.
    Printed,
    /// Both become a blank line, and so does an empty string - the
    /// three are indistinguishable downstream, which is what lets a
    /// NULL in a `||` concatenation take the same branch as no rows at
    /// all.
    Empty,
}

/// Each row's rendered value plus a newline; a value's own newlines
/// stay, so one row can span several lines.
#[must_use]
pub fn column_text(
    rows: &[serde_json::Map<String, serde_json::Value>],
    column: &str,
    nullish: Nullish,
) -> String {
    let mut text = String::new();
    for row in rows {
        match (row.get(column), nullish) {
            (Some(serde_json::Value::String(value)), _) => text.push_str(value),
            (None | Some(serde_json::Value::Null), Nullish::Empty) => {}
            (Some(other), _) => text.push_str(&other.to_string()),
            (None, Nullish::Printed) => text.push_str("undefined"),
        }
        text.push('\n');
    }
    text
}

/// [`column_text`] as lines, trailing newlines stripped.
#[must_use]
pub fn column_lines(
    rows: &[serde_json::Map<String, serde_json::Value>],
    column: &str,
    nullish: Nullish,
) -> Vec<String> {
    let mut text = column_text(rows, column, nullish);
    // NULs are dropped, so `sha256:<32 hex>\0<32 hex>` reaches the caller as
    // 64 hex digits and passes the backfill's grammar; `column_text`
    // keeps them.
    text.retain(|character| character != '\0');
    // An empty enumeration still yields a single blank line, and a
    // value carrying a newline becomes two lines.
    text.trim_end_matches('\n')
        .split('\n')
        .map(str::to_owned)
        .collect()
}

/// The rows of a `wrangler d1 execute --json` response.
///
/// # Errors
///
/// If the output is not the array-of-results shape wrangler documents;
/// the callers treat that as a failure, never as an empty result.
pub fn results(json: &str) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    let parsed: serde_json::Value = serde_json::from_str(json).context("parse wrangler output")?;
    let rows = parsed
        .get(0)
        .and_then(|first| first.get("results"))
        .and_then(serde_json::Value::as_array)
        .context("wrangler output has no results array")?;
    rows.iter()
        .map(|row| {
            row.as_object()
                .cloned()
                .context("a result row is not an object")
        })
        .collect()
}
