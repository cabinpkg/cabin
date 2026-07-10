//! Thin shim over [`cabin_registry_verify::inspect`] for the
//! GitHub Actions workflow.
//!
//! ```text
//! cabin-registry-verify <archive.tar.gz> <listing-entry.json>
//! ```
//!
//! `listing-entry.json` is one element of the admin listing's
//! `versions` array.  Limits come from the `VERIFY_*` environment
//! variables (empty means unset).  Prints one JSON object to
//! stdout - `{"verdict":"verified"}` or
//! `{"verdict":"rejected","reasons":[...]}` - and exits 0 for
//! either verdict.  Exit 2 is an operational failure with no
//! verdict: the caller must leave the version pending.

use std::path::PathBuf;
use std::process::ExitCode;

use cabin_registry_verify::{PendingVersion, Verdict, inspect, limits_from_env};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cabin-registry-verify: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let (Some(archive), Some(entry), None) = (args.next(), args.next(), args.next()) else {
        return Err("usage: cabin-registry-verify <archive.tar.gz> <listing-entry.json>".into());
    };
    let archive = PathBuf::from(archive);
    let entry = PathBuf::from(entry);

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

    let verdict = inspect(&archive, &pending, &limits).map_err(|err| err.to_string())?;
    let rendered = match verdict {
        Verdict::Verified => serde_json::json!({ "verdict": "verified" }),
        Verdict::Rejected(reasons) => serde_json::json!({
            "verdict": "rejected",
            "reasons": reasons.iter().map(|reason| reason.code()).collect::<Vec<_>>(),
        }),
    };
    println!("{rendered}");
    Ok(())
}
