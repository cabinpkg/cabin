use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use cabin_package::{PackageMetadata, StagedPackage};

use crate::error::RegistryError;
use crate::index::{insert_version, read_optional, render};
use crate::layout::FileRegistry;
use crate::lock::RegistryLock;

/// Inputs accepted by [`publish_to_registry`] and
/// [`validate_publish`].
#[derive(Debug, Clone)]
pub struct RegistryPublishRequest<'a> {
    pub registry_dir: &'a Path,
    pub staged: &'a StagedPackage,
}

/// What [`publish_to_registry`] (and its dry-run sibling) decided
/// happened.
///
/// `registry_modified` is `true` only when [`publish_to_registry`]
/// actually wrote bytes; [`validate_publish`] always returns `false`
/// Here.
#[derive(Debug, Clone)]
pub struct RegistryPublishOutcome {
    pub registry_dir: PathBuf,
    pub package_index_path: PathBuf,
    pub artifact_path: PathBuf,
    pub registry_modified: bool,
    pub registry_initialised: bool,
    pub source_path: String,
    pub checksum: String,
}

/// Mutate the file registry: place the artifact, then update the
/// per-package index file. Both writes go through `<file>.partial`
/// Rename guards; if the index update fails after the artifact
/// rename, the artifact is removed so the registry never holds an
/// orphaned binary.
pub fn publish_to_registry(
    request: &RegistryPublishRequest<'_>,
) -> Result<RegistryPublishOutcome, RegistryError> {
    ensure_path_safe_package_name(request.staged.name.as_str())?;
    let registry_dir = request.registry_dir;
    fs::create_dir_all(registry_dir).map_err(|source| RegistryError::Io {
        path: registry_dir.to_path_buf(),
        source,
    })?;
    let lock = RegistryLock::acquire(registry_dir)?;
    let result = publish_locked(request);
    // Drop runs even if `result` is Err, so the lock file is always
    // removed.
    drop(lock);
    result
}

/// Read-only counterpart to [`publish_to_registry`]: validate every
/// pre-write check (registry config, package-index name, duplicate
/// version, orphaned artifact) without writing anything.
pub fn validate_publish(
    request: &RegistryPublishRequest<'_>,
) -> Result<RegistryPublishOutcome, RegistryError> {
    ensure_path_safe_package_name(request.staged.name.as_str())?;
    let registry_dir = request.registry_dir;
    let registry = FileRegistry::inspect(registry_dir)?;
    let metadata = staged_metadata_for_registry(&registry, request.staged);
    plan_publish(&registry, &metadata, request.staged.archive_bytes.len()).map(|mut plan| {
        plan.registry_modified = false;
        plan
    })
}

/// / 14.6: defence-in-depth at the file-registry
/// boundary. `cabin-package` rejects unsafe names earlier, but
/// the registry crate is also reachable by tooling that bypasses
/// staging, so we re-check here before any path is built from
/// the package name. The predicate itself lives in `cabin-core`
/// So this crate, `cabin-package`, and `cabin-index-http` cannot
/// drift on the rule.
fn ensure_path_safe_package_name(name: &str) -> Result<(), RegistryError> {
    if !cabin_core::is_path_safe_package_name(name) {
        return Err(RegistryError::UnsafePackageName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn publish_locked(
    request: &RegistryPublishRequest<'_>,
) -> Result<RegistryPublishOutcome, RegistryError> {
    let registry = FileRegistry::open_or_initialise(request.registry_dir)?;
    let metadata = staged_metadata_for_registry(&registry, request.staged);
    let plan = plan_publish(&registry, &metadata, request.staged.archive_bytes.len())?;

    // Both paths come from `FileRegistry::artifact_path` /
    // `package_index_path`, which always nest at least one
    // directory below the registry root.  Use `if let` rather
    // than `.expect(...)` so a future change that returns a
    // bare filename surfaces as a clean skip rather than a panic
    // in a recoverable function.
    if let Some(parent) = plan.artifact_path.parent() {
        fs::create_dir_all(parent).map_err(|source| RegistryError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    if let Some(parent) = plan.package_index_path.parent() {
        fs::create_dir_all(parent).map_err(|source| RegistryError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    // Phase 1: place the artifact via partial rename.
    write_atomic(&plan.artifact_path, &request.staged.archive_bytes)?;

    // Phase 2: update the index. If anything goes wrong, undo the
    // artifact placement so the registry never carries an orphaned
    // file.
    let new_index = match read_optional(&plan.package_index_path) {
        Ok(existing) => match insert_version(existing, &metadata) {
            Ok(index) => index,
            Err(err) => {
                let _ = fs::remove_file(&plan.artifact_path);
                return Err(err);
            }
        },
        Err(err) => {
            let _ = fs::remove_file(&plan.artifact_path);
            return Err(err);
        }
    };

    let body = match render(&new_index) {
        Ok(body) => body,
        Err(err) => {
            let _ = fs::remove_file(&plan.artifact_path);
            return Err(err);
        }
    };

    if let Err(err) = write_atomic(&plan.package_index_path, body.as_bytes()) {
        let _ = fs::remove_file(&plan.artifact_path);
        return Err(err);
    }

    Ok(RegistryPublishOutcome {
        registry_modified: true,
        ..plan
    })
}

/// Build a [`RegistryPublishOutcome`] without writing anything.
/// Validates every pre-write rule:
///
/// - if the package index already lists this version, fail with
///   [`RegistryError::DuplicateVersion`];
/// - if an artifact file already exists for `(name, version)` but
///   The index does *not* yet record that version, fail with
///   [`RegistryError::OrphanedArtifact`];
/// - load and validate the existing index file (if any).
fn plan_publish(
    registry: &FileRegistry,
    metadata: &PackageMetadata,
    _archive_bytes_len: usize,
) -> Result<RegistryPublishOutcome, RegistryError> {
    let version = semver::Version::parse(&metadata.version).map_err(|err| {
        RegistryError::PackageIndexInvalid {
            path: PathBuf::new(),
            message: format!(
                "metadata version {:?} is not valid SemVer: {err}",
                metadata.version
            ),
        }
    })?;
    let package_index_path = registry.package_index_path(&metadata.name);
    let artifact_path = registry.artifact_path(&metadata.name, &version);

    let existing = read_optional(&package_index_path)?;
    let already_in_index = existing
        .as_ref()
        .is_some_and(|index| index.versions.contains_key(&metadata.version));

    if already_in_index {
        return Err(RegistryError::DuplicateVersion {
            name: metadata.name.clone(),
            version: metadata.version.clone(),
        });
    }
    if artifact_path.exists() {
        // Artifact present but index does not record this version: refuse
        // to silently overwrite.
        return Err(RegistryError::OrphanedArtifact {
            name: metadata.name.clone(),
            version: metadata.version.clone(),
        });
    }

    Ok(RegistryPublishOutcome {
        registry_dir: registry.root().to_path_buf(),
        package_index_path,
        artifact_path,
        registry_modified: true,
        registry_initialised: registry.was_initialised_now(),
        source_path: registry.relative_source_path(&metadata.name, &version),
        checksum: metadata.checksum.clone(),
    })
}

/// Re-render the staged package's metadata against the actual
/// registry on disk so the `source.path` field always points at
/// where the artifact will land.
fn staged_metadata_for_registry(
    registry: &FileRegistry,
    staged: &StagedPackage,
) -> PackageMetadata {
    let mut metadata = staged.metadata.clone();
    metadata.source.path = registry.relative_source_path(staged.name.as_str(), &staged.version);
    metadata
}

/// Atomic-ish file write: stream `body` into `<path>.partial`, then
/// rename the partial onto `path`. Removes the partial on failure so
/// A half-written file never lingers.
fn write_atomic(path: &Path, body: &[u8]) -> Result<(), RegistryError> {
    let mut tmp_os: OsString = path.as_os_str().to_owned();
    tmp_os.push(".partial");
    let tmp = PathBuf::from(tmp_os);
    if let Err(err) = fs::write(&tmp, body) {
        let _ = fs::remove_file(&tmp);
        return Err(RegistryError::Io {
            path: tmp,
            source: err,
        });
    }
    if let Err(err) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(RegistryError::Io {
            path: path.to_path_buf(),
            source: err,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::PackageName;
    use cabin_package::{PackageMetadata, SourceMetadata};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    fn staged(name: &str, version: &str, body: &[u8]) -> StagedPackage {
        let checksum = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(body);
            format!("sha256:{:x}", h.finalize())
        };
        StagedPackage {
            name: pkg(name),
            version: ver(version),
            archive_bytes: body.to_vec(),
            checksum: checksum.clone(),
            metadata: PackageMetadata {
                schema: 1,
                name: name.to_owned(),
                version: version.to_owned(),
                dependencies: BTreeMap::new(),
                dev_dependencies: BTreeMap::new(),
                system_dependencies: BTreeMap::new(),
                features: Default::default(),
                profiles: Default::default(),
                toolchain: Default::default(),
                build: Default::default(),
                compiler_wrapper: Default::default(),
                yanked: false,
                checksum,
                // `staged_metadata_for_registry` overrides this, but
                // give it a sane default for tests that bypass that
                // path.
                source: SourceMetadata {
                    kind: "archive".to_owned(),
                    path: format!("../artifacts/{name}/{name}-{version}.tar.gz"),
                    format: "tar.gz".to_owned(),
                },
            },
        }
    }

    #[test]
    fn publish_writes_layout_and_artifact() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.path().join("registry");
        let staged = staged("fmt", "10.2.1", b"hello world");
        let outcome = publish_to_registry(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &staged,
        })
        .unwrap();
        assert!(outcome.registry_modified);
        assert!(outcome.registry_initialised);
        assert!(outcome.artifact_path.is_file());
        assert!(outcome.package_index_path.is_file());
        // Lock file removed on success.
        assert!(!registry_dir.join(".cabin-registry.lock").exists());
        // Source path is registry-relative.
        assert_eq!(outcome.source_path, "../artifacts/fmt/fmt-10.2.1.tar.gz");
    }

    #[test]
    fn duplicate_publish_fails_and_does_not_mutate() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.path().join("registry");
        let s = staged("fmt", "10.2.1", b"first");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &s,
        })
        .unwrap();

        let again = staged("fmt", "10.2.1", b"second");
        let err = publish_to_registry(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &again,
        })
        .unwrap_err();
        match err {
            RegistryError::DuplicateVersion { name, version } => {
                assert_eq!(name, "fmt");
                assert_eq!(version, "10.2.1");
            }
            other => panic!("expected DuplicateVersion, got {other:?}"),
        }
        // Original artifact still present, unchanged.
        let body = fs::read(registry_dir.join("artifacts/fmt/fmt-10.2.1.tar.gz")).unwrap();
        assert_eq!(body, b"first");
    }

    #[test]
    fn second_version_is_appended_not_replaced() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.path().join("registry");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &staged("fmt", "10.1.0", b"v1"),
        })
        .unwrap();
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &staged("fmt", "10.2.1", b"v2"),
        })
        .unwrap();
        let body = fs::read_to_string(registry_dir.join("packages/fmt.json")).unwrap();
        assert!(body.contains("10.1.0"));
        assert!(body.contains("10.2.1"));
        assert!(
            registry_dir
                .join("artifacts/fmt/fmt-10.1.0.tar.gz")
                .is_file()
        );
        assert!(
            registry_dir
                .join("artifacts/fmt/fmt-10.2.1.tar.gz")
                .is_file()
        );
    }

    #[test]
    fn validate_publish_does_not_mutate_registry() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.path().join("registry");
        let s = staged("fmt", "10.2.1", b"hi");
        let outcome = validate_publish(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &s,
        })
        .unwrap();
        assert!(!outcome.registry_modified);
        assert!(outcome.registry_initialised);
        // Nothing should have been created.
        assert!(!registry_dir.exists() || !registry_dir.join("config.json").exists());
        assert!(!registry_dir.join(".cabin-registry.lock").exists());
    }

    #[test]
    fn validate_publish_detects_duplicate_against_existing_registry() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.path().join("registry");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &staged("fmt", "10.2.1", b"v1"),
        })
        .unwrap();
        let err = validate_publish(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &staged("fmt", "10.2.1", b"v2"),
        })
        .unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateVersion { .. }));
    }

    #[test]
    fn orphaned_artifact_is_reported() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.path().join("registry");
        // Initialise registry, then drop an artifact directly without
        // updating the index — that's the "orphan" state.
        FileRegistry::open_or_initialise(&registry_dir).unwrap();
        fs::create_dir_all(registry_dir.join("artifacts/fmt")).unwrap();
        fs::write(
            registry_dir.join("artifacts/fmt/fmt-10.2.1.tar.gz"),
            b"orphan",
        )
        .unwrap();

        let err = publish_to_registry(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &staged("fmt", "10.2.1", b"new bytes"),
        })
        .unwrap_err();
        assert!(matches!(err, RegistryError::OrphanedArtifact { .. }));
    }

    #[test]
    fn lock_collision_fails_clearly() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.path().join("registry");
        // Pre-create the lock file.
        fs::create_dir_all(&registry_dir).unwrap();
        fs::write(registry_dir.join(".cabin-registry.lock"), b"").unwrap();

        let err = publish_to_registry(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &staged("fmt", "10.2.1", b"x"),
        })
        .unwrap_err();
        assert!(matches!(err, RegistryError::Locked));
    }

    #[test]
    fn published_metadata_uses_registry_relative_source_path() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.path().join("registry");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: &registry_dir,
            staged: &staged("fmt", "10.2.1", b"x"),
        })
        .unwrap();
        let body = fs::read_to_string(registry_dir.join("packages/fmt.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let source = &value["versions"]["10.2.1"]["source"];
        assert_eq!(source["type"], "archive");
        assert_eq!(source["format"], "tar.gz");
        assert_eq!(source["path"], "../artifacts/fmt/fmt-10.2.1.tar.gz");
    }
}
