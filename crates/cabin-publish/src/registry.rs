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
    /// loader. `cabin` populates this when publishing a member
    /// of a workspace so that any `dep = { workspace = true }`
    /// Entry is substituted with its concrete requirement before
    /// the package metadata is written.  Standalone callers leave
    /// it as `None`.
    pub resolved_project: Option<cabin_core::Package>,
    /// Raw `[workspace.<kind>-dependencies]` strings for archive
    /// normalization.  Standalone callers pass the empty default.
    pub workspace_dep_requirements: cabin_core::WorkspaceDepRequirements,
}

/// What [`publish_to_file_registry`] / its dry-run sibling decided
/// happened.  Carries everything the CLI needs to render a human or
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
    pub registry_initialized: bool,
    pub dry_run: bool,
    /// Non-rejecting standard-compatibility lint messages (PL2, PL3)
    /// the CLI prints to stderr; the publish proceeded regardless.
    /// Deterministically ordered (by target, then `c` before `c++`).
    pub warnings: Vec<String>,
}

/// Stage the package, then write the result into the file registry.
///
/// # Errors
/// Returns [`PublishError::Package`] when staging the package fails
/// (propagated from `stage_with_project`), or
/// [`PublishError::Registry`] when the registry write fails -
/// propagated from `publish_to_registry` (unsafe package name,
/// duplicate version, registry config/index problems, or I/O).
pub fn publish_to_file_registry(
    workflow: RegistryPublishWorkflow<'_>,
) -> Result<RegistryPublishReport, PublishError> {
    let staged = stage_with_project(
        workflow.manifest_path,
        workflow.resolved_project,
        None,
        &workflow.workspace_dep_requirements,
    )?;
    require_scoped_name(&staged.name, workflow.manifest_path)?;
    require_scoped_dependency_names(&staged.metadata, workflow.manifest_path)?;
    // Reject on a PL1 error before touching the registry; a passing
    // check returns the PL2/PL3 warnings to surface.
    let warnings = evaluate_lints(&staged, workflow.registry_dir)?;
    let outcome = publish_to_registry(&RegistryPublishRequest {
        registry_dir: workflow.registry_dir,
        staged: &staged,
    })?;
    Ok(into_report(staged, outcome, false, warnings))
}

/// Stage the package and run every pre-write check against the file
/// registry without mutating it.  Returns a report whose
/// `registry_modified` flag is `false`.
///
/// # Errors
/// Returns [`PublishError::Package`] when staging the package fails
/// (propagated from `stage_with_project`), or
/// [`PublishError::Registry`] when a pre-write check fails -
/// propagated from `validate_publish` (unsafe package name,
/// duplicate version, or registry config/index problems).
pub fn dry_run_against_file_registry(
    workflow: RegistryPublishWorkflow<'_>,
) -> Result<RegistryPublishReport, PublishError> {
    let staged = stage_with_project(
        workflow.manifest_path,
        workflow.resolved_project,
        None,
        &workflow.workspace_dep_requirements,
    )?;
    require_scoped_name(&staged.name, workflow.manifest_path)?;
    require_scoped_dependency_names(&staged.metadata, workflow.manifest_path)?;
    let warnings = evaluate_lints(&staged, workflow.registry_dir)?;
    let outcome = validate_publish(&RegistryPublishRequest {
        registry_dir: workflow.registry_dir,
        staged: &staged,
    })?;
    Ok(into_report(staged, outcome, true, warnings))
}

/// The publish gate for bare names: registry packages are always
/// `<scope>/<name>`, so every publish workflow (file, staging
/// dry-run, and the CLI's remote flow) rejects a bare name right
/// after staging - before any lint, registry, or network work - with
/// the manifest line to change.  Local staging via `cabin package`
/// stays ungated.
///
/// # Errors
/// Returns [`PublishError::BarePackageName`] when `name` has no
/// scope.
pub fn require_scoped_name(
    name: &cabin_core::PackageName,
    manifest_path: &Path,
) -> Result<(), PublishError> {
    if !name.is_scoped() {
        return Err(PublishError::BarePackageName {
            name: name.as_str().to_owned(),
            manifest_path: manifest_path.display().to_string(),
        });
    }
    Ok(())
}

/// The publish gate for non-canonical dependency keys, fired beside
/// [`require_scoped_name`] in every publish workflow: the staged
/// metadata's `dependencies` and `dev-dependencies` maps key on
/// canonical `<scope>/<name>` registry names.  Dev-dependency keys
/// denote registry packages too (they resolve when building the
/// package's own tests), so one grammar covers both maps;
/// `system-dependencies` is deliberately not checked - its keys name
/// system packages, not registry packages.  The hosted registry
/// enforces the same rule server-side (`registry/src/publish.rs`), so
/// failing here is the local, pre-network version of the same `400`.
///
/// # Errors
/// Returns [`PublishError::InvalidDependencyName`] naming the first
/// offending table and key.
pub fn require_scoped_dependency_names(
    metadata: &cabin_package::metadata::PackageMetadata,
    manifest_path: &Path,
) -> Result<(), PublishError> {
    let tables = [
        ("dependencies", &metadata.dependencies),
        ("dev-dependencies", &metadata.dev_dependencies),
    ];
    for (table, entries) in tables {
        if let Some(name) = entries.keys().find(|name| !is_canonical_registry_key(name)) {
            return Err(PublishError::InvalidDependencyName {
                table,
                name: name.clone(),
                manifest_path: manifest_path.display().to_string(),
            });
        }
    }
    Ok(())
}

/// Mirror of the hosted registry's scope and package-name grammars
/// (`registry/src/routes.rs`; `cabin-registry-api` and
/// `cabin-index-http` keep the same mirror at their URL boundaries).
/// `PackageName`'s own grammar is looser - uppercase and `.` are
/// legal in local-only names - so a plain scoped check would let a
/// key the registry refuses fail publish only after staging and
/// network work.
fn is_canonical_registry_key(key: &str) -> bool {
    let Some((scope, name)) = key.split_once('/') else {
        return false;
    };
    let scope_ok = !scope.is_empty()
        && scope.len() <= 39
        && scope
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !scope.starts_with('-')
        && !scope.ends_with('-');
    let name_ok = !name.is_empty()
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    scope_ok && name_ok
}

/// Run the standard-compatibility lints against a staged package and
/// the registry it is being published into: PL1/PL2 from the resolved
/// manifest and PL3 against the registry's existing versions (its
/// baseline).  Returns the warnings to surface, or a
/// [`PublishError::StandardCompatibility`] when a PL1 error rejects the
/// publish before any write.
fn evaluate_lints(
    staged: &StagedPackage,
    registry_dir: &Path,
) -> Result<Vec<String>, PublishError> {
    let published = cabin_registry_file::read_published_standards(registry_dir, &staged.name)?;
    staged_lint_warnings(staged, &published)
}

/// The publish-time standard-compatibility lints over a staged
/// package and a caller-supplied baseline of already-published
/// versions: PL1/PL2 from the resolved manifest, PL3 against the
/// baseline.  The file-registry path reads its baseline with
/// `cabin_registry_file::read_published_standards`; the experimental
/// remote publish path feeds the versions fetched from the
/// registry's package index document, so both flows run the
/// identical checks.
///
/// # Errors
/// Returns [`PublishError::StandardCompatibility`] when a PL1 error
/// rejects the publish before any write.
pub fn staged_lint_warnings(
    staged: &StagedPackage,
    published: &[(semver::Version, cabin_core::StandardsMetadata)],
) -> Result<Vec<String>, PublishError> {
    let mut findings = crate::lints::manifest_findings(&staged.package);
    findings.extend(crate::lints::patch_release_findings(
        &staged.version,
        &staged.metadata.standards,
        published,
    ));
    crate::lints::split(findings).map_err(PublishError::StandardCompatibility)
}

fn into_report(
    staged: StagedPackage,
    outcome: RegistryPublishOutcome,
    dry_run: bool,
    warnings: Vec<String>,
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
        registry_initialized: outcome.registry_initialized,
        dry_run,
        warnings,
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
            .write_str("[package]\nname = \"fmtlib/fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let registry = dir.child("registry");
        let report = publish_to_file_registry(RegistryPublishWorkflow {
            manifest_path: manifest.path(),
            registry_dir: registry.path(),
            resolved_project: None,
            workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        })
        .unwrap();
        assert_eq!(report.name.as_str(), "fmtlib/fmt");
        assert_eq!(report.version.to_string(), "10.2.1");
        assert!(report.registry_modified);
        assert!(report.registry_initialized);
        assert!(!report.dry_run);
        assert!(report.package_index_path.is_file());
        assert!(report.artifact_path.is_file());
        assert_eq!(
            report.source_path,
            "../../artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1.zip"
        );
    }

    #[test]
    fn dry_run_against_registry_does_not_mutate() {
        let dir = TempDir::new().unwrap();
        let manifest = dir.child("cabin.toml");
        manifest
            .write_str("[package]\nname = \"fmtlib/fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let registry = dir.child("registry");
        let report = dry_run_against_file_registry(RegistryPublishWorkflow {
            manifest_path: manifest.path(),
            registry_dir: registry.path(),
            resolved_project: None,
            workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        })
        .unwrap();
        assert!(!report.registry_modified);
        assert!(report.dry_run);
        // Dry-run does not initialize on disk.
        registry
            .child("config.json")
            .assert(predicates::path::missing());
    }

    /// Both file-registry workflows fire the bare-name gate right
    /// after staging: no lint runs and no registry is initialized.
    #[test]
    fn file_registry_workflows_reject_bare_names() {
        let dir = TempDir::new().unwrap();
        let manifest = dir.child("cabin.toml");
        manifest
            .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
            .unwrap();
        let registry = dir.child("registry");
        let workflow = || RegistryPublishWorkflow {
            manifest_path: manifest.path(),
            registry_dir: registry.path(),
            resolved_project: None,
            workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
        };
        let err = publish_to_file_registry(workflow()).unwrap_err();
        assert!(matches!(err, PublishError::BarePackageName { .. }));
        let err = dry_run_against_file_registry(workflow()).unwrap_err();
        assert!(matches!(err, PublishError::BarePackageName { .. }));
        registry
            .child("config.json")
            .assert(predicates::path::missing());
    }

    /// The registry dependency maps key on canonical scoped names:
    /// both file workflows reject a bare or non-canonical key in
    /// `[dependencies]` and `[dev-dependencies]` alike, while system
    /// dependencies are exempt (their keys name system packages).
    #[test]
    fn file_registry_workflows_reject_non_canonical_dependency_names() {
        let dir = TempDir::new().unwrap();
        let registry = dir.child("registry");
        let manifest = dir.child("cabin.toml");
        let workflow = |manifest_body: &str| {
            manifest.write_str(manifest_body).unwrap();
            RegistryPublishWorkflow {
                manifest_path: manifest.path(),
                registry_dir: registry.path(),
                resolved_project: None,
                workspace_dep_requirements: cabin_core::WorkspaceDepRequirements::default(),
            }
        };
        // Bare, and scoped-but-local spellings the hosted grammar
        // refuses (uppercase, `.`): `PackageName` accepts them all,
        // so the gate must not.
        for key in ["zlib", "madler/Zlib", "madler/z.lib"] {
            let body = format!(
                "[package]\nname = \"fmtlib/fmt\"\nversion = \"10.2.1\"\n\
                 [dependencies]\n\"{key}\" = \"^1.3\"\n"
            );
            let err = publish_to_file_registry(workflow(&body)).unwrap_err();
            assert!(
                matches!(
                    &err,
                    PublishError::InvalidDependencyName { table, name, .. }
                        if *table == "dependencies" && name == key
                ),
                "key: {key}, err: {err}"
            );
            let err = dry_run_against_file_registry(workflow(&body)).unwrap_err();
            assert!(
                matches!(err, PublishError::InvalidDependencyName { .. }),
                "key: {key}"
            );
        }
        let err = publish_to_file_registry(workflow(
            "[package]\nname = \"fmtlib/fmt\"\nversion = \"10.2.1\"\n\
             [dev-dependencies]\ncatch2 = \"^3\"\n",
        ))
        .unwrap_err();
        assert!(matches!(
            err,
            PublishError::InvalidDependencyName {
                table: "dev-dependencies",
                ..
            }
        ));
        registry
            .child("config.json")
            .assert(predicates::path::missing());

        // Scoped registry dependencies and bare system dependencies
        // both publish fine: a `system = true` entry lands in the
        // metadata's exempt `system-dependencies` map.
        let report = publish_to_file_registry(workflow(
            "[package]\nname = \"fmtlib/fmt\"\nversion = \"10.2.1\"\n\
             [dependencies]\n\"madler/zlib\" = \"^1.3\"\n\
             openssl = { system = true, version = \">=3\" }\n",
        ))
        .unwrap();
        assert!(report.registry_modified);
    }
}
