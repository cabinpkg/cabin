use std::path::{Path, PathBuf};

use cabin_core::PackageName;
use cabin_package::{StagedPackage, stage_with_project};
use cabin_registry_file::{
    RegistryPublishOutcome, RegistryPublishRequest, publish_to_registry, validate_publish,
};

use crate::error::PublishError;

/// Inputs to [`publish_to_file_registry`] and
/// [`dry_run_against_file_registry`].
#[derive(Debug, Clone)]
pub struct RegistryPublishWorkflow<'a> {
    pub manifest_path: &'a Path,
    pub registry_dir: &'a Path,
    /// Pre-resolved `Package` from the workspace
    /// loader. `cabin-cli` populates this when publishing a member
    /// of a workspace so that any `dep = { workspace = true }`
    /// Entry is substituted with its concrete requirement before
    /// the package metadata is written. Standalone callers leave
    /// it as `None`.
    pub resolved_project: Option<cabin_core::Package>,
}

/// What [`publish_to_file_registry`] / its dry-run sibling decided
/// happened. Carries everything the CLI needs to render a human or
/// JSON report.
#[derive(Debug, Clone)]
pub struct RegistryPublishReport {
    pub name: PackageName,
    pub version: semver::Version,
    pub registry_dir: PathBuf,
    pub package_index_path: PathBuf,
    pub artifact_path: PathBuf,
    pub checksum: String,
    pub source_path: String,
    pub registry_modified: bool,
    pub registry_initialised: bool,
    pub dry_run: bool,
}

/// Stage the package, then write the result into the file registry.
pub fn publish_to_file_registry(
    workflow: RegistryPublishWorkflow<'_>,
) -> Result<RegistryPublishReport, PublishError> {
    let staged = stage_with_project(workflow.manifest_path, workflow.resolved_project, None)?;
    let outcome = publish_to_registry(&RegistryPublishRequest {
        registry_dir: workflow.registry_dir,
        staged: &staged,
    })?;
    Ok(into_report(staged, outcome, false))
}

/// Stage the package and run every pre-write check against the file
/// registry without mutating it. Returns a report whose
/// `registry_modified` flag is `false`.
pub fn dry_run_against_file_registry(
    workflow: RegistryPublishWorkflow<'_>,
) -> Result<RegistryPublishReport, PublishError> {
    let staged = stage_with_project(workflow.manifest_path, workflow.resolved_project, None)?;
    let outcome = validate_publish(&RegistryPublishRequest {
        registry_dir: workflow.registry_dir,
        staged: &staged,
    })?;
    Ok(into_report(staged, outcome, true))
}

fn into_report(
    staged: StagedPackage,
    outcome: RegistryPublishOutcome,
    dry_run: bool,
) -> RegistryPublishReport {
    RegistryPublishReport {
        name: staged.name,
        version: staged.version,
        registry_dir: outcome.registry_dir,
        package_index_path: outcome.package_index_path,
        artifact_path: outcome.artifact_path,
        checksum: outcome.checksum,
        source_path: outcome.source_path,
        registry_modified: outcome.registry_modified,
        registry_initialised: outcome.registry_initialised,
        dry_run,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn registry_publish_writes_layout() {
        let dir = TempDir::new().unwrap();
        let manifest = dir.child("cabin.toml");
        manifest
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let registry = dir.child("registry");
        let report = publish_to_file_registry(RegistryPublishWorkflow {
            manifest_path: manifest.path(),
            registry_dir: registry.path(),
            resolved_project: None,
        })
        .unwrap();
        assert_eq!(report.name.as_str(), "fmt");
        assert_eq!(report.version.to_string(), "10.2.1");
        assert!(report.registry_modified);
        assert!(report.registry_initialised);
        assert!(!report.dry_run);
        assert!(report.package_index_path.is_file());
        assert!(report.artifact_path.is_file());
        assert_eq!(report.source_path, "../artifacts/fmt/fmt-10.2.1.tar.gz");
    }

    #[test]
    fn dry_run_against_registry_does_not_mutate() {
        let dir = TempDir::new().unwrap();
        let manifest = dir.child("cabin.toml");
        manifest
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let registry = dir.child("registry");
        let report = dry_run_against_file_registry(RegistryPublishWorkflow {
            manifest_path: manifest.path(),
            registry_dir: registry.path(),
            resolved_project: None,
        })
        .unwrap();
        assert!(!report.registry_modified);
        assert!(report.dry_run);
        // Dry-run does not initialise on disk.
        registry
            .child("config.json")
            .assert(predicates::path::missing());
    }
}
