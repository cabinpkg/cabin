use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use cabin_core::{PackageName, StandardsMetadata};
use cabin_package::PackageMetadata;
use serde::{Deserialize, Serialize};

use crate::error::RegistryError;
use crate::layout::FileRegistry;

/// Schema version of a package index file whose revisions all resolve
/// as their version does.
pub const PACKAGE_INDEX_SCHEMA: u32 = 1;

/// Schema version of a package index file in which some revision
/// carries its own `resolution`; readers that predate it refuse the
/// file instead of resolving that revision with the current metadata.
const REVISION_RESOLUTION_SCHEMA: u32 = 2;

/// Read and parse the package index at `path`
/// (`<registry>/packages/<name>.json`).  Returns `Ok(None)` when the
/// file does not exist (a fresh package).
///
/// # Errors
/// Returns [`RegistryError::Io`] when reading fails for any reason
/// other than the file not existing,
/// [`RegistryError::PackageIndexJson`] when its contents are not
/// valid package-index JSON, and
/// [`RegistryError::PackageIndexUnsupportedSchema`] when the parsed
/// schema is neither [`PACKAGE_INDEX_SCHEMA`] nor the
/// per-revision-resolution schema.
pub fn read_optional(path: &Path) -> Result<Option<PackageIndex>, RegistryError> {
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(RegistryError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let index: PackageIndex =
        serde_json::from_str(&body).map_err(|error| RegistryError::PackageIndexJson {
            path: path.to_path_buf(),
            error,
        })?;
    if index.schema != PACKAGE_INDEX_SCHEMA && index.schema != REVISION_RESOLUTION_SCHEMA {
        return Err(RegistryError::PackageIndexUnsupportedSchema {
            path: path.to_path_buf(),
            schema: index.schema,
        });
    }
    Ok(Some(index))
}

/// Render `index` as deterministic, pretty-printed JSON with a
/// trailing newline.  `path` is the index file's on-disk location,
/// used only for error context.
///
/// `versions` is serialized in SemVer-ascending order for human
/// readers.  The on-disk shape matches what `cabin-index` reads back.
///
/// # Errors
/// Returns [`RegistryError::PackageIndexInvalid`] when a version key in
/// `index` is not valid `SemVer`, and [`RegistryError::Json`] when
/// serializing the document to JSON fails.
pub fn render(index: &PackageIndex, path: &Path) -> Result<String, RegistryError> {
    // `index.versions` iterates lexicographically ("10.x" < "9.x").
    // Re-insert in SemVer order into a `serde_json::Map`, which keeps
    // insertion order under the workspace's `preserve_order` feature.
    let mut versions: Vec<(semver::Version, &serde_json::Value)> = index
        .versions
        .iter()
        .map(|(k, v)| {
            let parsed =
                semver::Version::parse(k).map_err(|err| RegistryError::PackageIndexInvalid {
                    path: path.to_path_buf(),
                    message: format!("version key {k:?} is not valid SemVer: {err}"),
                })?;
            Ok((parsed, v))
        })
        .collect::<Result<_, RegistryError>>()?;
    versions.sort_by(|a, b| a.0.cmp(&b.0));
    let mut versions_obj = serde_json::Map::new();
    for (ver, value) in versions {
        versions_obj.insert(ver.to_string(), value.clone());
    }
    let document = serde_json::json!({
        "schema": index.schema,
        "name": index.name,
        "versions": serde_json::Value::Object(versions_obj),
    });
    let mut body = serde_json::to_string_pretty(&document)?;
    body.push('\n');
    Ok(body)
}

/// Read the already-published versions and their declared
/// standard-compatibility tables for `name` from the file registry at
/// `registry_dir` - the PL3 publish-lint baseline.
///
/// Returns an empty vector when the registry or the package has no
/// index yet (a first publish).  A version entry with no `standards`
/// field yields an empty table (absence = unconstrained), so
/// pre-`standards` entries compare as an all-unconstrained baseline.
/// Reads exactly the `<registry>/packages/<name>.json` the publish
/// path splices into, so the lint sees the same versions the write
/// will.
///
/// # Errors
/// Propagates [`RegistryError`] from opening the registry config
/// ([`FileRegistry::inspect`]) and reading/parsing the package index
/// ([`read_optional`]), returns [`RegistryError::PackageIndexInvalid`]
/// when a version key is not valid `SemVer`, and
/// [`RegistryError::PackageIndexJson`] when a version's `standards`
/// value is not a valid table.
pub fn read_published_standards(
    registry_dir: &Path,
    name: &PackageName,
) -> Result<Vec<(semver::Version, StandardsMetadata)>, RegistryError> {
    let registry = FileRegistry::inspect(registry_dir)?;
    let path = registry.package_index_path(name);
    let Some(index) = read_optional(&path)? else {
        return Ok(Vec::new());
    };
    let mut published = Vec::with_capacity(index.versions.len());
    for (version, value) in &index.versions {
        let version =
            semver::Version::parse(version).map_err(|err| RegistryError::PackageIndexInvalid {
                path: path.clone(),
                message: format!("version key {version:?} is not valid SemVer: {err}"),
            })?;
        let standards = match value.get("standards") {
            Some(standards) => serde_json::from_value::<StandardsMetadata>(standards.clone())
                .map_err(|error| RegistryError::PackageIndexJson {
                    path: path.clone(),
                    error,
                })?,
            None => StandardsMetadata::default(),
        };
        published.push((version, standards));
    }
    Ok(published)
}

/// What the publish path's index insertion decided about the
/// incoming revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertDisposition {
    /// The version (or the revision) did not exist before; the
    /// returned index carries it and must be written.
    Inserted,
    /// The exact revision is already recorded with the same
    /// checksum; the returned index is the unmodified input and
    /// nothing needs writing.
    NoOp,
}

/// The packaging-revision id a checksum claim mints.  Infallible:
/// the typed [`cabin_core::Checksum`] cannot hold a malformed claim.
pub(crate) fn revision_of(metadata: &PackageMetadata) -> &str {
    metadata.checksum.revision_id()
}

/// Insert `metadata` as a packaging revision of its version into
/// `existing` (or build a fresh index if `existing` is `None`),
/// stamping `published_at` on the new revision.
///
/// Revision semantics: a byte-identical republication is a no-op;
/// different bytes for an existing version require
/// `allow_new_revision` (the `--new-revision` opt-in).  The new
/// revision becomes the version's current one (a file registry has no
/// verification lifecycle), and its resolution metadata the version's;
/// superseded revisions stay listed and fetchable, each resolving as
/// it was published.
pub(crate) fn insert_version(
    existing: Option<PackageIndex>,
    metadata: &PackageMetadata,
    published_at: &str,
    allow_new_revision: bool,
) -> Result<(PackageIndex, InsertDisposition), RegistryError> {
    let revision = revision_of(metadata).to_owned();
    let mut index = match existing {
        Some(index) => {
            if index.name != metadata.name {
                return Err(RegistryError::PackageIndexNameMismatch {
                    name: metadata.name.clone(),
                    actual_name: index.name,
                });
            }
            index
        }
        None => PackageIndex {
            schema: PACKAGE_INDEX_SCHEMA,
            name: metadata.name.clone(),
            versions: BTreeMap::new(),
        },
    };

    let previous_revisions = match index.versions.get(&metadata.version) {
        None => serde_json::Map::new(),
        Some(entry) => {
            let revisions = entry
                .get("revisions")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| RegistryError::PackageIndexInvalid {
                    path: PathBuf::from(format!("packages/{}.json", metadata.name)),
                    message: format!("version {:?} carries no `revisions` map", metadata.version),
                })?;
            if let Some(prior) = revisions.get(revision.as_str()) {
                let prior_checksum = prior.get("checksum").and_then(serde_json::Value::as_str);
                if prior_checksum == Some(metadata.checksum.as_str()) {
                    // Byte-identical republication maps onto the
                    // existing revision.
                    return Ok((index, InsertDisposition::NoOp));
                }
                // Different bytes whose digests share the revision
                // prefix: astronomically unlikely, and silently
                // replacing either side would break immutability -
                // fail loudly.
                return Err(RegistryError::RevisionCollision {
                    name: metadata.name.clone(),
                    version: metadata.version.clone(),
                    revision,
                });
            }
            if !allow_new_revision {
                return Err(RegistryError::NewRevisionRequiresOptIn {
                    name: metadata.name.clone(),
                    version: metadata.version.clone(),
                });
            }
            let mut revisions = revisions.clone();
            keep_superseded_resolution(entry, metadata, &mut revisions)?;
            revisions
        }
    };

    // `yanked` is version-level registry state the staged metadata
    // knows nothing about (staging always writes `false`), so a
    // respin carries the recorded value forward - `--new-revision`
    // must never quietly un-yank a version.
    let yanked = index
        .versions
        .get(&metadata.version)
        .and_then(|entry| entry.get("yanked"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(metadata.yanked);
    let value = version_value_from_metadata(
        metadata,
        &revision,
        published_at,
        previous_revisions,
        yanked,
    )?;
    index.versions.insert(metadata.version.clone(), value);
    if index.versions.values().any(has_revision_resolution) {
        index.schema = REVISION_RESOLUTION_SCHEMA;
    }
    Ok((index, InsertDisposition::Inserted))
}

fn has_revision_resolution(entry: &serde_json::Value) -> bool {
    entry
        .get("revisions")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|revisions| {
            revisions
                .values()
                .any(|revision| revision.get("resolution").is_some())
        })
}

/// A respin that changes what resolution reads must not change how the
/// revisions before it resolve: each one still resolving as the version
/// does (no `resolution` of its own) records the version's current
/// resolution fields before the new revision's replace them.
fn keep_superseded_resolution(
    entry: &serde_json::Value,
    metadata: &PackageMetadata,
    revisions: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), RegistryError> {
    let mut current = serde_json::Map::new();
    let mut incoming = serde_json::Map::new();
    for (field, value) in resolution_fields(metadata)? {
        if let Some(stored) = entry.get(field) {
            current.insert(field.to_owned(), stored.clone());
        }
        if !value.is_null() {
            incoming.insert(field.to_owned(), value);
        }
    }
    if current == incoming {
        return Ok(());
    }
    for revision in revisions.values_mut() {
        if let Some(revision) = revision.as_object_mut()
            && revision
                .get("resolution")
                .is_none_or(serde_json::Value::is_null)
        {
            revision.insert(
                "resolution".to_owned(),
                serde_json::Value::Object(current.clone()),
            );
        }
    }
    Ok(())
}

/// The resolution fields of a metadata document, in the exact wire
/// encoding [`version_value_from_metadata`] emits (an
/// omitted-when-empty field projects as `Null`, matching a missing key
/// on the stored entry).
fn resolution_fields(
    metadata: &PackageMetadata,
) -> Result<[(&'static str, serde_json::Value); 4], RegistryError> {
    let dependencies = serde_json::to_value(&metadata.dependencies)?;
    let features = (!metadata.features.default.is_empty()
        || !metadata.features.features.is_empty())
    .then(|| serde_json::to_value(&metadata.features))
    .transpose()?
    .unwrap_or(serde_json::Value::Null);
    let standards = (!metadata.standards.is_empty())
        .then(|| serde_json::to_value(&metadata.standards))
        .transpose()?
        .unwrap_or(serde_json::Value::Null);
    let links = (!metadata.links.is_empty())
        .then(|| serde_json::to_value(&metadata.links))
        .transpose()?
        .unwrap_or(serde_json::Value::Null);
    Ok([
        ("dependencies", dependencies),
        ("features", features),
        ("standards", standards),
        ("links", links),
    ])
}

/// In-memory representation of one `<registry>/packages/<name>.json`
/// file.  The `versions` map keeps each version's payload as an
/// opaque [`serde_json::Value`] so the registry crate doesn't have
/// to mirror every `cabin-package` metadata field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageIndex {
    pub schema: u32,
    pub name: String,
    #[serde(default)]
    pub versions: BTreeMap<String, serde_json::Value>,
}

/// The per-version document written into `packages/<name>.json`,
/// projected from a [`PackageMetadata`].  The struct fixes the
/// published field set: a field added to [`PackageMetadata`] is not
/// published until it is added here.
///
/// Field declaration order is the wire order; `serde_json`'s
/// `preserve_order` keeps it.  The optional blocks are emitted only
/// when non-empty, matching the shape older readers and existing
/// fixtures expect for packages without that metadata.
///
/// `dev_dependencies` and `system_dependencies` are not projected: the
/// version document carries only the resolution-relevant
/// `dependencies`.  `cabin-index` reads an absent `dev-dependencies` or
/// `system-dependencies` key as empty.
#[derive(Serialize)]
struct IndexVersionWire<'a, D: Serialize> {
    dependencies: &'a D,
    yanked: bool,
    /// Current packaging revision (in a file registry: the one
    /// published last; there is no verification lifecycle).
    revision: &'a str,
    /// Every published revision, the current one included.  The map
    /// value is carried opaquely, so a recorded revision is rewritten
    /// unchanged except for the `resolution` that
    /// `keep_superseded_resolution` may add.
    revisions: serde_json::Map<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    features: Option<&'a cabin_core::Features>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profiles: Option<
        &'a std::collections::BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
    >,
    #[serde(skip_serializing_if = "Option::is_none")]
    toolchain: Option<&'a cabin_core::ToolchainSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<&'a cabin_core::ProfileSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiler_wrapper: Option<&'a cabin_core::CompilerWrapperRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a cabin_core::LanguageStandardSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    standards: Option<&'a cabin_core::StandardsMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    links: Option<&'a std::collections::BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    upstream: Option<&'a cabin_core::UpstreamProvenance>,
}

#[derive(Serialize)]
struct IndexSourceWire<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    path: &'a str,
    format: &'a str,
}

/// One entry of a version's `revisions` map as this writer emits it.
#[derive(Serialize)]
struct RevisionWire<'a> {
    checksum: &'a str,
    #[serde(rename = "published-at")]
    published_at: &'a str,
    source: IndexSourceWire<'a>,
}

fn version_value_from_metadata(
    metadata: &PackageMetadata,
    revision: &str,
    published_at: &str,
    mut revisions: serde_json::Map<String, serde_json::Value>,
    yanked: bool,
) -> Result<serde_json::Value, RegistryError> {
    revisions.insert(
        revision.to_owned(),
        serde_json::to_value(RevisionWire {
            checksum: metadata.checksum.as_str(),
            published_at,
            source: IndexSourceWire {
                kind: &metadata.source.kind,
                path: &metadata.source.path,
                format: &metadata.source.format,
            },
        })?,
    );
    let wire = IndexVersionWire {
        dependencies: &metadata.dependencies,
        yanked,
        revision,
        revisions,
        features: (!metadata.features.default.is_empty() || !metadata.features.features.is_empty())
            .then_some(&metadata.features),
        profiles: (!metadata.profiles.is_empty()).then_some(&metadata.profiles),
        toolchain: (!metadata.toolchain.is_empty()).then_some(&metadata.toolchain),
        build: (!metadata.build.is_empty()).then_some(&metadata.build),
        compiler_wrapper: metadata.compiler_wrapper.as_ref(),
        language: (!metadata.language.is_empty()).then_some(&metadata.language),
        standards: (!metadata.standards.is_empty()).then_some(&metadata.standards),
        links: (!metadata.links.is_empty()).then_some(&metadata.links),
        upstream: metadata.upstream.as_ref(),
    };
    Ok(serde_json::to_value(&wire)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_package::SourceMetadata;
    use std::collections::BTreeMap;

    /// `StandardsMetadata` denies unknown fields, so a registry-authored
    /// index file picks the key that is quoted back in the parse error.
    #[test]
    fn published_standards_parse_errors_cannot_smuggle_terminal_escapes() {
        let dir = assert_fs::TempDir::new().unwrap();
        let index = dir.path().join("packages/demo.json");
        fs::create_dir_all(index.parent().unwrap()).unwrap();
        fs::write(
            &index,
            r#"{"schema":1,"name":"demo","versions":{"1.0.0":{"standards":{"\u001b[2Khidden":1}}}}"#,
        )
        .unwrap();
        let name = PackageName::new("demo").unwrap();
        let rendered = read_published_standards(dir.path(), &name)
            .unwrap_err()
            .to_string();
        assert!(
            rendered.contains(r"\u{1b}[2Khidden"),
            "not escaped: {rendered}"
        );
        assert!(!rendered.contains('\u{1b}'), "not escaped: {rendered}");
    }

    /// Only a missing index means a fresh package; any other read
    /// failure must surface instead of reading as one.  Unix-only
    /// because the provoked error is `ENOTDIR`.
    #[cfg(unix)]
    #[test]
    fn read_optional_propagates_errors_other_than_not_found() {
        let dir = assert_fs::TempDir::new().unwrap();
        let file = dir.path().join("packages");
        fs::write(&file, "").unwrap();
        let err = read_optional(&file.join("demo.json")).unwrap_err();
        assert!(matches!(err, RegistryError::Io { .. }), "{err:?}");
        assert_eq!(
            read_optional(&dir.path().join("absent.json")).unwrap(),
            None
        );
    }

    const STAMP: &str = "2026-01-01T00:00:00Z";

    /// Metadata whose checksum is derived from `seed`, so two calls
    /// with different seeds model different archive bytes (distinct
    /// revisions) and equal seeds model byte-identical ones.
    fn metadata_with_bytes(name: &str, version: &str, seed: char) -> PackageMetadata {
        let hex: String = std::iter::repeat_n(seed, 64).collect();
        let revision = &hex[..16];
        PackageMetadata {
            schema: PACKAGE_INDEX_SCHEMA,
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
            checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
            source: SourceMetadata {
                kind: "archive".to_owned(),
                path: format!("../artifacts/{name}/{name}-{version}-{revision}.zip"),
                format: "zip".to_owned(),
            },
        }
    }

    fn metadata(name: &str, version: &str) -> PackageMetadata {
        metadata_with_bytes(name, version, 'a')
    }

    fn insert_new(
        existing: Option<PackageIndex>,
        metadata: &PackageMetadata,
    ) -> Result<PackageIndex, RegistryError> {
        let (index, disposition) = insert_version(existing, metadata, STAMP, false)?;
        assert_eq!(disposition, InsertDisposition::Inserted);
        Ok(index)
    }

    #[test]
    fn creates_new_index_from_first_version() {
        let meta = metadata("fmt", "10.2.1");
        let index = insert_new(None, &meta).unwrap();
        assert_eq!(index.schema, 1);
        assert_eq!(index.name, "fmt");
        let entry = &index.versions["10.2.1"];
        assert_eq!(entry["revision"], "aaaaaaaaaaaaaaaa");
        let revision = &entry["revisions"]["aaaaaaaaaaaaaaaa"];
        assert_eq!(revision["checksum"], meta.checksum.as_str());
        assert_eq!(revision["published-at"], STAMP);
        assert_eq!(revision["source"]["path"], meta.source.path);
    }

    #[test]
    fn appends_new_version_to_existing_index() {
        let initial = insert_new(None, &metadata("fmt", "10.1.0")).unwrap();
        let updated = insert_new(Some(initial), &metadata("fmt", "10.2.1")).unwrap();
        assert_eq!(updated.versions.len(), 2);
        assert!(updated.versions.contains_key("10.1.0"));
        assert!(updated.versions.contains_key("10.2.1"));
    }

    /// Byte-identical republication maps onto the recorded revision
    /// and changes nothing.
    #[test]
    fn identical_republication_is_a_no_op() {
        let initial = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let (index, disposition) = insert_version(
            Some(initial.clone()),
            &metadata("fmt", "10.2.1"),
            STAMP,
            false,
        )
        .unwrap();
        assert_eq!(disposition, InsertDisposition::NoOp);
        assert_eq!(index, initial);
        // The opt-in flag makes no difference to a byte-identical
        // republication.
        let (_, disposition) =
            insert_version(Some(index), &metadata("fmt", "10.2.1"), STAMP, true).unwrap();
        assert_eq!(disposition, InsertDisposition::NoOp);
    }

    /// `yanked` is version-level registry state, not staged
    /// metadata: a respin (whose staged document always carries
    /// `yanked: false`) must not quietly un-yank the version.
    #[test]
    fn a_respin_preserves_the_versions_yanked_state() {
        let mut initial = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let entry = initial.versions.get_mut("10.2.1").unwrap();
        entry["yanked"] = serde_json::Value::Bool(true);
        let (index, disposition) = insert_version(
            Some(initial),
            &metadata_with_bytes("fmt", "10.2.1", 'b'),
            STAMP,
            true,
        )
        .unwrap();
        assert_eq!(disposition, InsertDisposition::Inserted);
        assert_eq!(index.versions["10.2.1"]["yanked"], true);
        assert_eq!(
            index.versions["10.2.1"]["revisions"]
                .as_object()
                .unwrap()
                .len(),
            2,
            "the respin itself must still land"
        );
    }

    /// Different bytes for a published version demand the explicit
    /// `--new-revision` opt-in; the diagnostic explains the
    /// mechanism.
    #[test]
    fn different_bytes_require_the_new_revision_opt_in() {
        let initial = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let err = insert_version(
            Some(initial),
            &metadata_with_bytes("fmt", "10.2.1", 'b'),
            STAMP,
            false,
        )
        .unwrap_err();
        assert!(
            matches!(&err, RegistryError::NewRevisionRequiresOptIn { name, version }
                if name == "fmt" && version == "10.2.1"),
            "{err}"
        );
        assert!(err.to_string().contains("--new-revision"), "{err}");
    }

    /// With the opt-in, changed bytes become a new packaging revision
    /// that supersedes the old one; the superseded revision stays
    /// recorded.
    #[test]
    fn opt_in_appends_a_new_current_revision() {
        let initial = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let (index, disposition) = insert_version(
            Some(initial),
            &metadata_with_bytes("fmt", "10.2.1", 'b'),
            STAMP,
            true,
        )
        .unwrap();
        assert_eq!(disposition, InsertDisposition::Inserted);
        let entry = &index.versions["10.2.1"];
        assert_eq!(entry["revision"], "bbbbbbbbbbbbbbbb");
        let revisions = entry["revisions"].as_object().unwrap();
        assert_eq!(revisions.len(), 2);
        assert!(revisions.contains_key("aaaaaaaaaaaaaaaa"));
        assert!(revisions.contains_key("bbbbbbbbbbbbbbbb"));
    }

    /// Two different archives whose digests share the 16-hex revision
    /// prefix cannot coexist; the writer fails loudly instead of
    /// replacing either side.
    #[test]
    fn shared_revision_prefix_with_different_bytes_is_a_collision() {
        let initial = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let mut colliding = metadata("fmt", "10.2.1");
        colliding.checksum =
            cabin_core::Checksum::parse(&format!("sha256:{}{}", "a".repeat(16), "c".repeat(48)))
                .unwrap();
        let err = insert_version(Some(initial), &colliding, STAMP, true).unwrap_err();
        assert!(
            matches!(&err, RegistryError::RevisionCollision { revision, .. }
                if revision == "aaaaaaaaaaaaaaaa"),
            "{err}"
        );
    }

    /// A respin may change what resolution reads; every revision before
    /// it keeps resolving as it was published, recorded the first time
    /// the version's metadata moves on, and the file switches to the
    /// schema that tells older readers to refuse it.
    #[test]
    fn respins_keep_superseded_revisions_resolution() {
        use cabin_package::metadata::PackageDependencyEntry;
        let initial = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let unchanged = insert_version(
            Some(initial.clone()),
            &metadata_with_bytes("fmt", "10.2.1", 'b'),
            STAMP,
            true,
        )
        .unwrap()
        .0;
        assert_eq!(unchanged.schema, PACKAGE_INDEX_SCHEMA);
        assert!(!has_revision_resolution(&unchanged.versions["10.2.1"]));

        let mut with_dep = metadata_with_bytes("fmt", "10.2.1", 'b');
        with_dep.dependencies.insert(
            "acme/dep".to_owned(),
            PackageDependencyEntry::Bare("^1".to_owned()),
        );
        let (index, _) = insert_version(Some(initial), &with_dep, STAMP, true).unwrap();
        assert_eq!(index.schema, REVISION_RESOLUTION_SCHEMA);
        let entry = &index.versions["10.2.1"];
        assert_eq!(entry["dependencies"]["acme/dep"], "^1");
        let first = entry["revisions"]["aaaaaaaaaaaaaaaa"].clone();
        assert_eq!(
            first["resolution"],
            serde_json::json!({ "dependencies": {} })
        );
        assert!(
            entry["revisions"]["bbbbbbbbbbbbbbbb"]
                .get("resolution")
                .is_none()
        );

        let mut with_links = metadata_with_bytes("fmt", "10.2.1", 'c');
        with_links.links = BTreeMap::from([("fmt".to_owned(), "fmt".to_owned())]);
        let (index, _) = insert_version(Some(index), &with_links, STAMP, true).unwrap();
        let entry = &index.versions["10.2.1"];
        assert_eq!(entry["dependencies"], serde_json::json!({}));
        assert_eq!(entry["links"]["fmt"], "fmt");
        let revisions = &entry["revisions"];
        assert_eq!(revisions["aaaaaaaaaaaaaaaa"], first);
        assert_eq!(
            revisions["bbbbbbbbbbbbbbbb"]["resolution"],
            serde_json::json!({ "dependencies": { "acme/dep": "^1" } })
        );
        assert!(revisions["cccccccccccccccc"].get("resolution").is_none());
    }

    #[test]
    fn name_mismatch_fails() {
        let initial = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let err = insert_new(Some(initial), &metadata("spdlog", "1.13.0")).unwrap_err();
        assert!(matches!(
            err,
            RegistryError::PackageIndexNameMismatch { .. }
        ));
    }

    #[test]
    fn render_is_deterministic() {
        let first = insert_new(None, &metadata("fmt", "10.2.1"))
            .expect("insert_version failed during test setup");
        let index = insert_new(Some(first), &metadata("fmt", "10.1.0")).unwrap();
        let a = render(&index, Path::new("packages/fmt.json")).unwrap();
        let b = render(&index, Path::new("packages/fmt.json")).unwrap();
        assert_eq!(a, b);
        assert!(a.ends_with('\n'));
    }

    #[test]
    fn render_orders_versions_by_semver() {
        let first = insert_new(None, &metadata("fmt", "9.9.9"))
            .expect("insert_version failed during test setup");
        let second = insert_new(Some(first), &metadata("fmt", "10.1.0"))
            .expect("insert_version failed during test setup");
        let index = insert_new(Some(second), &metadata("fmt", "10.2.1")).unwrap();
        let body = render(&index, Path::new("packages/fmt.json")).unwrap();
        let pos_9 = body.find("\"9.9.9\"").unwrap();
        let pos_101 = body.find("\"10.1.0\"").unwrap();
        let pos_102 = body.find("\"10.2.1\"").unwrap();
        // SemVer order; lexicographic order would put "10.x" before
        // "9.9.9".
        assert!(pos_9 < pos_101);
        assert!(pos_101 < pos_102);
    }

    #[test]
    fn render_round_trips() {
        let index = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let body = render(&index, Path::new("packages/fmt.json")).unwrap();
        let parsed: PackageIndex = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, index);
    }

    /// A populated `standards` table is projected into the version
    /// document in the documented wire shape: `"none"` for forbidden,
    /// `{min}` for minima (an absent `max` omitted), unconstrained
    /// language keys omitted, and the two per-target flags emitted only
    /// when set.
    #[test]
    fn render_projects_standards_table() {
        use cabin_core::{CStandard, CxxStandard, Requirement, StandardsMetadata, TargetStandards};
        let mut meta = metadata("fmt", "10.2.1");
        let mut targets = BTreeMap::new();
        targets.insert(
            "fmt".to_owned(),
            TargetStandards {
                header_only: false,
                gnu_extensions: false,
                interface_c: Requirement::Forbidden,
                interface_cxx: Requirement::Min(CxxStandard::Cxx17),
            },
        );
        targets.insert(
            "clib".to_owned(),
            TargetStandards {
                header_only: false,
                gnu_extensions: true,
                interface_c: Requirement::Min(CStandard::C11),
                interface_cxx: Requirement::Unconstrained,
            },
        );
        meta.standards = StandardsMetadata { targets };

        let index = insert_new(None, &meta).unwrap();
        let body = render(&index, Path::new("packages/fmt.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let standards = &value["versions"]["10.2.1"]["standards"]["targets"];

        assert_eq!(standards["fmt"]["interface"]["c"], "none");
        assert_eq!(standards["fmt"]["interface"]["c++"]["min"], "c++17");
        assert!(standards["fmt"]["interface"]["c++"].get("max").is_none());
        assert!(standards["fmt"].get("header-only").is_none());

        assert_eq!(standards["clib"]["gnu-extensions"], true);
        assert_eq!(standards["clib"]["interface"]["c"]["min"], "c11");
        assert!(standards["clib"]["interface"].get("c++").is_none());
    }

    /// A package with no library-like standards omits the field, so
    /// existing entries stay byte-identical.
    #[test]
    fn render_omits_empty_standards() {
        let index = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let body = render(&index, Path::new("packages/fmt.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(value["versions"]["10.2.1"].get("standards").is_none());
    }

    /// A populated `links` table is projected into the version
    /// document target-keyed and sorted; a package without claims
    /// omits the field so existing entries stay byte-identical.
    #[test]
    fn render_projects_links_and_omits_empty() {
        let mut meta = metadata("zlib", "1.3.1");
        meta.links = BTreeMap::from([("z".to_owned(), "z".to_owned())]);
        let index = insert_new(None, &meta).unwrap();
        let body = render(&index, Path::new("packages/zlib.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["versions"]["1.3.1"]["links"]["z"], "z");

        let bare = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let body = render(&bare, Path::new("packages/fmt.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(value["versions"]["10.2.1"].get("links").is_none());
    }

    #[test]
    fn render_projects_upstream_provenance() {
        let sha = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23";
        let mut meta = metadata("fmt", "10.2.1");
        meta.upstream = Some(
            cabin_core::UpstreamProvenance::new(
                "https://example.com/fmt-10.2.1.tar.gz",
                &format!("sha256:{sha}"),
                "tar.gz",
                Some("fmt-10.2.1".to_owned()),
                vec![
                    cabin_core::UpstreamCopy::new("support/config.h.in".into(), "config.h".into())
                        .unwrap(),
                ],
                Vec::new(),
            )
            .unwrap(),
        );

        let index = insert_new(None, &meta).unwrap();
        let body = render(&index, Path::new("packages/fmt.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let upstream = &value["versions"]["10.2.1"]["upstream"];
        assert_eq!(upstream["url"], "https://example.com/fmt-10.2.1.tar.gz");
        assert_eq!(upstream["checksum"], format!("sha256:{sha}"));
        assert_eq!(upstream["format"], "tar.gz");
        assert_eq!(upstream["strip-prefix"], "fmt-10.2.1");
        assert_eq!(upstream["copy"][0]["from"], "support/config.h.in");
        assert_eq!(upstream["copy"][0]["to"], "config.h");
    }

    /// A package without provenance omits the field, so existing
    /// entries stay byte-identical.
    #[test]
    fn render_omits_absent_upstream() {
        let index = insert_new(None, &metadata("fmt", "10.2.1")).unwrap();
        let body = render(&index, Path::new("packages/fmt.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(value["versions"]["10.2.1"].get("upstream").is_none());
    }
}
