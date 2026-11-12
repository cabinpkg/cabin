//! Operator commands against the hosted registry Worker
//! (`registry/`), run by hand from the repository root - or, for
//! [`verify`], by the `registry-verify` workflow.
//!
//! Crate boundaries: these hold the operator's own credentials and talk
//! to the live service, which is what separates them from the static
//! guards in `xtask-registry-guard` - those read committed sources,
//! take no credentials and mutate nothing.
//!
//! The disclosure rule the commands keep (`registry/docs/runbook.md`,
//! "Logs") is written for an operator reading their
//! own terminal, and [`verify`] is the command whose audience is
//! neither an operator nor a private one: it runs unattended and its
//! output is a public CI log.  It prints
//! package names and versions, which the admin API already discloses to
//! any verify-scope holder, plus the verdicts and reason codes its own
//! verifier run computes - and never the token, the corpus, or archive
//! bytes.
//!
//! D1 and the R2 object commands go through the pinned `wrangler`
//! ([`wrangler`]), the only supported path to them from outside the
//! Worker.  Listing a bucket is the exception: wrangler exposes no
//! command for it, and no bulk mode for deleting one - so the wipe's
//! sweep calls Cloudflare's R2 REST API directly with the operator's
//! `CLOUDFLARE_API_TOKEN`.

pub mod diagnose;
pub mod governor;
pub mod launch_guard;
pub mod migrate;
pub mod verify;
pub mod wipe;

mod r2;
#[cfg(test)]
mod r2_mock;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// The registry's buckets, one per role - archives, uploads, download
/// logs, index - which the wipe empties.  The deploy guard
/// (`xtask-registry-guard`) keeps its own literals on purpose - it
/// asserts `wrangler.jsonc` declares these bindings, and a guard
/// importing the values it checks would check them against themselves.
pub const BUCKETS: [&str; 4] = [
    "cabin-registry-archives",
    "cabin-registry-uploads",
    "cabin-registry-download-logs",
    "cabin-registry-index",
];

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

/// The agent behind this crate's R2 and governor reads.  No redirects:
/// a redirect is the answer, never a step on the way to one - and a
/// redirected read is the one way a command could inspect a bucket or a
/// registry other than the one it names.  `ureq` returns the 3xx as
/// `Ok` with this set: the R2 listing then fails to parse and the
/// governor read fails its status check, each with the body the
/// redirect carried.
pub(crate) fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().redirects(0).build()
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
        // the governor command; no wrangler invocation needs it.
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

/// Each row's rendered value as lines, trailing newlines stripped; a
/// value's own newlines stay, so one row can span several lines.  A
/// JSON `null` and a missing key print themselves, as `null` and
/// `undefined`.
#[must_use]
pub fn column_lines(
    rows: &[serde_json::Map<String, serde_json::Value>],
    column: &str,
) -> Vec<String> {
    let mut text = String::new();
    for row in rows {
        match row.get(column) {
            Some(serde_json::Value::String(value)) => text.push_str(value),
            Some(other) => text.push_str(&other.to_string()),
            None => text.push_str("undefined"),
        }
        text.push('\n');
    }
    // NULs are dropped, so `sha256:<32 hex>\0<32 hex>` reaches the caller as
    // 64 hex digits.
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
