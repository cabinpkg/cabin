//! Metadata JSON view construction for `cabin metadata`.
//!
//! The CLI command owns orchestration; this module owns only the
//! serialisable view assembled from already-resolved typed inputs.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Serialize;

use cabin_core::{DependencySource, Package, PortDepSource};
use cabin_lockfile::Lockfile;
use cabin_workspace::PackageGraph;

/// Top-level `cabin metadata --format json` document.
#[derive(Serialize)]
pub(crate) struct MetadataView<'a> {
    workspace: Option<WorkspaceView<'a>>,
    pub(crate) packages: Vec<PackageView<'a>>,
    lockfile: Option<LockfileView<'a>>,
    /// Platform context used when evaluating
    /// `[target.'cfg(...)'.<kind>]` predicates. Always populated
    /// so consumers of the JSON view can see why a given dep is
    /// active or inactive without having to re-derive the host
    /// platform themselves.
    target_platform: TargetPlatformView,
    /// Build-profile context: the resolved `selected` profile
    /// plus every available profile name, plus the parsed
    /// definitions consumers can use to recompute fields without
    /// re-reading the manifest.
    profiles: ProfilesView,
    /// Resolved C/C++ toolchain plus per-tool source. Always
    /// populated so consumers can see which compiler / archiver
    /// a build would use without rerunning `cabin build`.
    toolchain: serde_json::Value,
    /// Loaded config files plus every effective config-derived
    /// setting. Always present (even when no files were loaded)
    /// so consumers can distinguish "config absent" from "config
    /// silent" without re-deriving discovery.
    config: serde_json::Value,
    /// Active patch entries after manifest+config merging and
    /// validation. Empty array when no patches apply.
    patches: serde_json::Value,
    /// Active source-replacement entries from the merged
    /// effective config. Empty array when none apply.
    source_replacements: serde_json::Value,
    /// Foundation ports prepared for this invocation. One entry
    /// per port directory, sorted by canonical port directory.
    /// Surfaces the upstream archive URL, SHA-256, declared
    /// `strip_prefix`, overlay manifest path, and the cache
    /// location each port was extracted into. Empty array when
    /// no port deps were declared.
    ports: Vec<PortView<'a>>,
}

#[derive(Serialize)]
struct PortView<'a> {
    name: &'a str,
    version: String,
    /// Authoritative source port directory (the one containing
    /// `port.toml` and the overlay `cabin.toml`).
    port_dir: &'a Path,
    /// Prepared cache directory: where the upstream archive was
    /// extracted and the overlay manifest copied. This is the
    /// path the workspace loader treats as the port's
    /// `manifest_dir`.
    source_dir: &'a Path,
    source: PortSourceView<'a>,
    overlay_manifest: &'a Path,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PortSourceView<'a> {
    Archive {
        url: &'a str,
        sha256: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        strip_prefix: Option<&'a str>,
    },
}

#[derive(Serialize)]
struct ProfilesView {
    /// Fully resolved selected profile (built-in or custom).
    selected: serde_json::Value,
    /// Sorted list of every profile name visible to the user
    /// (`dev`, `release`, plus any custom ones declared in the
    /// workspace root manifest).
    available: Vec<String>,
    /// Manifest-declared profile definitions, keyed by profile
    /// name in deterministic order. Omitted when the manifest
    /// declares none, so packages without `[profile.*]` tables
    /// keep their previous JSON shape.
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    definitions: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize)]
struct TargetPlatformView {
    os: String,
    arch: String,
    family: String,
    env: String,
    abi: String,
    target: String,
}

impl TargetPlatformView {
    fn from_platform(platform: &cabin_core::TargetPlatform) -> Self {
        Self {
            os: platform.os.clone(),
            arch: platform.arch.clone(),
            family: platform.family.clone(),
            env: platform.env.clone(),
            abi: platform.abi.clone(),
            target: platform.target.clone(),
        }
    }
}

#[derive(Serialize)]
struct LockfileView<'a> {
    path: &'a Path,
    version: u32,
    packages: Vec<LockedPackageView<'a>>,
}

#[derive(Serialize)]
struct LockedPackageView<'a> {
    name: &'a str,
    version: String,
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    checksum: Option<&'a str>,
    dependencies: Vec<&'a str>,
}

#[derive(Serialize)]
struct WorkspaceView<'a> {
    root: &'a Path,
    members: Vec<&'a str>,
    /// Members listed under `[workspace.default-members]`.
    /// Empty when none are declared so the JSON shape stays
    /// stable for callers that do not use default-members.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    default_members: Vec<&'a str>,
    /// Directory paths the loader removed via
    /// `[workspace.exclude]`, normalised relative to the workspace
    /// root. Empty when no excludes are declared.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    excluded_members: Vec<&'a Path>,
    /// Members the user requested via CLI flags (or the
    /// documented "current package" fallback). Sorted by package
    /// name for deterministic output.
    selected_packages: Vec<&'a str>,
}

#[derive(Serialize)]
pub(crate) struct PackageView<'a> {
    pub(crate) name: &'a str,
    pub(crate) version: String,
    manifest_path: &'a Path,
    /// Cabin package dependencies (`[dependencies]`,
    /// `[dev-dependencies]`).
    /// Every entry carries an explicit
    /// `dependency_kind` field so consumers can filter by kind
    /// without re-parsing the manifest. The list is sorted by
    /// `(dependency_kind, name)` for deterministic output.
    dependencies: Vec<DependencyView<'a>>,
    /// `system = true` declarations. Externally provided
    /// (system libraries, SDKs, installed tools) - never resolved
    /// through the Cabin registry. Sorted by name. Omitted when
    /// no system dependencies are declared so packages without
    /// them keep their previous JSON shape.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system_dependencies: Vec<SystemDependencyView<'a>>,
    targets: &'a [cabin_core::Target],
    pub(crate) is_root: bool,
    pub(crate) is_primary: bool,
    /// Declared `[features]`. `None` when the manifest has
    /// no features so older callers and tools see the same JSON
    /// shape they always have.
    #[serde(skip_serializing_if = "Option::is_none")]
    features: Option<&'a cabin_core::Features>,
    /// Resolved per-package configuration. Always populated
    /// (defaults expand even when the user passes no flags); kept
    /// optional so packages with zero declarations keep their
    /// previous JSON shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    configuration: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct DependencyView<'a> {
    name: &'a str,
    /// Manifest section the dependency was declared in. Always
    /// emitted (even for normal dependencies) so consumers do not
    /// have to special-case the implicit-default case.
    dependency_kind: cabin_core::DependencyKind,
    /// Whether the dependency is optional. Omitted when `false`
    /// so packages without optional deps keep their previous
    /// JSON shape.
    #[serde(skip_serializing_if = "is_false_dep")]
    optional: bool,
    /// Per-edge feature requests on the dependency package.
    /// Omitted when empty.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    features: &'a [String],
    /// Whether this edge requests the dependency's `default`
    /// feature. Omitted when `true` (the documented default) so
    /// the JSON shape stays stable for packages that do not opt
    /// out.
    #[serde(skip_serializing_if = "is_true_dep")]
    default_features: bool,
    /// Canonical inner-expression form of an optional `cfg(...)`
    /// predicate copied from the manifest. Omitted when no
    /// `[target.'cfg(...)']` table guarded this dependency.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    /// Whether the `target` predicate matches the host platform.
    /// `true` for unconditional dependencies (the documented
    /// default); `false` when a `cfg(...)` predicate fails on
    /// the current host. Always emitted so consumers can decide
    /// whether to surface the dependency without re-evaluating
    /// the predicate.
    active: bool,
    #[serde(flatten)]
    source: DependencySourceView<'a>,
}

fn is_false_dep<T>(value: &T) -> bool
where
    T: PartialEq + Default,
{
    *value == T::default()
}

fn is_true_dep<T>(value: &T) -> bool
where
    T: PartialEq + Default + std::ops::Not<Output = T>,
{
    *value == !T::default()
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DependencySourceView<'a> {
    Path {
        path: &'a Path,
    },
    Version {
        requirement: String,
    },
    /// A foundation-port dependency. The `path` points to the
    /// port directory (containing `port.toml` and the overlay
    /// manifest) relative to the declaring package's manifest.
    Port {
        path: &'a Path,
    },
    /// An unresolved `{ workspace = true }` opt-in. The
    /// Workspace loader normally rewrites these into `Path` /
    /// `Version` before metadata is serialised, so this variant
    /// only surfaces when the user inspects a member manifest in
    /// isolation.
    Workspace,
}

#[derive(Serialize)]
struct SystemDependencyView<'a> {
    name: &'a str,
    /// Manifest section the system dependency was declared in.
    dependency_kind: cabin_core::DependencyKind,
    version: &'a str,
    /// Canonical inner-expression form of an optional `cfg(...)`
    /// predicate copied from the manifest. Omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    /// Whether the `target` predicate matches the host platform.
    /// `true` for unconditional system declarations.
    active: bool,
}

/// Bundle of inputs the metadata JSON view needs.
///
/// Threading the growing set of build-configuration inputs through
/// a typed struct keeps `MetadataView::from_graph_and_lock`'s
/// surface stable: every consumer reads fields by name instead of
/// remembering positional order.
pub(crate) struct MetadataInputs<'a> {
    pub(crate) graph: &'a PackageGraph,
    pub(crate) lockfile: Option<&'a Lockfile>,
    pub(crate) lockfile_path: &'a Path,
    pub(crate) configurations: &'a HashMap<usize, cabin_core::BuildConfiguration>,
    pub(crate) selection: &'a cabin_workspace::ResolvedSelection,
    pub(crate) profile: &'a cabin_core::ResolvedProfile,
    pub(crate) manifest_profiles:
        &'a BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
    pub(crate) toolchain: &'a cabin_core::ResolvedToolchain,
    pub(crate) build_flags: &'a HashMap<usize, cabin_core::ResolvedProfileFlags>,
    pub(crate) detection: Option<&'a cabin_core::ToolchainDetectionReport>,
    /// Resolved compiler-cache wrapper, if any. `None` is rendered
    /// as `toolchain.compiler_wrapper = null` so consumers do not
    /// have to special-case the absence.
    pub(crate) compiler_wrapper: Option<&'a cabin_core::ResolvedCompilerWrapper>,
    /// Merged effective config. Surfaced as a top-level `config`
    /// block so consumers can audit which files contributed and
    /// which effective values came from the config layer vs. CLI
    /// vs. env vs. manifest defaults.
    pub(crate) config: &'a cabin_config::EffectiveConfig,
    /// Active patch set after manifest+config merging and
    /// validation. Empty when no patches apply.
    pub(crate) active_patches: &'a cabin_workspace::ActivePatchSet,
    /// Whether `--no-patches` was supplied on the CLI. Used to
    /// suppress the source-replacement view when the user
    /// disabled the local-policy layer entirely.
    pub(crate) no_patches: bool,
    /// Prepared foundation ports. Each entry's provenance is
    /// rendered under the `ports` array of the metadata
    /// document.
    pub(crate) ports: &'a [cabin_port::PreparedPort],
}

impl<'a> MetadataView<'a> {
    pub(crate) fn from_graph_and_lock(inputs: &MetadataInputs<'a>) -> Self {
        let mut view = Self::from_inputs(inputs);
        view.lockfile = inputs.lockfile.map(|lock| LockfileView {
            path: inputs.lockfile_path,
            version: lock.version,
            packages: lock
                .packages
                .iter()
                .map(|p| LockedPackageView {
                    name: p.name.as_str(),
                    version: p.version.to_string(),
                    source: p.source.as_str(),
                    checksum: p.checksum.as_deref(),
                    dependencies: p.dependencies.iter().map(|d| d.as_str()).collect(),
                })
                .collect(),
        });
        view
    }

    fn from_inputs(inputs: &MetadataInputs<'a>) -> Self {
        let graph = inputs.graph;
        let configurations = inputs.configurations;
        let selection = inputs.selection;
        let profile = inputs.profile;
        let manifest_profiles = inputs.manifest_profiles;
        let toolchain_resolved = inputs.toolchain;
        let build_flags = inputs.build_flags;
        let detection = inputs.detection;
        let host_platform = cabin_core::TargetPlatform::current();
        let workspace = if graph.is_workspace_root {
            let mut members: Vec<&str> = graph
                .primary_packages
                .iter()
                .map(|i| graph.packages[*i].package.name.as_str())
                .collect();
            members.sort();
            let mut default_members: Vec<&str> = graph
                .default_members
                .iter()
                .map(|i| graph.packages[*i].package.name.as_str())
                .collect();
            default_members.sort();
            let mut excluded_members: Vec<&Path> =
                graph.excluded_members.iter().map(|p| p.as_path()).collect();
            excluded_members.sort();
            let mut selected_packages: Vec<&str> = selection
                .packages
                .iter()
                .map(|i| graph.packages[*i].package.name.as_str())
                .collect();
            selected_packages.sort();
            Some(WorkspaceView {
                root: &graph.root_dir,
                members,
                default_members,
                excluded_members,
                selected_packages,
            })
        } else {
            None
        };

        let packages: Vec<PackageView<'_>> = graph
            .packages
            .iter()
            .enumerate()
            .map(|(idx, pkg)| {
                let package: &Package = &pkg.package;
                let features = if package.features.default.is_empty()
                    && package.features.features.is_empty()
                {
                    None
                } else {
                    Some(&package.features)
                };
                let configuration = if features.is_some() {
                    configurations.get(&idx).map(|cfg| cfg.as_json())
                } else {
                    None
                };
                let system_dependencies: Vec<SystemDependencyView<'_>> = package
                    .system_dependencies
                    .iter()
                    .map(|sd| SystemDependencyView {
                        name: sd.name.as_str(),
                        dependency_kind: sd.kind,
                        version: sd.version.as_str(),
                        target: sd.condition.as_ref().map(ToString::to_string),
                        active: sd
                            .condition
                            .as_ref()
                            .map(|c| c.evaluate(&host_platform))
                            .unwrap_or(true),
                    })
                    .collect();
                PackageView {
                    name: package.name.as_str(),
                    version: package.version.to_string(),
                    manifest_path: &pkg.manifest_path,
                    dependencies: package
                        .dependencies
                        .iter()
                        .map(|d| DependencyView {
                            name: d.name.as_str(),
                            dependency_kind: d.kind,
                            optional: d.optional,
                            features: d.features.as_slice(),
                            default_features: d.default_features,
                            target: d.condition.as_ref().map(ToString::to_string),
                            active: d.matches_platform(&host_platform),
                            source: match &d.source {
                                DependencySource::Path(p) => DependencySourceView::Path { path: p },
                                DependencySource::Version(req) => DependencySourceView::Version {
                                    requirement: req.to_string(),
                                },
                                DependencySource::Port(PortDepSource::Path(p)) => DependencySourceView::Port { path: p },
                                DependencySource::Port(PortDepSource::Builtin(_)) => {
                                    unreachable!("builtin port resolution lands in a later task");
                                }
                                DependencySource::Workspace => DependencySourceView::Workspace,
                            },
                        })
                        .collect(),
                    system_dependencies,
                    targets: &package.targets,
                    is_root: graph.root_package == Some(idx),
                    is_primary: graph.primary_packages.contains(&idx),
                    features,
                    configuration,
                }
            })
            .collect();

        // Build the profile section once, deterministically: the
        // selected profile's resolved fields, every available
        // profile name (built-ins plus manifest-declared customs),
        // and the manifest definitions exactly as parsed.
        let available: Vec<String> = cabin_core::available_profile_names(manifest_profiles)
            .into_iter()
            .map(|n| n.as_str().to_owned())
            .collect();
        let mut definitions: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for (name, def) in manifest_profiles {
            let value = serde_json::to_value(def).unwrap_or(serde_json::Value::Null);
            definitions.insert(name.as_str().to_owned(), value);
        }
        let profiles = ProfilesView {
            selected: profile.as_json(),
            available,
            definitions,
        };

        // Toolchain block: resolved tool kind / spec / source plus
        // a per-package summary of active build flags. Generated
        // here so the metadata view's contract for toolchain /
        // build-flag visibility lives next to the rest of the
        // build-configuration shape it returns.
        let mut per_package_flags: serde_json::Map<String, serde_json::Value> =
            serde_json::Map::new();
        for (idx, _pkg) in graph.packages.iter().enumerate() {
            if let Some(flags) = build_flags.get(&idx)
                && !flags.is_empty()
            {
                let name = graph.packages[idx].package.name.as_str().to_owned();
                per_package_flags.insert(name, flags.as_json());
            }
        }
        // Detected toolchain identity / capabilities. Populated
        // when the caller supplied a detection report; absent
        // when detection failed (e.g. `cabin metadata` chose to
        // continue rather than abort) or when the caller did not
        // run detection at all. Always present in the JSON so
        // consumers can distinguish "we didn't try" from "we
        // ran it" via the `null` value.
        let detected_view = match detection {
            Some(report) => report.as_json(),
            None => serde_json::Value::Null,
        };
        // Compiler-cache wrapper sub-block. `null` when no wrapper
        // is selected so the field is always present and consumers
        // do not need to special-case the absence.
        let compiler_wrapper_view = match inputs.compiler_wrapper {
            Some(w) => w.as_json(),
            None => serde_json::Value::Null,
        };
        let toolchain_view = serde_json::json!({
            "tools": toolchain_resolved.as_json(),
            "detected": detected_view,
            "compiler_wrapper": compiler_wrapper_view,
            "build_flags_per_package": serde_json::Value::Object(per_package_flags),
        });
        let config_view = crate::config_glue::config_view_json(inputs.config);
        let patches_view = crate::patch_glue::patch_view_json(inputs.active_patches);
        let source_replacements_view = crate::patch_glue::source_replacement_view_json(
            &inputs.config.source_replacements,
            inputs.no_patches,
        );

        let mut ports: Vec<PortView<'_>> = inputs
            .ports
            .iter()
            .map(|prepared| {
                let cabin_port::PortProvenance {
                    url,
                    sha256_hex,
                    strip_prefix,
                    overlay_manifest,
                } = &prepared.provenance;
                PortView {
                    name: prepared.name.as_str(),
                    version: prepared.version.to_string(),
                    port_dir: match &prepared.origin {
                        cabin_port::PortOrigin::PortDir(p) => p.as_path(),
                        cabin_port::PortOrigin::Builtin(_) => {
                            todo!("builtin-origin ports are surfaced in cabin metadata in Task 8")
                        }
                    },
                    source_dir: prepared.source_dir.as_path(),
                    source: PortSourceView::Archive {
                        url: url.as_str(),
                        sha256: format!("sha256:{sha256_hex}"),
                        strip_prefix: strip_prefix.as_deref(),
                    },
                    overlay_manifest: overlay_manifest
                        .as_deref()
                        .expect("PortDir origin always has an overlay_manifest path"),
                }
            })
            .collect();
        ports.sort_by(|a, b| a.port_dir.cmp(b.port_dir));

        Self {
            workspace,
            packages,
            lockfile: None,
            target_platform: TargetPlatformView::from_platform(&host_platform),
            profiles,
            toolchain: toolchain_view,
            config: config_view,
            patches: patches_view,
            source_replacements: source_replacements_view,
            ports,
        }
    }
}
