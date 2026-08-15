//! Remote publication through the existing `cabin publish` flow.
//!
//! The whole preflighted batch uploads through ONE
//! `cabin -Z remote-registry publish` invocation carrying a
//! `--manifest-path` per scratch package, in publication order (the
//! client publishes the batch in exactly the order the flags are
//! given), so staging, scoped-name gates, credential lookup - the
//! trusted-publishing exchange included, which mints exactly one
//! token per invocation - the registry's `config.json` API discovery,
//! publish lints, and the framed uploads all run exactly the code
//! path an ordinary publish runs.  This tool adds only the ordering.
//!
//! Versions are never skipped based on the public index: pending
//! (not yet verified) versions are invisible there, so the only
//! correct dedupe is the registry's own byte-identical idempotency -
//! `cabin publish` reports a no-op for identical bytes.  Divergent
//! bytes do not fail here: every upload passes `--new-revision`, so
//! they land as a new packaging revision of the same immutable
//! version.
//!
//! Rate-limit pacing lives in the client since the batch became one
//! invocation: `cabin publish` waits out the registry's `429` answers
//! per upload on the typed error.  The stderr-parsing retry loop this
//! module used to carry is gone with the subprocess-per-package shape
//! that forced it - the typed in-process signal cannot be spoofed by
//! server-controlled detail text, which is what the anchored-line
//! parsing existed to defend against.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Upload every conversion in one `cabin publish` invocation, in
/// publication order.
///
/// Every upload passes `--new-revision`: the committed ports are
/// the source of truth, so a change that reaches this tool is
/// the deliberate intent to respin the published version - identical
/// bytes still no-op through the registry's idempotency, and changed
/// bytes become a new packaging revision of the same upstream
/// version.
///
/// # Errors
/// Returns an error when the `cabin publish` invocation cannot start
/// or exits non-zero.
pub fn publish_all(package_dirs: &[&Path], index_url: &str, cabin: &Path) -> Result<()> {
    let mut cmd = Command::new(cabin);
    cmd.arg("-Z")
        .arg("remote-registry")
        .arg("publish")
        .arg("--new-revision")
        // Reruns after a partial publish hit the same drained publish
        // bucket, and every attempt charges it - so even a one-port
        // tree waits out 429s instead of failing the run.
        .arg("--retry-rate-limits");
    for package_dir in package_dirs {
        cmd.arg("--manifest-path")
            .arg(package_dir.join("cabin.toml"));
    }
    cmd.arg("--index-url").arg(index_url);
    // Inherited stdio on purpose: the client's progress, rate-limit
    // pacing warnings, and - under GitHub Actions - its `::add-mask::`
    // workflow commands must reach the real output streams unaltered.
    let status = cmd
        .status()
        .with_context(|| format!("running {} publish", cabin.display()))?;
    if !status.success() {
        bail!("publishing the port batch failed ({status})");
    }
    Ok(())
}
