//! Thin shim over [`cabin_registry_verify::inspect`] and the name
//! advisories for the GitHub Actions workflow.
//!
//! ```text
//! cabin-registry-verify <archive.zip> <listing-entry.json> [--upstream <upstream-archive>]
//! cabin-registry-verify --name-advisories <listing-entry.json> <corpus.json>
//! ```
//!
//! `listing-entry.json` is one element of the admin listing's
//! `versions` array.  Limits come from the `VERIFY_*` environment
//! variables (empty means unset).  The inspect form prints one JSON
//! object to stdout - `{"verdict":"verified"}` or
//! `{"verdict":"rejected","reasons":[...]}` - and exits 0 for
//! either verdict.  The advisory form takes the corpus response
//! (`GET /api/v1/admin/packages`) and prints
//! `{"advice":"proceed"}` or
//! `{"advice":"abstain","findings":[...]}`; the workflow runs it
//! **before** downloading the archive and, on abstain, renders no
//! verdict at all.  Exit 2 is an operational failure with no
//! verdict: the caller must leave the version pending.

use std::path::PathBuf;
use std::process::ExitCode;

use cabin_registry_verify::{PendingVersion, Verdict, inspect, limits_from_env, names};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cabin-registry-verify: {message}");
            ExitCode::from(2)
        }
    }
}

const USAGE: &str = "usage: cabin-registry-verify <archive.zip> <listing-entry.json> \
     [--upstream <upstream-archive>] | \
     cabin-registry-verify --name-advisories <listing-entry.json> <corpus.json>";

fn run() -> Result<(), String> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let Some(([first, second], rest)) = args.split_first_chunk() else {
        return Err(USAGE.into());
    };
    if first == "--name-advisories" {
        let [corpus] = rest else {
            return Err(USAGE.into());
        };
        return advise(&PathBuf::from(second), &PathBuf::from(corpus));
    }
    let upstream = match rest {
        [] => None,
        [flag, path] if flag == "--upstream" => Some(PathBuf::from(path)),
        _ => return Err(USAGE.into()),
    };
    let archive = PathBuf::from(first);
    let entry = PathBuf::from(second);

    // Lossy conversion feeds a non-Unicode value into the integer
    // parse, which reports it as invalid instead of silently using
    // the default.
    let limits = limits_from_env(|name| {
        std::env::var_os(name).map(|value| value.to_string_lossy().into_owned())
    })
    .map_err(|err| err.to_string())?;

    let entry_bytes = std::fs::read(&entry)
        .map_err(|err| format!("failed to read {}: {err}", entry.display()))?;
    let pending: PendingVersion = serde_json::from_slice(&entry_bytes)
        .map_err(|err| format!("failed to parse {}: {err}", entry.display()))?;

    let verdict =
        inspect(&archive, &pending, &limits, upstream.as_deref()).map_err(|err| err.to_string())?;
    let rendered = match verdict {
        Verdict::Verified => serde_json::json!({ "verdict": "verified" }),
        Verdict::Rejected(reasons) => serde_json::json!({
            "verdict": "rejected",
            "reasons": reasons.iter().map(ToString::to_string).collect::<Vec<_>>(),
        }),
    };
    println!("{rendered}");
    Ok(())
}

/// The advisory mode: needs the listing entry and the corpus only,
/// never the archive bytes - malformed inputs are operational
/// failures (exit 2, the version stays pending), never verdicts.
fn advise(entry: &PathBuf, corpus: &PathBuf) -> Result<(), String> {
    let read = |path: &PathBuf| {
        std::fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))
    };
    let pending: PendingVersion = serde_json::from_slice(&read(entry)?)
        .map_err(|err| format!("failed to parse {}: {err}", entry.display()))?;
    let corpus_value: names::Corpus = serde_json::from_slice(&read(corpus)?)
        .map_err(|err| format!("failed to parse {}: {err}", corpus.display()))?;
    let Some((scope, name)) = pending.name.split_once('/') else {
        return Err(format!(
            "listing entry name {:?} is not a canonical <scope>/<name>",
            pending.name
        ));
    };

    let findings = names::advise(scope, name, &corpus_value);
    let rendered = if findings.is_empty() {
        serde_json::json!({ "advice": "proceed" })
    } else {
        serde_json::json!({
            "advice": "abstain",
            "findings": findings.iter().map(ToString::to_string).collect::<Vec<_>>(),
        })
    };
    println!("{rendered}");
    Ok(())
}
