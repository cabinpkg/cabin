use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cabin_core::{
    Condition, Dependency, DependencyKind, DependencySource, Features, Package, PackageName,
    PortDepSource, SystemDependency, Target, TargetKind, TargetName,
};
use serde::{Deserialize, Serialize};

use crate::error::ManifestError;
use crate::raw::{RawDependency, RawDependencyTable, RawManifest, RawPackage, RawTarget};

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
        || t.cache.is_some();
    let base = if has_base_flags {
        Some(crate::raw::RawProfileFlags {
            defines: t.defines,
            include_dirs: t.include_dirs,
            cflags: t.cflags,
            cxxflags: t.cxxflags,
            ldflags: t.ldflags,
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
    //    tables (`dependencies`, `dev-dependencies`,
    //    `system-dependencies`). Anything else under such an
    //    entry is rejected.
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

    let root_settings = root_settings_from_parts(
        profiles.clone(),
        toolchain_decl.clone(),
        general_wrapper_request,
        patches.clone(),
        &conditional_targets,
    )?;

    let package = match package {
        Some(raw_project) => Some(project_from_raw(ProjectFromRawInput {
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
        })?),
        None => {
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
        }
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

    // Collect kinded package dependencies. Iteration is sorted
    // by (kind, name) so callers see deterministic output.
    // Unconditional dependency tables come first, then each
    // conditional `[target.'cfg(...)'.<kind>]` block in
    // declaration order — but each conditional dep carries its
    // own `Condition`, so consumers filter at iteration time.
    //
    // Each entry routes to one of two homes based on the
    // `system` flag on its table form:
    //   - `system = true` → typed `SystemDependency` value
    //     (probed via pkg-config at build time), routed onto
    //     `Package.system_dependencies` and carrying the
    //     surrounding `DependencyKind` so per-kind activation
    //     filtering matches the Cabin-package rules.
    //   - default (or `system = false`) → typed `Dependency`
    //     value, routed onto `Package.dependencies`.
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
    for cond_target in &conditional_targets {
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

/// Validate every `[profile.<name>]` table and convert it into a
/// typed [`cabin_core::ProfileDefinition`]. Errors short-circuit
/// the whole manifest because partial profile state would be
/// surprising downstream.
fn profiles_from_raw(
    raw: BTreeMap<String, crate::raw::RawProfile>,
) -> Result<BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>, ManifestError> {
    let mut out = BTreeMap::new();
    for (name, raw_profile) in raw {
        let pname = cabin_core::ProfileName::new(name.clone())
            .map_err(|err| ManifestError::InvalidProfileName { value: err.0 })?;
        let inherits = raw_profile
            .inherits
            .clone()
            .map(|v| {
                cabin_core::ProfileName::new(v).map_err(|err| {
                    ManifestError::InvalidInheritedProfileName {
                        profile: pname.as_str().to_owned(),
                        value: err.0,
                    }
                })
            })
            .transpose()?;
        let build = profile_flags_from_overrides(&raw_profile, &pname)?;
        out.insert(
            pname.clone(),
            cabin_core::ProfileDefinition {
                name: pname,
                inherits,
                debug: raw_profile.debug,
                opt_level: raw_profile.opt_level,
                assertions: raw_profile.assertions,
                build,
            },
        );
    }
    Ok(out)
}

/// Convert one `[toolchain]` table into a typed
/// [`cabin_core::ToolchainDecl`]. The future-feature `compiler-family`,
/// `compiler-version`, and similar capability-style fields are
/// rejected at the TOML layer because [`crate::raw::RawToolchain`]
/// uses `deny_unknown_fields`.
fn toolchain_decl_from_raw_ref(
    raw: &crate::raw::RawToolchain,
) -> Result<cabin_core::ToolchainDecl, ManifestError> {
    let cc = raw.cc.as_deref().map(parse_tool_spec).transpose()?;
    let cxx = raw.cxx.as_deref().map(parse_tool_spec).transpose()?;
    let ar = raw.ar.as_deref().map(parse_tool_spec).transpose()?;
    Ok(cabin_core::ToolchainDecl { cc, cxx, ar })
}

fn parse_tool_spec(raw: &str) -> Result<cabin_core::ToolSpec, ManifestError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ManifestError::EmptyToolSpec);
    }
    Ok(cabin_core::ToolSpec::parse(trimmed.to_owned()))
}

/// Build a per-profile [`cabin_core::ProfileFlags`] from the
/// flag-override fields declared directly on a `[profile.<name>]`
/// table. Returns `None` when the user supplied no overrides at
/// all; the resolver will then fall back to the base
/// `[profile]` layer for this profile.
///
/// Flag fields are `Option<Vec<...>>` to preserve the distinction
/// between "the user did not override this field" and "the user
/// explicitly set this field to an empty list"; the conversion
/// below collapses both into the legacy `Vec<...>` shape because
/// the typed [`cabin_core::ProfileFlags`] cannot represent that
/// distinction yet. Override-vs-append precedence is documented
/// at the resolver layer.
fn profile_flags_from_overrides(
    raw: &crate::raw::RawProfile,
    pname: &cabin_core::ProfileName,
) -> Result<Option<cabin_core::ProfileFlags>, ManifestError> {
    if raw.defines.is_none()
        && raw.include_dirs.is_none()
        && raw.cflags.is_none()
        && raw.cxxflags.is_none()
        && raw.ldflags.is_none()
    {
        return Ok(None);
    }
    let decl = cabin_core::ProfileFlags {
        defines: raw.defines.clone().unwrap_or_default(),
        include_dirs: raw.include_dirs.clone().unwrap_or_default(),
        cflags: raw.cflags.clone().unwrap_or_default(),
        cxxflags: raw.cxxflags.clone().unwrap_or_default(),
        ldflags: raw.ldflags.clone().unwrap_or_default(),
    };
    decl.validate().map_err(|err| {
        let _ = pname;
        ManifestError::InvalidBuildFlags(err)
    })?;
    Ok(Some(decl))
}

fn build_flags_decl_from_raw_ref(
    raw: &crate::raw::RawProfileFlags,
) -> Result<cabin_core::ProfileFlags, ManifestError> {
    let decl = cabin_core::ProfileFlags {
        defines: raw.defines.clone(),
        include_dirs: raw.include_dirs.clone(),
        cflags: raw.cflags.clone(),
        cxxflags: raw.cxxflags.clone(),
        ldflags: raw.ldflags.clone(),
    };
    decl.validate().map_err(ManifestError::InvalidBuildFlags)?;
    Ok(decl)
}

/// Convert a raw `[patch]` table into typed
/// [`cabin_core::PatchManifestSettings`]. The only supported
/// source kind is `path = "..."`; every other key is rejected
/// by `deny_unknown_fields` on [`crate::raw::RawPatch`].
fn patch_settings_from_raw(
    raw: BTreeMap<String, crate::raw::RawPatch>,
) -> Result<cabin_core::PatchManifestSettings, ManifestError> {
    use cabin_core::{PatchSource, PatchValidationError};

    let mut entries = BTreeMap::new();
    for (name, row) in raw {
        let package = PackageName::new(name).map_err(ManifestError::Validation)?;
        let crate::raw::RawPatch { path } = row;
        let path = path.ok_or_else(|| ManifestError::InvalidPatch {
            package: package.as_str().to_owned(),
            source: PatchValidationError::MissingSource {
                package: package.as_str().to_owned(),
            },
        })?;
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Err(ManifestError::InvalidPatch {
                package: package.as_str().to_owned(),
                source: PatchValidationError::MissingSource {
                    package: package.as_str().to_owned(),
                },
            });
        }
        entries.insert(
            package,
            PatchSource::Path {
                path: PathBuf::from(trimmed),
            },
        );
    }
    Ok(cabin_core::PatchManifestSettings { entries })
}

/// Extract a `[profile.cache] compiler-wrapper = "..."` declaration
/// from a `[profile]` table (or any of the same shape: profile / target-
/// conditioned). Returns `None` when neither `[profile.cache]` nor its
/// `compiler-wrapper` field is present. `section` is the TOML path
/// echoed back in the error message so the user sees exactly which
/// table they need to fix.
fn compiler_wrapper_request_from_raw_build_ref(
    raw: &crate::raw::RawProfileFlags,
    section: &str,
) -> Result<Option<cabin_core::CompilerWrapperRequest>, ManifestError> {
    let Some(cache) = raw.cache.as_ref() else {
        return Ok(None);
    };
    let Some(value) = cache.compiler_wrapper.as_deref() else {
        return Ok(None);
    };
    let request = cabin_core::CompilerWrapperRequest::parse(value).map_err(|source| {
        ManifestError::InvalidCompilerWrapper {
            section: section.to_owned(),
            source,
        }
    })?;
    Ok(Some(request))
}

fn features_from_raw(mut raw: BTreeMap<String, Vec<String>>) -> Features {
    let default = raw
        .remove(cabin_core::DEFAULT_FEATURE_KEY)
        .unwrap_or_default();
    Features {
        default,
        features: raw,
    }
}

fn target_from_raw(name: String, raw: RawTarget) -> Result<Target, ManifestError> {
    let RawTarget {
        kind,
        sources,
        include_dirs,
        defines,
        deps,
    } = raw;

    let target_name = TargetName::new(name.clone())?;
    let kind = parse_target_kind(&name, &kind)?;

    if kind.is_header_only() && !sources.is_empty() {
        return Err(ManifestError::HeaderOnlyDeclaresSources { target: name });
    }

    Ok(Target {
        name: target_name,
        kind,
        sources,
        include_dirs,
        defines,
        deps,
    })
}

/// Raw shape of one `[target.'cfg(...)'.<...>]` entry, after we
/// have decided the entry is a target-conditional dep table
/// (i.e. the outer name is a `cfg(...)` expression). Captured
/// up-front so we can fold these into `Package::dependencies`
/// alongside the unconditional ones.
struct RawConditionalTarget {
    condition: Condition,
    deps: BTreeMap<String, RawDependency>,
    dev_deps: BTreeMap<String, RawDependency>,
    toolchain: Option<crate::raw::RawToolchain>,
    profile: Option<crate::raw::RawProfileFlags>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConditionalTargetTable {
    #[serde(default)]
    dependencies: BTreeMap<String, RawDependency>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: BTreeMap<String, RawDependency>,
    #[serde(default)]
    toolchain: Option<crate::raw::RawToolchain>,
    #[serde(default)]
    profile: Option<crate::raw::RawProfileFlags>,
}

/// Whether a `[target.<NAME>]` entry name should be interpreted
/// as a `cfg(...)` expression. Cabin's existing buildable-target
/// names cannot contain whitespace or parentheses, so the rule
/// is unambiguous: any name that lexically starts with `cfg(`
/// and ends with `)` is treated as a cfg expression.
fn is_cfg_expression(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.starts_with("cfg(") && trimmed.ends_with(')')
}

fn parse_conditional_target_entry(
    raw_target_name: &str,
    raw_value: toml::Value,
) -> Result<RawConditionalTarget, ManifestError> {
    let condition = Condition::parse_cfg(raw_target_name).map_err(|source| {
        ManifestError::InvalidTargetCfg {
            raw: raw_target_name.to_owned(),
            source,
        }
    })?;
    let typed: RawConditionalTargetTable =
        raw_value.try_into().map_err(|err: toml::de::Error| {
            ManifestError::InvalidConditionalTargetTable {
                raw: raw_target_name.to_owned(),
                source: Box::new(err),
            }
        })?;
    Ok(RawConditionalTarget {
        condition,
        deps: typed.dependencies,
        dev_deps: typed.dev_dependencies,
        toolchain: typed.toolchain,
        profile: typed.profile,
    })
}

fn parse_target_table(
    raw: BTreeMap<String, toml::Value>,
) -> Result<BTreeMap<String, RawTarget>, ManifestError> {
    let mut out = BTreeMap::new();
    for (name, value) in raw {
        let typed: RawTarget = value
            .try_into()
            .map_err(|err: toml::de::Error| ManifestError::Toml(err))?;
        out.insert(name, typed);
    }
    Ok(out)
}

/// Inspect `raw` and route it onto either `dep_models`
/// (Cabin-package dependency) or `system_models` (system-sourced
/// dependency, probed via pkg-config at build time). The
/// `system = true` flag on a `RawDependencyTable` is the only
/// signal that selects the system path; bare-string entries
/// (`name = "^1"`) always mean registry source.
fn route_dependency_from_raw(
    name: String,
    raw: RawDependency,
    kind: DependencyKind,
    condition: Option<Condition>,
    dep_models: &mut Vec<Dependency>,
    system_models: &mut Vec<SystemDependency>,
) -> Result<(), ManifestError> {
    if let RawDependency::Table(ref table) = raw
        && table.system
    {
        // Route to the system path. Take ownership for clean
        // destructuring without aliasing the borrow.
        let RawDependency::Table(table) = raw else {
            unreachable!("guarded by matches! above");
        };
        system_models.push(system_dependency_from_raw_table(
            name, table, kind, condition,
        )?);
        return Ok(());
    }
    dep_models.push(package_dependency_from_raw(name, raw, kind, condition)?);
    Ok(())
}

fn package_dependency_from_raw(
    name: String,
    raw: RawDependency,
    kind: DependencyKind,
    condition: Option<Condition>,
) -> Result<Dependency, ManifestError> {
    let section = kind.manifest_section();
    let raw_outcome: (DependencySource, bool, Vec<String>, bool) = match raw {
        RawDependency::String(s) => (
            DependencySource::Version(parse_version_req(&name, &s)?),
            false,
            Vec::new(),
            true,
        ),
        RawDependency::Table(RawDependencyTable {
            path,
            version,
            port,
            port_path,
            workspace,
            system,
            optional,
            features,
            default_features,
        }) => {
            // The router catches `system = true`. Reaching this
            // arm with `system = true` is an internal invariant
            // violation; fail loudly so a future refactor cannot
            // silently drop the system path.
            debug_assert!(!system, "router should have routed system deps");
            if system {
                return Err(ManifestError::SystemConflictsWith {
                    name,
                    section,
                    field: "system",
                    detail: "system = true must be routed before package_dependency_from_raw",
                });
            }

            // `port` / `port-path` are mutually exclusive with every
            // other source form and do not support feature gating
            // for this milestone. Check both conditions before
            // routing through the path/version/workspace
            // selector so a port dep cannot silently shadow a
            // mistakenly-set field.
            let port_builtin = port.unwrap_or(false);
            match (port_builtin, port_path) {
                (true, Some(_)) => {
                    return Err(ManifestError::PortDependencyHasOtherSource {
                        name,
                        conflicting: "port-path",
                    });
                }
                (true, None) => {
                    if path.is_some() {
                        return Err(ManifestError::PortDependencyHasOtherSource {
                            name,
                            conflicting: "path",
                        });
                    }
                    if workspace.is_some() {
                        return Err(ManifestError::PortDependencyHasOtherSource {
                            name,
                            conflicting: "workspace",
                        });
                    }
                    if features.is_some() {
                        return Err(ManifestError::PortDependencyUnsupportedOption {
                            name,
                            conflicting: "features",
                        });
                    }
                    if default_features.is_some() {
                        return Err(ManifestError::PortDependencyUnsupportedOption {
                            name,
                            conflicting: "default-features",
                        });
                    }
                    if optional.is_some() {
                        return Err(ManifestError::PortDependencyUnsupportedOption {
                            name,
                            conflicting: "optional",
                        });
                    }
                    let req_str = version.ok_or_else(|| {
                        ManifestError::PortDependencyMissingVersion { name: name.clone() }
                    })?;
                    let version_req = parse_version_req(&name, &req_str)?;
                    (
                        DependencySource::Port(PortDepSource::Builtin {
                            name: PackageName::new(name.clone())?,
                            version_req,
                        }),
                        false,
                        Vec::new(),
                        true,
                    )
                }
                (false, Some(port_path_value)) => {
                    if path.is_some() {
                        return Err(ManifestError::PortDependencyHasOtherSource {
                            name,
                            conflicting: "path",
                        });
                    }
                    if version.is_some() {
                        return Err(ManifestError::PortDependencyHasOtherSource {
                            name,
                            conflicting: "version",
                        });
                    }
                    if workspace.is_some() {
                        return Err(ManifestError::PortDependencyHasOtherSource {
                            name,
                            conflicting: "workspace",
                        });
                    }
                    if features.is_some() {
                        return Err(ManifestError::PortDependencyUnsupportedOption {
                            name,
                            conflicting: "features",
                        });
                    }
                    if default_features.is_some() {
                        return Err(ManifestError::PortDependencyUnsupportedOption {
                            name,
                            conflicting: "default-features",
                        });
                    }
                    if optional.is_some() {
                        return Err(ManifestError::PortDependencyUnsupportedOption {
                            name,
                            conflicting: "optional",
                        });
                    }
                    (
                        DependencySource::Port(PortDepSource::Path(PathBuf::from(port_path_value))),
                        false,
                        Vec::new(),
                        true,
                    )
                }
                (false, None) => {
                    // `optional = true` is supported only for normal
                    // dependencies. Dev declarations remain not-optional
                    // in this step.
                    let optional_flag = optional.unwrap_or(false);
                    if optional_flag && !matches!(kind, DependencyKind::Normal) {
                        return Err(ManifestError::OptionalNotSupportedForKind { name, kind });
                    }

                    let features_vec = features.unwrap_or_default();
                    if features_vec.iter().any(String::is_empty) {
                        return Err(ManifestError::EmptyDependencyFeatureName { name });
                    }
                    let default_features_flag = default_features.unwrap_or(true);

                    let workspace_flag = workspace.unwrap_or(false);
                    // `workspace = false` is treated as if the field were
                    // absent so it never collides with a path/version source.
                    let workspace_set = workspace.is_some();
                    let resolved_source = match (path, version, workspace_flag, workspace_set) {
                        (Some(_), Some(_), _, _) => {
                            return Err(ManifestError::DependencyHasPathAndVersion { name });
                        }
                        (Some(_), _, true, _) | (_, Some(_), true, _) => {
                            return Err(ManifestError::WorkspaceDependencyHasOtherSource { name });
                        }
                        (Some(path), None, false, _) => DependencySource::Path(PathBuf::from(path)),
                        (None, Some(req), false, _) => {
                            DependencySource::Version(parse_version_req(&name, &req)?)
                        }
                        (None, None, true, _) => DependencySource::Workspace,
                        (None, None, false, true) => {
                            return Err(ManifestError::WorkspaceDependencyExplicitlyDisabled {
                                name,
                            });
                        }
                        (None, None, false, false) => {
                            return Err(ManifestError::DependencyMissingSource { name });
                        }
                    };
                    (
                        resolved_source,
                        optional_flag,
                        features_vec,
                        default_features_flag,
                    )
                }
            }
        }
    };
    let (source, optional, features, default_features) = raw_outcome;
    // `workspace = true` inside a target-conditional table is
    // not currently supported — workspace inheritance has no
    // per-condition table to look up against, and silently
    // pretending the lookup is unconditional would be
    // surprising. Reject explicitly so users get a clear
    // signal.
    if let (Some(cond), DependencySource::Workspace) = (&condition, &source) {
        return Err(ManifestError::WorkspaceInsideConditionalTarget {
            name,
            condition: cond.to_string(),
        });
    }
    let package_name = PackageName::new(name)?;
    Ok(Dependency {
        name: package_name,
        source,
        kind,
        optional,
        features,
        default_features,
        condition,
    })
}

/// Produce a `SystemDependency` from a `[dependencies]` /
/// `[dev-dependencies]` entry that
/// carries `system = true`. Only `version` is permitted
/// alongside the flag; every other field is rejected with a
/// clear error so users learn the rule.
fn system_dependency_from_raw_table(
    name: String,
    table: RawDependencyTable,
    kind: DependencyKind,
    condition: Option<Condition>,
) -> Result<SystemDependency, ManifestError> {
    let section = kind.manifest_section();
    let RawDependencyTable {
        path,
        version,
        port,
        port_path,
        workspace,
        system,
        optional,
        features,
        default_features,
    } = table;
    debug_assert!(system, "router only dispatches here when system = true");
    let _ = system;

    // Reject every field that has no meaning alongside
    // `system = true`. The order matches the user-visible field
    // order so the first conflict reported is the one earliest
    // in the table.
    let forbidden: &[(&'static str, bool)] = &[
        ("path", path.is_some()),
        ("port", port == Some(true)),
        ("port-path", port_path.is_some()),
        ("workspace", workspace.is_some()),
        ("features", features.is_some()),
        ("default-features", default_features.is_some()),
        ("optional", optional.is_some()),
    ];
    for &(field, present) in forbidden {
        if present {
            return Err(ManifestError::SystemConflictsWith {
                name,
                section,
                field,
                detail: "the field is incompatible with `system = true`",
            });
        }
    }

    let version = version
        .ok_or_else(|| ManifestError::SystemDependencyMissingVersion { name: name.clone() })?;
    let package_name = PackageName::new(name)?;
    Ok(SystemDependency {
        name: package_name,
        version,
        kind,
        condition,
    })
}

fn parse_version_req(dep_name: &str, raw: &str) -> Result<semver::VersionReq, ManifestError> {
    cabin_core::version_req::parse_lenient(raw).map_err(|source| {
        ManifestError::InvalidDependencyRequirement {
            name: dep_name.to_owned(),
            requirement: raw.to_owned(),
            source,
        }
    })
}

fn parse_target_kind(target_name: &str, value: &str) -> Result<TargetKind, ManifestError> {
    match value {
        "library" => Ok(TargetKind::Library),
        "header_only" => Ok(TargetKind::HeaderOnly),
        "executable" => Ok(TargetKind::Executable),
        "test" => Ok(TargetKind::Test),
        "example" => Ok(TargetKind::Example),
        other => Err(ManifestError::UnknownTargetType {
            target: target_name.to_owned(),
            value: other.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{DependencyKind, ValidationError};

    fn parse_project(input: &str) -> Package {
        parse_manifest_str(input)
            .expect("manifest should parse")
            .package
            .expect("manifest should contain [package]")
    }

    fn parse_project_err(input: &str) -> ManifestError {
        parse_manifest_str(input).expect_err("manifest should fail to parse")
    }

    const MINIMAL: &str = r#"
        [package]
        name = "hello"
        version = "0.1.0"
    "#;

    const FULL: &str = r#"
        [package]
        name = "hello"
        version = "0.1.0"

        [target.hello]
        type = "executable"
        sources = ["src/main.cc"]
        include_dirs = ["include"]
        defines = ["HELLO=1"]
        deps = []
    "#;

    #[test]
    fn parses_minimal_manifest() {
        let package = parse_project(MINIMAL);
        assert_eq!(package.name.as_str(), "hello");
        assert_eq!(package.version.to_string(), "0.1.0");
        assert!(package.targets.is_empty());
        assert!(package.dependencies.is_empty());
    }

    #[test]
    fn parses_executable_target() {
        let package = parse_project(FULL);
        assert_eq!(package.targets.len(), 1);
        let target = &package.targets[0];
        assert_eq!(target.name.as_str(), "hello");
        assert_eq!(target.kind, TargetKind::Executable);
        assert_eq!(
            target.sources,
            vec![std::path::PathBuf::from("src/main.cc")]
        );
        assert_eq!(
            target.include_dirs,
            vec![std::path::PathBuf::from("include")]
        );
        assert_eq!(target.defines, vec!["HELLO=1".to_string()]);
        assert!(target.deps.is_empty());
    }

    #[test]
    fn target_unknown_fields_are_rejected() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.hello]
            type = "library"
            sources = ["src/lib.cc"]
            include-dirs = ["include"]
        "#;
        let err = parse_project_err(manifest);
        match err {
            ManifestError::Toml(source) => {
                let message = source.to_string();
                assert!(
                    message.contains("unknown field `include-dirs`"),
                    "unexpected error: {message}"
                );
            }
            other => panic!("expected TOML parse error, got {other:?}"),
        }
    }

    #[test]
    fn header_only_kind_is_accepted() {
        let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"

            [target.hdr]
            type = "header_only"
            include_dirs = ["include"]
        "#;
        let package = parse_project(manifest);
        let target = &package.targets[0];
        assert_eq!(target.kind, TargetKind::HeaderOnly);
        assert!(target.sources.is_empty());
    }

    #[test]
    fn header_only_rejects_sources() {
        let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"

            [target.hdr]
            type = "header_only"
            sources = ["src/empty.cc"]
            include_dirs = ["include"]
        "#;
        let err = parse_project_err(manifest);
        match err {
            ManifestError::HeaderOnlyDeclaresSources { target } => {
                assert_eq!(target, "hdr");
            }
            other => panic!("expected HeaderOnlyDeclaresSources, got {other:?}"),
        }
    }

    #[test]
    fn executable_accepts_mixed_c_and_cpp_sources() {
        // Target kinds describe artifact role only; the parser
        // accepts both C and C++ source extensions under any
        // executable / library / test / example target. Source-
        // language classification is per-file in the planner.
        let manifest = r#"
            [package]
            name = "exe"
            version = "0.1.0"

            [target.exe]
            type = "executable"
            sources = ["src/main.c", "src/helper.cc"]
        "#;
        let package = parse_project(manifest);
        let target = &package.targets[0];
        assert_eq!(target.kind, TargetKind::Executable);
        assert_eq!(
            target.sources,
            vec![
                std::path::PathBuf::from("src/main.c"),
                std::path::PathBuf::from("src/helper.cc"),
            ]
        );
    }

    #[test]
    fn omitted_optional_arrays_default_to_empty() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.hello]
            type = "executable"
        "#;
        let package = parse_project(manifest);
        let target = &package.targets[0];
        assert!(target.sources.is_empty());
        assert!(target.include_dirs.is_empty());
        assert!(target.defines.is_empty());
        assert!(target.deps.is_empty());
    }

    #[test]
    fn parses_all_supported_target_kinds() {
        let manifest = r#"
            [package]
            name = "many"
            version = "0.1.0"

            [target.a]
            type = "library"

            [target.b]
            type = "executable"

            [target.c]
            type = "test"
            sources = ["tests/e.cc"]

            [target.d]
            type = "example"
            sources = ["examples/f.cc"]

            [target.e]
            type = "header_only"
            include_dirs = ["include"]
        "#;
        let package = parse_project(manifest);
        let kinds: Vec<TargetKind> = package.targets.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TargetKind::Library,
                TargetKind::Executable,
                TargetKind::Test,
                TargetKind::Example,
                TargetKind::HeaderOnly,
            ]
        );
    }

    /// The old `c_*` / `cpp_*` target kind strings are no longer
    /// recognized. A manifest using either must fail with
    /// [`ManifestError::UnknownTargetType`] so existing users see
    /// an explicit migration prompt rather than silent acceptance.
    #[test]
    fn legacy_c_and_cpp_target_kinds_are_rejected() {
        for legacy in [
            "cpp_library",
            "cpp_header_only",
            "cpp_executable",
            "cpp_test",
            "cpp_example",
            "c_library",
            "c_header_only",
            "c_executable",
            "c_test",
            "c_example",
        ] {
            let manifest = format!(
                "[package]\nname = \"hello\"\nversion = \"0.1.0\"\n\n[target.hello]\ntype = \"{legacy}\"\n"
            );
            let err = parse_manifest_str(&manifest).expect_err("legacy kind should be rejected");
            match err {
                ManifestError::UnknownTargetType { target, value } => {
                    assert_eq!(target, "hello", "wrong target name in error for {legacy}");
                    assert_eq!(value, legacy, "wrong value in error for {legacy}");
                }
                other => panic!("expected UnknownTargetType for {legacy}, got {other:?}"),
            }
        }
    }

    #[test]
    fn empty_manifest_errors() {
        let err = parse_manifest_str("").unwrap_err();
        assert!(matches!(err, ManifestError::EmptyManifest));
    }

    #[test]
    fn missing_project_name_errors() {
        let manifest = r#"
            [package]
            version = "0.1.0"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(matches!(err, ManifestError::Toml(_)));
    }

    #[test]
    fn invalid_semver_errors() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "not-a-version"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::Version { value, .. } => assert_eq!(value, "not-a-version"),
            other => panic!("expected ManifestError::Version, got {other:?}"),
        }
    }

    #[test]
    fn unknown_target_type_errors() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.hello]
            type = "wasm_executable"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::UnknownTargetType { target, value } => {
                assert_eq!(target, "hello");
                assert_eq!(value, "wasm_executable");
            }
            other => panic!("expected UnknownTargetType, got {other:?}"),
        }
    }

    #[test]
    fn unknown_target_dep_now_passes_through_to_planner() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.hello]
            type = "executable"
            deps = ["external"]
        "#;
        let package = parse_project(manifest);
        assert_eq!(package.targets[0].deps[0].as_str(), "external");
    }

    #[test]
    fn empty_package_name_errors() {
        let manifest = r#"
            [package]
            name = ""
            version = "0.1.0"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::Validation(ValidationError::EmptyPackageName)
        ));
    }

    #[test]
    fn whitespace_package_name_errors() {
        let manifest = r#"
            [package]
            name = "hello world"
            version = "0.1.0"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::Validation(ValidationError::PackageNameContainsWhitespace(_))
        ));
    }

    #[test]
    fn empty_target_name_errors() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.""]
            type = "executable"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::Validation(ValidationError::EmptyTargetName)
        ));
    }

    #[test]
    fn whitespace_target_name_errors() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target."has space"]
            type = "executable"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(matches!(
            err,
            ManifestError::Validation(ValidationError::TargetNameContainsWhitespace(_))
        ));
    }

    /// A quoted target name with path metacharacters must be rejected
    /// at manifest-parse time. The build planner joins
    /// `target.name.as_str()` into object, executable, and Cargo target
    /// directories, so accepting `[target."/tmp/out"]` would let a
    /// malicious package write artifacts outside `--build-dir`.
    #[test]
    fn path_unsafe_target_name_errors() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target."/tmp/out"]
            type = "executable"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(
            matches!(
                err,
                ManifestError::Validation(ValidationError::UnsafeTargetName(_))
            ),
            "expected UnsafeTargetName, got {err:?}"
        );
    }

    /// Cross-package dep references use the `package:target` form. The
    /// `:` is outside the path-safe target-name grammar, so deps are
    /// stored as raw strings (not `TargetName`) and validated only at
    /// resolution time. Pin the round-trip so the type relaxation does
    /// not silently regress.
    #[test]
    fn qualified_cross_package_dep_round_trips_as_raw_string() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.exe]
            type = "executable"
            deps = ["other-pkg:lib"]
        "#;
        let package = parse_project(manifest);
        assert_eq!(package.targets[0].deps, vec!["other-pkg:lib".to_owned()]);
    }

    #[test]
    fn parse_target_kind_rejects_unknown() {
        let err = parse_target_kind("t", "exe").unwrap_err();
        match err {
            ManifestError::UnknownTargetType { target, value } => {
                assert_eq!(target, "t");
                assert_eq!(value, "exe");
            }
            other => panic!("expected UnknownTargetType, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // [dependencies] / [workspace] (path deps, workspace tables)
    // -------------------------------------------------------------------

    #[test]
    fn parses_path_dependency() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            greet = { path = "../greet" }
        "#;
        let package = parse_project(manifest);
        assert_eq!(package.dependencies.len(), 1);
        let dep = &package.dependencies[0];
        assert_eq!(dep.name.as_str(), "greet");
        assert_eq!(
            dep.source,
            DependencySource::Path(PathBuf::from("../greet"))
        );
    }

    #[test]
    fn parses_workspace_with_members() {
        let manifest = r#"
            [workspace]
            members = ["packages/*", "tools/hello"]
        "#;
        let parsed = parse_manifest_str(manifest).unwrap();
        assert!(parsed.package.is_none());
        let ws = parsed.workspace.expect("[workspace] should be present");
        assert_eq!(ws.members, vec!["packages/*", "tools/hello"]);
    }

    #[test]
    fn pure_workspace_preserves_root_policy_settings() {
        let manifest = r#"
            [workspace]
            members = ["packages/*"]

            [profile.release]
            opt-level = 0

            [toolchain]
            cxx = "clang++"

            [profile.cache]
            compiler-wrapper = "ccache"

            [patch]
            fmt = { path = "../fmt" }
        "#;
        let parsed = parse_manifest_str(manifest).unwrap();
        assert!(parsed.package.is_none());

        let release = cabin_core::ProfileName::new("release").unwrap();
        assert_eq!(
            parsed
                .root_settings
                .profiles
                .get(&release)
                .and_then(|p| p.opt_level),
            Some(cabin_core::OptLevel::O0)
        );
        assert_eq!(
            parsed
                .root_settings
                .toolchain
                .general
                .get(cabin_core::ToolKind::CxxCompiler)
                .map(cabin_core::ToolSpec::display)
                .as_deref(),
            Some("clang++")
        );
        assert_eq!(
            parsed.root_settings.compiler_wrapper.general,
            Some(cabin_core::CompilerWrapperRequest::Use {
                wrapper: cabin_core::CompilerWrapperKind::Ccache,
            })
        );
        assert_eq!(parsed.root_settings.patches.entries.len(), 1);
    }

    #[test]
    fn parses_workspace_with_root_project() {
        let manifest = r#"
            [package]
            name = "root"
            version = "0.1.0"

            [workspace]
            members = ["packages/*"]
        "#;
        let parsed = parse_manifest_str(manifest).unwrap();
        assert!(parsed.package.is_some());
        assert!(parsed.workspace.is_some());
    }

    #[test]
    fn invalid_workspace_members_errors() {
        let manifest = r#"
            [workspace]
            members = "not-an-array"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(matches!(err, ManifestError::Toml(_)));
    }

    // -------------------------------------------------------------------
    // versioned dependencies
    // -------------------------------------------------------------------

    #[test]
    fn parses_string_version_dependency() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">=10.0.0 <11.0.0"
        "#;
        let package = parse_project(manifest);
        let dep = &package.dependencies[0];
        assert_eq!(dep.name.as_str(), "fmt");
        match &dep.source {
            DependencySource::Version(req) => {
                assert!(req.matches(&semver::Version::parse("10.2.1").unwrap()));
                assert!(!req.matches(&semver::Version::parse("11.0.0").unwrap()));
            }
            other => panic!("expected Version source, got {other:?}"),
        }
    }

    #[test]
    fn parses_table_version_dependency() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { version = "^1.13.0" }
        "#;
        let package = parse_project(manifest);
        match &package.dependencies[0].source {
            DependencySource::Version(req) => {
                assert!(req.matches(&semver::Version::parse("1.13.5").unwrap()));
                assert!(!req.matches(&semver::Version::parse("2.0.0").unwrap()));
            }
            other => panic!("expected Version source, got {other:?}"),
        }
    }

    #[test]
    fn parses_mixed_path_and_version_dependencies() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            greet = { path = "../greet" }
            fmt = ">=10"
        "#;
        let package = parse_project(manifest);
        assert_eq!(package.dependencies.len(), 2);
        // BTreeMap iteration is sorted; 'fmt' < 'greet'.
        let fmt = &package.dependencies[0];
        let greet = &package.dependencies[1];
        assert_eq!(fmt.name.as_str(), "fmt");
        assert!(matches!(fmt.source, DependencySource::Version(_)));
        assert_eq!(greet.name.as_str(), "greet");
        assert!(matches!(greet.source, DependencySource::Path(_)));
    }

    #[test]
    fn dependency_with_both_path_and_version_errors() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            greet = { path = "../greet", version = "1.0" }
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::DependencyHasPathAndVersion { name } => assert_eq!(name, "greet"),
            other => panic!("expected DependencyHasPathAndVersion, got {other:?}"),
        }
    }

    #[test]
    fn dependency_with_neither_path_nor_version_errors() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            greet = { }
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::DependencyMissingSource { name } => assert_eq!(name, "greet"),
            other => panic!("expected DependencyMissingSource, got {other:?}"),
        }
    }

    #[test]
    fn invalid_string_version_requirement_errors() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">>>"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::InvalidDependencyRequirement {
                name, requirement, ..
            } => {
                assert_eq!(name, "fmt");
                assert_eq!(requirement, ">>>");
            }
            other => panic!("expected InvalidDependencyRequirement, got {other:?}"),
        }
    }

    #[test]
    fn dependency_table_with_truly_unknown_field_errors_at_toml_layer() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { version = "1.0", "made-up-key" = true }
        "#;
        // Fields that aren't in the typed `RawDependencyTable` shape
        // are still rejected at the TOML layer via `deny_unknown_fields`.
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(matches!(err, ManifestError::Toml(_)));
    }

    #[test]
    fn parses_supported_version_requirement_shapes() {
        // Smoke-test the requirement subset called out in the spec.
        let cases: &[(&str, &str)] = &[
            ("=1.2.3", "1.2.3"),
            (">=1.2.3", "1.2.3"),
            (">1.2.3", "1.2.4"),
            ("<=1.2.3", "1.2.3"),
            ("<2.0.0", "1.9.9"),
            (">=1.2.3 <2.0.0", "1.5.0"),
            ("^1.2.3", "1.5.0"),
            ("^0.2.3", "0.2.4"),
            ("^0.0.3", "0.0.3"),
            ("*", "9.9.9"),
        ];
        for (req_str, version_str) in cases {
            let manifest = format!(
                r#"
                [package]
                name = "app"
                version = "0.1.0"

                [dependencies]
                pkg = "{req_str}"
                "#,
            );
            let package = parse_project(&manifest);
            match &package.dependencies[0].source {
                DependencySource::Version(req) => {
                    let v = semver::Version::parse(version_str).unwrap();
                    assert!(
                        req.matches(&v),
                        "requirement {req_str:?} should match {version_str}"
                    );
                }
                other => panic!("expected Version source for {req_str:?}, got {other:?}"),
            }
        }
    }

    // -----------------------------------------------------------------
    // features
    // -----------------------------------------------------------------

    #[test]
    fn features_parse_with_default_and_implications() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            default = ["simd"]
            simd = []
            ssl = []
            full = ["simd", "ssl"]
        "#;
        let package = parse_project(manifest);
        assert_eq!(package.features.default, vec!["simd".to_string()]);
        assert_eq!(package.features.features.len(), 3);
        assert_eq!(
            package.features.features["full"],
            vec!["simd".to_string(), "ssl".into()]
        );
    }

    #[test]
    fn features_reject_reserved_default_as_normal_feature() {
        // The `default` key is reserved.  TOML may reject this
        // as a duplicate key before Cabin's semantic validation
        // runs; either failure mode protects the invariant.
        let conflict = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            default = []
            "default" = []
        "#;
        let err = parse_manifest_str(conflict);
        assert!(err.is_err());
    }

    #[test]
    fn features_unknown_reference_errors() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            full = ["ssl"]
        "#;
        match parse_manifest_str(manifest).unwrap_err() {
            ManifestError::Validation(ValidationError::UnknownFeatureReference {
                referrer,
                referenced,
            }) => {
                assert_eq!(referrer, "full");
                assert_eq!(referenced, "ssl");
            }
            other => panic!("expected UnknownFeatureReference, got {other:?}"),
        }
    }

    #[test]
    fn features_invalid_name_errors() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            "foo/bar" = []
        "#;
        match parse_manifest_str(manifest).unwrap_err() {
            ManifestError::Validation(ValidationError::InvalidConfigName { kind, value }) => {
                assert_eq!(kind, "feature");
                assert_eq!(value, "foo/bar");
            }
            other => panic!("expected InvalidConfigName, got {other:?}"),
        }
    }

    #[test]
    fn features_cycle_errors() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [features]
            a = ["b"]
            b = ["a"]
        "#;
        match parse_manifest_str(manifest).unwrap_err() {
            ManifestError::Validation(ValidationError::FeatureCycle(_)) => {}
            other => panic!("expected FeatureCycle, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Dependency kinds: parsing the package-dep tables, rejecting
    // unsupported syntax, and round-tripping kind information into
    // `Package::dependencies`.
    // -----------------------------------------------------------------

    fn deps_of_kind(package: &Package, kind: DependencyKind) -> Vec<&cabin_core::Dependency> {
        package.dependencies_of_kind(kind).collect()
    }

    #[test]
    fn parses_normal_dependencies_with_explicit_kind() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">=10"
        "#,
        );
        let deps = deps_of_kind(&package, DependencyKind::Normal);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name.as_str(), "fmt");
        assert_eq!(deps[0].kind, DependencyKind::Normal);
    }

    #[test]
    fn parses_each_package_dep_kind_section() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">=10"

            [dev-dependencies]
            gtest = "^1.14"
        "#,
        );
        for (kind, expected_name) in [
            (DependencyKind::Normal, "fmt"),
            (DependencyKind::Dev, "gtest"),
        ] {
            let deps = deps_of_kind(&package, kind);
            assert_eq!(deps.len(), 1, "{kind:?} should have one dep");
            assert_eq!(deps[0].name.as_str(), expected_name);
            assert_eq!(deps[0].kind, kind);
        }
    }

    #[test]
    fn unknown_top_level_table_is_rejected_by_deny_unknown_fields() {
        // Generic coverage that any unrecognized top-level table is
        // rejected by serde's `deny_unknown_fields`. Use a token
        // that does not correspond to any past or planned feature
        // so future grammar changes are not pinned to a specific
        // name.
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [not-a-real-table]
            anything = 1
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(
            matches!(err, ManifestError::Toml(_) | ManifestError::TomlAt(_)),
            "expected unknown-table TOML error, got {err:?}"
        );
    }

    #[test]
    fn parses_system_dependencies() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            zlib = { version = ">=1.2", system = true }
            openssl = { version = ">=3", system = true }
        "#,
        );
        assert_eq!(package.system_dependencies.len(), 2);
        // System deps round-trip through metadata sorted by name (BTreeMap iteration).
        let by_name: BTreeMap<&str, &cabin_core::SystemDependency> = package
            .system_dependencies
            .iter()
            .map(|sd| (sd.name.as_str(), sd))
            .collect();
        assert_eq!(by_name["openssl"].version, ">=3");
        assert_eq!(by_name["openssl"].kind, DependencyKind::Normal);
        assert_eq!(by_name["zlib"].version, ">=1.2");
        assert_eq!(by_name["zlib"].kind, DependencyKind::Normal);
    }

    #[test]
    fn system_dependency_rejects_required_field() {
        // `required` was removed from the manifest surface: every
        // `system = true` dep is required. The unknown field is
        // rejected by `deny_unknown_fields` on the raw table; the
        // resulting diagnostic must name the field by name so users
        // know what to remove.
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            openssl = { version = ">=3", system = true, required = false }
        "#;
        let err = parse_project_err(manifest);
        let rendered = err.to_string();
        assert!(
            rendered.contains("unknown field `required`"),
            "diagnostic should name the unknown field; got {rendered}",
        );
    }

    #[test]
    fn system_flag_routes_per_kind() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            zlib = { version = ">=1.2", system = true }

            [dev-dependencies]
            gtest = { version = "^1.14", system = true }
        "#,
        );
        let by_name: BTreeMap<&str, &cabin_core::SystemDependency> = package
            .system_dependencies
            .iter()
            .map(|sd| (sd.name.as_str(), sd))
            .collect();
        assert_eq!(by_name["zlib"].kind, DependencyKind::Normal);
        assert_eq!(by_name["gtest"].kind, DependencyKind::Dev);
    }

    #[test]
    fn same_package_name_across_different_kinds_is_allowed() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = ">=10"

            [dev-dependencies]
            fmt = ">=10"
        "#,
        );
        // The duplicate-policy spec: same name across kinds is allowed.
        assert_eq!(deps_of_kind(&package, DependencyKind::Normal).len(), 1);
        assert_eq!(deps_of_kind(&package, DependencyKind::Dev).len(), 1);
    }

    #[test]
    fn dev_dependency_path_dependency_is_accepted_in_metadata() {
        // Dev deps with `path = "..."` are declaration-only; we
        // accept them at parse time and surface them through
        // `package.dependencies` so `cabin metadata` lists them.
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dev-dependencies]
            harness = { path = "../harness" }
        "#,
        );
        let deps = deps_of_kind(&package, DependencyKind::Dev);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name.as_str(), "harness");
        assert_eq!(deps[0].kind, DependencyKind::Dev);
        assert!(matches!(deps[0].source, DependencySource::Path(_)));
    }

    #[test]
    fn optional_dependency_is_parsed_with_optional_flag() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { version = ">=10", optional = true }
        "#;
        let package = parse_project(manifest);
        let dep = package
            .dependencies
            .iter()
            .find(|d| d.name.as_str() == "fmt")
            .unwrap();
        assert!(dep.optional);
        assert_eq!(dep.kind, DependencyKind::Normal);
        assert!(dep.features.is_empty());
        assert!(dep.default_features);
    }

    #[test]
    fn optional_on_dev_dependency_is_rejected() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dev-dependencies]
            gtest = { version = "^1", optional = true }
        "#;
        match parse_manifest_str(manifest).unwrap_err() {
            ManifestError::OptionalNotSupportedForKind { name, kind } => {
                assert_eq!(name, "gtest");
                assert_eq!(kind, DependencyKind::Dev);
            }
            other => panic!("expected OptionalNotSupportedForKind, got {other:?}"),
        }
    }

    #[test]
    fn optional_on_system_dependency_is_rejected() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            zlib = { version = ">=1.2", system = true, optional = true }
        "#;
        // The parser enforces that `system = true` is incompatible
        // with `optional` (and `path`, `workspace`, `features`,
        // `default-features`, `git`, `registry`, `source`). The
        // first conflicting field in declaration order surfaces
        // via `SystemConflictsWith`.
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::SystemConflictsWith { name, field, .. } => {
                assert_eq!(name, "zlib");
                assert_eq!(field, "optional");
            }
            other => panic!("expected SystemConflictsWith, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_dependency_section_yields_toml_error() {
        // `[test-dependencies]` is not a recognized top-level
        // section. `RawManifest` declares `deny_unknown_fields`
        // so a typo cannot silently drop dependencies — the
        // TOML layer surfaces an `unknown field` error pointing
        // at the offending section name.
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [test-dependencies]
            gtest = "^1"
        "#;
        let err = parse_project_err(manifest);
        let rendered = format!("{err}");
        assert!(
            rendered.contains("test-dependencies"),
            "error should name the offending section: {rendered}"
        );
    }

    #[test]
    fn target_cfg_dependency_round_trips_condition() {
        // `[target.'cfg(...)'.<kind>]` lands as ordinary
        // `Dependency` entries with `condition` populated.
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.dependencies]
            fmt = ">=10"

            [target.'cfg(arch = "x86_64")'.dev-dependencies]
            gtest = "^1.14"

            [target.'cfg(arch = "aarch64")'.dependencies]
            zlib = { version = ">=1.2", system = true }
        "#,
        );
        let normal = deps_of_kind(&package, DependencyKind::Normal);
        assert_eq!(normal.len(), 1);
        assert_eq!(normal[0].name.as_str(), "fmt");
        assert_eq!(
            normal[0].condition.as_ref().map(ToString::to_string),
            Some("os = \"linux\"".to_owned()),
        );
        let dev = deps_of_kind(&package, DependencyKind::Dev);
        assert_eq!(dev.len(), 1);
        assert_eq!(
            dev[0].condition.as_ref().map(ToString::to_string),
            Some("arch = \"x86_64\"".to_owned()),
        );
        assert_eq!(package.system_dependencies.len(), 1);
        let sys = &package.system_dependencies[0];
        assert_eq!(sys.name.as_str(), "zlib");
        assert_eq!(
            sys.condition.as_ref().map(ToString::to_string),
            Some("arch = \"aarch64\"".to_owned()),
        );
    }

    #[test]
    fn target_cfg_workspace_inheritance_is_rejected() {
        // `workspace = true` inside `[target.'cfg(...)'.<kind>]`
        // is rejected so a single workspace key never silently
        // means different things on different hosts.
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.dependencies]
            fmt = { workspace = true }
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(
            matches!(err, ManifestError::WorkspaceInsideConditionalTarget { .. }),
            "expected WorkspaceInsideConditionalTarget, got {err:?}",
        );
    }

    #[test]
    fn target_cfg_invalid_predicate_yields_clear_error() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(host_endian = "little")'.dependencies]
            fmt = ">=10"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::InvalidTargetCfg { raw, .. } => {
                assert!(raw.contains("host_endian"), "{raw}");
            }
            other => panic!("expected InvalidTargetCfg, got {other:?}"),
        }
    }

    #[test]
    fn manifest_without_profile_tables_round_trips() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"
        "#,
        );
        assert!(package.profiles.is_empty());
    }

    #[test]
    fn dev_override_is_parsed() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.dev]
            opt-level = 1
            assertions = false
        "#,
        );
        let dev_name = cabin_core::ProfileName::new("dev").unwrap();
        let dev = package.profiles.get(&dev_name).expect("dev profile parsed");
        assert!(dev.inherits.is_none());
        assert_eq!(dev.opt_level, Some(cabin_core::OptLevel::O1));
        assert_eq!(dev.assertions, Some(false));
        assert!(dev.debug.is_none());
    }

    #[test]
    fn release_override_is_parsed_with_string_opt_level() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.release]
            opt-level = "s"
            debug = true
        "#,
        );
        let release_name = cabin_core::ProfileName::new("release").unwrap();
        let r = package.profiles.get(&release_name).unwrap();
        assert_eq!(r.opt_level, Some(cabin_core::OptLevel::S));
        assert_eq!(r.debug, Some(true));
    }

    #[test]
    fn custom_profile_with_inherits_is_parsed() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.relwithdebinfo]
            inherits = "release"
            debug = true
        "#,
        );
        let custom = cabin_core::ProfileName::new("relwithdebinfo").unwrap();
        let p = package.profiles.get(&custom).unwrap();
        assert_eq!(
            p.inherits.as_ref().map(cabin_core::ProfileName::as_str),
            Some("release")
        );
        assert_eq!(p.debug, Some(true));
        assert!(p.opt_level.is_none());
    }

    #[test]
    fn profile_release_accepts_cxxflags_override() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.release]
            opt-level = 3
            cxxflags = ["-march=native"]
        "#;
        let package = parse_project(manifest);
        let release =
            cabin_core::ProfileName::new("release".to_owned()).expect("valid profile name");
        let prof = package.profiles.get(&release).expect("release profile");
        let build = prof.build.as_ref().expect("override build flags");
        assert_eq!(build.cxxflags, vec!["-march=native".to_owned()]);
    }

    #[test]
    fn top_level_profile_accepts_cflags_cxxflags_and_ldflags() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile]
            cflags = ["-std=c99"]
            cxxflags = ["-fno-rtti"]
            ldflags = ["-Wl,--as-needed"]
        "#;
        let package = parse_project(manifest);
        let build = &package.build.general;
        assert_eq!(build.cflags, vec!["-std=c99".to_owned()]);
        assert_eq!(build.cxxflags, vec!["-fno-rtti".to_owned()]);
        assert_eq!(build.ldflags, vec!["-Wl,--as-needed".to_owned()]);
    }

    #[test]
    fn unknown_profile_field_is_rejected() {
        // Generic coverage for `deny_unknown_fields` on
        // `[profile.<name>]`. The field name is intentionally a
        // sentinel so this test is not pinned to a specific
        // hypothetical knob.
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.release]
            not-a-real-key = "x"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(matches!(err, ManifestError::Toml(_)));
        let msg = err.to_string();
        assert!(msg.contains("not-a-real-key"), "{msg}");
    }

    #[test]
    fn invalid_opt_level_is_rejected() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.release]
            opt-level = "fast"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid opt-level"), "{msg}");
    }

    #[test]
    fn invalid_profile_name_is_rejected() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.".release"]
            opt-level = 0
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::InvalidProfileName { value } => {
                assert_eq!(value, ".release");
            }
            other => panic!("expected InvalidProfileName, got {other:?}"),
        }
    }

    #[test]
    fn workspace_dependency_kind_is_preserved() {
        // The `workspace = true` opt-in is only valid inside a
        // package-dep section; the kind on the resulting
        // `Dependency` matches the section it was declared in.
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { workspace = true }

            [dev-dependencies]
            gtest = { workspace = true }
        "#,
        );
        let deps = deps_of_kind(&package, DependencyKind::Normal);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].source, DependencySource::Workspace);
        let deps = deps_of_kind(&package, DependencyKind::Dev);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].source, DependencySource::Workspace);
    }

    // ---------------------------------------------------------------
    // Foundation-port dependency form
    // ---------------------------------------------------------------

    #[test]
    fn parses_port_dependency() {
        let package = parse_project(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1" }
        "#,
        );
        let deps = deps_of_kind(&package, DependencyKind::Normal);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name.as_str(), "zlib");
        match &deps[0].source {
            DependencySource::Port(PortDepSource::Path(p)) => {
                assert_eq!(p, &PathBuf::from("../ports/zlib/1.3.1"));
            }
            other => panic!("expected Port, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_combined_with_path() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", path = "../zlib" }
        "#,
        );
        match err {
            ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
                assert_eq!(conflicting, "path");
            }
            other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_combined_with_version() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", version = "1.0" }
        "#,
        );
        match err {
            ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
                assert_eq!(conflicting, "version");
            }
            other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_combined_with_workspace() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", workspace = true }
        "#,
        );
        match err {
            ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
                assert_eq!(conflicting, "workspace");
            }
            other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_combined_with_system() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", system = true, version = "1.0" }
        "#,
        );
        // The system router runs first and surfaces a clear
        // SystemConflictsWith for the `port-path` field.
        match err {
            ManifestError::SystemConflictsWith { field, .. } => {
                assert_eq!(field, "port-path");
            }
            other => panic!("expected SystemConflictsWith on `port-path`, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_with_features() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", features = ["x"] }
        "#,
        );
        match err {
            ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
                assert_eq!(conflicting, "features");
            }
            other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_with_default_features() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", default-features = false }
        "#,
        );
        match err {
            ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
                assert_eq!(conflicting, "default-features");
            }
            other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_with_optional() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", optional = true }
        "#,
        );
        match err {
            ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
                assert_eq!(conflicting, "optional");
            }
            other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
        }
    }

    #[test]
    fn parses_builtin_port_with_version_requirement() {
        let package = parse_project(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3" }
        "#,
        );
        let deps = deps_of_kind(&package, DependencyKind::Normal);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name.as_str(), "zlib");
        match &deps[0].source {
            DependencySource::Port(PortDepSource::Builtin { name, version_req }) => {
                assert_eq!(name.as_str(), "zlib");
                assert_eq!(version_req.to_string(), "^1.3");
            }
            other => panic!("expected Builtin, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_true_without_version() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true }
        "#,
        );
        assert!(
            matches!(err, ManifestError::PortDependencyMissingVersion { ref name } if name == "zlib"),
            "expected PortDependencyMissingVersion, got {err:?}"
        );
    }

    #[test]
    fn rejects_port_path_with_version() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1", version = "^1.3" }
        "#,
        );
        match err {
            ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
                assert_eq!(conflicting, "version");
            }
            other => panic!("expected PortDependencyHasOtherSource for version, got {other:?}"),
        }
    }

    #[test]
    fn parses_path_port_dependency_via_port_path_field() {
        let package = parse_project(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port-path = "../ports/zlib/1.3.1" }
        "#,
        );
        let deps = deps_of_kind(&package, DependencyKind::Normal);
        assert_eq!(deps.len(), 1);
        match &deps[0].source {
            DependencySource::Port(PortDepSource::Path(p)) => {
                assert_eq!(p, &PathBuf::from("../ports/zlib/1.3.1"));
            }
            other => panic!("expected Path, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_true_combined_with_port_path() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, port-path = "../ports/zlib/1.3.1" }
        "#,
        );
        match err {
            ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
                assert_eq!(conflicting, "port-path");
            }
            other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_true_combined_with_path() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", path = "../zlib" }
        "#,
        );
        match err {
            ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
                assert_eq!(conflicting, "path");
            }
            other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_true_combined_with_workspace() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", workspace = true }
        "#,
        );
        match err {
            ManifestError::PortDependencyHasOtherSource { conflicting, .. } => {
                assert_eq!(conflicting, "workspace");
            }
            other => panic!("expected PortDependencyHasOtherSource, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_true_combined_with_features() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", features = ["x"] }
        "#,
        );
        match err {
            ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
                assert_eq!(conflicting, "features");
            }
            other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_true_combined_with_default_features() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", default-features = false }
        "#,
        );
        match err {
            ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
                assert_eq!(conflicting, "default-features");
            }
            other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_true_combined_with_optional() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "^1.3", optional = true }
        "#,
        );
        match err {
            ManifestError::PortDependencyUnsupportedOption { conflicting, .. } => {
                assert_eq!(conflicting, "optional");
            }
            other => panic!("expected PortDependencyUnsupportedOption, got {other:?}"),
        }
    }

    #[test]
    fn rejects_port_true_with_invalid_version() {
        let err = parse_project_err(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = true, version = "not-a-version" }
        "#,
        );
        match err {
            ManifestError::InvalidDependencyRequirement {
                name, requirement, ..
            } => {
                assert_eq!(name, "zlib");
                assert_eq!(requirement, "not-a-version");
            }
            other => panic!("expected InvalidDependencyRequirement, got {other:?}"),
        }
    }

    #[test]
    fn treats_port_false_as_absent() {
        let package = parse_project(
            r#"
            [package]
            name = "consumer"
            version = "0.1.0"

            [dependencies]
            zlib = { port = false, path = "../zlib" }
        "#,
        );
        let deps = deps_of_kind(&package, DependencyKind::Normal);
        match &deps[0].source {
            DependencySource::Path(p) => assert_eq!(p, &PathBuf::from("../zlib")),
            other => panic!("expected Path (port = false is treated as absent), got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // [profile.cache] / [target.'cfg(...)'.profile.cache] parsing
    // ---------------------------------------------------------------

    #[test]
    fn build_cache_parses_general_compiler_wrapper() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.cache]
            compiler-wrapper = "ccache"
        "#,
        );
        assert_eq!(
            package.compiler_wrapper.general,
            Some(cabin_core::CompilerWrapperRequest::Use {
                wrapper: cabin_core::CompilerWrapperKind::Ccache,
            })
        );
        assert!(package.compiler_wrapper.conditional.is_empty());
    }

    #[test]
    fn build_cache_accepts_none_to_explicitly_disable() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.cache]
            compiler-wrapper = "none"
        "#,
        );
        assert_eq!(
            package.compiler_wrapper.general,
            Some(cabin_core::CompilerWrapperRequest::Disabled)
        );
    }

    #[test]
    fn target_conditional_build_cache_collects_per_condition() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [target.'cfg(os = "linux")'.profile.cache]
            compiler-wrapper = "sccache"
        "#,
        );
        assert!(package.compiler_wrapper.general.is_none());
        assert_eq!(package.compiler_wrapper.conditional.len(), 1);
        let entry = &package.compiler_wrapper.conditional[0];
        assert_eq!(
            entry.request,
            cabin_core::CompilerWrapperRequest::Use {
                wrapper: cabin_core::CompilerWrapperKind::Sccache,
            }
        );
    }

    #[test]
    fn unsupported_compiler_wrapper_value_is_rejected() {
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.cache]
            compiler-wrapper = "fastcache"
        "#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("[profile.cache]") && message.contains("fastcache"),
            "expected error to point at the offending section + value, got: {message}"
        );
    }

    #[test]
    fn empty_compiler_wrapper_value_is_rejected() {
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.cache]
            compiler-wrapper = ""
        "#,
        )
        .unwrap_err();
        match err {
            ManifestError::InvalidCompilerWrapper { source, .. } => assert!(matches!(
                source,
                cabin_core::CompilerWrapperParseError::Empty
            )),
            other => panic!("expected InvalidCompilerWrapper, got {other:?}"),
        }
    }

    #[test]
    fn build_cache_does_not_alter_build_flags_decl() {
        // The cache sub-table must not bleed into the per-package
        // `[profile]` flag layers — defines / include dirs etc. stay
        // exactly what the user declared, so existing manifests
        // without a `[profile.cache]` block continue to round-trip
        // byte-for-byte.
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile]
            defines = ["FOO=1"]

            [profile.cache]
            compiler-wrapper = "ccache"
        "#,
        );
        assert_eq!(package.build.general.defines, vec!["FOO=1".to_owned()]);
        assert_eq!(
            package.compiler_wrapper.general,
            Some(cabin_core::CompilerWrapperRequest::Use {
                wrapper: cabin_core::CompilerWrapperKind::Ccache,
            })
        );
    }

    // ---------------------------------------------------------------
    // [patch] table parsing
    // ---------------------------------------------------------------

    #[test]
    fn patch_table_parses_local_path_entry() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            fmt = { path = "../fmt" }
        "#,
        );
        let entries = &package.patches.entries;
        assert_eq!(entries.len(), 1);
        let key = cabin_core::PackageName::new("fmt").unwrap();
        match entries.get(&key) {
            Some(cabin_core::PatchSource::Path { path }) => {
                assert_eq!(path, &PathBuf::from("../fmt"));
            }
            other => panic!("expected Path patch, got {other:?}"),
        }
    }

    #[test]
    fn patch_table_rejects_unknown_field_via_serde() {
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            fmt = { branch = "main" }
        "#,
        )
        .unwrap_err();
        // `deny_unknown_fields` on the row catches this.
        assert!(matches!(err, ManifestError::Toml(_)));
    }

    #[test]
    fn patch_table_rejects_empty_path() {
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            fmt = { path = "" }
        "#,
        )
        .unwrap_err();
        match err {
            ManifestError::InvalidPatch { package, source } => {
                assert_eq!(package, "fmt");
                assert!(matches!(
                    source,
                    cabin_core::PatchValidationError::MissingSource { .. }
                ));
            }
            other => panic!("expected InvalidPatch, got {other:?}"),
        }
    }

    #[test]
    fn patch_table_rejects_invalid_package_name() {
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            "evil/name" = { path = "../fmt" }
        "#,
        )
        .unwrap_err();
        // Goes through the shared PackageName validator.
        let message = err.to_string();
        assert!(
            message.contains("evil/name"),
            "expected the offending name in the error, got: {message}"
        );
    }

    #[test]
    fn unknown_package_field_is_rejected() {
        // Generic coverage that any unrecognized field on
        // `[package]` is rejected by serde's `deny_unknown_fields`.
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"
            not-a-real-key = "x"
        "#,
        )
        .unwrap_err();
        match err {
            ManifestError::Toml(source) => {
                let message = source.to_string();
                assert!(
                    message.contains("unknown field"),
                    "unexpected error: {message}"
                );
            }
            other => panic!("expected TOML parse error, got {other:?}"),
        }
    }
}
