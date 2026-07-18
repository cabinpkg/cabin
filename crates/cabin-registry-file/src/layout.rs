use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use cabin_core::PackageName;
use cabin_core::registry::{REGISTRY_CONFIG_SCHEMA, REGISTRY_KIND, relative_subdir_is_safe};
use serde::{Deserialize, Serialize};

use crate::atomic::atomically_write;
use crate::error::RegistryError;

/// Filename of the top-level registry config inside `<registry>/`.
pub const REGISTRY_CONFIG_FILENAME: &str = "config.json";

/// Default subdirectory names. `config.json` may override them, but
/// the defaults are the only shapes the file registry emits.
const DEFAULT_PACKAGES_DIR: &str = "packages";
const DEFAULT_ARTIFACTS_DIR: &str = "artifacts";

/// Parsed and validated `<registry>/config.json`.  Schema-version `1`
/// only.
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
    /// initializes a fresh registry.
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

/// A loaded or freshly-initialized file registry.  Keeps the on-disk
/// root and parsed config together so the publish flow can resolve
/// every path through it.
#[derive(Debug, Clone)]
pub struct FileRegistry {
    root: PathBuf,
    config: RegistryConfig,
    /// `true` if the on-disk registry was missing and this invocation
    /// initialized it (or *would* initialize it in dry-run mode).
    initialized_now: bool,
}

impl FileRegistry {
    /// Open an existing registry at `root`.  Fails if `config.json` is
    /// missing or invalid.
    ///
    /// # Errors
    /// Returns [`RegistryError::InvalidConfig`] when `config.json` is
    /// absent or fails validation (unsupported schema/kind, or an
    /// empty/unsafe `packages`/`artifacts` subdir),
    /// [`RegistryError::Io`] when the file cannot be read, and
    /// [`RegistryError::ConfigJson`] when its contents are not valid
    /// config JSON (including unknown fields).
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
    ///
    /// # Errors
    /// When `config.json` already exists, propagates the errors of
    /// [`Self::open`].  Otherwise returns [`RegistryError::Io`] if
    /// creating the layout directories or writing `config.json` fails,
    /// or [`RegistryError::Json`] if serializing the default config
    /// fails.
    pub fn open_or_initialize(root: &Path) -> Result<Self, RegistryError> {
        let config_path = root.join(REGISTRY_CONFIG_FILENAME);
        if config_path.is_file() {
            return Self::open(root);
        }
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

    /// Inspect-only counterpart to [`Self::open_or_initialize`].  If
    /// `config.json` is present the registry is opened and validated;
    /// otherwise the report describes the layout that *would* be
    /// initialized, without touching the filesystem.
    ///
    /// # Errors
    /// Propagates the errors of [`Self::open`] when `config.json` is
    /// present; otherwise infallible.
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
    /// (or, for [`Self::inspect`], would have to).  Surfaced in the
    /// publish report so dry-run output can say "registry would be
    /// initialized".
    pub fn was_initialized_now(&self) -> bool {
        self.initialized_now
    }

    /// Directory containing per-package index files
    /// (`packages/<name>.json` for a bare name,
    /// `packages/<scope>/<name>.json` for a scoped one).
    pub fn packages_dir(&self) -> PathBuf {
        self.root.join(&self.config.packages)
    }

    /// Directory containing per-package artifact directories
    /// (`artifacts/<name>/` for a bare name,
    /// `artifacts/<scope>/<name>/` for a scoped one).
    pub fn artifacts_dir(&self) -> PathBuf {
        self.root.join(&self.config.artifacts)
    }

    /// Absolute path of the package index file for `name`.  A scoped
    /// name nests its `.json` under a scope directory; every path
    /// segment is one `path_components` element, never the full
    /// name string.
    pub fn package_index_path(&self, name: &PackageName) -> PathBuf {
        let mut path = self.packages_dir();
        if let Some(scope) = name.scope() {
            path.push(scope);
        }
        path.join(format!("{}.json", name.base_name()))
    }

    /// Absolute path of the per-package artifact directory for
    /// `name`.
    pub fn artifact_dir_for(&self, name: &PackageName) -> PathBuf {
        name.path_components()
            .fold(self.artifacts_dir(), |dir, c| dir.join(c))
    }

    /// Absolute path of the artifact for one resolved
    /// (name, version).  The filename flattens a scoped name to
    /// `<scope>-<name>` so a downloaded archive stays
    /// self-identifying outside the registry tree - the same shape
    /// the hosted registry serves.
    pub fn artifact_path(&self, name: &PackageName, version: &semver::Version) -> PathBuf {
        self.artifact_dir_for(name)
            .join(format!("{}-{version}.zip", name.artifact_stem()))
    }

    /// `source.path` value to embed in package index metadata, given
    /// the `(name, version)` pair.  The path is forward-slashed and
    /// relative to the package index file's parent directory so
    /// static sparse-HTTP serving sees consistent links.
    ///
    /// The climb back to the registry root is one `..` per *normal
    /// path component* of the configured `packages` subdir (which
    /// may be nested, e.g. `a/b`) plus one for a scoped name's scope
    /// directory; the descent then mirrors
    /// [`FileRegistry::artifact_path`].  Counting and rendering use
    /// the same `Path::components` semantics as the config
    /// validation, so legal-but-unnormalized values (`./packages`,
    /// `packages/`, platform separators) climb and render exactly
    /// where the index document really sits.  For the default config
    /// this yields the same canonical shapes the hosted registry
    /// validates: `../artifacts/<name>/<name>-<version>.zip` and
    /// `../../artifacts/<scope>/<name>/<scope>-<name>-<version>.zip`.
    pub fn relative_source_path(&self, name: &PackageName, version: &semver::Version) -> String {
        let climb =
            subdir_normal_components(&self.config.packages).count() + usize::from(name.is_scoped());
        let mut out = String::new();
        for _ in 0..climb {
            out.push_str("../");
        }
        let descent: Vec<&str> = subdir_normal_components(&self.config.artifacts)
            .chain(name.path_components())
            .collect();
        out.push_str(&descent.join("/"));
        let _ = write!(out, "/{}-{version}.zip", name.artifact_stem());
        out
    }
}

/// Normal path components of a config-declared subdirectory, under
/// the same path semantics [`validate_subdir`] accepts: `.` segments
/// are legal and contribute neither depth nor a rendered segment.
/// Non-UTF-8 components cannot occur (the value comes from JSON).
fn subdir_normal_components(value: &str) -> impl Iterator<Item = &str> {
    Path::new(value).components().filter_map(|c| match c {
        std::path::Component::Normal(part) => part.to_str(),
        _ => None,
    })
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
    if !relative_subdir_is_safe(value) {
        return Err(RegistryError::InvalidConfig {
            path: path.to_path_buf(),
            message: format!("{field} must be a relative subdirectory, not {value:?}"),
        });
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
        let fmt = PackageName::new("fmt").unwrap();
        assert_eq!(
            registry.package_index_path(&fmt),
            dir.path().join("packages/fmt.json")
        );
        assert_eq!(
            registry.artifact_path(&fmt, &v),
            dir.path().join("artifacts/fmt/fmt-10.2.1.zip")
        );
        assert_eq!(
            registry.relative_source_path(&fmt, &v),
            "../artifacts/fmt/fmt-10.2.1.zip"
        );
    }

    /// A scoped name nests one directory per component in both trees
    /// and flattens to `<scope>-<name>` inside the artifact filename;
    /// the relative source link climbs one extra level because the
    /// index document sits inside the scope directory.  The shapes
    /// match what the hosted registry serves and validates.
    #[test]
    fn scoped_paths_nest_per_component() {
        let dir = TempDir::new().unwrap();
        let registry = FileRegistry::open_or_initialize(dir.path()).unwrap();
        let v = semver::Version::parse("1.0.0").unwrap();
        let name = PackageName::new("fmtlib/fmt").unwrap();
        assert_eq!(
            registry.package_index_path(&name),
            dir.path().join("packages/fmtlib/fmt.json")
        );
        assert_eq!(
            registry.artifact_path(&name, &v),
            dir.path().join("artifacts/fmtlib/fmt/fmtlib-fmt-1.0.0.zip")
        );
        assert_eq!(
            registry.relative_source_path(&name, &v),
            "../../artifacts/fmtlib/fmt/fmtlib-fmt-1.0.0.zip"
        );
    }

    /// The configured `packages` / `artifacts` subdirs may be nested
    /// (`a/b` is legal); the relative source link climbs one `..`
    /// per normal component of the packages subdir, so the emitted
    /// link resolves from the index document's real location.
    #[test]
    fn relative_source_path_climbs_nested_packages_subdirs() {
        let dir = TempDir::new().unwrap();
        dir.child("config.json")
            .write_str(
                r#"{"schema":1,"kind":"file-registry","packages":"meta/packages","artifacts":"blobs"}"#,
            )
            .unwrap();
        let registry = FileRegistry::open_or_initialize(dir.path()).unwrap();
        let v = semver::Version::parse("1.0.0").unwrap();
        assert_eq!(
            registry.relative_source_path(&PackageName::new("fmt").unwrap(), &v),
            "../../blobs/fmt/fmt-1.0.0.zip"
        );
        assert_eq!(
            registry.relative_source_path(&PackageName::new("fmtlib/fmt").unwrap(), &v),
            "../../../blobs/fmtlib/fmt/fmtlib-fmt-1.0.0.zip"
        );
    }

    /// Legal-but-unnormalized subdir values (`./packages`, trailing
    /// separator, a bare `.`) contribute depth and rendered segments
    /// by their *normal* path components only - matching where the
    /// index document actually lands on disk.
    #[test]
    fn relative_source_path_normalizes_dot_and_trailing_separators() {
        let dir = TempDir::new().unwrap();
        dir.child("config.json")
            .write_str(
                r#"{"schema":1,"kind":"file-registry","packages":"./packages/","artifacts":"./blobs"}"#,
            )
            .unwrap();
        let registry = FileRegistry::open_or_initialize(dir.path()).unwrap();
        let v = semver::Version::parse("1.0.0").unwrap();
        assert_eq!(
            registry.relative_source_path(&PackageName::new("fmt").unwrap(), &v),
            "../blobs/fmt/fmt-1.0.0.zip"
        );

        let dir = TempDir::new().unwrap();
        dir.child("config.json")
            .write_str(r#"{"schema":1,"kind":"file-registry","packages":".","artifacts":"blobs"}"#)
            .unwrap();
        let registry = FileRegistry::open_or_initialize(dir.path()).unwrap();
        // Index docs sit at the registry root: no climb at all for a
        // bare name, one level for a scoped one.
        assert_eq!(
            registry.relative_source_path(&PackageName::new("fmt").unwrap(), &v),
            "blobs/fmt/fmt-1.0.0.zip"
        );
        assert_eq!(
            registry.relative_source_path(&PackageName::new("fmtlib/fmt").unwrap(), &v),
            "../blobs/fmtlib/fmt/fmtlib-fmt-1.0.0.zip"
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
