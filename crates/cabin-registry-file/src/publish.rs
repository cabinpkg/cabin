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
    pub checksum: cabin_core::Checksum,
    pub revision: String,
    pub no_op: bool,
}

/// Mutate the file registry: place the artifact, then update the
/// per-package index file.  Each write is an atomic rename.  If the
/// index write fails, the just-placed artifact is removed.  A crash
/// between the two renames, or a failed removal, leaves an artifact
/// the index does not record; repeating the publish then fails with
/// [`RegistryError::OrphanedArtifact`] until the file is removed.
///
/// # Errors
/// Returns [`RegistryError::Io`] if the registry directory
/// cannot be created, and [`RegistryError::Locked`] if another process
/// holds the lock.  Once locked, propagates every error from the write
/// path, including registry initialization
/// ([`RegistryError::InvalidConfig`], [`RegistryError::ConfigJson`],
/// [`RegistryError::Json`]), the revision rules
/// ([`RegistryError::NewRevisionRequiresOptIn`],
/// [`RegistryError::RevisionCollision`]),
/// [`RegistryError::OrphanedArtifact`], index parse/render failures,
/// [`RegistryError::Io`] from the atomic writes, and
/// [`RegistryError::PublishRollback`] when the index write fails and
/// removing the just-placed artifact fails too.
pub fn publish_to_registry(
    request: &RegistryPublishRequest<'_>,
) -> Result<RegistryPublishOutcome, RegistryError> {
    let registry_dir = request.registry_dir;
    fs::create_dir_all(registry_dir).map_err(|source| RegistryError::Io {
        path: registry_dir.to_path_buf(),
        source,
    })?;
    let lock = RegistryLock::acquire(registry_dir)?;
    let result = publish_locked(request);
    drop(lock);
    result
}

/// Read-only counterpart to [`publish_to_registry`]: validate every
/// pre-write check (registry config, package-index name, revision
/// rules, orphaned artifact) without writing anything.
///
/// # Errors
/// Propagates the registry-open errors of
/// [`FileRegistry::inspect`], and propagates the pre-write checks
/// (`plan_publish`): [`RegistryError::NewRevisionRequiresOptIn`],
/// [`RegistryError::RevisionCollision`],
/// [`RegistryError::OrphanedArtifact`],
/// [`RegistryError::PackageIndexInvalid`] for a non-SemVer metadata
/// version, and the existing-index read errors of [`read_optional`].
pub fn validate_publish(
    request: &RegistryPublishRequest<'_>,
) -> Result<RegistryPublishOutcome, RegistryError> {
    let registry_dir = request.registry_dir;
    let registry = FileRegistry::inspect(registry_dir)?;
    let metadata = staged_metadata_for_registry(&registry, request.staged);
    plan_publish(&registry, request, &metadata).map(|mut plan| {
        plan.outcome.registry_modified = false;
        plan.outcome
    })
}

fn publish_locked(
    request: &RegistryPublishRequest<'_>,
) -> Result<RegistryPublishOutcome, RegistryError> {
    let registry = FileRegistry::open_or_initialize(request.registry_dir)?;
    let metadata = staged_metadata_for_registry(&registry, request.staged);
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

    // Phase 2: update the index.  On failure, remove the phase-1
    // artifact: `plan_publish` found no file at that path and the
    // lock excludes other publishers, so the removal deletes only
    // this run's bytes.
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

/// Both registry paths nest at least one directory below the registry
/// root, so `parent()` is always `Some`.
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
/// [`insert_version`] (no-op / opt-in / collision), and the
/// per-revision orphaned-artifact
/// check (an artifact file present on disk for a revision the index
/// does not record is refused rather than silently overwritten).
fn plan_publish(
    registry: &FileRegistry,
    request: &RegistryPublishRequest<'_>,
    metadata: &PackageMetadata,
) -> Result<PublishPlan, RegistryError> {
    let staged = request.staged;
    // Paths derive from the *typed* staged identity; staging derives
    // the metadata from the same package, so the two agree.
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
    let revision = crate::index::revision_of(metadata).to_owned();
    let artifact_path = registry.artifact_path(&staged.name, &version, &revision);

    let existing = read_optional(&package_index_path)?;
    let (new_index, disposition) =
        insert_version(existing, metadata, &publish_stamp(), request.new_revision)?;

    if disposition == InsertDisposition::Inserted && artifact_path.exists() {
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
) -> PackageMetadata {
    let mut metadata = staged.metadata.clone();
    let revision = crate::index::revision_of(&metadata).to_owned();
    metadata.source.path = registry.relative_source_path(&staged.name, &staged.version, &revision);
    metadata
}

/// UTC `YYYY-MM-DDTHH:MM:SSZ` stamp for a freshly published
/// revision.  Publish time is genuinely new information, so this is
/// the one wall-clock read in the crate; everything else in the
/// written registry stays a pure function of its inputs.
fn publish_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, tod) = (secs / 86_400, secs % 86_400);
    // Civil-from-days (Howard Hinnant's algorithm), valid for every
    // date this code will ever stamp.
    let (year, month, day) = {
        let z = i64::try_from(days).unwrap_or(0) + 719_468;
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
        staged.checksum.revision_id()
    }

    fn staged(name: &str, version: &str, body: &[u8]) -> StagedPackage {
        let checksum = cabin_core::Checksum::of_bytes(body);
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
                links: Default::default(),
                upstream: None,
                yanked: false,
                checksum,
                // `staged_metadata_for_registry` overrides this.
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
        // Lock released on success: a fresh acquire succeeds.
        RegistryLock::acquire(registry_dir.path()).unwrap();
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
        assert_eq!(
            revisions[rev_of(&first)]["checksum"],
            first.checksum.as_str()
        );
        assert_eq!(
            revisions[rev_of(&second)]["checksum"],
            second.checksum.as_str()
        );
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
        // Hold the lock the way a concurrent publish would.
        let _held = RegistryLock::acquire(registry_dir.path()).unwrap();

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
}
