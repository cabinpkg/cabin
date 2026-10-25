//! Remote publication through the existing `cabin publish` flow.
//!
//! The preflighted packages upload in consecutive batches of
//! `BATCH_SIZE`, each through ONE `cabin -Z remote-registry publish`
//! invocation carrying a `--manifest-path` per scratch package, in
//! publication order (the client publishes a batch in exactly the order
//! the flags are given, and the batches run in order), so staging,
//! scoped-name gates, credential lookup, the registry's `config.json`
//! API discovery, publish lints, and the framed uploads all run exactly
//! the code path an ordinary publish runs.  This tool adds only the
//! ordering and the batching.
//!
//! Batching keeps a large set from riding one credential: under GitHub
//! Actions the client exchanges the run's OIDC token for a
//! trusted-publishing token that expires 30 minutes later, once per
//! invocation, and revokes it best-effort when the invocation exits.  A
//! batch that fails - an upload refused, a token rejected or expired by
//! an unusually slow batch - stops the run before the next batch, and
//! the run fails.
//!
//! Versions are never skipped based on the public index: pending
//! (not yet verified) versions are invisible there, so the only
//! correct dedupe is the registry's own byte-identical idempotency -
//! `cabin publish` reports a no-op for identical bytes.  That is also
//! how a failed run resumes: the rerun publishes every batch again from
//! the first, and what already landed answers as no-ops.  Divergent
//! bytes do not fail here: every upload passes `--new-revision`, so
//! they land as a new packaging revision of the same immutable
//! version.
//!
//! Rate-limit pacing lives in the client: `cabin publish` waits out
//! the registry's `429` answers per upload on the typed error.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Packages per `cabin publish` invocation.  The workflow's token draws
/// on the operator class's publish bucket (burst 100, refill 60 a
/// minute), so even a drained bucket paces a batch for under a minute,
/// leaving the rest of the token's 30 minutes to the uploads; and 200
/// ports still need only four exchanges.
const BATCH_SIZE: usize = 50;

/// Upload every conversion in publication order, one `cabin publish`
/// invocation per batch.
///
/// Every upload passes `--new-revision`: the committed ports are
/// the source of truth, so a change that reaches this tool is
/// the deliberate intent to respin the published version - identical
/// bytes still no-op through the registry's idempotency, and changed
/// bytes become a new packaging revision of the same upstream
/// version.
///
/// # Errors
/// Returns an error naming the first batch whose `cabin publish`
/// invocation cannot start or exits non-zero; later batches never run.
pub fn publish_all(package_dirs: &[&Path], index_url: &str, cabin: &Path) -> Result<()> {
    publish_in_batches(package_dirs, |batch| {
        // Inherited stdio on purpose: the client's progress, rate-limit
        // pacing warnings, and - under GitHub Actions - its
        // `::add-mask::` workflow commands must reach the real output
        // streams unaltered.
        let status = publish_command(cabin, batch, index_url)
            .status()
            .with_context(|| format!("running {} publish", cabin.display()))?;
        if !status.success() {
            bail!("`cabin publish` failed ({status})");
        }
        Ok(())
    })
}

fn publish_in_batches(
    package_dirs: &[&Path],
    mut publish: impl FnMut(&[&Path]) -> Result<()>,
) -> Result<()> {
    let batches = package_dirs.len().div_ceil(BATCH_SIZE);
    for (index, batch) in package_dirs.chunks(BATCH_SIZE).enumerate() {
        let number = index + 1;
        let range = format!(
            "{} .. {}",
            batch[0].display(),
            batch[batch.len() - 1].display()
        );
        println!(
            "publishing batch {number}/{batches} ({} packages: {range})",
            batch.len()
        );
        publish(batch).with_context(|| {
            format!(
                "publishing batch {number}/{batches} ({range}) failed; a rerun resumes by \
                 republishing every package (published ones answer as byte-identical no-ops)"
            )
        })?;
    }
    Ok(())
}

fn publish_command(cabin: &Path, batch: &[&Path], index_url: &str) -> Command {
    let mut cmd = Command::new(cabin);
    cmd.arg("-Z")
        .arg("remote-registry")
        .arg("publish")
        .arg("--new-revision")
        // Reruns after a partial publish hit the same drained publish
        // bucket, and every attempt charges it - so even a one-port
        // tree waits out 429s instead of failing the run.
        .arg("--retry-rate-limits");
    for package_dir in batch {
        cmd.arg("--manifest-path")
            .arg(package_dir.join("cabin.toml"));
    }
    cmd.arg("--index-url").arg(index_url);
    cmd
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn dirs(count: usize) -> Vec<PathBuf> {
        (0..count)
            .map(|i| PathBuf::from(format!("pkg-{i}")))
            .collect()
    }

    /// The batches a run makes, as the package dirs each one carried.
    fn batches(package_dirs: &[PathBuf]) -> Vec<Vec<PathBuf>> {
        let refs: Vec<&Path> = package_dirs.iter().map(PathBuf::as_path).collect();
        let mut seen = Vec::new();
        publish_in_batches(&refs, |batch| {
            seen.push(batch.iter().map(|dir| dir.to_path_buf()).collect());
            Ok(())
        })
        .unwrap();
        seen
    }

    #[test]
    fn batches_cover_every_package_once_in_publication_order() {
        // Zero packages must run nothing: `cabin publish` without a
        // `--manifest-path` would publish the current directory.
        for (count, sizes) in [
            (0, vec![]),
            (1, vec![1]),
            (BATCH_SIZE, vec![BATCH_SIZE]),
            (BATCH_SIZE + 1, vec![BATCH_SIZE, 1]),
            (2 * BATCH_SIZE + 7, vec![BATCH_SIZE, BATCH_SIZE, 7]),
        ] {
            let package_dirs = dirs(count);
            let seen = batches(&package_dirs);
            assert_eq!(seen.iter().map(Vec::len).collect::<Vec<_>>(), sizes);
            assert_eq!(seen.concat(), package_dirs, "{count} packages");
        }
    }

    /// A failed batch (an upload refused, a token rejected or expired)
    /// ends the run before the next batch and names the batch.
    #[test]
    fn a_failed_batch_stops_the_run() {
        let package_dirs = dirs(2 * BATCH_SIZE + 1);
        let refs: Vec<&Path> = package_dirs.iter().map(PathBuf::as_path).collect();
        let mut attempted = Vec::new();
        let err = publish_in_batches(&refs, |batch| {
            attempted.push(batch[0].to_path_buf());
            if attempted.len() == 2 {
                bail!("`cabin publish` failed (exit status: 1)");
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(attempted, [refs[0], refs[BATCH_SIZE]]);
        let message = format!("{err:#}");
        assert!(
            message.contains("batch 2/3 (pkg-50 .. pkg-99) failed"),
            "{message}"
        );
        assert!(message.contains("exit status: 1"), "{message}");
    }

    #[test]
    fn a_batch_is_one_publish_invocation_over_exactly_its_packages() {
        let batch = [Path::new("a"), Path::new("b")];
        let cmd = publish_command(Path::new("cabin"), &batch, "https://registry.example");
        let args: Vec<_> = cmd.get_args().map(|arg| arg.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "-Z",
                "remote-registry",
                "publish",
                "--new-revision",
                "--retry-rate-limits",
                "--manifest-path",
                Path::new("a").join("cabin.toml").to_str().unwrap(),
                "--manifest-path",
                Path::new("b").join("cabin.toml").to_str().unwrap(),
                "--index-url",
                "https://registry.example",
            ]
        );
    }
}
