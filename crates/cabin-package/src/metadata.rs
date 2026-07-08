use std::collections::BTreeMap;

use cabin_core::{
    CompilerWrapperRequest, Condition, Dependency, DependencyKind, DependencySource, Features,
    LanguageStandardSettings, Package, ProfileDefinition, ProfileName, ProfileSettings,
    StandardsMetadata, SystemDependency, ToolchainSettings,
};
use serde::{Deserialize, Serialize};

use crate::error::PackageError;

/// Schema version emitted by [`canonical_metadata`].  Bumping this
/// requires a coordinated change to package-index readers and the
/// file-registry writer.
pub const PACKAGE_METADATA_SCHEMA: u32 = 1;

/// Canonical per-version metadata document.  Mirrors what a
/// file-registry insertion path writes into a `<package>.json`
/// version entry, and what `dist/` contains during a publish
/// dry-run.
///
/// Field order matches the on-disk JSON shape so
/// `serde_json::to_string_pretty` emits a stable layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageMetadata {
    pub schema: u32,
    pub name: String,
    pub version: String,
    /// Normal-kind versioned registry dependencies.  Packaging
    /// rejects path / workspace deps, so each entry here is a
    /// `name -> entry` pair from `[dependencies]` (entry is a
    /// bare requirement string when the dependency has no
    /// optional / features / default-features overrides, or a
    /// table that records them).
    pub dependencies: BTreeMap<String, PackageDependencyEntry>,
    /// Versioned `[dev-dependencies]`.  Omitted when empty.
    #[serde(
        rename = "dev-dependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub dev_dependencies: BTreeMap<String, PackageDependencyEntry>,
    /// `system-dependencies` field.  Each entry is
    /// `name -> { version, dependency_kind, target }`.  Omitted when
    /// empty.
    #[serde(
        rename = "system-dependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub system_dependencies: BTreeMap<String, SystemDependencyEntry>,
    /// Declared `[features]`.  Omitted from the JSON when the
    /// package has no features so older callers see the same shape
    /// they always have.
    #[serde(skip_serializing_if = "is_empty_features")]
    pub features: Features,
    /// Manifest-declared `[profile.<name>]` tables, preserved so
    /// downstream consumers can reproduce the same compile flags
    /// from a published source archive.  Omitted when empty so
    /// older readers and packages without custom profiles see
    /// the same shape they always have.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<ProfileName, ProfileDefinition>,
    /// Manifest-declared `[toolchain]` plus any
    /// `[target.'cfg(...)'.toolchain]` tables.  Preserved so a
    /// consumer who rebuilds from source can see what tools the
    /// package author requested.  Environment- or CLI-derived
    /// selections are deliberately not written here.  Omitted
    /// when the manifest declares no toolchain table.
    #[serde(skip_serializing_if = "ToolchainSettings::is_empty")]
    pub toolchain: ToolchainSettings,
    /// Manifest-declared `[profile]` plus any general
    /// `[target.'cfg(...)'.profile]` and named
    /// `[target.'cfg(...)'.profile.<name>]` tables.  Preserved so a consumer can
    /// reproduce the package author's defines / include directories / extra
    /// args.  Omitted when empty.
    ///
    /// Note: when this metadata describes a registry dependency, the
    /// consumer drops the raw `cflags` / `cxxflags` / `ldflags`
    /// arrays during flag resolution (they are honored only for
    /// local packages); `defines` / `include_dirs` are still applied.
    #[serde(skip_serializing_if = "ProfileSettings::is_empty")]
    pub build: ProfileSettings,
    /// Manifest-declared `[build] compiler-wrapper`. Omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compiler_wrapper: Option<CompilerWrapperRequest>,
    /// Manifest-declared `[package]`-level `c-standard` /
    /// `cxx-standard` / `interface-c-standard` /
    /// `interface-cxx-standard` fields.  Preserved (round-trip only)
    /// so future resolver work can read requirements without
    /// extracting the archive; pkg-config-style local build state
    /// never lands here.  Omitted when the manifest declares none.
    #[serde(default, skip_serializing_if = "LanguageStandardSettings::is_empty")]
    pub language: LanguageStandardSettings,
    /// Declared per-target standard-compatibility table (spec D9
    /// `ReqOf`, header-only inference applied), mirroring the index
    /// entry's `standards` field so file-registry publish can splice
    /// it in without re-deriving.  Omitted when the package has no
    /// library-like targets (absence = unconstrained).  See
    /// [`cabin_core::StandardsMetadata`] and
    /// `docs/design/standard-compatibility/registry-index.md`.
    #[serde(default, skip_serializing_if = "StandardsMetadata::is_empty")]
    pub standards: StandardsMetadata,
    pub yanked: bool,
    pub checksum: String,
    pub source: SourceMetadata,
}

/// On-disk shape of one `system-dependencies` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemDependencyEntry {
    pub version: String,
    /// Dependency table the system declaration came from.
    /// Defaults to `normal` for older package metadata.
    #[serde(default)]
    pub dependency_kind: DependencyKind,
    /// Canonical inner-expression form of an optional `cfg(...)`
    /// predicate.  Omitted from the JSON when absent so older
    /// readers see the same shape they always have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Condition>,
}

impl SystemDependencyEntry {
    fn from(dep: &SystemDependency) -> Self {
        Self {
            version: dep.version.clone(),
            dependency_kind: dep.kind,
            target: dep.condition.clone(),
        }
    }
}

/// On-disk shape of one Cabin package dependency entry inside
/// `dependencies` / `dev-dependencies` of the canonical metadata
/// document.
///
/// The simplest case - a bare version requirement with no
/// optional / features / default-features overrides - serializes
/// as a string so existing readers and existing on-disk metadata
/// stay byte-identical.  Anything that needs the richer fields
/// serializes as a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageDependencyEntry {
    Bare(String),
    Rich(PackageDependencyTable),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageDependencyTable {
    pub version: String,
    /// Whether this dependency is optional (only included when
    /// enabled by a feature).  Omitted when `false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
    /// Per-edge feature requests.  Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Whether this edge requests the dependency's `default`
    /// feature.  Defaults to `true`; the field is omitted when
    /// `true` so the on-disk shape stays compact.
    #[serde(
        default = "default_true",
        rename = "default-features",
        skip_serializing_if = "is_true"
    )]
    pub default_features: bool,
    /// Canonical inner-expression form of an optional `cfg(...)`
    /// predicate (e.g. `os = "linux"`).  Round-tripped through
    /// `Condition`'s string serde.  Omitted when absent so older
    /// metadata stays byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Condition>,
}

// `serde(skip_serializing_if = "...")` calls the predicate with a
// reference to the field, so these must take `&bool` even though a
// `bool` is cheap to copy.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(value: &bool) -> bool {
    *value
}
fn default_true() -> bool {
    true
}

impl PackageDependencyEntry {
    /// Build the metadata shape for a typed [`Dependency`].
    /// Returns `Some` for versioned dependencies (the only kind
    /// that round-trips through canonical metadata) and `None`
    /// otherwise.
    pub fn from_dependency(dep: &Dependency) -> Option<Self> {
        let DependencySource::Version(req) = &dep.source else {
            return None;
        };
        let version = req.to_string();
        if !dep.optional
            && dep.features.is_empty()
            && dep.default_features
            && dep.condition.is_none()
        {
            return Some(PackageDependencyEntry::Bare(version));
        }
        Some(PackageDependencyEntry::Rich(PackageDependencyTable {
            version,
            optional: dep.optional,
            features: dep.features.clone(),
            default_features: dep.default_features,
            target: dep.condition.clone(),
        }))
    }

    /// The version requirement string this entry encodes.
    pub fn version(&self) -> &str {
        match self {
            PackageDependencyEntry::Bare(v) => v.as_str(),
            PackageDependencyEntry::Rich(t) => t.version.as_str(),
        }
    }
}

fn is_empty_features(f: &Features) -> bool {
    f.default.is_empty() && f.features.is_empty()
}

/// Companion to [`PackageMetadata::source`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceMetadata {
    #[serde(rename = "type")]
    pub kind: String,
    pub path: String,
    pub format: String,
}

/// Build the canonical [`PackageMetadata`] document for `package`,
/// referring to a freshly-archived source tree by `checksum`.
///
/// `source.path` is the file-registry relative reference
/// (`../artifacts/<name>/<name>-<version>.tar.gz`).  Dry-run staging
/// records the same shape as a package-index `source` block, without
/// publishing that path, so registry publish can reuse the
/// metadata without re-deriving it.
pub fn canonical_metadata(package: &Package, checksum: &str) -> PackageMetadata {
    let mut dependencies: BTreeMap<String, PackageDependencyEntry> = BTreeMap::new();
    let mut dev_dependencies: BTreeMap<String, PackageDependencyEntry> = BTreeMap::new();
    for dep in &package.dependencies {
        let Some(entry) = PackageDependencyEntry::from_dependency(dep) else {
            // Path / workspace deps are rejected during
            // validation, so they never reach this point.
            continue;
        };
        let target = match dep.kind {
            DependencyKind::Normal => &mut dependencies,
            DependencyKind::Dev => &mut dev_dependencies,
        };
        target.insert(dep.name.as_str().to_owned(), entry);
    }
    let mut system_dependencies: BTreeMap<String, SystemDependencyEntry> = BTreeMap::new();
    for sd in &package.system_dependencies {
        system_dependencies.insert(sd.name.as_str().to_owned(), SystemDependencyEntry::from(sd));
    }

    let name = package.name.as_str().to_owned();
    let version = package.version.to_string();
    let source_path = format!("../artifacts/{name}/{name}-{version}.tar.gz");

    PackageMetadata {
        schema: PACKAGE_METADATA_SCHEMA,
        name,
        version,
        dependencies,
        dev_dependencies,
        system_dependencies,
        features: package.features.clone(),
        profiles: package.profiles.clone(),
        toolchain: package.toolchain.clone(),
        build: package.build.clone(),
        compiler_wrapper: package.compiler_wrapper.clone(),
        language: package.language,
        standards: StandardsMetadata::from_package(package),
        yanked: false,
        checksum: checksum.to_owned(),
        source: SourceMetadata {
            kind: "archive".to_owned(),
            path: source_path,
            format: "tar.gz".to_owned(),
        },
    }
}

/// Render the metadata document as deterministic, pretty-printed
/// JSON with a trailing newline, suitable for direct on-disk writes.
///
/// # Errors
/// Returns [`PackageError::Metadata`] if `serde_json` fails to
/// serialize the metadata document.
pub fn render_canonical_json(metadata: &PackageMetadata) -> Result<String, PackageError> {
    let mut body = serde_json::to_string_pretty(metadata)?;
    body.push('\n');
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{
        CompilerWrapperRequest, Dependency, DependencySource, Package, PackageName,
        SystemDependency, ToolSpec,
    };
    use camino::Utf8PathBuf;

    fn pkg(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn ver(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    fn version_dep(name: &str, req: &str) -> Dependency {
        Dependency {
            name: pkg(name),
            source: DependencySource::Version(semver::VersionReq::parse(req).unwrap()),
            kind: cabin_core::DependencyKind::Normal,
            optional: false,
            features: Vec::new(),
            default_features: true,
            condition: None,
            ignore_interface_standard: false,
        }
    }

    fn path_dep(name: &str, path: &str) -> Dependency {
        Dependency {
            name: pkg(name),
            source: DependencySource::Path(Utf8PathBuf::from(path)),
            kind: cabin_core::DependencyKind::Normal,
            optional: false,
            features: Vec::new(),
            default_features: true,
            condition: None,
            ignore_interface_standard: false,
        }
    }

    fn package(name: &str, version: &str, deps: Vec<Dependency>) -> Package {
        Package::new(pkg(name), ver(version), Vec::new(), deps).unwrap()
    }

    #[test]
    fn metadata_carries_schema_name_version_and_checksum() {
        let proj = package("fmt", "10.2.1", Vec::new());
        let meta = canonical_metadata(&proj, "sha256:deadbeef");
        assert_eq!(meta.schema, 1);
        assert_eq!(meta.name, "fmt");
        assert_eq!(meta.version, "10.2.1");
        assert!(!meta.yanked);
        assert_eq!(meta.checksum, "sha256:deadbeef");
    }

    #[test]
    fn metadata_preserves_compiler_conditioned_profile_flags() {
        let mut proj = package("fmt", "10.2.1", Vec::new());
        proj.build
            .conditional
            .push(cabin_core::ConditionalProfileFlags {
                condition: cabin_core::Condition::parse_cfg(
                    r#"cfg(all(cxx = "clang", cxx_version = ">=18"))"#,
                )
                .unwrap(),
                profile: Some(ProfileName::new("release").unwrap()),
                flags: cabin_core::ProfileFlags {
                    cxxflags: vec!["-stdlib=libc++".into()],
                    ..Default::default()
                },
            });
        proj.build
            .conditional
            .push(cabin_core::ConditionalProfileFlags {
                condition: cabin_core::Condition::parse_cfg(r#"cfg(os = "linux")"#).unwrap(),
                profile: None,
                flags: cabin_core::ProfileFlags {
                    ldflags: vec!["-Wl,--as-needed".into()],
                    ..Default::default()
                },
            });
        let meta = canonical_metadata(&proj, "sha256:abc");
        let json = serde_json::to_string(&meta).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value["build"]["conditional"][0]["condition"],
            r#"all(cxx = "clang", cxx_version = ">=18")"#
        );
        assert_eq!(value["build"]["conditional"][0]["profile"], "release");
        assert_eq!(
            value["build"]["conditional"][0]["cxxflags"][0],
            "-stdlib=libc++"
        );
        assert!(
            value["build"]["conditional"][1].get("profile").is_none(),
            "general conditional layers must retain their old JSON shape",
        );
    }

    #[test]
    fn metadata_includes_versioned_dependencies() {
        let proj = package(
            "spdlog",
            "1.13.0",
            vec![version_dep("fmt", ">=10.0.0, <11.0.0")],
        );
        let meta = canonical_metadata(&proj, "sha256:abc");
        assert_eq!(meta.dependencies.len(), 1);
        assert!(meta.dependencies.contains_key("fmt"));
    }

    #[test]
    fn metadata_excludes_path_dependencies() {
        // Validation rejects path deps before we reach this code, but
        // the function itself ignores them so it stays
        // composable.
        let proj = package(
            "demo",
            "0.1.0",
            vec![path_dep("local", "../local"), version_dep("fmt", "^10")],
        );
        let meta = canonical_metadata(&proj, "sha256:abc");
        assert_eq!(meta.dependencies.len(), 1);
        assert!(meta.dependencies.contains_key("fmt"));
        assert!(!meta.dependencies.contains_key("local"));
    }

    #[test]
    fn metadata_source_path_is_file_registry_relative() {
        let proj = package("fmt", "10.2.1", Vec::new());
        let meta = canonical_metadata(&proj, "sha256:x");
        assert_eq!(meta.source.kind, "archive");
        assert_eq!(meta.source.format, "tar.gz");
        assert_eq!(meta.source.path, "../artifacts/fmt/fmt-10.2.1.tar.gz");
    }

    #[test]
    fn metadata_preserves_manifest_compiler_wrapper_settings() {
        let proj = package("fmt", "10.2.1", Vec::new()).with_compiler_wrapper(Some(
            CompilerWrapperRequest::Use {
                wrapper: ToolSpec::Name("ccache".into()),
            },
        ));
        let meta = canonical_metadata(&proj, "sha256:x");
        let body = render_canonical_json(&meta).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value["compiler_wrapper"],
            serde_json::json!({"kind": "use", "wrapper": "ccache"})
        );
    }

    #[test]
    fn metadata_preserves_system_dependency_kind() {
        let mut proj = package("fmt", "10.2.1", Vec::new());
        proj.system_dependencies.push(SystemDependency {
            name: pkg("openssl"),
            version: ">=3".to_owned(),
            kind: cabin_core::DependencyKind::Dev,
            condition: None,
        });
        let meta = canonical_metadata(&proj, "sha256:x");
        let body = render_canonical_json(&meta).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            value["system-dependencies"]["openssl"]["dependency_kind"],
            "dev"
        );
    }

    #[test]
    fn render_is_deterministic() {
        let proj = package(
            "spdlog",
            "1.13.0",
            vec![version_dep("fmt", ">=10.0.0, <11.0.0")],
        );
        let meta = canonical_metadata(&proj, "sha256:abc");
        let a = render_canonical_json(&meta).unwrap();
        let b = render_canonical_json(&meta).unwrap();
        assert_eq!(a, b);
        // Field order matches struct order so consumers can rely on
        // it.
        let s_pos = a.find("\"schema\"").unwrap();
        let n_pos = a.find("\"name\"").unwrap();
        let v_pos = a.find("\"version\"").unwrap();
        let d_pos = a.find("\"dependencies\"").unwrap();
        let c_pos = a.find("\"checksum\"").unwrap();
        let src_pos = a.find("\"source\"").unwrap();
        assert!(s_pos < n_pos);
        assert!(n_pos < v_pos);
        assert!(v_pos < d_pos);
        assert!(d_pos < c_pos);
        assert!(c_pos < src_pos);
    }

    #[test]
    fn render_ends_with_newline() {
        let proj = package("fmt", "10.2.1", Vec::new());
        let meta = canonical_metadata(&proj, "sha256:x");
        let body = render_canonical_json(&meta).unwrap();
        assert!(body.ends_with('\n'));
    }

    #[test]
    fn metadata_omits_empty_declarations() {
        let proj = package("fmt", "10.2.1", Vec::new());
        let body = render_canonical_json(&canonical_metadata(&proj, "sha256:x")).unwrap();
        assert!(!body.contains("\"features\""));
    }

    #[test]
    fn metadata_includes_declared_features() {
        use std::collections::BTreeMap;
        let mut fmap: BTreeMap<String, Vec<String>> = BTreeMap::new();
        fmap.insert("simd".into(), vec![]);
        fmap.insert("ssl".into(), vec![]);
        let features = cabin_core::Features {
            default: vec!["simd".into()],
            features: fmap,
        };
        let package = Package::with_config(cabin_core::PackageConfigInput {
            name: pkg("demo"),
            version: ver("1.0.0"),
            targets: Vec::new(),
            dependencies: Vec::new(),
            system_dependencies: Vec::new(),
            features,
        })
        .unwrap();
        let meta = canonical_metadata(&package, "sha256:abc");
        let body = render_canonical_json(&meta).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["features"]["default"][0], "simd");
        assert_eq!(value["features"]["features"]["simd"], serde_json::json!([]));
    }
}
