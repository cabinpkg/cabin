//! Operator commands against the hosted registry Worker
//! (`registry/`), run by hand from the repository root.
//!
//! Crate boundaries: these hold the operator's own credentials and talk
//! to the live service, which is what separates them from the static
//! guards in `xtask-registry-guard` - those read committed sources,
//! take no credentials and mutate nothing.
//!
//! Every command shells out to the pinned `wrangler` through
//! [`wrangler`]: the CLI is the only supported path to D1 and R2 from
//! outside the Worker.

pub mod backfill;
pub mod diagnose;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// The pinned wrangler.  `registry/scripts/lib.sh` and
/// `.github/workflows/registry.yml` (`wranglerVersion`) pin it
/// independently; all three move together.
pub const WRANGLER: &str = "wrangler@4.112.0";

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

/// `wrangler`, pinned, run from `registry/`.  Held apart from the
/// argument lists so no command can reach an unpinned CLI.
pub fn wrangler(arguments: &[&str]) -> Command {
    let mut command = Command::new("npx");
    command
        .arg("--yes")
        .arg(WRANGLER)
        .args(arguments)
        // The shell piped only stdout; wrangler's diagnostics went to
        // the operator's terminal, and a caller that wants them quiet
        // says so itself.
        .stderr(Stdio::inherit())
        // `Command::output` would give the child no stdin at all,
        // where every one of these ran attached to the operator's
        // terminal.
        .stdin(Stdio::inherit())
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

pub fn step(message: &str) {
    println!("==> {message}");
}

/// A D1 value as the shell's `node` printed it: a JSON string keeps its
/// own text, an array joins its elements with `,` (which is how D1
/// hands back a BLOB column), anything else takes its JSON form, so
/// `null` prints as `null`.
///
/// Ceiling: a JSON object would print as JSON where JavaScript printed
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
/// If the row carries neither pair. The shell printed
/// `undefined: undefined` for such a row and carried on; a row that is
/// not the shape the query asked for means the answer is not the one
/// asked for, and this crate reports that rather than showing the
/// operator a line they cannot act on.
pub fn key_value(row: &serde_json::Map<String, serde_json::Value>) -> Result<(String, String)> {
    let (Some(key), Some(value)) = (row.get("key"), row.get("value")) else {
        bail!("a key/value row carries no `key` and `value` pair");
    };
    Ok((display(key), display(value)))
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
