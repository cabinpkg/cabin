use crate::error::ManifestError;
use crate::raw::{RawDependency, RawManifest, RawPackage, RawTarget};
use cabin_core::{Dependency, DependencyKind, Package, PackageName, SystemDependency};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

mod dependency;
mod profile;
mod target;
#[cfg(test)]
mod tests;

use self::dependency::route_dependency_from_raw;
use self::profile::{
    build_flags_decl_from_raw_ref, compiler_wrapper_request_from_raw_build_ref, features_from_raw,
    patch_settings_from_raw, profiles_from_raw, toolchain_decl_from_raw_ref,
};
use self::target::{
    RawConditionalTarget, is_cfg_expression, parse_conditional_target_entry, parse_target_table,
    target_from_raw,
};

/// A `cabin.toml` after parsing.
///
/// Either or both of `package` and `workspace` may be present:
/// - a regular package manifest has `package = Some(...)`, `workspace = None`;
/// - a pure workspace root has `package = None`, `workspace = Some(...)`;
/// - a workspace root that is also a package has both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedManifest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<Package>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceTable>,
    #[serde(default, skip_serializing_if = "RootSettings::is_empty")]
    pub root_settings: RootSettings,
}

/// Root-manifest policy settings that apply even when the root
/// manifest is a pure `[workspace]` manifest with no `[package]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RootSettings {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
    #[serde(
        default,
        skip_serializing_if = "cabin_core::ToolchainSettings::is_empty"
    )]
    pub toolchain: cabin_core::ToolchainSettings,
    #[serde(
        default,
        skip_serializing_if = "cabin_core::CompilerWrapperManifestSettings::is_empty"
    )]
    pub compiler_wrapper: cabin_core::CompilerWrapperManifestSettings,
    #[serde(
        default,
        skip_serializing_if = "cabin_core::PatchManifestSettings::is_empty"
    )]
    pub patches: cabin_core::PatchManifestSettings,
}

impl RootSettings {
    pub(crate) fn is_empty(&self) -> bool {
        self.profiles.is_empty()
            && self.toolchain.is_empty()
            && self.compiler_wrapper.is_empty()
            && self.patches.is_empty()
    }
}

/// `[workspace]` table contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceTable {
    /// Member patterns as written in the manifest. Resolution against the
    /// filesystem (including glob expansion) is `cabin-workspace`'s job.
    pub members: Vec<String>,
    /// Paths or `pattern/*` globs that are *not* workspace
    /// members even when `members` would otherwise match them.
    /// Filesystem resolution lives in `cabin-workspace`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Subset of `members` operated on by default when the
    /// user passes no package-selection flags at the workspace root.
    /// Each entry must resolve to a member after `members`/`exclude`
    /// Expansion — `cabin-workspace` enforces this.
    #[serde(
        default,
        rename = "default-members",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub default_members: Vec<String>,
    /// Shared `[workspace.dependencies]` (normal-kind) requirements
    /// that members may opt into via `dep = { workspace = true }`
    /// inside `[dependencies]`. Stored as the original requirement
    /// strings; `cabin-workspace` parses them at member load time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
    /// Shared `[workspace.dev-dependencies]`. Members opt in via
    /// `dep = { workspace = true }` inside `[dev-dependencies]`.
    #[serde(
        default,
        rename = "dev-dependencies",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub dev_dependencies: BTreeMap<String, String>,
}

/// Read and parse `cabin.toml` from `path`.
///
/// Errors from the TOML parser are wrapped in
/// [`ManifestError::TomlAt`] so the diagnostic layer can render
/// a source-annotated snippet pointing at the offending region.
///
/// # Errors
/// Returns [`ManifestError::Io`] when `path` cannot be read. TOML
/// syntax/deserialization failures are returned as
/// [`ManifestError::TomlAt`] (the source-annotated form of
/// [`ManifestError::Toml`]); all other validation failures from
/// [`parse_manifest_str`] are propagated unchanged.
pub fn load_manifest(path: impl AsRef<Path>) -> Result<ParsedManifest, ManifestError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_manifest_str(&text).map_err(|err| match err {
        ManifestError::Toml(source) => ManifestError::TomlAt(Box::new(
            crate::error::ManifestParseError::from_toml(path.to_path_buf(), text, source),
        )),
        other => other,
    })
}

/// Parse the contents of a `cabin.toml` from an in-memory string.
///
/// # Errors
/// Returns [`ManifestError::Toml`] when `input` is not valid TOML
/// or fails deserialization into the raw manifest schema, and
/// propagates the validation variants of [`ManifestError`] raised
/// while converting the raw manifest (e.g.
/// [`ManifestError::EmptyManifest`] when neither `[package]` nor
/// `[workspace]` is present, plus the dependency, target, profile,
/// toolchain, and patch validation errors).
pub fn parse_manifest_str(input: &str) -> Result<ParsedManifest, ManifestError> {
    let raw: RawManifest = toml::from_str(input)?;
    parsed_from_raw(raw)
}

/// Split the unified `[profile]` parent table into the legacy
/// `(top-level flags, named variants)` pair the rest of the parser
/// already operates on. The base-flag fields live directly on
/// `[profile]`; named profiles live in
/// [`crate::raw::RawProfileTable::variants`].
fn split_profile_table(
    table: Option<crate::raw::RawProfileTable>,
) -> (
    Option<crate::raw::RawProfileFlags>,
    BTreeMap<String, crate::raw::RawProfile>,
) {
    let Some(t) = table else {
        return (None, BTreeMap::new());
    };
    let has_base_flags = !t.defines.is_empty()
        || !t.include_dirs.is_empty()
        || !t.cflags.is_empty()
        || !t.cxxflags.is_empty()
        || !t.ldflags.is_empty()
        || !t.link_libs.is_empty()
        || t.cache.is_some();
    let base = if has_base_flags {
        Some(crate::raw::RawProfileFlags {
            defines: t.defines,
            include_dirs: t.include_dirs,
            cflags: t.cflags,
            cxxflags: t.cxxflags,
            ldflags: t.ldflags,
            link_libs: t.link_libs,
            cache: t.cache,
        })
    } else {
        None
    };
    (base, t.variants)
}

fn parsed_from_raw(raw: RawManifest) -> Result<ParsedManifest, ManifestError> {
    let RawManifest {
        package,
        target,
        dependencies,
        dev_dependencies,
        workspace,
        features,
        profile,
        toolchain,
        patch,
    } = raw;
    let (build, profile) = split_profile_table(profile);
    let patches = patch_settings_from_raw(patch)?;
    let profiles = profiles_from_raw(profile)?;
    let toolchain_decl = toolchain
        .as_ref()
        .map(toolchain_decl_from_raw_ref)
        .transpose()?
        .unwrap_or_default();
    let general_wrapper_request = build
        .as_ref()
        .map(|raw| compiler_wrapper_request_from_raw_build_ref(raw, "[profile.cache]"))
        .transpose()?
        .flatten();
    let build_decl = build
        .as_ref()
        .map(build_flags_decl_from_raw_ref)
        .transpose()?
        .unwrap_or_default();

    if package.is_none() && workspace.is_none() {
        return Err(ManifestError::EmptyManifest);
    }

    // Split `[target.*]` entries into two groups:
    //
    // 1. Target-specific dependency tables — entry name is a
    //    `cfg(...)` expression. Their values are conditional dep
    //    tables (`dependencies`, `dev-dependencies`), plus
    //    `toolchain` / `profile` overrides. Anything else under
    //    such an entry is rejected.
    // 2. Buildable C/C++ targets — entry name is a target
    //    identifier and the value is a `RawTarget`-shaped table.
    //    These must *not* contain conditional dep sub-tables;
    //    that mistake surfaces with a clear "not supported"
    //    error so users do not silently lose deps to the wrong
    //    schema.
    let mut conditional_targets: Vec<RawConditionalTarget> = Vec::new();
    let mut buildable_targets: BTreeMap<String, toml::Value> = BTreeMap::new();
    for (raw_target_name, raw_value) in target {
        if is_cfg_expression(&raw_target_name) {
            conditional_targets.push(parse_conditional_target_entry(&raw_target_name, raw_value)?);
        } else {
            // Reject buildable target tables that contain dep
            // sub-tables. These are almost always typos of the
            // `cfg(...)` form (e.g. forgetting the quotes).
            if let Some(table) = raw_value.as_table() {
                for forbidden in ["dependencies", "dev-dependencies", "system-dependencies"] {
                    if table.contains_key(forbidden) {
                        return Err(ManifestError::TargetSpecificDependenciesNotSupported {
                            section: format!("[target.{raw_target_name}.{forbidden}]"),
                        });
                    }
                }
            }
            buildable_targets.insert(raw_target_name, raw_value);
        }
    }
    let target = parse_target_table(buildable_targets)?;

    // Reject `cfg(feature = ...)` on tables where it can't be honored,
    // before the package / workspace-root split. Both paths capture
    // conditional toolchain / `profile.cache` settings (the root via
    // `root_settings_from_parts`), and those are evaluated with an
    // empty feature set — so a feature-gated entry would be silently
    // ignored. Running the check here covers a pure workspace root,
    // which never reaches `project_from_raw`.
    reject_unsupported_feature_conditions(&conditional_targets)?;

    let root_settings = root_settings_from_parts(
        profiles.clone(),
        toolchain_decl.clone(),
        general_wrapper_request,
        patches.clone(),
        &conditional_targets,
    )?;

    let package = if let Some(raw_project) = package {
        Some(project_from_raw(ProjectFromRawInput {
            package: raw_project,
            targets: target,
            dependencies,
            dev_dependencies,
            conditional_targets,
            raw_features: features,
            profiles,
            toolchain_general: toolchain_decl,
            build_general: build_decl,
            general_wrapper_request,
            patches,
        })?)
    } else {
        // No [package]: there must be no [target.*] / [dependencies] tables either.
        if !target.is_empty() {
            return Err(ManifestError::EmptyManifest);
        }
        // Conditional dep tables without a `[package]` are
        // ignored, like the unconditional ones.
        let _ = conditional_targets;
        // Profile / toolchain / build tables in a pure-
        // workspace root have no package to apply against
        // locally; the workspace loader passes them down to
        // members or, for toolchain settings, applies them
        // workspace-wide.
        let _ = profiles;
        let _ = toolchain_decl;
        let _ = build_decl;
        let _ = general_wrapper_request;
        let _ = patches;
        // Dependency tables without [package] are silently ignored — a pure
        // workspace root has nothing to apply them to. The [workspace.*]
        // tables below still flow through.
        None
    };

    let workspace = workspace.map(|w| WorkspaceTable {
        members: w.members,
        exclude: w.exclude,
        default_members: w.default_members,
        dependencies: w.dependencies,
        dev_dependencies: w.dev_dependencies,
    });

    Ok(ParsedManifest {
        package,
        workspace,
        root_settings,
    })
}

/// Reject `cfg(feature = ...)` on conditional tables that cannot honor
/// it. Feature conditions are only meaningful on flag (`.profile`)
/// tables: feature resolution walks the dependency graph, so a feature
/// gating a dependency would be circular, and toolchain / compiler-
/// wrapper (`profile.cache`) selection is evaluated platform-only (with
/// an empty feature set), so a feature-gated entry there would silently
/// never match. Called before the package / workspace-root split so it
/// applies to both (a pure workspace root still captures conditional
/// toolchain / `profile.cache` settings via `root_settings_from_parts`).
fn reject_unsupported_feature_conditions(
    conditional_targets: &[RawConditionalTarget],
) -> Result<(), ManifestError> {
    for cond_target in conditional_targets {
        if !cond_target.condition.references_feature() {
            continue;
        }
        let condition = cond_target.condition.to_string();
        if !cond_target.deps.is_empty() {
            return Err(ManifestError::FeatureConditionNotAllowedHere {
                condition,
                table: "dependencies",
            });
        }
        if !cond_target.dev_deps.is_empty() {
            return Err(ManifestError::FeatureConditionNotAllowedHere {
                condition,
                table: "dev-dependencies",
            });
        }
        if cond_target.toolchain.is_some() {
            return Err(ManifestError::FeatureConditionNotAllowedHere {
                condition,
                table: "toolchain",
            });
        }
        if cond_target
            .profile
            .as_ref()
            .is_some_and(|profile| profile.cache.is_some())
        {
            return Err(ManifestError::FeatureConditionNotAllowedHere {
                condition,
                table: "profile.cache",
            });
        }
    }
    Ok(())
}

fn root_settings_from_parts(
    profiles: BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
    toolchain_general: cabin_core::ToolchainDecl,
    general_wrapper_request: Option<cabin_core::CompilerWrapperRequest>,
    patches: cabin_core::PatchManifestSettings,
    conditional_targets: &[RawConditionalTarget],
) -> Result<RootSettings, ManifestError> {
    let toolchain = cabin_core::ToolchainSettings {
        general: toolchain_general,
        conditional: conditional_toolchains_from_raw(conditional_targets)?,
    };
    let compiler_wrapper = cabin_core::CompilerWrapperManifestSettings {
        general: general_wrapper_request,
        conditional: conditional_wrappers_from_raw(conditional_targets)?,
    };
    Ok(RootSettings {
        profiles,
        toolchain,
        compiler_wrapper,
        patches,
    })
}

fn conditional_toolchains_from_raw(
    conditional_targets: &[RawConditionalTarget],
) -> Result<Vec<cabin_core::ConditionalToolchainDecl>, ManifestError> {
    let mut conditional_toolchains: Vec<cabin_core::ConditionalToolchainDecl> = Vec::new();
    for cond_target in conditional_targets {
        if let Some(raw_tool) = &cond_target.toolchain {
            let decl = toolchain_decl_from_raw_ref(raw_tool)?;
            if !decl.is_empty() {
                conditional_toolchains.push(cabin_core::ConditionalToolchainDecl {
                    condition: cond_target.condition.clone(),
                    toolchain: decl,
                });
            }
        }
    }
    Ok(conditional_toolchains)
}

fn conditional_wrappers_from_raw(
    conditional_targets: &[RawConditionalTarget],
) -> Result<Vec<cabin_core::ConditionalCompilerWrapperDecl>, ManifestError> {
    let mut conditional_wrappers: Vec<cabin_core::ConditionalCompilerWrapperDecl> = Vec::new();
    for cond_target in conditional_targets {
        if let Some(raw_profile) = &cond_target.profile {
            let section = format!(
                "[target.'cfg({condition})'.profile.cache]",
                condition = cond_target.condition
            );
            if let Some(request) =
                compiler_wrapper_request_from_raw_build_ref(raw_profile, &section)?
            {
                conditional_wrappers.push(cabin_core::ConditionalCompilerWrapperDecl {
                    condition: cond_target.condition.clone(),
                    request,
                });
            }
        }
    }
    Ok(conditional_wrappers)
}

/// Bundled inputs for [`project_from_raw`].
///
/// `cabin.toml` parsing pulls every top-level table out of the
/// deserialized raw shape, then hands them all to one final
/// resolution step. The struct keeps that hand-off readable and
/// lets new top-level tables land without rewriting positional
/// argument lists at every call site.
struct ProjectFromRawInput {
    package: RawPackage,
    targets: BTreeMap<String, RawTarget>,
    dependencies: BTreeMap<String, RawDependency>,
    dev_dependencies: BTreeMap<String, RawDependency>,
    conditional_targets: Vec<RawConditionalTarget>,
    raw_features: BTreeMap<String, Vec<String>>,
    profiles: BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
    toolchain_general: cabin_core::ToolchainDecl,
    build_general: cabin_core::ProfileFlags,
    general_wrapper_request: Option<cabin_core::CompilerWrapperRequest>,
    patches: cabin_core::PatchManifestSettings,
}

fn project_from_raw(input: ProjectFromRawInput) -> Result<Package, ManifestError> {
    let ProjectFromRawInput {
        package,
        targets,
        dependencies,
        dev_dependencies,
        conditional_targets,
        raw_features,
        profiles,
        toolchain_general,
        build_general,
        general_wrapper_request,
        patches,
    } = input;
    let RawPackage { name, version } = package;

    let package_name = PackageName::new(name)?;
    let parsed_version =
        semver::Version::parse(&version).map_err(|source| ManifestError::Version {
            value: version,
            source,
        })?;

    let mut target_models = Vec::with_capacity(targets.len());
    for (target_name, raw_target) in targets {
        target_models.push(target_from_raw(target_name, raw_target)?);
    }

    let (dep_models, system_models) =
        collect_dependency_models(dependencies, dev_dependencies, &conditional_targets)?;

    let features = features_from_raw(raw_features);

    // Collect target-conditioned [target.'cfg(...)'.toolchain] /
    // [target.'cfg(...)'.profile] entries alongside the conditional
    // dependency tables so the typed Package carries the full
    // settings struct.
    let conditional_toolchains = conditional_toolchains_from_raw(&conditional_targets)?;
    let mut conditional_build_flags: Vec<cabin_core::ConditionalProfileFlags> = Vec::new();
    let conditional_wrappers = conditional_wrappers_from_raw(&conditional_targets)?;
    for cond_target in &conditional_targets {
        if let Some(raw_profile) = &cond_target.profile {
            let decl = build_flags_decl_from_raw_ref(raw_profile)?;
            if !decl.is_empty() {
                conditional_build_flags.push(cabin_core::ConditionalProfileFlags {
                    condition: cond_target.condition.clone(),
                    flags: decl,
                });
            }
        }
    }

    let toolchain_settings = cabin_core::ToolchainSettings {
        general: toolchain_general,
        conditional: conditional_toolchains,
    };
    let build_settings = cabin_core::ProfileSettings {
        general: build_general,
        conditional: conditional_build_flags,
    };
    let compiler_wrapper_settings = cabin_core::CompilerWrapperManifestSettings {
        general: general_wrapper_request,
        conditional: conditional_wrappers,
    };

    Ok(Package::with_config(cabin_core::PackageConfigInput {
        name: package_name,
        version: parsed_version,
        targets: target_models,
        dependencies: dep_models,
        system_dependencies: system_models,
        features,
    })?
    .with_profiles(profiles)
    .with_toolchain(toolchain_settings)
    .with_build(build_settings)
    .with_compiler_wrapper(compiler_wrapper_settings)
    .with_patches(patches))
}

/// Collect kinded package dependencies. Iteration is sorted
/// by (kind, name) so callers see deterministic output.
/// Unconditional dependency tables come first, then each
/// conditional `[target.'cfg(...)'.<kind>]` block in
/// declaration order — but each conditional dep carries its
/// own `Condition`, so consumers filter at iteration time.
///
/// Each entry routes to one of two homes based on the
/// `system` flag on its table form:
///   - `system = true` → typed `SystemDependency` value
///     (probed via pkg-config at build time), routed onto
///     `Package.system_dependencies` and carrying the
///     surrounding `DependencyKind` so per-kind activation
///     filtering matches the Cabin-package rules.
///   - default (or `system = false`) → typed `Dependency`
///     value, routed onto `Package.dependencies`.
fn collect_dependency_models(
    dependencies: BTreeMap<String, RawDependency>,
    dev_dependencies: BTreeMap<String, RawDependency>,
    conditional_targets: &[RawConditionalTarget],
) -> Result<(Vec<Dependency>, Vec<SystemDependency>), ManifestError> {
    let unconditional_capacity = dependencies.len() + dev_dependencies.len();
    let conditional_capacity: usize = conditional_targets
        .iter()
        .map(|t| t.deps.len() + t.dev_deps.len())
        .sum();
    let mut dep_models: Vec<Dependency> =
        Vec::with_capacity(unconditional_capacity + conditional_capacity);
    let mut system_models: Vec<SystemDependency> = Vec::new();
    for (kind, raw_table) in [
        (DependencyKind::Normal, dependencies),
        (DependencyKind::Dev, dev_dependencies),
    ] {
        for (dep_name, raw_dep) in raw_table {
            route_dependency_from_raw(
                dep_name,
                raw_dep,
                kind,
                None,
                &mut dep_models,
                &mut system_models,
            )?;
        }
    }
    for cond_target in conditional_targets {
        let condition = Some(cond_target.condition.clone());
        for (kind, raw_table) in [
            (DependencyKind::Normal, &cond_target.deps),
            (DependencyKind::Dev, &cond_target.dev_deps),
        ] {
            for (dep_name, raw_dep) in raw_table {
                route_dependency_from_raw(
                    dep_name.clone(),
                    raw_dep.clone(),
                    kind,
                    condition.clone(),
                    &mut dep_models,
                    &mut system_models,
                )?;
            }
        }
    }
    Ok((dep_models, system_models))
}
