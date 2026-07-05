use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cabin_core::registry::{REGISTRY_CONFIG_SCHEMA, REGISTRY_KIND, relative_subdir_is_safe};
use cabin_core::{Condition, DependencyKind, PackageName};
use serde::Deserialize;

use crate::error::IndexError;
use crate::model::{
    ArchiveFormat, IndexEntry, IndexPackageDependency, IndexSystemDependency, PackageIndex,
    SourceArtifact, SourceArtifactKind, SourceLocation, VersionMetadata,
};

/// How to interpret a `source.path` value when parsing one
/// `<name>.json` file.
///
/// `cabin-index`'s file loader uses [`SourceContext::LocalDir`] (the
/// package file's parent directory) to resolve relative paths into
/// absolute filesystem paths. `cabin-index-http` uses
/// [`SourceContext::HttpUrl`] (the package metadata URL) to resolve
/// against an HTTP base.  Both feed [`parse_package_entry`].
pub enum SourceContext<'a> {
    /// Resolve relative `source.path` values against this filesystem
    /// directory; produce [`SourceLocation::LocalPath`].
    LocalDir(&'a Path),
    /// Caller-supplied resolver.  Used by the HTTP loader to convert
    /// `source.path` strings into absolute URLs without coupling
    /// `cabin-index` to a URL parser.
    HttpUrl(&'a dyn Fn(&str) -> Result<String, IndexError>),
}

impl std::fmt::Debug for SourceContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceContext::LocalDir(path) => f
                .debug_tuple("SourceContext::LocalDir")
                .field(path)
                .finish(),
            SourceContext::HttpUrl(_) => f.write_str("SourceContext::HttpUrl(<resolver>)"),
        }
    }
}

/// Load every `<package>.json` file under `path` into a [`PackageIndex`].
///
/// `path` must point at an existing directory.  Two on-disk shapes
/// are accepted:
///
/// 1. **Registry-root layout**.  When `path/config.json`
///    exists it must be a valid Cabin file-registry config; package
///    index files are read from `path/<config.packages>/`.  Source
///    paths recorded in those package files resolve relative to the
///    package files' parent directory (`path/<config.packages>/`),
///    so the published `"../artifacts/<name>/<name>-<version>.tar.gz"`
///    form lands at `path/artifacts/<name>/<name>-<version>.tar.gz`.
///    The `config.artifacts` field is accepted for schema
///    compatibility but is not consulted during resolution.
/// 2. **Flat layout**.  Used by hand-written
///    fixtures that drop `<name>.json` directly under `path` with no
///    `config.json`.  Source paths resolve relative to `path`.
///
/// Files whose names do not end in `.json` are ignored.  The
/// `config.json` file at the registry root is itself excluded from
/// the package scan.
///
/// # Errors
/// Returns [`IndexError::NotADirectory`] when `path` or the resolved
/// packages directory is not a directory, and [`IndexError::Io`] when
/// the directory cannot be read or an entry cannot be iterated.  When a
/// registry-root `config.json` is present it propagates the config
/// errors (`Io` / `Json` / [`IndexError::InvalidRegistryConfig`]), and
/// it propagates any per-package parse error from `parse_package_entry`.
pub fn load_index(path: impl AsRef<Path>) -> Result<PackageIndex, IndexError> {
    let path = path.as_ref();
    if !path.is_dir() {
        return Err(IndexError::NotADirectory {
            path: path.to_path_buf(),
        });
    }

    let packages_dir = if path.join("config.json").is_file() {
        load_registry_config(path)?
    } else {
        path.to_path_buf()
    };

    if !packages_dir.is_dir() {
        return Err(IndexError::NotADirectory { path: packages_dir });
    }

    let entries = std::fs::read_dir(&packages_dir).map_err(|source| IndexError::Io {
        path: packages_dir.clone(),
        source,
    })?;
    let mut packages: BTreeMap<PackageName, IndexEntry> = BTreeMap::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| IndexError::Io {
            path: packages_dir.clone(),
            source,
        })?;
        let entry_path = entry.path();
        if entry_path.is_file() && entry_path.extension().and_then(|e| e.to_str()) == Some("json") {
            // Skip the registry config when the loader was pointed
            // at a flat layout that *also* happens to contain a
            // `config.json` (defensive: the registry-root branch
            // above already short-circuits this case).
            let stem = entry_path.file_stem().and_then(|s| s.to_str());
            if stem == Some("config") && packages_dir == path {
                continue;
            }
            paths.push(entry_path);
        }
    }
    paths.sort();

    for entry_path in paths {
        let pkg = load_package_file(&entry_path)?;
        packages.insert(pkg.name.clone(), pkg);
    }

    Ok(PackageIndex {
        root: path.to_path_buf(),
        packages,
    })
}

/// Read and validate `<root>/config.json` (file-registry
/// layout) and return the directory where package index files live
/// (`<root>/<config.packages>`).
fn load_registry_config(root: &Path) -> Result<PathBuf, IndexError> {
    let config_path = root.join("config.json");
    let body = std::fs::read_to_string(&config_path).map_err(|source| IndexError::Io {
        path: config_path.clone(),
        source,
    })?;
    let raw: RawRegistryConfig =
        serde_json::from_str(&body).map_err(|source| IndexError::Json {
            path: config_path.clone(),
            source,
        })?;
    if raw.schema != REGISTRY_CONFIG_SCHEMA {
        return Err(IndexError::InvalidRegistryConfig {
            path: config_path,
            message: format!("unsupported schema version {}", raw.schema),
        });
    }
    if raw.kind != REGISTRY_KIND {
        return Err(IndexError::InvalidRegistryConfig {
            path: config_path,
            message: format!("unsupported kind {:?}", raw.kind),
        });
    }
    if raw.packages.is_empty() {
        return Err(IndexError::InvalidRegistryConfig {
            path: config_path,
            message: "`packages` must be a non-empty relative directory".to_owned(),
        });
    }
    if !relative_subdir_is_safe(&raw.packages) {
        return Err(IndexError::InvalidRegistryConfig {
            path: config_path,
            message: format!(
                "`packages` must be a relative subdirectory, got {:?}",
                raw.packages
            ),
        });
    }
    Ok(root.join(raw.packages))
}

fn load_package_file(path: &Path) -> Result<IndexEntry, IndexError> {
    let body = std::fs::read_to_string(path).map_err(|source| IndexError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();
    let parent_dir = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    parse_package_entry(
        &body,
        Some(&stem),
        &SourceContext::LocalDir(&parent_dir),
        Some(path),
    )
}

/// Parse one `<name>.json` package-index document.
///
/// Both the local file loader and `cabin-index-http`'s package fetcher
/// land here; `context` decides how each entry's `source.path` becomes
/// A [`SourceLocation`] and `name_hint` (when supplied) is checked
/// against the JSON's `name` field for the same name-mismatch error
/// the file loader emits.
///
/// `error_path` is purely informational: when the caller is reading a
/// file on disk it carries that path so [`IndexError`] variants
/// surface useful context.  HTTP callers may pass `None`.
///
/// # Errors
/// Returns [`IndexError::Json`] on malformed JSON,
/// [`IndexError::UnsupportedSchema`] when `schema` is not `1`,
/// [`IndexError::NameMismatch`] when `name_hint` disagrees with the
/// declared `name`, [`IndexError::InvalidPackageName`] for an invalid
/// package, dependency, or system-dependency name,
/// [`IndexError::InvalidVersion`] for a non-SemVer version, and
/// [`IndexError::InvalidRequirement`] for an unparsable dependency
/// requirement.  It also propagates the source-artifact errors
/// ([`IndexError::UnsupportedSourceType`],
/// [`IndexError::UnsupportedSourceFormat`],
/// [`IndexError::MissingSourcePath`], and any error returned by the
/// [`SourceContext::HttpUrl`] resolver).
pub fn parse_package_entry(
    body: &str,
    name_hint: Option<&str>,
    context: &SourceContext<'_>,
    error_path: Option<&Path>,
) -> Result<IndexEntry, IndexError> {
    let raw: RawIndexFile = serde_json::from_str(body).map_err(|source| IndexError::Json {
        path: error_path.map(Path::to_path_buf).unwrap_or_default(),
        source,
    })?;

    if raw.schema != 1 {
        return Err(IndexError::UnsupportedSchema {
            path: error_path.map(Path::to_path_buf).unwrap_or_default(),
            schema: raw.schema,
        });
    }

    if let Some(stem) = name_hint
        && raw.name != stem
    {
        return Err(IndexError::NameMismatch {
            path: error_path.map(Path::to_path_buf).unwrap_or_default(),
            declared: raw.name,
            expected: stem.to_owned(),
        });
    }

    let package_name = validated_package_name(&raw.name)?;

    let mut versions: BTreeMap<semver::Version, VersionMetadata> = BTreeMap::new();
    for (ver_str, raw_ver) in raw.versions {
        let version =
            semver::Version::parse(&ver_str).map_err(|source| IndexError::InvalidVersion {
                package: raw.name.clone(),
                value: ver_str.clone(),
                source,
            })?;
        let dependencies = parse_kinded_dependencies(&raw.name, &ver_str, raw_ver.dependencies)?;
        let dev_dependencies =
            parse_kinded_dependencies(&raw.name, &ver_str, raw_ver.dev_dependencies)?;
        let mut system_dependencies: BTreeMap<PackageName, IndexSystemDependency> = BTreeMap::new();
        for (sys_name, raw_sys) in raw_ver.system_dependencies {
            let validated = validated_package_name(&sys_name)?;
            // Same platform-only invariant as package dependencies:
            // system-dependency activation never sees toolchain
            // detection, so a compiler-conditioned gate is rejected.
            reject_compiler_condition(&raw.name, &ver_str, &sys_name, raw_sys.target.as_ref())?;
            system_dependencies.insert(
                validated,
                IndexSystemDependency {
                    version: raw_sys.version,
                    dependency_kind: raw_sys.dependency_kind,
                    condition: raw_sys.target,
                },
            );
        }
        let source = match raw_ver.source {
            None => None,
            Some(raw_source) => Some(parse_source_artifact(
                raw_source, &raw.name, &ver_str, context,
            )?),
        };
        versions.insert(
            version,
            VersionMetadata {
                dependencies,
                dev_dependencies,
                system_dependencies,
                yanked: raw_ver.yanked,
                checksum: raw_ver.checksum,
                source,
                features: raw_ver.features,
                profiles: raw_ver.profiles,
                toolchain: raw_ver.toolchain,
                build: raw_ver.build,
                compiler_wrapper: raw_ver.compiler_wrapper,
                language: raw_ver.language,
                standards: raw_ver.standards,
            },
        );
    }

    Ok(IndexEntry {
        name: package_name,
        versions,
    })
}

/// Validate `name` as a [`PackageName`], mapping failure to
/// [`IndexError::InvalidPackageName`].
fn validated_package_name(name: &str) -> Result<PackageName, IndexError> {
    PackageName::new(name).map_err(|err| IndexError::InvalidPackageName {
        package: name.to_owned(),
        message: err.to_string(),
    })
}

/// Parse one per-kind dependency table (`dependencies` /
/// `dev_dependencies`) of an index version entry. `package` and
/// `version` only feed the error context.
fn parse_kinded_dependencies(
    package: &str,
    version: &str,
    raw_table: BTreeMap<String, RawIndexPackageDep>,
) -> Result<BTreeMap<PackageName, IndexPackageDependency>, IndexError> {
    let mut deps: BTreeMap<PackageName, IndexPackageDependency> = BTreeMap::new();
    for (dep_name, raw_dep) in raw_table {
        let dep_name_validated = validated_package_name(&dep_name)?;
        let req_str = raw_dep.version_str();
        let req = cabin_core::version_req::parse_lenient(req_str).map_err(|source| {
            IndexError::InvalidRequirement {
                package: package.to_owned(),
                version: version.to_owned(),
                dep: dep_name.clone(),
                requirement: req_str.to_owned(),
                source,
            }
        })?;
        let optional = raw_dep.optional();
        let features = raw_dep.features().to_vec();
        let default_features = raw_dep.default_features();
        let condition = raw_dep.condition().cloned();
        // Index dependency gates are evaluated platform-only (no
        // toolchain detection runs before resolution), and `cabin
        // publish` cannot produce compiler-conditioned dependencies -
        // the manifest layer rejects them.  Reject hand-authored
        // entries here so an index can never gate resolver / prefetch
        // edges on the local compiler.
        reject_compiler_condition(package, version, &dep_name, condition.as_ref())?;
        deps.insert(
            dep_name_validated,
            IndexPackageDependency {
                req,
                optional,
                features,
                default_features,
                condition,
            },
        );
    }
    Ok(deps)
}

/// Reject a compiler-referencing (`cc` / `cxx` / `cc_version` /
/// `cxx_version`) `target` condition on an index dependency entry.
/// Index gates are evaluated with a platform-only context - no
/// toolchain detection runs before resolution - so a compiler leaf
/// would evaluate against family `unknown` and could activate edges
/// for nonsense reasons.  The manifest layer already rejects these on
/// dependency tables, so only hand-authored index entries can carry
/// them; refuse to load such an entry.
fn reject_compiler_condition(
    package: &str,
    version: &str,
    dep: &str,
    condition: Option<&Condition>,
) -> Result<(), IndexError> {
    if let Some(cond) = condition
        && cond.references_compiler()
    {
        return Err(IndexError::CompilerConditionedDependency {
            package: package.to_owned(),
            version: version.to_owned(),
            dep: dep.to_owned(),
            condition: cond.to_string(),
        });
    }
    Ok(())
}

/// Parse and resolve a `source` block on an index version entry.
///
/// Validates `type` and `format` (`archive` / `tar.gz` only), then
/// hands the raw `path` value to `context` to decide whether it
/// becomes a [`SourceLocation::LocalPath`] or a
/// [`SourceLocation::HttpUrl`].
fn parse_source_artifact(
    raw: RawSourceArtifact,
    package: &str,
    version: &str,
    context: &SourceContext<'_>,
) -> Result<SourceArtifact, IndexError> {
    let RawSourceArtifact { kind, path, format } = raw;
    let kind = match kind.as_str() {
        "archive" => SourceArtifactKind::Archive,
        other => {
            return Err(IndexError::UnsupportedSourceType {
                package: package.to_owned(),
                version: version.to_owned(),
                value: other.to_owned(),
            });
        }
    };
    let format = match format.as_str() {
        "tar.gz" => ArchiveFormat::TarGz,
        other => {
            return Err(IndexError::UnsupportedSourceFormat {
                package: package.to_owned(),
                version: version.to_owned(),
                value: other.to_owned(),
            });
        }
    };
    if path.is_empty() {
        return Err(IndexError::MissingSourcePath {
            package: package.to_owned(),
            version: version.to_owned(),
        });
    }

    let location = match context {
        SourceContext::LocalDir(parent_dir) => {
            let raw_path = PathBuf::from(&path);
            let resolved = if raw_path.is_absolute() {
                raw_path
            } else {
                parent_dir.join(raw_path)
            };
            SourceLocation::LocalPath(resolved)
        }
        SourceContext::HttpUrl(resolver) => SourceLocation::HttpUrl(resolver(&path)?),
    };

    Ok(SourceArtifact {
        kind,
        format,
        location,
    })
}

// ---------------------------------------------------------------------------
// raw serde-shaped types - kept private to this crate.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndexFile {
    schema: u32,
    name: String,
    #[serde(default)]
    versions: BTreeMap<String, RawVersion>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVersion {
    /// Normal-kind dependencies.  Each entry may be a bare
    /// requirement string (oldest shape) or a table that records
    /// `optional` / `features` / `default-features` overrides.
    #[serde(default)]
    dependencies: BTreeMap<String, RawIndexPackageDep>,
    /// `[dev-dependencies]` of this version.
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, RawIndexPackageDep>,
    /// `system-dependencies` field of this version.  Each entry
    /// uses the system-dep schema (free-form `version`).  Defaults
    /// to empty.
    #[serde(default, rename = "system-dependencies")]
    system_dependencies: BTreeMap<String, RawIndexSystemDependency>,
    #[serde(default)]
    yanked: bool,
    #[serde(default)]
    checksum: Option<String>,
    #[serde(default)]
    source: Option<RawSourceArtifact>,
    /// Declared `[features]`.  Optional; older registry
    /// entries that omit the field continue to load.
    #[serde(default)]
    features: Option<serde_json::Value>,
    /// Declared `[profile.*]` tables.  Optional; older registries
    /// that omit the field continue to load.  The loader does not
    /// validate the inner shape - the file-registry writer
    /// already produced it from a typed Cabin model.
    #[serde(default)]
    profiles: Option<serde_json::Value>,
    /// Declared `[toolchain]` block.  Optional; older registries
    /// that omit the field continue to load.
    #[serde(default)]
    toolchain: Option<serde_json::Value>,
    /// Declared `[profile]` block.  Optional; older registries
    /// that omit the field continue to load.
    #[serde(default)]
    build: Option<serde_json::Value>,
    /// Declared `[build] compiler-wrapper`. Optional; older registries
    /// that omit the field continue to load.
    #[serde(default)]
    compiler_wrapper: Option<serde_json::Value>,
    /// Declared `[package]`-level language standard fields.
    /// Optional; older registries that omit the field continue to
    /// load.
    #[serde(default)]
    language: Option<serde_json::Value>,
    /// Declared per-target standard-compatibility table.  Optional;
    /// absence (all pre-`standards` entries) is an empty table, i.e.
    /// unconstrained.  Parsed into the typed
    /// [`cabin_core::StandardsMetadata`], which rejects a populated
    /// reserved `max` and a bare-string cell.
    #[serde(default)]
    standards: cabin_core::StandardsMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndexSystemDependency {
    version: String,
    #[serde(default)]
    dependency_kind: DependencyKind,
    /// Canonical inner-expression form of a `cfg(...)` predicate.
    /// Optional so older registries that do not use target-
    /// specific system declarations stay readable.
    #[serde(default)]
    target: Option<Condition>,
}

/// On-disk shape of one Cabin package dependency entry inside an
/// index version document.  Either a bare requirement string
/// (oldest shape) or a table that records
/// `optional` / `features` / `default-features`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawIndexPackageDep {
    Bare(String),
    Rich(RawIndexPackageDepTable),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndexPackageDepTable {
    version: String,
    #[serde(default)]
    optional: bool,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default = "default_true_for_index", rename = "default-features")]
    default_features: bool,
    /// Canonical inner-expression form of a `cfg(...)` predicate
    /// (`os = "linux"`, `all(os = "linux", arch = "x86_64")`, …).
    /// Round-tripped via `Condition`'s string serde so older
    /// metadata that omits this field continues to parse.
    #[serde(default)]
    target: Option<Condition>,
}

fn default_true_for_index() -> bool {
    true
}

impl RawIndexPackageDep {
    fn version_str(&self) -> &str {
        match self {
            RawIndexPackageDep::Bare(s) => s.as_str(),
            RawIndexPackageDep::Rich(t) => t.version.as_str(),
        }
    }

    fn optional(&self) -> bool {
        match self {
            RawIndexPackageDep::Bare(_) => false,
            RawIndexPackageDep::Rich(t) => t.optional,
        }
    }

    fn features(&self) -> &[String] {
        match self {
            RawIndexPackageDep::Bare(_) => &[],
            RawIndexPackageDep::Rich(t) => &t.features,
        }
    }

    fn default_features(&self) -> bool {
        match self {
            RawIndexPackageDep::Bare(_) => true,
            RawIndexPackageDep::Rich(t) => t.default_features,
        }
    }

    fn condition(&self) -> Option<&Condition> {
        match self {
            RawIndexPackageDep::Bare(_) => None,
            RawIndexPackageDep::Rich(t) => t.target.as_ref(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSourceArtifact {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    format: String,
}

/// Parser-side mirror of `cabin-registry-file::RegistryConfig`.  We
/// re-implement it here rather than reaching into that crate so the
/// `cabin-index` read path stays free of registry-mutation
/// dependencies.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistryConfig {
    schema: u32,
    kind: String,
    packages: String,
    #[serde(default, rename = "artifacts")]
    _artifacts: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    #[test]
    fn loads_simple_package_index() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:x" }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        assert_eq!(index.packages.len(), 1);
        let entry = index
            .package(&PackageName::new("fmt").unwrap())
            .expect("fmt entry");
        assert_eq!(entry.versions.len(), 1);
        let (ver, meta) = entry.versions.iter().next().unwrap();
        assert_eq!(ver, &semver::Version::parse("10.2.1").unwrap());
        assert!(!meta.yanked);
        assert_eq!(meta.checksum.as_deref(), Some("sha256:x"));
    }

    #[test]
    fn loads_multiple_versions_and_yanked() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": true },
                    "10.1.0": { "dependencies": {} }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index.package(&PackageName::new("fmt").unwrap()).unwrap();
        assert_eq!(entry.versions.len(), 2);
        let yanked = entry
            .versions
            .get(&semver::Version::parse("10.2.1").unwrap())
            .unwrap();
        assert!(yanked.yanked);
        let stable = entry
            .versions
            .get(&semver::Version::parse("10.1.0").unwrap())
            .unwrap();
        assert!(!stable.yanked);
    }

    #[test]
    fn loads_package_with_dependencies() {
        let dir = TempDir::new().unwrap();
        dir.child("spdlog.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "spdlog",
                "versions": {
                    "1.13.0": {
                        "dependencies": { "fmt": ">=10.0.0 <11.0.0" },
                        "yanked": false
                    }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index.package(&PackageName::new("spdlog").unwrap()).unwrap();
        let meta = entry
            .versions
            .get(&semver::Version::parse("1.13.0").unwrap())
            .unwrap();
        let entry = meta
            .dependencies
            .get(&PackageName::new("fmt").unwrap())
            .unwrap();
        assert!(
            entry
                .req
                .matches(&semver::Version::parse("10.5.0").unwrap())
        );
        assert!(
            !entry
                .req
                .matches(&semver::Version::parse("11.0.0").unwrap())
        );
    }

    #[test]
    fn loads_system_dependency_with_dependency_kind_field() {
        let dir = TempDir::new().unwrap();
        dir.child("demo.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "demo",
                "versions": {
                    "0.1.0": {
                        "dependencies": {},
                        "system-dependencies": {
                            "openssl": {
                                "version": ">=3",
                                "dependency_kind": "dev"
                            }
                        }
                    }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index.package(&PackageName::new("demo").unwrap()).unwrap();
        let meta = entry
            .versions
            .get(&semver::Version::parse("0.1.0").unwrap())
            .unwrap();
        let dep = meta
            .system_dependencies
            .get(&PackageName::new("openssl").unwrap())
            .unwrap();
        assert_eq!(dep.version, ">=3");
        assert_eq!(dep.dependency_kind, DependencyKind::Dev);
    }

    #[test]
    fn compiler_conditioned_dependency_target_is_rejected() {
        // A platform-conditioned target loads; a compiler-conditioned
        // one must refuse to load - index gates are evaluated
        // platform-only, so `cc = "unknown"` / `not(cxx = ...)` would
        // otherwise evaluate true and feed bogus resolver edges.
        let dir = TempDir::new().unwrap();
        dir.child("spdlog.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "spdlog",
                "versions": {
                    "1.0.0": {
                        "dependencies": {
                            "epoll": { "version": "^1", "target": "os = \"linux\"" },
                            "fmt": { "version": "^10", "target": "not(cxx = \"gcc\")" }
                        }
                    }
                }
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        match err {
            IndexError::CompilerConditionedDependency {
                package,
                version,
                dep,
                condition,
            } => {
                assert_eq!(package, "spdlog");
                assert_eq!(version, "1.0.0");
                assert_eq!(dep, "fmt");
                assert_eq!(condition, r#"not(cxx = "gcc")"#);
            }
            other => panic!("expected CompilerConditionedDependency, got {other:?}"),
        }
    }

    #[test]
    fn compiler_conditioned_system_dependency_target_is_rejected() {
        let dir = TempDir::new().unwrap();
        dir.child("demo.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "demo",
                "versions": {
                    "0.1.0": {
                        "dependencies": {},
                        "system-dependencies": {
                            "openssl": { "version": ">=3", "target": "cc = \"unknown\"" }
                        }
                    }
                }
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        match err {
            IndexError::CompilerConditionedDependency { dep, condition, .. } => {
                assert_eq!(dep, "openssl");
                assert_eq!(condition, r#"cc = "unknown""#);
            }
            other => panic!("expected CompilerConditionedDependency, got {other:?}"),
        }
    }

    #[test]
    fn name_filename_mismatch_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(r#"{ "schema": 1, "name": "different", "versions": {} }"#)
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        match err {
            IndexError::NameMismatch {
                declared, expected, ..
            } => {
                assert_eq!(declared, "different");
                assert_eq!(expected, "fmt");
            }
            other => panic!("expected NameMismatch, got {other:?}"),
        }
    }

    #[test]
    fn invalid_schema_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(r#"{ "schema": 99, "name": "fmt", "versions": {} }"#)
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        assert!(matches!(
            err,
            IndexError::UnsupportedSchema { schema: 99, .. }
        ));
    }

    #[test]
    fn invalid_version_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": { "abc": { "dependencies": {} } }
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        match err {
            IndexError::InvalidVersion { package, value, .. } => {
                assert_eq!(package, "fmt");
                assert_eq!(value, "abc");
            }
            other => panic!("expected InvalidVersion, got {other:?}"),
        }
    }

    #[test]
    fn invalid_requirement_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("spdlog.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "spdlog",
                "versions": {
                    "1.0.0": { "dependencies": { "fmt": ">>>" } }
                }
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        match err {
            IndexError::InvalidRequirement {
                package,
                version,
                dep,
                requirement,
                ..
            } => {
                assert_eq!(package, "spdlog");
                assert_eq!(version, "1.0.0");
                assert_eq!(dep, "fmt");
                assert_eq!(requirement, ">>>");
            }
            other => panic!("expected InvalidRequirement, got {other:?}"),
        }
    }

    #[test]
    fn unknown_field_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {},
                "extra": "nope"
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        assert!(matches!(err, IndexError::Json { .. }));
    }

    #[test]
    fn missing_directory_errors() {
        let dir = TempDir::new().unwrap();
        let err = load_index(dir.path().join("does-not-exist")).unwrap_err();
        assert!(matches!(err, IndexError::NotADirectory { .. }));
    }

    #[test]
    fn ignores_non_json_files() {
        let dir = TempDir::new().unwrap();
        dir.child("README.md").write_str("ignored").unwrap();
        dir.child("fmt.json")
            .write_str(r#"{ "schema": 1, "name": "fmt", "versions": {} }"#)
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        assert_eq!(index.packages.len(), 1);
    }

    // -------------------------------------------------------------------
    // source artifact metadata
    // -------------------------------------------------------------------

    #[test]
    fn loads_source_artifact_with_relative_path() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "dependencies": {},
                        "yanked": false,
                        "checksum": "sha256:abc",
                        "source": {
                            "type": "archive",
                            "path": "../artifacts/fmt-10.2.1.tar.gz",
                            "format": "tar.gz"
                        }
                    }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index.package(&PackageName::new("fmt").unwrap()).unwrap();
        let meta = entry
            .versions
            .get(&semver::Version::parse("10.2.1").unwrap())
            .unwrap();
        let source = meta.source.as_ref().expect("source must be parsed");
        assert_eq!(source.kind, crate::SourceArtifactKind::Archive);
        assert_eq!(source.format, crate::ArchiveFormat::TarGz);
        // Relative path resolved against the index file's directory.
        match &source.location {
            SourceLocation::LocalPath(p) => {
                assert_eq!(p, &dir.path().join("../artifacts/fmt-10.2.1.tar.gz"));
            }
            SourceLocation::HttpUrl(_) => panic!("expected LocalPath, got {:?}", source.location),
        }
    }

    #[test]
    fn loads_source_artifact_with_absolute_path() {
        let dir = TempDir::new().unwrap();
        let abs = if cfg!(windows) {
            "C:/artifacts/fmt-10.2.1.tar.gz".to_string()
        } else {
            "/var/artifacts/fmt-10.2.1.tar.gz".to_string()
        };
        let body = format!(
            r#"{{
                "schema": 1,
                "name": "fmt",
                "versions": {{
                    "10.2.1": {{
                        "dependencies": {{}},
                        "yanked": false,
                        "checksum": "sha256:abc",
                        "source": {{ "type": "archive", "path": "{abs}", "format": "tar.gz" }}
                    }}
                }}
            }}"#
        );
        dir.child("fmt.json").write_str(&body).unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index.package(&PackageName::new("fmt").unwrap()).unwrap();
        let meta = entry
            .versions
            .get(&semver::Version::parse("10.2.1").unwrap())
            .unwrap();
        let source = meta.source.as_ref().unwrap();
        match &source.location {
            SourceLocation::LocalPath(p) => assert_eq!(p, &std::path::PathBuf::from(abs)),
            SourceLocation::HttpUrl(_) => panic!("expected LocalPath, got {:?}", source.location),
        }
    }

    #[test]
    fn unsupported_source_type_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "source": { "type": "http", "path": "x", "format": "tar.gz" }
                    }
                }
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        match err {
            IndexError::UnsupportedSourceType {
                package,
                version,
                value,
            } => {
                assert_eq!(package, "fmt");
                assert_eq!(version, "10.2.1");
                assert_eq!(value, "http");
            }
            other => panic!("expected UnsupportedSourceType, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_source_format_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "source": { "type": "archive", "path": "x", "format": "tar.zst" }
                    }
                }
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        match err {
            IndexError::UnsupportedSourceFormat { value, .. } => assert_eq!(value, "tar.zst"),
            other => panic!("expected UnsupportedSourceFormat, got {other:?}"),
        }
    }

    #[test]
    fn missing_source_path_errors() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "source": { "type": "archive", "path": "", "format": "tar.gz" }
                    }
                }
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        assert!(matches!(err, IndexError::MissingSourcePath { .. }));
    }

    // -------------------------------------------------------------------
    // registry-root layout
    // -------------------------------------------------------------------

    #[test]
    fn loads_registry_root_layout() {
        let dir = TempDir::new().unwrap();
        dir.child("config.json")
            .write_str(
                r#"{
                "schema": 1,
                "kind": "file-registry",
                "packages": "packages",
                "artifacts": "artifacts"
            }"#,
            )
            .unwrap();
        dir.child("packages/fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "dependencies": {},
                        "yanked": false,
                        "checksum": "sha256:abc",
                        "source": {
                            "type": "archive",
                            "path": "../artifacts/fmt/fmt-10.2.1.tar.gz",
                            "format": "tar.gz"
                        }
                    }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index.package(&PackageName::new("fmt").unwrap()).unwrap();
        let meta = entry
            .versions
            .get(&semver::Version::parse("10.2.1").unwrap())
            .unwrap();
        let source = meta.source.as_ref().unwrap();
        // `../artifacts/...` resolves against `packages/` to the
        // registry's artifacts directory.
        let expected = dir
            .path()
            .join("packages/../artifacts/fmt/fmt-10.2.1.tar.gz");
        match &source.location {
            SourceLocation::LocalPath(p) => assert_eq!(p, &expected),
            SourceLocation::HttpUrl(_) => panic!("expected LocalPath, got {:?}", source.location),
        }
    }

    #[test]
    fn registry_root_layout_rejects_invalid_kind() {
        let dir = TempDir::new().unwrap();
        dir.child("config.json")
            .write_str(
                r#"{
                "schema": 1,
                "kind": "http-registry",
                "packages": "packages",
                "artifacts": "artifacts"
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        match err {
            IndexError::InvalidRegistryConfig { message, .. } => {
                assert!(message.contains("http-registry"));
            }
            other => panic!("expected InvalidRegistryConfig, got {other:?}"),
        }
    }

    #[test]
    fn registry_root_layout_rejects_traversal_in_packages_dir() {
        let dir = TempDir::new().unwrap();
        dir.child("config.json")
            .write_str(
                r#"{
                "schema": 1,
                "kind": "file-registry",
                "packages": "../escape",
                "artifacts": "artifacts"
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        assert!(matches!(err, IndexError::InvalidRegistryConfig { .. }));
    }

    #[test]
    fn flat_layout_with_config_named_file_is_treated_as_registry() {
        // A flat layout that happens to contain a top-level
        // `config.json` is interpreted as a registry root, not as a
        // package called "config".  This keeps the rule deterministic
        // without ever silently parsing a config as a package.
        let dir = TempDir::new().unwrap();
        dir.child("config.json")
            .write_str(
                r#"{
                "schema": 1,
                "kind": "file-registry",
                "packages": "packages",
                "artifacts": "artifacts"
            }"#,
            )
            .unwrap();
        // The flat-style fmt.json sitting at the root must NOT be
        // loaded once we've decided this is registry layout.
        dir.child("fmt.json")
            .write_str(r#"{ "schema": 1, "name": "fmt", "versions": {} }"#)
            .unwrap();
        // The registry's packages dir is empty.
        dir.child("packages").create_dir_all().unwrap();
        let index = load_index(dir.path()).unwrap();
        assert!(index.packages.is_empty());
    }

    // -----------------------------------------------------------------
    // feature fields are optional and round-trip.
    // -----------------------------------------------------------------

    #[test]
    fn index_loads_without_feature_fields() {
        // Older index entries - those that predate the field's introduction - must
        // continue to parse so back-compat with on-disk fixtures is
        // preserved.
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:x" }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index
            .package(&PackageName::new("fmt").unwrap())
            .expect("fmt entry");
        let (_, meta) = entry.versions.iter().next().unwrap();
        assert!(meta.features.is_none());
    }

    #[test]
    fn index_preserves_feature_field() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "dependencies": {},
                        "yanked": false,
                        "checksum": "sha256:x",
                        "features": { "default": ["simd"], "features": { "simd": [], "ssl": [] } }
                    }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index
            .package(&PackageName::new("fmt").unwrap())
            .expect("fmt entry");
        let (_, meta) = entry.versions.iter().next().unwrap();
        let features = meta.features.as_ref().expect("features preserved");
        assert_eq!(features["default"][0], "simd");
        assert_eq!(features["features"]["ssl"], serde_json::json!([]));
    }

    #[test]
    fn index_preserves_compiler_wrapper_field() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": {
                        "dependencies": {},
                        "yanked": false,
                        "checksum": "sha256:x",
                        "compiler_wrapper": {
                            "general": { "kind": "use", "wrapper": "ccache" }
                        }
                    }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index
            .package(&PackageName::new("fmt").unwrap())
            .expect("fmt entry");
        let (_, meta) = entry.versions.iter().next().unwrap();
        assert_eq!(
            meta.compiler_wrapper.as_ref().unwrap()["general"],
            serde_json::json!({"kind": "use", "wrapper": "ccache"})
        );
    }

    // -----------------------------------------------------------------
    // standard-compatibility table
    // -----------------------------------------------------------------

    /// A published `standards` table parses into the typed per-target
    /// requirements and flags: `"none"` -> forbidden, `{min}` ->
    /// minimum, an omitted language key -> unconstrained, and the two
    /// per-target flags survive.
    #[test]
    fn loads_standards_table() {
        use cabin_core::{CStandard, CxxStandard, Requirement};
        let dir = TempDir::new().unwrap();
        dir.child("mixed.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "mixed",
                "versions": {
                    "1.0.0": {
                        "dependencies": {},
                        "standards": {
                            "targets": {
                                "cxxlib": { "interface": { "c": "none", "c++": { "min": "c++17" } } },
                                "hdr": {
                                    "header-only": true,
                                    "interface": { "c": "none", "c++": { "min": "c++20" } }
                                },
                                "clib": {
                                    "gnu-extensions": true,
                                    "interface": { "c": { "min": "c11" } }
                                }
                            }
                        }
                    }
                }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index.package(&PackageName::new("mixed").unwrap()).unwrap();
        let meta = entry
            .versions
            .get(&semver::Version::parse("1.0.0").unwrap())
            .unwrap();

        let cxxlib = &meta.standards.targets["cxxlib"];
        assert_eq!(cxxlib.interface_c, Requirement::Forbidden);
        assert_eq!(cxxlib.interface_cxx, Requirement::Min(CxxStandard::Cxx17));
        assert!(!cxxlib.header_only);
        assert!(!cxxlib.gnu_extensions);

        let hdr = &meta.standards.targets["hdr"];
        assert!(hdr.header_only);
        assert_eq!(hdr.interface_cxx, Requirement::Min(CxxStandard::Cxx20));

        let clib = &meta.standards.targets["clib"];
        assert!(clib.gnu_extensions);
        assert_eq!(clib.interface_c, Requirement::Min(CStandard::C11));
        // Omitted `c++` key means unconstrained for that language.
        assert_eq!(clib.interface_cxx, Requirement::Unconstrained);
    }

    /// An entry with no `standards` field (every pre-`standards`
    /// entry) loads with an empty, all-unconstrained table.
    #[test]
    fn index_without_standards_is_unconstrained() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": { "10.2.1": { "dependencies": {} } }
            }"#,
            )
            .unwrap();
        let index = load_index(dir.path()).unwrap();
        let entry = index.package(&PackageName::new("fmt").unwrap()).unwrap();
        let (_, meta) = entry.versions.iter().next().unwrap();
        assert!(meta.standards.is_empty());
    }

    /// A populated reserved `max` is rejected (range requirements are
    /// a future version); the loader surfaces it as a JSON error.
    #[test]
    fn rejects_populated_max_in_standards() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "1.0.0": {
                        "dependencies": {},
                        "standards": {
                            "targets": {
                                "lib": { "interface": { "c++": { "min": "c++17", "max": "c++20" } } }
                            }
                        }
                    }
                }
            }"#,
            )
            .unwrap();
        let err = load_index(dir.path()).unwrap_err();
        match err {
            IndexError::Json { source, .. } => {
                assert!(
                    source.to_string().contains("reserved for a future version"),
                    "unexpected error: {source}"
                );
            }
            other => panic!("expected Json error, got {other:?}"),
        }
    }

    /// A bare standard string is not a valid cell.
    #[test]
    fn rejects_bare_standard_cell_in_standards() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt.json")
            .write_str(
                r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "1.0.0": {
                        "dependencies": {},
                        "standards": { "targets": { "lib": { "interface": { "c++": "c++17" } } } }
                    }
                }
            }"#,
            )
            .unwrap();
        assert!(matches!(
            load_index(dir.path()).unwrap_err(),
            IndexError::Json { .. }
        ));
    }
}
