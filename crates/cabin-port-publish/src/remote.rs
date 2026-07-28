//! Remote publication through the existing `cabin publish` flow.
//!
//! Each preflighted scratch package uploads via
//! `cabin -Z remote-registry publish --manifest-path ... --index-url ...`,
//! so staging, scoped-name gates, credential lookup, the registry's
//! `config.json` API discovery, publish lints, and the framed upload
//! all run exactly the code path an ordinary publish runs - this
//! tool adds only the ordering.
//!
//! Versions are never skipped based on the public index: pending
//! (not yet verified) versions are invisible there, so the only
//! correct dedupe is the registry's own byte-identical idempotency -
//! `cabin publish` reports a no-op for identical bytes and fails on
//! divergent ones (published versions are immutable).

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::plan::PortConversion;

/// Upload every conversion, in publication order.
///
/// # Errors
/// Returns an error when a `cabin publish` invocation cannot start
/// or exits non-zero.  A divergent-bytes conflict for an existing
/// version surfaces here with guidance to bump the packaging
/// revision instead of editing the published version.
pub fn publish_all(
    conversions: &[PortConversion],
    package_dirs: &[&Path],
    index_url: &str,
    cabin: &Path,
) -> Result<()> {
    for (conversion, package_dir) in conversions.iter().zip(package_dirs) {
        let status = Command::new(cabin)
            .arg("-Z")
            .arg("remote-registry")
            .arg("publish")
            .arg("--manifest-path")
            .arg(package_dir.join("cabin.toml"))
            .arg("--index-url")
            .arg(index_url)
            .status()
            .with_context(|| format!("running {} publish", cabin.display()))?;
        if !status.success() {
            bail!(
                "publishing {} {} failed ({status}); if the version already exists with \
                 different bytes, bump the recipe's `{}` sidecar instead of editing the \
                 published version",
                conversion.scoped_name.as_str(),
                conversion.published_version,
                crate::plan::REVISION_FILENAME
            );
        }
    }
    Ok(())
}
