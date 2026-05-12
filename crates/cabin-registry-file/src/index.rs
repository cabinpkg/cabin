use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use cabin_package::PackageMetadata;
use serde::{Deserialize, Serialize};

use crate::error::RegistryError;

/// Schema version this crate emits and accepts in package index
/// files. Matches the index shape.
pub const PACKAGE_INDEX_SCHEMA: u32 = 1;

/// Read `<registry>/packages/<name>.json`, plus return the parsed
/// document. Returns `Ok(None)` when the file does not exist (a
/// fresh package).
pub fn read_optional(path: &Path) -> Result<Option<PackageIndex>, RegistryError> {
    if !path.exists() {
        return Ok(None);
    }
    let body = fs::read_to_string(path).map_err(|source| RegistryError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let index: PackageIndex =
        serde_json::from_str(&body).map_err(|source| RegistryError::PackageIndexJson {
            path: path.to_path_buf(),
            source,
        })?;
    if index.schema != PACKAGE_INDEX_SCHEMA {
        return Err(RegistryError::PackageIndexUnsupportedSchema {
            path: path.to_path_buf(),
            schema: index.schema,
        });
    }
    Ok(Some(index))
}

/// Render `index` as deterministic, pretty-printed JSON with a
/// trailing newline.
///
/// `versions` is serialised in **SemVer-ascending** order so existing
/// versions stay grouped together for human readers, regardless of
/// what order they were inserted in. The on-disk shape matches what
/// `cabin-index` reads back.
pub fn render(index: &PackageIndex) -> Result<String, RegistryError> {
    // Build the JSON value by hand so we can pin version order. A
    // plain `serde_json::Map` would sort keys lexicographically,
    // which makes "10.x" < "9.x" — confusing for humans.
    let mut versions: Vec<(semver::Version, &serde_json::Value)> = index
        .versions
        .iter()
        .map(|(k, v)| {
            let parsed =
                semver::Version::parse(k).map_err(|err| RegistryError::PackageIndexInvalid {
                    path: std::path::PathBuf::new(),
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

/// Insert `metadata` as a new version into `existing` (or build a
/// fresh index if `existing` is `None`). Errors out on duplicate
/// versions and on package-name mismatches.
pub(crate) fn insert_version(
    existing: Option<PackageIndex>,
    metadata: &PackageMetadata,
) -> Result<PackageIndex, RegistryError> {
    let value = version_value_from_metadata(metadata)?;
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
    if index.versions.contains_key(&metadata.version) {
        return Err(RegistryError::DuplicateVersion {
            name: metadata.name.clone(),
            version: metadata.version.clone(),
        });
    }
    index.versions.insert(metadata.version.clone(), value);
    Ok(index)
}

/// In-memory representation of one `<registry>/packages/<name>.json`
/// File. The `versions` map keeps each version's payload as an
/// opaque [`serde_json::Value`] so the registry crate doesn't have
/// to mirror every `cabin-package` metadata field; callers feed in
/// new versions via `insert_version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageIndex {
    pub schema: u32,
    pub name: String,
    #[serde(default)]
    pub versions: BTreeMap<String, serde_json::Value>,
}

fn version_value_from_metadata(
    metadata: &PackageMetadata,
) -> Result<serde_json::Value, RegistryError> {
    let mut value = serde_json::json!({
        "dependencies": metadata.dependencies,
        "yanked": metadata.yanked,
        "checksum": metadata.checksum,
        "source": {
            "type": metadata.source.kind,
            "path": metadata.source.path,
            "format": metadata.source.format,
        },
    });
    let obj = value
        .as_object_mut()
        .expect("object literal is always Value::Object");
    // Include feature declarations only when the package actually
    // declared them so existing test fixtures and older readers see
    // the same JSON shape they always have when the package has no
    // feature metadata.
    if !metadata.features.default.is_empty() || !metadata.features.features.is_empty() {
        obj.insert(
            "features".to_owned(),
            serde_json::to_value(&metadata.features)?,
        );
    }
    if !metadata.profiles.is_empty() {
        obj.insert(
            "profiles".to_owned(),
            serde_json::to_value(&metadata.profiles)?,
        );
    }
    if !metadata.toolchain.is_empty() {
        obj.insert(
            "toolchain".to_owned(),
            serde_json::to_value(&metadata.toolchain)?,
        );
    }
    if !metadata.build.is_empty() {
        obj.insert("build".to_owned(), serde_json::to_value(&metadata.build)?);
    }
    if !metadata.compiler_wrapper.is_empty() {
        obj.insert(
            "compiler_wrapper".to_owned(),
            serde_json::to_value(&metadata.compiler_wrapper)?,
        );
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_package::SourceMetadata;
    use std::collections::BTreeMap;

    fn metadata(name: &str, version: &str) -> PackageMetadata {
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
            yanked: false,
            checksum: format!("sha256:{}", "a".repeat(64)),
            source: SourceMetadata {
                kind: "archive".to_owned(),
                path: format!("../artifacts/{name}/{name}-{version}.tar.gz"),
                format: "tar.gz".to_owned(),
            },
        }
    }

    #[test]
    fn creates_new_index_from_first_version() {
        let meta = metadata("fmt", "10.2.1");
        let index = insert_version(None, &meta).unwrap();
        assert_eq!(index.schema, 1);
        assert_eq!(index.name, "fmt");
        assert!(index.versions.contains_key("10.2.1"));
    }

    #[test]
    fn appends_new_version_to_existing_index() {
        let initial = insert_version(None, &metadata("fmt", "10.1.0")).unwrap();
        let updated = insert_version(Some(initial), &metadata("fmt", "10.2.1")).unwrap();
        assert_eq!(updated.versions.len(), 2);
        assert!(updated.versions.contains_key("10.1.0"));
        assert!(updated.versions.contains_key("10.2.1"));
    }

    #[test]
    fn duplicate_version_fails() {
        let initial = insert_version(None, &metadata("fmt", "10.2.1")).unwrap();
        let err = insert_version(Some(initial), &metadata("fmt", "10.2.1")).unwrap_err();
        assert!(matches!(err, RegistryError::DuplicateVersion { .. }));
    }

    #[test]
    fn name_mismatch_fails() {
        let initial = insert_version(None, &metadata("fmt", "10.2.1")).unwrap();
        // Existing index says "fmt" but caller hands us spdlog.
        let err = insert_version(Some(initial), &metadata("spdlog", "1.13.0")).unwrap_err();
        assert!(matches!(
            err,
            RegistryError::PackageIndexNameMismatch { .. }
        ));
    }

    #[test]
    fn render_is_deterministic() {
        let index = insert_version(
            insert_version(None, &metadata("fmt", "10.2.1")).ok(),
            &metadata("fmt", "10.1.0"),
        )
        .unwrap();
        let a = render(&index).unwrap();
        let b = render(&index).unwrap();
        assert_eq!(a, b);
        assert!(a.ends_with('\n'));
    }

    #[test]
    fn render_orders_versions_by_semver() {
        let index = insert_version(
            insert_version(
                insert_version(None, &metadata("fmt", "9.9.9")).ok(),
                &metadata("fmt", "10.1.0"),
            )
            .ok(),
            &metadata("fmt", "10.2.1"),
        )
        .unwrap();
        let body = render(&index).unwrap();
        let pos_9 = body.find("\"9.9.9\"").unwrap();
        let pos_101 = body.find("\"10.1.0\"").unwrap();
        let pos_102 = body.find("\"10.2.1\"").unwrap();
        // 9.9.9 < 10.1.0 < 10.2.1 by SemVer despite lexicographic
        // would say "10.x" < "9.9.9".
        assert!(pos_9 < pos_101);
        assert!(pos_101 < pos_102);
    }

    #[test]
    fn render_round_trips() {
        let index = insert_version(None, &metadata("fmt", "10.2.1")).unwrap();
        let body = render(&index).unwrap();
        let parsed: PackageIndex = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, index);
    }
}
