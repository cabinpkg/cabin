use std::fs;
use std::path::{Path, PathBuf};

use cabin_package::{PackageMetadata, StagedPackage};

use crate::atomic::atomically_write;
use crate::error::RegistryError;
use crate::index::{InsertDisposition, insert_version, read_optional, render};
use crate::layout::FileRegistry;
use crate::lock::RegistryLock;

/// Inputs accepted by [`publish_to_registry`] and
/// [`validate_publish`].
#[derive(Debug, Clone)]
pub struct RegistryPublishRequest<'a> {
    pub registry_dir: &'a Path,
    pub staged: &'a StagedPackage,
    /// The `--new-revision` opt-in: allow different bytes for an
    /// already-published version to land as a new packaging revision
    /// of it.  Without it such a publish fails with a diagnostic
    /// explaining the revision mechanism, so a forgotten version
    /// bump can never respin a version by accident.
    pub new_revision: bool,
}

/// What [`publish_to_registry`] (and its dry-run sibling) decided
/// happened.
///
/// `registry_modified` is `true` only when [`publish_to_registry`]
/// wrote bytes; [`validate_publish`] always returns `false`
/// here.  `no_op` reports a byte-identical republication that mapped
/// onto the already-recorded revision.
#[derive(Debug, Clone)]
pub struct RegistryPublishOutcome {
    pub registry_dir: PathBuf,
    pub package_index_path: PathBuf,
    pub artifact_path: PathBuf,
    pub registry_modified: bool,
    pub registry_initialized: bool,
    pub source_path: String,
    pub checksum: String,
    pub revision: String,
    pub no_op: bool,
}

/// Mutate the file registry: place the artifact, then update the
/// per-package index file.  Both writes go through atomic-rename
/// guards; if the index update fails after the artifact rename,
/// the artifact is removed so the registry never holds an
/// orphaned binary.
///
/// # Errors
/// Returns [`RegistryError::BarePackageName`] for an unscoped name,
/// [`RegistryError::StagedMetadataNameMismatch`] /
/// [`RegistryError::StagedMetadataVersionMismatch`] when the staged
/// metadata disagrees with the typed staged identity,
/// [`RegistryError::VersionBuildMetadata`] for a version carrying
/// `SemVer` build metadata,
/// [`RegistryError::StagedChecksumMismatch`] when a staged checksum
/// claim does not name the staged archive bytes,
/// [`RegistryError::Io`] if the registry directory
/// cannot be created, and [`RegistryError::Locked`] if another process
/// holds the lock.  Once locked, propagates every error from the write
/// path, including registry initialization
/// ([`RegistryError::InvalidConfig`], [`RegistryError::ConfigJson`],
/// [`RegistryError::Json`]), the revision rules
/// ([`RegistryError::NewRevisionRequiresOptIn`],
/// [`RegistryError::RevisionCollision`],
/// [`RegistryError::RevisionChangesResolverMetadata`]),
/// [`RegistryError::OrphanedArtifact`], index parse/render failures,
/// and [`RegistryError::Io`] from the atomic writes.
pub fn publish_to_registry(
    request: &RegistryPublishRequest<'_>,
) -> Result<RegistryPublishOutcome, RegistryError> {
    ensure_publishable_registry_name(request.staged)?;
    ensure_staged_checksum_matches(request.staged)?;
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
/// pre-write check (registry config, package-index name, revision
/// rules, orphaned artifact) without writing anything.
///
/// # Errors
/// Returns [`RegistryError::BarePackageName`] for an unscoped name,
/// [`RegistryError::StagedMetadataNameMismatch`] /
/// [`RegistryError::StagedMetadataVersionMismatch`] when the staged
/// metadata disagrees with the typed staged identity,
/// [`RegistryError::VersionBuildMetadata`] for a version carrying
/// `SemVer` build metadata,
/// [`RegistryError::StagedChecksumMismatch`] when a staged checksum
/// claim does not name the staged archive bytes, propagates the
/// registry-open errors of
/// [`FileRegistry::inspect`], and propagates the pre-write checks
/// (`plan_publish`): [`RegistryError::NewRevisionRequiresOptIn`],
/// [`RegistryError::RevisionCollision`],
/// [`RegistryError::RevisionChangesResolverMetadata`],
/// [`RegistryError::OrphanedArtifact`],
/// [`RegistryError::PackageIndexInvalid`] for a non-SemVer metadata
/// version, and the existing-index read errors of [`read_optional`].
pub fn validate_publish(
    request: &RegistryPublishRequest<'_>,
) -> Result<RegistryPublishOutcome, RegistryError> {
    ensure_publishable_registry_name(request.staged)?;
    ensure_staged_checksum_matches(request.staged)?;
    let registry_dir = request.registry_dir;
    let registry = FileRegistry::inspect(registry_dir)?;
    let metadata = staged_metadata_for_registry(&registry, request.staged)?;
    plan_publish(&registry, request, &metadata).map(|mut plan| {
        plan.outcome.registry_modified = false;
        plan.outcome
    })
}

/// Defense-in-depth at the file-registry boundary.  Registry
/// packages are always scoped (`<scope>/<name>`): `cabin-publish`
/// rejects bare names earlier with an actionable diagnostic, but
/// this crate is also reachable by tooling that bypasses that flow,
/// so re-require the invariant before anything is written.  Legacy
/// bare-name registries stay readable and vendorable - only new
/// publication is gated.  The staged metadata must also agree with
/// the typed staged name, because every registry path is derived
/// from the latter while the former is what lands in the index
/// document.
fn ensure_publishable_registry_name(staged: &StagedPackage) -> Result<(), RegistryError> {
    if !staged.name.is_scoped() {
        return Err(RegistryError::BarePackageName {
            name: staged.name.as_str().to_owned(),
        });
    }
    if staged.metadata.name != staged.name.as_str() {
        return Err(RegistryError::StagedMetadataNameMismatch {
            staged: staged.name.as_str().to_owned(),
            metadata: staged.metadata.name.clone(),
        });
    }
    // The same location/document agreement holds for the version: the
    // artifact and index paths derive from the typed staged version
    // while the metadata's string lands in the index document.
    if staged.metadata.version != staged.version.to_string() {
        return Err(RegistryError::StagedMetadataVersionMismatch {
            staged: staged.version.to_string(),
            metadata: staged.metadata.version.clone(),
        });
    }
    // Registry versions are plain upstream versions - the loader
    // rejects build-metadata version keys outright, so writing one
    // here would wedge the package index for every later read.
    // `cabin publish` refuses this earlier (`validate_publishable`);
    // this is the boundary for tooling that skips that flow.
    if !staged.version.build.is_empty() {
        return Err(RegistryError::VersionBuildMetadata {
            version: staged.version.to_string(),
        });
    }
    Ok(())
}

/// Defense-in-depth beside [`ensure_publishable_registry_name`], for
/// the same reason: the packaging revision and the index checksum
/// both derive from the staged checksum claims, and the whole
/// revision contract (byte-identical no-ops, collision detection,
/// the immutable triple) assumes those claims name the archive bytes
/// actually written.  The staging path computes them from the bytes,
/// but tooling can construct a [`StagedPackage`] directly, so
/// recompute the digest and refuse a lying claim before anything
/// derives from it.
fn ensure_staged_checksum_matches(staged: &StagedPackage) -> Result<(), RegistryError> {
    use sha2::{Digest, Sha256};
    let computed = format!(
        "sha256:{}",
        cabin_core::hash::hex_digest(&Sha256::digest(&staged.archive_bytes))
    );
    for claimed in [&staged.checksum, &staged.metadata.checksum] {
        if claimed != &computed {
            return Err(RegistryError::StagedChecksumMismatch {
                name: staged.name.as_str().to_owned(),
                claimed: claimed.clone(),
                computed,
            });
        }
    }
    Ok(())
}

fn publish_locked(
    request: &RegistryPublishRequest<'_>,
) -> Result<RegistryPublishOutcome, RegistryError> {
    let registry = FileRegistry::open_or_initialize(request.registry_dir)?;
    let metadata = staged_metadata_for_registry(&registry, request.staged)?;
    let plan = plan_publish(&registry, request, &metadata)?;

    if plan.disposition == InsertDisposition::NoOp {
        // Byte-identical republication: the index already records
        // this revision.  The no-op is also the self-heal path:
        // re-place a missing artifact file, and rewrite one whose
        // bytes drifted from the staged archive (truncation or
        // corruption would fail checksum verification on every
        // fetch, and republishing is the natural repair).
        let mut outcome = plan.outcome;
        let stored = fs::read(&outcome.artifact_path).ok();
        if stored.as_deref() != Some(request.staged.archive_bytes.as_slice()) {
            ensure_parent_dir(&outcome.artifact_path)?;
            atomically_write(&outcome.artifact_path, &request.staged.archive_bytes)?;
            outcome.registry_modified = true;
        }
        return Ok(outcome);
    }

    ensure_parent_dir(&plan.outcome.artifact_path)?;
    ensure_parent_dir(&plan.outcome.package_index_path)?;

    // Phase 1: place the artifact via atomic rename.
    atomically_write(&plan.outcome.artifact_path, &request.staged.archive_bytes)?;

    // Phase 2: update the index.  If anything goes wrong, undo the
    // artifact placement so the registry never carries an orphaned
    // file.
    let write_index = || -> Result<(), RegistryError> {
        let body = render(&plan.new_index, &plan.outcome.package_index_path)?;
        atomically_write(&plan.outcome.package_index_path, body.as_bytes())
    };
    if let Err(err) = write_index() {
        // If the rollback itself fails the registry is left with an
        // orphaned artifact; surface that now (with the remedy)
        // instead of letting the *next* publish fail with a bare
        // `OrphanedArtifact` whose cause is long gone.
        if let Err(cleanup) = fs::remove_file(&plan.outcome.artifact_path) {
            return Err(RegistryError::PublishRollback {
                index_error: Box::new(err),
                artifact_path: plan.outcome.artifact_path.clone(),
                cleanup,
            });
        }
        return Err(err);
    }

    Ok(RegistryPublishOutcome {
        registry_modified: true,
        ..plan.outcome
    })
}

/// Both registry paths always nest at least one directory below the
/// registry root.  Use `if let` rather than `.expect(...)` so a
/// future change that returns a bare filename surfaces as a clean
/// skip rather than a panic in a recoverable function.
fn ensure_parent_dir(path: &Path) -> Result<(), RegistryError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| RegistryError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

/// Everything [`publish_locked`] needs decided before it writes:
/// the reported outcome, whether the write phase runs at all, and
/// the fully-spliced index document to render when it does.
struct PublishPlan {
    outcome: RegistryPublishOutcome,
    disposition: InsertDisposition,
    new_index: crate::index::PackageIndex,
}

/// Build a [`PublishPlan`] without writing anything.  Validates
/// every pre-write rule: the revision semantics of
/// [`insert_version`] (no-op / opt-in / collision / resolver-
/// metadata invariance), and the per-revision orphaned-artifact
/// check (an artifact file present on disk for a revision the index
/// does not record is refused rather than silently overwritten).
fn plan_publish(
    registry: &FileRegistry,
    request: &RegistryPublishRequest<'_>,
    metadata: &PackageMetadata,
) -> Result<PublishPlan, RegistryError> {
    let staged = request.staged;
    // Paths derive from the *typed* staged identity;
    // `ensure_publishable_registry_name` already pinned the
    // metadata's name to it.
    let package_index_path = registry.package_index_path(&staged.name);
    let version = semver::Version::parse(&metadata.version).map_err(|err| {
        RegistryError::PackageIndexInvalid {
            path: package_index_path.clone(),
            message: format!(
                "metadata version {:?} is not valid SemVer: {err}",
                metadata.version
            ),
        }
    })?;
    let revision = crate::index::revision_of(metadata)?.to_owned();
    let artifact_path = registry.artifact_path(&staged.name, &version, &revision);

    let existing = read_optional(&package_index_path)?;
    let (new_index, disposition) =
        insert_version(existing, metadata, &publish_stamp(), request.new_revision)?;

    if disposition == InsertDisposition::Inserted && artifact_path.exists() {
        // Artifact present but index does not record this revision:
        // refuse to silently overwrite.
        return Err(RegistryError::OrphanedArtifact {
            name: metadata.name.clone(),
            version: metadata.version.clone(),
            revision,
        });
    }

    Ok(PublishPlan {
        outcome: RegistryPublishOutcome {
            registry_dir: registry.root().to_path_buf(),
            package_index_path,
            artifact_path,
            registry_modified: false,
            registry_initialized: registry.was_initialized_now(),
            source_path: registry.relative_source_path(&staged.name, &version, &revision),
            checksum: metadata.checksum.clone(),
            no_op: disposition == InsertDisposition::NoOp,
            revision,
        },
        disposition,
        new_index,
    })
}

/// Re-render the staged package's metadata against the actual
/// registry on disk so the `source.path` field always points at
/// where the artifact will land.
fn staged_metadata_for_registry(
    registry: &FileRegistry,
    staged: &StagedPackage,
) -> Result<PackageMetadata, RegistryError> {
    let mut metadata = staged.metadata.clone();
    let revision = crate::index::revision_of(&metadata)?.to_owned();
    metadata.source.path = registry.relative_source_path(&staged.name, &staged.version, &revision);
    Ok(metadata)
}

/// UTC `YYYY-MM-DDTHH:MM:SSZ` stamp for a freshly published
/// revision.  Publish time is genuinely new information, so this is
/// the one wall-clock read in the crate; everything else in the
/// written registry stays a pure function of its inputs.
fn publish_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, tod) = (secs / 86_400, secs % 86_400);
    // Civil-from-days (Howard Hinnant's algorithm), valid for every
    // date this code will ever stamp.
    let (year, month, day) = {
        let z = days as i64 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = era * 400 + yoe + i64::from(m <= 2);
        (y, m, d)
    };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use cabin_core::PackageName;
    use cabin_package::{PackageMetadata, SourceMetadata};
    use predicates::prelude::*;
    use std::collections::BTreeMap;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    /// The packaging-revision id a staged package will publish under.
    fn rev_of(staged: &StagedPackage) -> &str {
        let hex = staged.checksum.strip_prefix("sha256:").unwrap();
        &hex[..cabin_core::registry::PACKAGING_REVISION_HEX_LEN]
    }

    fn staged(name: &str, version: &str, body: &[u8]) -> StagedPackage {
        let checksum = {
            use cabin_core::hash::hex_digest;
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(body);
            format!("sha256:{}", hex_digest(&h.finalize()))
        };
        StagedPackage {
            name: pkg(name),
            version: ver(version),
            archive_bytes: body.to_vec(),
            checksum: checksum.clone(),
            package: cabin_core::Package::new(pkg(name), ver(version), Vec::new(), Vec::new())
                .unwrap(),
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
                language: Default::default(),
                standards: Default::default(),
                upstream: None,
                yanked: false,
                checksum,
                // `staged_metadata_for_registry` overrides this, but
                // give it a sane default for tests that bypass that
                // path.
                source: SourceMetadata {
                    kind: "archive".to_owned(),
                    path: {
                        let n = pkg(name);
                        let climb = if n.is_scoped() { "../../" } else { "../" };
                        let dirs = n.path_components().collect::<Vec<_>>().join("/");
                        format!(
                            "{climb}artifacts/{dirs}/{}-{version}.zip",
                            n.artifact_stem()
                        )
                    },
                    format: "zip".to_owned(),
                },
            },
        }
    }

    #[test]
    fn publish_writes_layout_and_artifact() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        let staged = staged("fmtlib/fmt", "10.2.1", b"hello world");
        let outcome = publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &staged,
            new_revision: false,
        })
        .unwrap();
        assert!(outcome.registry_modified);
        assert!(outcome.registry_initialized);
        assert!(outcome.artifact_path.is_file());
        assert!(outcome.package_index_path.is_file());
        // Lock file removed on success.
        registry_dir
            .child(".cabin-registry.lock")
            .assert(predicate::path::missing());
        // Source path is registry-relative and revision-qualified.
        assert_eq!(
            outcome.source_path,
            format!(
                "../../artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1-{}.zip",
                rev_of(&staged)
            )
        );
    }

    #[test]
    fn duplicate_publish_fails_and_does_not_mutate() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        let s = staged("fmtlib/fmt", "10.2.1", b"first");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &s,
            new_revision: false,
        })
        .unwrap();

        let again = staged("fmtlib/fmt", "10.2.1", b"second");
        let err = publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &again,
            new_revision: false,
        })
        .unwrap_err();
        match err {
            RegistryError::NewRevisionRequiresOptIn { name, version } => {
                assert_eq!(name, "fmtlib/fmt");
                assert_eq!(version, "10.2.1");
            }
            other => panic!("expected NewRevisionRequiresOptIn, got {other:?}"),
        }
        // Original artifact still present, unchanged.
        let body = fs::read(registry_dir.path().join(format!(
            "artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1-{}.zip",
            rev_of(&s)
        )))
        .unwrap();
        assert_eq!(body, b"first");
    }

    /// Republishing byte-identical content maps onto the recorded
    /// revision: success, `no_op`, and no bytes rewritten.
    #[test]
    fn identical_republication_no_ops() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        let s = staged("fmtlib/fmt", "10.2.1", b"same bytes");
        let request = RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &s,
            new_revision: false,
        };
        let first = publish_to_registry(&request).unwrap();
        assert!(!first.no_op);
        let index_before = fs::read_to_string(&first.package_index_path).unwrap();

        let second = publish_to_registry(&request).unwrap();
        assert!(second.no_op);
        assert!(!second.registry_modified);
        assert_eq!(second.revision, rev_of(&s));
        assert_eq!(
            fs::read_to_string(&second.package_index_path).unwrap(),
            index_before
        );

        // A missing artifact file is re-placed by the no-op path.
        fs::remove_file(&first.artifact_path).unwrap();
        let healed = publish_to_registry(&request).unwrap();
        assert!(healed.no_op);
        assert!(healed.registry_modified);
        assert_eq!(fs::read(&healed.artifact_path).unwrap(), b"same bytes");

        // So is one whose bytes drifted from the recorded revision:
        // every fetch would fail checksum verification, and the
        // byte-identical republish is the natural repair.
        fs::write(&first.artifact_path, b"corrupted").unwrap();
        let healed = publish_to_registry(&request).unwrap();
        assert!(healed.no_op);
        assert!(healed.registry_modified);
        assert_eq!(fs::read(&healed.artifact_path).unwrap(), b"same bytes");
    }

    /// The `--new-revision` opt-in publishes changed bytes as a new
    /// current revision; the superseded revision's artifact and index
    /// entry stay in place.
    #[test]
    fn opt_in_publishes_a_new_revision_beside_the_old_one() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        let first = staged("fmtlib/fmt", "10.2.1", b"first");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &first,
            new_revision: false,
        })
        .unwrap();

        let second = staged("fmtlib/fmt", "10.2.1", b"second");
        let outcome = publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &second,
            new_revision: true,
        })
        .unwrap();
        assert!(!outcome.no_op);
        assert_eq!(outcome.revision, rev_of(&second));

        let body =
            fs::read_to_string(registry_dir.path().join("packages/fmtlib/fmt.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let entry = &value["versions"]["10.2.1"];
        assert_eq!(entry["revision"], rev_of(&second));
        let revisions = entry["revisions"].as_object().unwrap();
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[rev_of(&first)]["checksum"], first.checksum);
        assert_eq!(revisions[rev_of(&second)]["checksum"], second.checksum);
        // Both revisions' bytes remain fetchable.
        for (staged, bytes) in [(&first, b"first".as_slice()), (&second, b"second")] {
            let path = registry_dir.path().join(format!(
                "artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1-{}.zip",
                rev_of(staged)
            ));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn second_version_is_appended_not_replaced() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        let v1 = staged("fmtlib/fmt", "10.1.0", b"v1");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &v1,
            new_revision: false,
        })
        .unwrap();
        let v2 = staged("fmtlib/fmt", "10.2.1", b"v2");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &v2,
            new_revision: false,
        })
        .unwrap();
        let body =
            fs::read_to_string(registry_dir.path().join("packages/fmtlib/fmt.json")).unwrap();
        assert!(body.contains("10.1.0"));
        assert!(body.contains("10.2.1"));
        registry_dir
            .child(format!(
                "artifacts/fmtlib/fmt/fmtlib-fmt-10.1.0-{}.zip",
                rev_of(&v1)
            ))
            .assert(predicate::path::is_file());
        registry_dir
            .child(format!(
                "artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1-{}.zip",
                rev_of(&v2)
            ))
            .assert(predicate::path::is_file());
    }

    #[test]
    fn validate_publish_does_not_mutate_registry() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        let s = staged("fmtlib/fmt", "10.2.1", b"hi");
        let outcome = validate_publish(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &s,
            new_revision: false,
        })
        .unwrap();
        assert!(!outcome.registry_modified);
        assert!(outcome.registry_initialized);
        // Nothing should have been created.
        registry_dir
            .child("config.json")
            .assert(predicate::path::missing());
        registry_dir
            .child(".cabin-registry.lock")
            .assert(predicate::path::missing());
    }

    #[test]
    fn validate_publish_detects_missing_opt_in_against_existing_registry() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &staged("fmtlib/fmt", "10.2.1", b"v1"),
            new_revision: false,
        })
        .unwrap();
        let err = validate_publish(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &staged("fmtlib/fmt", "10.2.1", b"v2"),
            new_revision: false,
        })
        .unwrap_err();
        assert!(matches!(
            err,
            RegistryError::NewRevisionRequiresOptIn { .. }
        ));
        // The dry run reports the respin outcome without writing when
        // the opt-in is given.
        let outcome = validate_publish(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &staged("fmtlib/fmt", "10.2.1", b"v2"),
            new_revision: true,
        })
        .unwrap();
        assert!(!outcome.registry_modified);
        assert!(!outcome.no_op);
    }

    #[test]
    fn orphaned_artifact_is_reported() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        // Initialize registry, then drop an artifact directly (at the
        // revision-qualified path the incoming publish will target)
        // without updating the index - that's the "orphan" state.
        FileRegistry::open_or_initialize(registry_dir.path()).unwrap();
        let incoming = staged("fmtlib/fmt", "10.2.1", b"new bytes");
        registry_dir
            .child(format!(
                "artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1-{}.zip",
                rev_of(&incoming)
            ))
            .write_binary(b"orphan")
            .unwrap();

        let err = publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &incoming,
            new_revision: false,
        })
        .unwrap_err();
        assert!(matches!(err, RegistryError::OrphanedArtifact { .. }));
    }

    #[test]
    fn lock_collision_fails_clearly() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        // Pre-create the lock file.
        registry_dir.create_dir_all().unwrap();
        registry_dir
            .child(".cabin-registry.lock")
            .write_binary(b"")
            .unwrap();

        let err = publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &staged("fmtlib/fmt", "10.2.1", b"x"),
            new_revision: false,
        })
        .unwrap_err();
        assert!(matches!(err, RegistryError::Locked));
    }

    #[test]
    fn published_metadata_uses_registry_relative_source_path() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        let s = staged("fmtlib/fmt", "10.2.1", b"x");
        publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &s,
            new_revision: false,
        })
        .unwrap();
        let body =
            fs::read_to_string(registry_dir.path().join("packages/fmtlib/fmt.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let source = &value["versions"]["10.2.1"]["revisions"][rev_of(&s)]["source"];
        assert_eq!(source["type"], "archive");
        assert_eq!(source["format"], "zip");
        assert_eq!(
            source["path"],
            format!(
                "../../artifacts/fmtlib/fmt/fmtlib-fmt-10.2.1-{}.zip",
                rev_of(&s)
            )
        );
    }

    /// Registry packages are always scoped: a bare name is refused
    /// at this boundary even when the caller bypasses
    /// `cabin-publish`, and nothing is written.
    #[test]
    fn bare_names_cannot_be_published() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        for run in [publish_to_registry, validate_publish] {
            let err = run(&RegistryPublishRequest {
                registry_dir: registry_dir.path(),
                staged: &staged("fmt", "10.2.1", b"x"),
                new_revision: false,
            })
            .unwrap_err();
            match err {
                RegistryError::BarePackageName { name } => assert_eq!(name, "fmt"),
                other => panic!("expected BarePackageName, got {other:?}"),
            }
        }
        registry_dir
            .child("config.json")
            .assert(predicate::path::missing());
    }

    /// The index document location derives from the typed staged
    /// name; metadata that disagrees is refused rather than written
    /// somewhere its own `name` field contradicts.
    #[test]
    fn mismatched_metadata_name_is_refused() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        let mut s = staged("fmtlib/fmt", "10.2.1", b"x");
        s.metadata.name = "fmtlib/other".to_owned();
        let err = publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &s,
            new_revision: false,
        })
        .unwrap_err();
        assert!(matches!(
            err,
            RegistryError::StagedMetadataNameMismatch { .. }
        ));
    }

    /// The staged identity check covers the version like the name:
    /// paths derive from the typed version, the index document
    /// carries the metadata string, and a build-metadata version
    /// would wedge the index (the loader rejects such keys), so all
    /// three shapes are refused before anything is written.
    #[test]
    fn mismatched_or_build_metadata_versions_are_refused() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");

        let mut version_mismatch = staged("fmtlib/fmt", "10.2.1", b"x");
        version_mismatch.metadata.version = "10.2.2".to_owned();
        let err = publish_to_registry(&RegistryPublishRequest {
            registry_dir: registry_dir.path(),
            staged: &version_mismatch,
            new_revision: false,
        })
        .unwrap_err();
        assert!(matches!(
            err,
            RegistryError::StagedMetadataVersionMismatch { .. }
        ));

        let with_build = staged("fmtlib/fmt", "10.2.1+cabin.1", b"x");
        for outcome in [
            publish_to_registry(&RegistryPublishRequest {
                registry_dir: registry_dir.path(),
                staged: &with_build,
                new_revision: false,
            }),
            validate_publish(&RegistryPublishRequest {
                registry_dir: registry_dir.path(),
                staged: &with_build,
                new_revision: false,
            }),
        ] {
            assert!(matches!(
                outcome.unwrap_err(),
                RegistryError::VersionBuildMetadata { .. }
            ));
        }
        registry_dir
            .child("config.json")
            .assert(predicate::path::missing());
    }

    /// The revision id and index checksum derive from the staged
    /// claims, so a claim that does not name the staged archive bytes
    /// is refused before anything derives from it - a lying triple
    /// would be immutable and permanently unverifiable.  Both claim
    /// fields are checked, and the dry-run path refuses identically.
    #[test]
    fn lying_staged_checksum_is_refused() {
        let dir = TempDir::new().unwrap();
        let registry_dir = dir.child("registry");
        let honest = staged("fmtlib/fmt", "10.2.1", b"real bytes");
        let lying_claim = staged("fmtlib/fmt", "10.2.1", b"other bytes").checksum;
        let mut top_level = honest.clone();
        top_level.checksum.clone_from(&lying_claim);
        let mut in_metadata = honest.clone();
        in_metadata.metadata.checksum.clone_from(&lying_claim);
        for lying in [&top_level, &in_metadata] {
            let err = publish_to_registry(&RegistryPublishRequest {
                registry_dir: registry_dir.path(),
                staged: lying,
                new_revision: false,
            })
            .unwrap_err();
            assert!(matches!(err, RegistryError::StagedChecksumMismatch { .. }));
            let err = validate_publish(&RegistryPublishRequest {
                registry_dir: registry_dir.path(),
                staged: lying,
                new_revision: false,
            })
            .unwrap_err();
            assert!(matches!(err, RegistryError::StagedChecksumMismatch { .. }));
        }
        registry_dir
            .child("config.json")
            .assert(predicate::path::missing());
    }
}
