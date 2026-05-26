use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic::atomically_write;
use crate::error::RegistryError;

/// Filename of the top-level registry config inside `<registry>/`.
pub const REGISTRY_CONFIG_FILENAME: &str = "config.json";

/// Schema version emitted by [`FileRegistry::initialize`].
const REGISTRY_CONFIG_SCHEMA: u32 = 1;

/// `kind` field that identifies a Cabin file registry on disk.
const REGISTRY_KIND: &str = "file-registry";

/// Default subdirectory names. `config.json` may override them, but
/// the defaults are the only shapes the file registry emits.
const DEFAULT_PACKAGES_DIR: &str = "packages";
const DEFAULT_ARTIFACTS_DIR: &str = "artifacts";

/// Parsed and validated `<registry>/config.json`. Schema-version `1`
/// Only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryConfig {
    pub schema: u32,
    pub kind: String,
    pub packages: String,
    pub artifacts: String,
}

impl RegistryConfig {
    /// Default config emitted when `cabin publish --registry-dir`
    /// Initializes a fresh registry.
    pub fn default_v1() -> Self {
        Self {
            schema: REGISTRY_CONFIG_SCHEMA,
            kind: REGISTRY_KIND.to_owned(),
            packages: DEFAULT_PACKAGES_DIR.to_owned(),
            artifacts: DEFAULT_ARTIFACTS_DIR.to_owned(),
        }
    }

    fn validate(&self, path: &Path) -> Result<(), RegistryError> {
        if self.schema != REGISTRY_CONFIG_SCHEMA {
            return Err(RegistryError::InvalidConfig {
                path: path.to_path_buf(),
                message: format!(
                    "unsupported schema version {} (expected {REGISTRY_CONFIG_SCHEMA})",
                    self.schema
                ),
            });
        }
        if self.kind != REGISTRY_KIND {
            return Err(RegistryError::InvalidConfig {
                path: path.to_path_buf(),
                message: format!(
                    "unsupported kind {:?} (expected {REGISTRY_KIND:?})",
                    self.kind
                ),
            });
        }
        validate_subdir(path, "packages", &self.packages)?;
        validate_subdir(path, "artifacts", &self.artifacts)?;
        Ok(())
    }
}

/// A loaded or freshly-initialized file registry. Keeps the on-disk
/// root and parsed config together so the publish flow can resolve
/// every path through it.
#[derive(Debug, Clone)]
pub struct FileRegistry {
    root: PathBuf,
    config: RegistryConfig,
    /// `true` if the on-disk registry was missing and we just
    /// initialized it (or *would* initialize it in dry-run mode).
    initialized_now: bool,
}

impl FileRegistry {
    /// Open an existing registry at `root`. Fails if `config.json` is
    /// missing or invalid.
    pub fn open(root: &Path) -> Result<Self, RegistryError> {
        let root = root.to_path_buf();
        let config_path = root.join(REGISTRY_CONFIG_FILENAME);
        if !config_path.is_file() {
            return Err(RegistryError::InvalidConfig {
                path: root,
                message: format!("{REGISTRY_CONFIG_FILENAME} is missing or invalid"),
            });
        }
        let body = fs::read_to_string(&config_path).map_err(|source| RegistryError::Io {
            path: config_path.clone(),
            source,
        })?;
        let config: RegistryConfig =
            serde_json::from_str(&body).map_err(|source| RegistryError::ConfigJson {
                path: config_path.clone(),
                source,
            })?;
        config.validate(&config_path)?;
        Ok(Self {
            root,
            config,
            initialized_now: false,
        })
    }

    /// Open the registry, creating `config.json` and the layout
    /// directories if `root` does not yet contain them.
    pub fn open_or_initialize(root: &Path) -> Result<Self, RegistryError> {
        let config_path = root.join(REGISTRY_CONFIG_FILENAME);
        if config_path.is_file() {
            return Self::open(root);
        }
        // Greenfield registry: create the directory tree + write a
        // default config.json.
        fs::create_dir_all(root).map_err(|source| RegistryError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let config = RegistryConfig::default_v1();
        let body = serde_json::to_string_pretty(&config)?;
        let mut body = body;
        body.push('\n');
        atomically_write(&config_path, body.as_bytes())?;
        fs::create_dir_all(root.join(&config.packages)).map_err(|source| RegistryError::Io {
            path: root.join(&config.packages),
            source,
        })?;
        fs::create_dir_all(root.join(&config.artifacts)).map_err(|source| RegistryError::Io {
            path: root.join(&config.artifacts),
            source,
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            config,
            initialized_now: true,
        })
    }

    /// Inspect-only counterpart to [`Self::open_or_initialize`]. If
    /// `config.json` is present the registry is opened and validated;
    /// otherwise the report describes the layout that *would* be
    /// initialized, without touching the filesystem.
    pub fn inspect(root: &Path) -> Result<Self, RegistryError> {
        if root.join(REGISTRY_CONFIG_FILENAME).is_file() {
            return Self::open(root);
        }
        Ok(Self {
            root: root.to_path_buf(),
            config: RegistryConfig::default_v1(),
            initialized_now: true,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> &RegistryConfig {
        &self.config
    }

    /// Whether the most recent open call had to create `config.json`
    /// (or, for [`Self::inspect`], would have to). Surfaced in the
    /// publish report so dry-run output can say "registry would be
    /// initialized".
    pub fn was_initialized_now(&self) -> bool {
        self.initialized_now
    }

    /// Directory containing per-package index files
    /// (`packages/<name>.json`).
    pub fn packages_dir(&self) -> PathBuf {
        self.root.join(&self.config.packages)
    }

    /// Directory containing per-package artifact directories
    /// (`artifacts/<name>/`).
    pub fn artifacts_dir(&self) -> PathBuf {
        self.root.join(&self.config.artifacts)
    }

    /// Absolute path of the package index file for `name`.
    pub fn package_index_path(&self, name: &str) -> PathBuf {
        self.packages_dir().join(format!("{name}.json"))
    }

    /// Absolute path of the per-package artifact directory for
    /// `name`.
    pub fn artifact_dir_for(&self, name: &str) -> PathBuf {
        self.artifacts_dir().join(name)
    }

    /// Absolute path of the artifact for one resolved
    /// (name, version).
    pub fn artifact_path(&self, name: &str, version: &semver::Version) -> PathBuf {
        self.artifact_dir_for(name)
            .join(format!("{name}-{version}.tar.gz"))
    }

    /// `source.path` value to embed in package index metadata, given
    /// the `(name, version)` pair. The path is forward-slashed and
    /// relative to the package index file's parent directory so
    /// static sparse-HTTP serving sees consistent links.
    pub fn relative_source_path(&self, name: &str, version: &semver::Version) -> String {
        // packages/<name>.json -> ../<artifacts>/<name>/<name>-<version>.tar.gz
        format!(
            "../{}/{name}/{name}-{version}.tar.gz",
            self.config.artifacts
        )
    }
}

/// Reject `..`-bearing or absolute config-declared subdirectories;
/// the registry must stay self-contained.
fn validate_subdir(path: &Path, field: &str, value: &str) -> Result<(), RegistryError> {
    if value.is_empty() {
        return Err(RegistryError::InvalidConfig {
            path: path.to_path_buf(),
            message: format!("{field} is empty"),
        });
    }
    let candidate = Path::new(value);
    if candidate.is_absolute() {
        return Err(RegistryError::InvalidConfig {
            path: path.to_path_buf(),
            message: format!("{field} must be a relative subdirectory, not {value:?}"),
        });
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(RegistryError::InvalidConfig {
                    path: path.to_path_buf(),
                    message: format!("{field} contains an unsupported component in {value:?}"),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use predicates::prelude::*;

    #[test]
    fn open_or_initialize_creates_layout() {
        let dir = TempDir::new().unwrap();
        let registry = FileRegistry::open_or_initialize(dir.path()).unwrap();
        assert!(registry.was_initialized_now());
        let config = dir.child(REGISTRY_CONFIG_FILENAME);
        config.assert(predicate::path::is_file());
        dir.child("packages").assert(predicate::path::is_dir());
        dir.child("artifacts").assert(predicate::path::is_dir());
        let body = fs::read_to_string(config.path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["kind"], "file-registry");
    }

    #[test]
    fn open_existing_registry_succeeds() {
        let dir = TempDir::new().unwrap();
        FileRegistry::open_or_initialize(dir.path()).unwrap();
        let opened = FileRegistry::open(dir.path()).unwrap();
        assert!(!opened.was_initialized_now());
    }

    #[test]
    fn open_rejects_missing_config() {
        let dir = TempDir::new().unwrap();
        let err = FileRegistry::open(dir.path()).unwrap_err();
        match err {
            RegistryError::InvalidConfig { message, .. } => {
                assert!(message.contains(REGISTRY_CONFIG_FILENAME));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_schema() {
        let dir = TempDir::new().unwrap();
        dir.child(REGISTRY_CONFIG_FILENAME)
            .write_str(
                r#"{"schema":99,"kind":"file-registry","packages":"packages","artifacts":"artifacts"}"#,
            )
            .unwrap();
        let err = FileRegistry::open(dir.path()).unwrap_err();
        match err {
            RegistryError::InvalidConfig { message, .. } => {
                assert!(message.contains("99"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_kind() {
        let dir = TempDir::new().unwrap();
        dir.child(REGISTRY_CONFIG_FILENAME)
            .write_str(
                r#"{"schema":1,"kind":"http-registry","packages":"packages","artifacts":"artifacts"}"#,
            )
            .unwrap();
        let err = FileRegistry::open(dir.path()).unwrap_err();
        assert!(matches!(err, RegistryError::InvalidConfig { .. }));
    }

    #[test]
    fn rejects_unknown_config_field() {
        let dir = TempDir::new().unwrap();
        dir.child(REGISTRY_CONFIG_FILENAME)
            .write_str(
                r#"{"schema":1,"kind":"file-registry","packages":"packages","artifacts":"artifacts","extra":"nope"}"#,
            )
            .unwrap();
        let err = FileRegistry::open(dir.path()).unwrap_err();
        assert!(matches!(err, RegistryError::ConfigJson { .. }));
    }

    #[test]
    fn rejects_traversal_in_subdir() {
        let dir = TempDir::new().unwrap();
        dir.child(REGISTRY_CONFIG_FILENAME)
            .write_str(
                r#"{"schema":1,"kind":"file-registry","packages":"../escape","artifacts":"artifacts"}"#,
            )
            .unwrap();
        let err = FileRegistry::open(dir.path()).unwrap_err();
        match err {
            RegistryError::InvalidConfig { message, .. } => {
                assert!(message.contains("escape"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn paths_are_deterministic() {
        let dir = TempDir::new().unwrap();
        let registry = FileRegistry::open_or_initialize(dir.path()).unwrap();
        let v = semver::Version::parse("10.2.1").unwrap();
        assert_eq!(
            registry.package_index_path("fmt"),
            dir.path().join("packages/fmt.json")
        );
        assert_eq!(
            registry.artifact_path("fmt", &v),
            dir.path().join("artifacts/fmt/fmt-10.2.1.tar.gz")
        );
        assert_eq!(
            registry.relative_source_path("fmt", &v),
            "../artifacts/fmt/fmt-10.2.1.tar.gz"
        );
    }

    #[test]
    fn inspect_does_not_create_layout() {
        let dir = TempDir::new().unwrap();
        let registry = FileRegistry::inspect(dir.path()).unwrap();
        assert!(registry.was_initialized_now());
        dir.child(REGISTRY_CONFIG_FILENAME)
            .assert(predicate::path::missing());
        dir.child("packages").assert(predicate::path::missing());
    }
}
