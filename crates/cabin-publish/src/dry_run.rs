use std::path::{Path, PathBuf};

use cabin_core::PackageName;
use cabin_package::{stage_with_project, write_staged};

use crate::error::PublishError;

/// Inputs to [`dry_run`].
#[derive(Debug, Clone)]
pub struct DryRunRequest<'a> {
    pub manifest_path: &'a Path,
    pub output_dir: &'a Path,
    /// Pre-resolved `Package` from the workspace
    /// loader.  See the corresponding field on
    /// `RegistryPublishWorkflow`.  Standalone callers leave it
    /// `None`.
    pub resolved_project: Option<cabin_core::Package>,
    /// Raw `[workspace.<kind>-dependencies]` strings for archive
    /// normalization.  Standalone callers pass the empty default.
    pub workspace_dep_requirements: cabin_core::WorkspaceDepRequirements,
}

/// Result of a publish dry run.
///
/// `registry_modified` is always `false` for the dry-run flow -
/// the field is kept on the surface so JSON consumers can read
/// it and so registry-aware publish paths can flip it when they
/// mutate a registry.
#[derive(Debug, Clone)]
pub struct DryRunReport {
    pub name: PackageName,
    pub version: semver::Version,
    pub archive_path: PathBuf,
    pub metadata_path: PathBuf,
    pub checksum: String,
    pub registry_modified: bool,
    /// Non-rejecting standard-compatibility lint messages (PL2) the
    /// CLI prints to stderr.  A staging-only dry-run runs PL1/PL2 only.
    pub warnings: Vec<String>,
    /// `true` when the patch-release requirement check (PL3) was
    /// skipped because this staging-only dry-run has no registry to
    /// compare against, and the package has a `standards` table that
    /// check would otherwise apply to.  The CLI says so in its output
    /// rather than letting a patch release look silently clean.
    pub standards_check_skipped: bool,
}

/// Run the publish dry-run pipeline: validate the package,
/// build a deterministic source archive, generate canonical
/// per-version metadata, write both into the output directory, and
/// return a [`DryRunReport`].
///
/// PL1/PL2 run here too (they need only the resolved manifest); a PL1
/// error rejects the dry-run before any staging output is written.
/// PL3 is registry-backed, so a staging-only dry-run has no baseline
/// to compare against and skips it, flagging the skip in the report.
///
/// # Errors
/// Returns [`PublishError::StandardCompatibility`] when a PL1 lint
/// rejects the package, and [`PublishError::Package`] when staging,
/// archiving, or writing the artifacts fails - it propagates every
/// `cabin_package::PackageError` raised by `stage_with_project` /
/// `write_staged` (manifest validation, unresolved workspace
/// dependencies, I/O, or a conflicting non-identical file already
/// present in `output_dir`).
pub fn dry_run(request: DryRunRequest<'_>) -> Result<DryRunReport, PublishError> {
    let staged = stage_with_project(
        request.manifest_path,
        request.resolved_project,
        Some(request.output_dir),
        &request.workspace_dep_requirements,
    )?;
    // Reject on a PL1 error before writing any staging output.
    let warnings = crate::lints::split(crate::lints::manifest_findings(&staged.package))
        .map_err(PublishError::StandardCompatibility)?;
    let standards_check_skipped = !staged.metadata.standards.is_empty();
    let artifact = write_staged(&staged, request.output_dir)?;
    Ok(DryRunReport {
        name: artifact.name,
        version: artifact.version,
        archive_path: artifact.archive_path,
        metadata_path: artifact.metadata_path,
        checksum: artifact.checksum,
        registry_modified: false,
        warnings,
        standards_check_skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn dry_run_produces_archive_and_metadata() {
        let dir = TempDir::new().unwrap();
        let manifest = dir.child("cabin.toml");
        manifest
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let out = dir.child("dist");
        let report = dry_run(DryRunRequest {
            manifest_path: manifest.path(),
            output_dir: out.path(),
            resolved_project: None,
            workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        })
        .unwrap();
        assert_eq!(report.name.as_str(), "fmt");
        assert_eq!(report.version.to_string(), "10.2.1");
        assert!(report.archive_path.is_file());
        assert!(report.metadata_path.is_file());
        assert!(report.checksum.starts_with("sha256:"));
        assert!(!report.registry_modified);
    }

    #[test]
    fn dry_run_is_idempotent_for_same_input() {
        let dir = TempDir::new().unwrap();
        let manifest = dir.child("cabin.toml");
        manifest
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let out = dir.child("dist");
        let first = dry_run(DryRunRequest {
            manifest_path: manifest.path(),
            output_dir: out.path(),
            resolved_project: None,
            workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        })
        .unwrap();
        let second = dry_run(DryRunRequest {
            manifest_path: manifest.path(),
            output_dir: out.path(),
            resolved_project: None,
            workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        })
        .unwrap();
        assert_eq!(first.checksum, second.checksum);
    }
}
