use std::path::{Path, PathBuf};

use cabin_core::PackageName;
use cabin_package::{PackageRequest, PackagedArtifact, package_with_project};

use crate::error::PublishError;

/// Inputs to [`dry_run`].
#[derive(Debug, Clone)]
pub struct DryRunRequest<'a> {
    pub manifest_path: &'a Path,
    pub output_dir: &'a Path,
    /// Pre-resolved `Package` from the workspace
    /// loader. See the corresponding field on
    /// `RegistryPublishWorkflow`. Standalone callers leave it
    /// `None`.
    pub resolved_project: Option<cabin_core::Package>,
}

/// Result of a publish dry run.
///
/// `registry_modified` is always `false` for the dry-run flow —
/// the field is kept on the surface so JSON consumers can read
/// it and so registry-aware publish paths can flip it when they
/// actually mutate a registry.
#[derive(Debug, Clone)]
pub struct DryRunReport {
    pub name: PackageName,
    pub version: semver::Version,
    pub archive_path: PathBuf,
    pub metadata_path: PathBuf,
    pub checksum: String,
    pub registry_modified: bool,
}

/// Run the publish dry-run pipeline: validate the package,
/// build a deterministic source archive, generate canonical
/// per-version metadata, write both into the output directory, and
/// return a [`DryRunReport`].
pub fn dry_run(request: DryRunRequest<'_>) -> Result<DryRunReport, PublishError> {
    let PackagedArtifact {
        name,
        version,
        archive_path,
        metadata_path,
        checksum,
    } = package_with_project(
        PackageRequest {
            manifest_path: request.manifest_path,
            output_dir: request.output_dir,
        },
        request.resolved_project,
    )?;
    Ok(DryRunReport {
        name,
        version,
        archive_path,
        metadata_path,
        checksum,
        registry_modified: false,
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
        })
        .unwrap();
        let second = dry_run(DryRunRequest {
            manifest_path: manifest.path(),
            output_dir: out.path(),
            resolved_project: None,
        })
        .unwrap();
        assert_eq!(first.checksum, second.checksum);
    }
}
