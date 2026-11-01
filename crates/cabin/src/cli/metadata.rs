//! `cabin metadata`: [`metadata`] resolves the workspace, and
//! [`MetadataView`] is the serializable view assembled from the
//! resolved typed inputs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use cabin_core::{DependencySource, Package};
use cabin_lockfile::Lockfile;
use cabin_workspace::PackageGraph;

use crate::cli::build_prep::{
    InspectionPrep, resolve_compiler_wrapper_layered, resolve_inspection_prep,
};
use crate::cli::term_verbosity::Reporter;
use crate::cli::{
    ManifestArgs, ResolveFormat, augment_build_flags, build_selection_request,
    build_workspace_selection, compiler_wrapper_override_from_args, compute_feature_resolution,
    lockfile_path_for, read_optional_lockfile, resolve_build_configurations,
    resolve_invocation_manifest, resolve_per_package_build_flags,
    workspace_compiler_wrapper_settings,
};

/// Top-level `cabin metadata --format json` document.
#[derive(Serialize)]
pub(crate) struct MetadataView<'a> {
    workspace: Option<WorkspaceView<'a>>,
    pub(crate) packages: Vec<PackageView<'a>>,
    lockfile: Option<LockfileView<'a>>,
    /// The platform `[target.'cfg(...)'.<kind>]` predicates are
    /// evaluated for: the `--target` triple, or the build machine.
    /// Lets consumers see why a given dep is active or inactive.
    target_platform: TargetPlatformView,
    /// Build-profile context: the resolved `selected` profile
    /// plus every available profile name, plus the parsed
    /// definitions consumers can use to recompute fields without
    /// re-reading the manifest.
    profiles: ProfilesView,
    /// Resolved C/C++ toolchain plus per-tool source.  Always
    /// populated so consumers can see which compiler / archiver
    /// a build would use without rerunning `cabin build`.
    toolchain: serde_json::Value,
    /// Loaded config files plus every effective config-derived
    /// setting.  Always present (even when no files were loaded)
    /// so consumers can distinguish "config absent" from "config
    /// silent" without re-deriving discovery.
    config: serde_json::Value,
    /// Active patch entries after manifest+config merging and
    /// validation.  Empty array when no patches apply.
    patches: serde_json::Value,
    /// Active source-replacement entries from the merged
    /// effective config.  Empty array when none apply.
    source_replacements: serde_json::Value,
    /// The root's `[system-provided]` table as declared, keyed by
    /// package name.  What a build's probe finds is not shown.
    system_provided:
        &'a std::collections::BTreeMap<cabin_core::PackageName, cabin_core::SystemProvider>,
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
    /// name in deterministic order.  Omitted when the manifest
    /// declares none.
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
    /// Omitted when none are declared.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    default_members: Vec<&'a str>,
    /// Directory paths the loader removed via
    /// `[workspace.exclude]`, normalized relative to the workspace
    /// root.  Omitted when no excludes are declared.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    excluded_members: Vec<&'a Path>,
    /// Members the user requested via CLI flags (or the
    /// documented "current package" fallback).  Sorted by package
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
    /// without re-parsing the manifest.  The list is sorted by
    /// `(dependency_kind, name)` for deterministic output.
    dependencies: Vec<DependencyView<'a>>,
    /// `system = true` declarations.  Externally provided
    /// (system libraries, SDKs, installed tools) - never resolved
    /// through the Cabin registry.  Sorted by name.  Omitted when
    /// no system dependencies are declared.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system_dependencies: Vec<SystemDependencyView<'a>>,
    targets: &'a [cabin_core::Target],
    pub(crate) is_root: bool,
    pub(crate) is_primary: bool,
    /// Declared `[features]`.  Omitted when the manifest has no
    /// features.
    #[serde(skip_serializing_if = "Option::is_none")]
    features: Option<&'a cabin_core::Features>,
    /// Resolved per-package configuration.  Present only when the
    /// package declares features or language standards.
    #[serde(skip_serializing_if = "Option::is_none")]
    configuration: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct DependencyView<'a> {
    name: &'a str,
    /// Manifest section the dependency was declared in.  Always
    /// emitted (even for normal dependencies) so consumers do not
    /// have to special-case the implicit-default case.
    dependency_kind: cabin_core::DependencyKind,
    /// Whether the dependency is optional.  Omitted when `false`.
    #[serde(skip_serializing_if = "is_false_dep")]
    optional: bool,
    /// Per-edge feature requests on the dependency package.
    /// Omitted when empty.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    features: &'a [String],
    /// Whether this edge requests the dependency's `default`
    /// feature.  Omitted when `true` (the documented default).
    #[serde(skip_serializing_if = "is_true_dep")]
    default_features: bool,
    /// Canonical inner-expression form of an optional `cfg(...)`
    /// predicate copied from the manifest.  Omitted when no
    /// `[target.'cfg(...)']` table guarded this dependency.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    /// Whether the `target` predicate matches the platform in
    /// `target_platform`.  `true` for unconditional dependencies.
    /// Always emitted so consumers can decide whether to surface
    /// the dependency without re-evaluating the predicate.
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
    /// An unresolved `{ workspace = true }` opt-in.  The
    /// workspace loader normally rewrites these into `Path` /
    /// `Version` before metadata is serialized, so this variant
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
    /// predicate copied from the manifest.  Omitted when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    /// Whether the `target` predicate matches the platform in
    /// `target_platform`.  `true` for unconditional system
    /// declarations.
    active: bool,
}

/// Bundle of inputs the metadata JSON view needs.
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
    /// The platform `cfg(...)` conditions evaluate for.
    pub(crate) platform: &'a cabin_core::TargetPlatform,
    pub(crate) build_flags: &'a HashMap<usize, cabin_core::ResolvedProfileFlags>,
    pub(crate) detection: Option<&'a cabin_core::ToolchainDetectionReport>,
    pub(crate) sysroot: Option<&'a crate::cli::build_prep::SysrootChoice>,
    /// Resolved compiler wrapper, if any. `None` is rendered
    /// as `toolchain.compiler_wrapper = null` so consumers do not
    /// have to special-case the absence.
    pub(crate) compiler_wrapper: Option<&'a cabin_core::ResolvedCompilerWrapper>,
    /// Merged effective config.  Surfaced as a top-level `config`
    /// block so consumers can audit which files contributed and
    /// which effective values came from the config layer vs.  CLI
    /// vs. env vs. manifest defaults.
    pub(crate) config: &'a cabin_config::EffectiveConfig,
    /// Effective `[resolver] incompatible-standards` value and the
    /// layer it came from (env > config > builtin default).  Resolved
    /// in the command entry because the env read is fallible; the view
    /// builders stay infallible and just render it.
    pub(crate) resolver_incompatible_standards: (
        cabin_core::IncompatibleStandards,
        cabin_core::ConfigValueSource,
    ),
    /// Build-job count a build without `-j` would use, and its layer
    /// (env > config); `None` leaves the count to the build backend.
    pub(crate) build_jobs: Option<(cabin_core::BuildJobs, cabin_core::ConfigValueSource)>,
    /// Color choice and verbosity a command run without terminal flags
    /// uses.
    pub(crate) terminal: crate::term_setup::TerminalSettings,
    /// Active patch set after manifest+config merging and
    /// validation.  Empty when no patches apply.
    pub(crate) active_patches: &'a cabin_workspace::ActivePatchSet,
    /// Whether `--no-patches` was supplied on the CLI.  Used to
    /// suppress the source-replacement view when the user
    /// disabled the local-policy layer entirely.
    pub(crate) no_patches: bool,
}

/// Build the JSON view for a single workspace package.
fn build_package_view<'a>(
    graph: &PackageGraph,
    idx: usize,
    pkg: &'a cabin_workspace::WorkspacePackage,
    configurations: &HashMap<usize, cabin_core::BuildConfiguration>,
    platform: &cabin_core::TargetPlatform,
) -> PackageView<'a> {
    let package: &Package = &pkg.package;
    let features = if package.features.default.is_empty() && package.features.features.is_empty() {
        None
    } else {
        Some(&package.features)
    };
    // The configuration block appears once a package declares
    // something it resolves: features or language standards.
    let declares_language =
        !package.language.is_empty() || package.targets.iter().any(|t| !t.language.is_empty());
    let configuration = if features.is_some() || declares_language {
        configurations
            .get(&idx)
            .map(cabin_core::BuildConfiguration::as_json)
    } else {
        None
    };
    let mut system_dependencies: Vec<SystemDependencyView<'_>> = package
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
                .is_none_or(|c| c.evaluate(&cabin_core::ConditionContext::platform_only(platform))),
        })
        .collect();
    // The stable sorts below uphold the documented ordering contract
    // on `PackageView` even when conditional (target-specific)
    // declarations sit interleaved in manifest order; ties keep
    // declaration order.
    system_dependencies.sort_by(|a, b| a.name.cmp(b.name));
    let mut dependencies: Vec<DependencyView<'_>> = package
        .dependencies
        .iter()
        .map(|d| DependencyView {
            name: d.name.as_str(),
            dependency_kind: d.kind,
            optional: d.optional,
            features: d.features.as_slice(),
            default_features: d.default_features,
            target: d.condition.as_ref().map(ToString::to_string),
            active: d.matches_platform(platform),
            source: match &d.source {
                DependencySource::Path(p) => DependencySourceView::Path {
                    path: p.as_std_path(),
                },
                DependencySource::Version(req) => DependencySourceView::Version {
                    requirement: req.to_string(),
                },
                DependencySource::Workspace => DependencySourceView::Workspace,
            },
        })
        .collect();
    dependencies.sort_by(|a, b| (a.dependency_kind, a.name).cmp(&(b.dependency_kind, b.name)));
    PackageView {
        name: package.name.as_str(),
        version: package.version.to_string(),
        manifest_path: &pkg.manifest_path,
        dependencies,
        system_dependencies,
        targets: &package.targets,
        is_root: graph.root_package == Some(idx),
        is_primary: graph.primary_packages.contains(&idx),
        features,
        configuration,
    }
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
                    source: cabin_lockfile::PACKAGE_SOURCE_INDEX,
                    checksum: p.checksum.as_ref().map(cabin_core::Checksum::as_str),
                    dependencies: p
                        .dependencies
                        .iter()
                        .map(cabin_core::PackageName::as_str)
                        .collect(),
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
        let platform = inputs.platform;
        let workspace = if graph.is_workspace_root {
            let mut members: Vec<&str> = graph
                .primary_packages
                .iter()
                .map(|i| graph.packages[*i].package.name.as_str())
                .collect();
            members.sort_unstable();
            let mut default_members: Vec<&str> = graph
                .default_members
                .iter()
                .map(|i| graph.packages[*i].package.name.as_str())
                .collect();
            default_members.sort_unstable();
            let mut excluded_members: Vec<&Path> = graph
                .excluded_members
                .iter()
                .map(std::path::PathBuf::as_path)
                .collect();
            excluded_members.sort();
            let mut selected_packages: Vec<&str> = selection
                .packages
                .iter()
                .map(|i| graph.packages[*i].package.name.as_str())
                .collect();
            selected_packages.sort_unstable();
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
            .map(|(idx, pkg)| build_package_view(graph, idx, pkg, configurations, platform))
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
        // a per-package summary of active build flags.
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
        // Detected toolchain identity / capabilities; `null` when
        // detection failed or did not run.
        let detected_view = match detection {
            Some(report) => report.as_json(),
            None => serde_json::Value::Null,
        };
        let compiler_wrapper_view = match inputs.compiler_wrapper {
            Some(w) => w.as_json(),
            None => serde_json::Value::Null,
        };
        // `source` tells a configured sysroot (`cli` or a config
        // file's source) from an Apple SDK Cabin found (`detected`).
        let sysroot_view = match inputs.sysroot {
            Some(choice) => serde_json::json!({
                "path": choice.sysroot.path.as_str(),
                "source": choice.source,
            }),
            None => serde_json::Value::Null,
        };
        let toolchain_view = serde_json::json!({
            "tools": toolchain_resolved.as_json(),
            "detected": detected_view,
            "sysroot": sysroot_view,
            "compiler_wrapper": compiler_wrapper_view,
            "build_flags_per_package": serde_json::Value::Object(per_package_flags),
        });
        let config_view = crate::cli::config::config_view_json(
            inputs.config,
            inputs.resolver_incompatible_standards,
            inputs.build_jobs,
            inputs.terminal,
        );
        let patches_view = crate::cli::patch::patch_view_json(inputs.active_patches);
        let source_replacements_view = crate::cli::patch::source_replacement_view_json(
            &inputs.config.source_replacements,
            inputs.no_patches,
        );

        Self {
            workspace,
            packages,
            lockfile: None,
            target_platform: TargetPlatformView::from_platform(platform),
            profiles,
            toolchain: toolchain_view,
            config: config_view,
            patches: patches_view,
            source_replacements: source_replacements_view,
            system_provided: &graph.root_settings.system_provided.entries,
        }
    }
}

/// Run `cabin metadata`: resolve the workspace and emit the JSON
/// (or minimal human) view assembled by [`MetadataView`].
pub(crate) fn metadata(args: &ManifestArgs, reporter: Reporter) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let crate::cli::workspace_prep::WorkspacePrep {
        effective_config,
        active_patches,
        graph,
    } = crate::cli::workspace_prep::load_workspace_and_patches(&manifest_path, args.no_patches)?;
    // `cabin metadata` never reaches the network, but reject
    // `--offline` paired with a URL registry source so the
    // metadata view documents the same offline contract the
    // build / fetch / resolve commands enforce.
    let resolved_index_for_offline_check =
        crate::cli::config::resolve_index_source(None, None, &effective_config)?;
    let metadata_offline = crate::cli::config::effective_offline(args.offline)?;
    crate::cli::config::enforce_offline_index_source(
        metadata_offline,
        resolved_index_for_offline_check.as_ref(),
    )?;
    let lockfile_path = lockfile_path_for(&manifest_path);
    let lockfile = read_optional_lockfile(&lockfile_path)?;
    let request = build_selection_request(
        &args.selection.features,
        args.selection.all_features,
        args.selection.no_default_features,
    );
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;
    // Run the cross-package feature resolver so unknown features,
    // `dep:` entries on non-optional deps, and other feature-graph
    // errors surface here and in `cabin build`.
    let platform = args.toolchain.platform();
    let feature_resolution = compute_feature_resolution(
        &graph,
        &resolved_selection,
        &request,
        Some(&platform),
        &BTreeSet::new(),
    )?;
    // The detection report reaches the JSON view as `null` when
    // subprocess detection fails.
    let InspectionPrep {
        manifest_profiles,
        profile,
        toolchain,
        detection: detection_report,
        sysroot,
    } = resolve_inspection_prep(
        &graph,
        args.profile.as_deref(),
        &args.toolchain,
        &platform,
        &effective_config,
        reporter,
    )?;
    // Fails soft on subprocess errors, unlike the build-side
    // resolution, so a missing wrapper executable cannot block
    // inspection of the rest of the workspace.
    let compiler_wrapper = match resolve_compiler_wrapper_layered(
        compiler_wrapper_override_from_args(&args.toolchain)?,
        workspace_compiler_wrapper_settings(&graph).as_ref(),
        &effective_config,
    ) {
        Ok(w) => w,
        Err(err) => {
            reporter.warning(format_args!("compiler-wrapper resolution failed: {err}"));
            None
        }
    };
    let mut toolchain_summary =
        cabin_core::ToolchainSummary::from_resolved_parts(&toolchain, compiler_wrapper.as_ref());
    toolchain_summary.target = args.toolchain.target.as_ref().map(|t| t.target.clone());
    toolchain_summary.sysroot = sysroot.as_ref().and_then(|choice| choice.written.clone());
    let (build_flags, _standard_flag_conflicts) = resolve_per_package_build_flags(
        &graph,
        &profile,
        &platform,
        &feature_resolution,
        detection_report.as_ref(),
    );
    // `cabin metadata` does not opt into dev-dep activation;
    // dev-kind system deps stay declaration-only here so the
    // probe step matches the Cabin-package activation rule.
    let dev_for: BTreeSet<String> = BTreeSet::new();
    let build_flags = augment_build_flags(&graph, &platform, &dev_for, build_flags, reporter)?;
    let configurations = resolve_build_configurations(
        &graph,
        &feature_resolution,
        &profile,
        &toolchain_summary,
        &build_flags,
    )?;
    let resolver_incompatible_standards =
        crate::cli::config::resolve_incompatible_standards_sourced(&effective_config)?;
    // `cabin metadata` has no `-j`; report what a build without one uses.
    let build_jobs = crate::cli::config::resolve_build_jobs_sourced(None, &effective_config)?;
    let terminal = crate::term_setup::configured_terminal_settings()?;
    let view = MetadataView::from_graph_and_lock(&MetadataInputs {
        graph: &graph,
        lockfile: lockfile.as_ref(),
        lockfile_path: &lockfile_path,
        configurations: &configurations,
        selection: &resolved_selection,
        profile: &profile,
        manifest_profiles: &manifest_profiles,
        toolchain: &toolchain,
        platform: &platform,
        build_flags: &build_flags,
        detection: detection_report.as_ref(),
        sysroot: sysroot.as_ref(),
        compiler_wrapper: compiler_wrapper.as_ref(),
        config: &effective_config,
        active_patches: &active_patches,
        no_patches: args.no_patches,
        resolver_incompatible_standards,
        build_jobs,
        terminal,
    });
    match args.format {
        ResolveFormat::Json => {
            crate::print_pretty_json(&view, "failed to serialize metadata as JSON")?;
        }
        ResolveFormat::Human => {
            // Minimal: JSON is the contract for tooling.
            for pkg in &view.packages {
                println!(
                    "{} {} ({})",
                    pkg.name,
                    pkg.version,
                    if pkg.is_root {
                        "root"
                    } else if pkg.is_primary {
                        "primary"
                    } else {
                        "dep"
                    }
                );
            }
        }
    }
    Ok(())
}
