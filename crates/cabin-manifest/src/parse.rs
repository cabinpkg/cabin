use std::path::{Path, PathBuf};

use cabin_core::{
    Condition, Dependency, DependencyKind, DependencySource, Features, OptionDecl, Package,
    PackageName, RustTarget, SystemDependency, Target, TargetKind, TargetName, VariantDecl,
};
use serde::{Deserialize, Serialize};

use crate::error::ManifestError;
use crate::raw::{
    RawDependency, RawDependencyTable, RawManifest, RawOption, RawPackage, RawTarget, RawVariant,
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
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub profiles:
        std::collections::BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
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
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub dependencies: std::collections::BTreeMap<String, String>,
    /// Shared `[workspace.build-dependencies]`. Members opt in via
    /// `dep = { workspace = true }` inside `[build-dependencies]`.
    #[serde(
        default,
        rename = "build-dependencies",
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub build_dependencies: std::collections::BTreeMap<String, String>,
    /// Shared `[workspace.dev-dependencies]`. Members opt in via
    /// `dep = { workspace = true }` inside `[dev-dependencies]`.
    #[serde(
        default,
        rename = "dev-dependencies",
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub dev_dependencies: std::collections::BTreeMap<String, String>,
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
    std::collections::BTreeMap<String, crate::raw::RawProfile>,
) {
    let Some(t) = table else {
        return (None, std::collections::BTreeMap::new());
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
        build_dependencies,
        dev_dependencies,
        workspace,
        features,
        options,
        variants,
        profile,
        toolchain,
        patch,
        lint,
    } = raw;
    let (build, profile) = split_profile_table(profile);
    let patches = patch_settings_from_raw(patch)?;
    let lint_settings = lint_settings_from_raw(lint)?;
    let ParsedProfiles {
        definitions: profiles,
        wrapper_overrides: profile_wrapper_overrides,
    } = profiles_from_raw(profile)?;
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
    //    tables (`dependencies`, `build-dependencies`,
    //    `dev-dependencies`,
    //    `system-dependencies`). Anything else under such an
    //    entry is rejected.
    // 2. Buildable C++ / Rust targets — entry name is a target
    //    identifier and the value is a `RawTarget`-shaped table.
    //    These must *not* contain conditional dep sub-tables;
    //    that mistake surfaces with a clear "not supported"
    //    error so users do not silently lose deps to the wrong
    //    schema.
    let mut conditional_targets: Vec<RawConditionalTarget> = Vec::new();
    let mut buildable_targets: std::collections::BTreeMap<String, toml::Value> =
        std::collections::BTreeMap::new();
    for (raw_target_name, raw_value) in target {
        if is_cfg_expression(&raw_target_name) {
            conditional_targets.push(parse_conditional_target_entry(&raw_target_name, raw_value)?);
        } else {
            // Reject buildable target tables that contain dep
            // sub-tables. These are almost always typos of the
            // `cfg(...)` form (e.g. forgetting the quotes).
            if let Some(table) = raw_value.as_table() {
                for forbidden in [
                    "dependencies",
                    "build-dependencies",
                    "dev-dependencies",
                    "system-dependencies",
                ] {
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
        profile_wrapper_overrides.clone(),
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
            build_dependencies,
            dev_dependencies,
            conditional_targets,
            raw_features: features,
            raw_options: options,
            raw_variants: variants,
            profiles,
            profile_wrapper_overrides,
            toolchain_general: toolchain_decl,
            build_general: build_decl,
            general_wrapper_request,
            patches,
            lint_settings,
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
            let _ = profile_wrapper_overrides;
            let _ = toolchain_decl;
            let _ = build_decl;
            let _ = general_wrapper_request;
            let _ = patches;
            // Manifest lint settings only make sense alongside
            // a `[package]`.  Pure-workspace roots have no
            // package to bind them to, so the parsed value is
            // dropped here.
            let _ = lint_settings;
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
        build_dependencies: w.build_dependencies,
        dev_dependencies: w.dev_dependencies,
    });

    Ok(ParsedManifest {
        package,
        workspace,
        root_settings,
    })
}

fn root_settings_from_parts(
    profiles: std::collections::BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
    profile_wrapper_overrides: std::collections::BTreeMap<
        cabin_core::ProfileName,
        cabin_core::CompilerWrapperRequest,
    >,
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
        profile_overrides: profile_wrapper_overrides,
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
    targets: std::collections::BTreeMap<String, RawTarget>,
    dependencies: std::collections::BTreeMap<String, RawDependency>,
    build_dependencies: std::collections::BTreeMap<String, RawDependency>,
    dev_dependencies: std::collections::BTreeMap<String, RawDependency>,
    conditional_targets: Vec<RawConditionalTarget>,
    raw_features: std::collections::BTreeMap<String, Vec<String>>,
    raw_options: std::collections::BTreeMap<String, RawOption>,
    raw_variants: std::collections::BTreeMap<String, RawVariant>,
    profiles: std::collections::BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
    profile_wrapper_overrides:
        std::collections::BTreeMap<cabin_core::ProfileName, cabin_core::CompilerWrapperRequest>,
    toolchain_general: cabin_core::ToolchainDecl,
    build_general: cabin_core::ProfileFlags,
    general_wrapper_request: Option<cabin_core::CompilerWrapperRequest>,
    patches: cabin_core::PatchManifestSettings,
    lint_settings: cabin_core::LintSettings,
}

fn project_from_raw(input: ProjectFromRawInput) -> Result<Package, ManifestError> {
    let ProjectFromRawInput {
        package,
        targets,
        dependencies,
        build_dependencies,
        dev_dependencies,
        conditional_targets,
        raw_features,
        raw_options,
        raw_variants,
        profiles,
        profile_wrapper_overrides,
        toolchain_general,
        build_general,
        general_wrapper_request,
        patches,
        lint_settings,
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
    let unconditional_capacity =
        dependencies.len() + build_dependencies.len() + dev_dependencies.len();
    let conditional_capacity: usize = conditional_targets
        .iter()
        .map(|t| t.deps.len() + t.build_deps.len() + t.dev_deps.len())
        .sum();
    let mut dep_models: Vec<Dependency> =
        Vec::with_capacity(unconditional_capacity + conditional_capacity);
    let mut system_models: Vec<SystemDependency> = Vec::new();
    for (kind, raw_table) in [
        (DependencyKind::Normal, dependencies),
        (DependencyKind::Build, build_dependencies),
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
            (DependencyKind::Build, &cond_target.build_deps),
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
    let options = options_from_raw(raw_options)?;
    let variants = variants_from_raw(raw_variants);

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
        profile_overrides: profile_wrapper_overrides,
    };

    Ok(Package::with_config(cabin_core::PackageConfigInput {
        name: package_name,
        version: parsed_version,
        targets: target_models,
        dependencies: dep_models,
        system_dependencies: system_models,
        features,
        options,
        variants,
    })?
    .with_profiles(profiles)
    .with_toolchain(toolchain_settings)
    .with_build(build_settings)
    .with_compiler_wrapper(compiler_wrapper_settings)
    .with_patches(patches)
    .with_lint(lint_settings))
}

/// Validate every `[profile.<name>]` table and convert it into a
/// typed [`cabin_core::ProfileDefinition`]. Errors short-circuit
/// the whole manifest because partial profile state would be
/// surprising downstream.
///
/// Also returns the per-profile compiler-cache wrapper overrides
/// keyed by profile name so the caller can attach them to the
/// workspace-root `Package::compiler_wrapper`. The per-profile
/// overlay TOML surface is not wired yet, so this map is empty.
fn profiles_from_raw(
    raw: std::collections::BTreeMap<String, crate::raw::RawProfile>,
) -> Result<ParsedProfiles, ManifestError> {
    let mut out = std::collections::BTreeMap::new();
    let wrapper_overrides = std::collections::BTreeMap::new();
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
    Ok(ParsedProfiles {
        definitions: out,
        wrapper_overrides,
    })
}

/// Profile parsing output bundled into one struct so the
/// signature stays readable as new per-profile fields land.
struct ParsedProfiles {
    /// Per-profile definitions keyed by profile name.
    definitions: std::collections::BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
    /// Per-profile compiler-cache wrapper overrides.
    wrapper_overrides:
        std::collections::BTreeMap<cabin_core::ProfileName, cabin_core::CompilerWrapperRequest>,
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

/// Convert one `[lint]` table into typed
/// [`cabin_core::LintSettings`].
///
/// Today the only supported sub-table is `[lint.cpplint]`,
/// with a single `filters` key.  Unknown sub-tables or
/// unknown keys inside known sub-tables are rejected by
/// `deny_unknown_fields` on the raw types so a typo never
/// silently disables a manifest-declared setting.  Filter
/// strings are not parsed by Cabin — cpplint's grammar is
/// opaque to us — but the empty string is rejected to catch
/// a copy-paste mistake before it surfaces as a confusing
/// cpplint error.
fn lint_settings_from_raw(
    raw: crate::raw::RawLint,
) -> Result<cabin_core::LintSettings, ManifestError> {
    let crate::raw::RawLint { cpplint } = raw;
    let cpplint = match cpplint {
        Some(c) => {
            let crate::raw::RawCpplintLint { filters } = c;
            for entry in &filters {
                if entry.trim().is_empty() {
                    return Err(ManifestError::InvalidLintFilter {
                        section: "lint.cpplint.filters",
                        reason: "filter entries must be non-empty",
                    });
                }
            }
            cabin_core::CpplintLintSettings { filters }
        }
        None => cabin_core::CpplintLintSettings::default(),
    };
    Ok(cabin_core::LintSettings { cpplint })
}

/// Convert a raw `[patch]` table into typed
/// [`cabin_core::PatchManifestSettings`]. Each entry must use a
/// supported source kind (currently only `path = "..."`); the
/// recognised but unsupported keys (`git`, `url`, `version`)
/// surface a stable error message rather than `deny_unknown_fields`'s
/// generic wording so users see exactly which alternative they
/// asked for.
fn patch_settings_from_raw(
    raw: std::collections::BTreeMap<String, crate::raw::RawPatch>,
) -> Result<cabin_core::PatchManifestSettings, ManifestError> {
    use cabin_core::{PatchSource, PatchValidationError};

    let mut entries = std::collections::BTreeMap::new();
    for (name, row) in raw {
        let package = cabin_core::PackageName::new(name).map_err(ManifestError::Validation)?;
        let crate::raw::RawPatch {
            path,
            git,
            url,
            version,
        } = row;
        if let Some(_value) = git {
            return Err(ManifestError::InvalidPatch {
                package: package.as_str().to_owned(),
                source: PatchValidationError::UnsupportedSourceKind { kind: "git".into() },
            });
        }
        if let Some(_value) = url {
            return Err(ManifestError::InvalidPatch {
                package: package.as_str().to_owned(),
                source: PatchValidationError::UnsupportedSourceKind { kind: "url".into() },
            });
        }
        if let Some(_value) = version {
            return Err(ManifestError::InvalidPatch {
                package: package.as_str().to_owned(),
                source: PatchValidationError::UnsupportedSourceKind {
                    kind: "version".into(),
                },
            });
        }
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
                path: std::path::PathBuf::from(trimmed),
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

fn features_from_raw(mut raw: std::collections::BTreeMap<String, Vec<String>>) -> Features {
    let default = raw
        .remove(cabin_core::DEFAULT_FEATURE_KEY)
        .unwrap_or_default();
    Features {
        default,
        features: raw,
    }
}

fn options_from_raw(
    raw: std::collections::BTreeMap<String, RawOption>,
) -> Result<std::collections::BTreeMap<String, OptionDecl>, ManifestError> {
    let mut out = std::collections::BTreeMap::new();
    for (name, decl) in raw {
        out.insert(name.clone(), option_decl_from_raw(name, decl)?);
    }
    Ok(out)
}

fn option_decl_from_raw(name: String, raw: RawOption) -> Result<OptionDecl, ManifestError> {
    let RawOption {
        ty,
        default,
        values,
    } = raw;
    match ty.as_str() {
        "bool" => {
            if values.is_some() {
                return Err(ManifestError::OptionValuesNotAllowed { option: name, ty });
            }
            let default = default
                .ok_or_else(|| ManifestError::OptionMissingDefault {
                    option: name.clone(),
                })?
                .as_bool()
                .ok_or_else(|| ManifestError::OptionDefaultWrongType {
                    option: name.clone(),
                    ty: ty.clone(),
                })?;
            Ok(OptionDecl::Bool { default })
        }
        "enum" => {
            let values = values.ok_or_else(|| ManifestError::EnumOptionNoValues {
                option: name.clone(),
            })?;
            let default = default
                .ok_or_else(|| ManifestError::OptionMissingDefault {
                    option: name.clone(),
                })?
                .as_str()
                .ok_or_else(|| ManifestError::OptionDefaultWrongType {
                    option: name.clone(),
                    ty: ty.clone(),
                })?
                .to_owned();
            Ok(OptionDecl::Enum { values, default })
        }
        "string" => {
            if values.is_some() {
                return Err(ManifestError::OptionValuesNotAllowed { option: name, ty });
            }
            let default = default
                .ok_or_else(|| ManifestError::OptionMissingDefault {
                    option: name.clone(),
                })?
                .as_str()
                .ok_or_else(|| ManifestError::OptionDefaultWrongType {
                    option: name.clone(),
                    ty: ty.clone(),
                })?
                .to_owned();
            Ok(OptionDecl::String { default })
        }
        "integer" => {
            if values.is_some() {
                return Err(ManifestError::OptionValuesNotAllowed { option: name, ty });
            }
            let default = default
                .ok_or_else(|| ManifestError::OptionMissingDefault {
                    option: name.clone(),
                })?
                .as_integer()
                .ok_or_else(|| ManifestError::OptionDefaultWrongType {
                    option: name.clone(),
                    ty: ty.clone(),
                })?;
            Ok(OptionDecl::Integer { default })
        }
        other => Err(ManifestError::UnsupportedOptionType {
            option: name,
            ty: other.to_owned(),
        }),
    }
}

fn variants_from_raw(
    raw: std::collections::BTreeMap<String, RawVariant>,
) -> std::collections::BTreeMap<String, VariantDecl> {
    raw.into_iter()
        .map(|(name, v)| {
            (
                name,
                VariantDecl {
                    values: v.values,
                    default: v.default,
                },
            )
        })
        .collect()
}

fn target_from_raw(name: String, raw: RawTarget) -> Result<Target, ManifestError> {
    let RawTarget {
        kind,
        sources,
        include_dirs,
        defines,
        deps,
        manifest_path,
        crate_type,
        crate_name,
        features,
        default_features,
    } = raw;

    let target_name = TargetName::new(name.clone())?;
    let kind = parse_target_kind(&name, &kind)?;

    if kind == TargetKind::CppHeaderOnly && !sources.is_empty() {
        return Err(ManifestError::HeaderOnlyDeclaresSources { target: name });
    }

    let rust = build_rust_target(
        &name,
        kind,
        manifest_path,
        crate_type,
        crate_name,
        features,
        default_features,
    )?;

    Ok(Target {
        name: target_name,
        kind,
        sources,
        include_dirs,
        defines,
        deps,
        rust,
    })
}

fn build_rust_target(
    target_name: &str,
    kind: TargetKind,
    manifest_path: Option<String>,
    crate_type: Option<String>,
    crate_name: Option<String>,
    features: Vec<String>,
    default_features: Option<bool>,
) -> Result<Option<RustTarget>, ManifestError> {
    let is_rust = matches!(kind, TargetKind::RustLibrary | TargetKind::RustExecutable);
    let any_rust_field = manifest_path.is_some()
        || crate_type.is_some()
        || crate_name.is_some()
        || !features.is_empty()
        || default_features.is_some();

    if !is_rust {
        if any_rust_field {
            return Err(ManifestError::RustFieldOnNonRustTarget {
                target: target_name.to_owned(),
                kind: kind.as_str(),
            });
        }
        return Ok(None);
    }

    let manifest_path = manifest_path.ok_or_else(|| ManifestError::RustMissingManifestPath {
        target: target_name.to_owned(),
    })?;
    if manifest_path.is_empty() {
        return Err(ManifestError::RustMissingManifestPath {
            target: target_name.to_owned(),
        });
    }

    Ok(Some(RustTarget {
        manifest_path: PathBuf::from(manifest_path),
        // Default to `staticlib` so the most common case Just Works.
        // `cabin-rust` validates the value at planning time so we do
        // not duplicate the supported-set in two places.
        crate_type: crate_type.unwrap_or_else(|| "staticlib".to_owned()),
        crate_name,
        features,
        default_features: default_features.unwrap_or(true),
    }))
}

/// Raw shape of one `[target.'cfg(...)'.<...>]` entry, after we
/// have decided the entry is a target-conditional dep table
/// (i.e. the outer name is a `cfg(...)` expression). Captured
/// up-front so we can fold these into `Package::dependencies`
/// alongside the unconditional ones.
struct RawConditionalTarget {
    condition: Condition,
    deps: std::collections::BTreeMap<String, RawDependency>,
    build_deps: std::collections::BTreeMap<String, RawDependency>,
    dev_deps: std::collections::BTreeMap<String, RawDependency>,
    toolchain: Option<crate::raw::RawToolchain>,
    profile: Option<crate::raw::RawProfileFlags>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConditionalTargetTable {
    #[serde(default)]
    dependencies: std::collections::BTreeMap<String, RawDependency>,
    #[serde(default, rename = "build-dependencies")]
    build_dependencies: std::collections::BTreeMap<String, RawDependency>,
    #[serde(default, rename = "dev-dependencies")]
    dev_dependencies: std::collections::BTreeMap<String, RawDependency>,
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
        build_deps: typed.build_dependencies,
        dev_deps: typed.dev_dependencies,
        toolchain: typed.toolchain,
        profile: typed.profile,
    })
}

fn parse_target_table(
    raw: std::collections::BTreeMap<String, toml::Value>,
) -> Result<std::collections::BTreeMap<String, RawTarget>, ManifestError> {
    let mut out = std::collections::BTreeMap::new();
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
            workspace,
            system,
            optional,
            git,
            registry,
            source,
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
            // Reject reserved-future fields with explicit errors so
            // users get an actionable message rather than a generic
            // "unknown field".
            if git.is_some() {
                return Err(ManifestError::UnsupportedDependencyField {
                    name,
                    section,
                    field: "git",
                    detail: "Git source dependencies are not currently supported",
                });
            }
            if registry.is_some() {
                return Err(ManifestError::UnsupportedDependencyField {
                    name,
                    section,
                    field: "registry",
                    detail: "named-registry sources are not currently supported",
                });
            }
            if source.is_some() {
                return Err(ManifestError::UnsupportedDependencyField {
                    name,
                    section,
                    field: "source",
                    detail: "alternate-source dependencies are not currently supported",
                });
            }

            // `optional = true` is supported for normal / build
            // dependencies. Dev declarations remain not-optional
            // in this step.
            let optional_flag = optional.unwrap_or(false);
            if optional_flag && !matches!(kind, DependencyKind::Normal | DependencyKind::Build) {
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
                    return Err(ManifestError::WorkspaceDependencyExplicitlyDisabled { name });
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
/// `[build-dependencies]` / `[dev-dependencies]` entry that
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
        workspace,
        system,
        optional,
        git,
        registry,
        source,
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
        ("workspace", workspace.is_some()),
        ("git", git.is_some()),
        ("registry", registry.is_some()),
        ("source", source.is_some()),
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
        "cpp_library" => Ok(TargetKind::CppLibrary),
        "cpp_header_only" => Ok(TargetKind::CppHeaderOnly),
        "cpp_executable" => Ok(TargetKind::CppExecutable),
        "cpp_test" => Ok(TargetKind::CppTest),
        "cpp_example" => Ok(TargetKind::CppExample),
        "rust_library" => Ok(TargetKind::RustLibrary),
        "rust_executable" => Ok(TargetKind::RustExecutable),
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
        type = "cpp_executable"
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
    fn parses_cpp_executable_target() {
        let package = parse_project(FULL);
        assert_eq!(package.targets.len(), 1);
        let target = &package.targets[0];
        assert_eq!(target.name.as_str(), "hello");
        assert_eq!(target.kind, TargetKind::CppExecutable);
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
            type = "cpp_library"
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
    fn cpp_header_only_kind_is_accepted() {
        let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"

            [target.hdr]
            type = "cpp_header_only"
            include_dirs = ["include"]
        "#;
        let package = parse_project(manifest);
        let target = &package.targets[0];
        assert_eq!(target.kind, TargetKind::CppHeaderOnly);
        assert!(target.sources.is_empty());
    }

    #[test]
    fn cpp_header_only_rejects_sources() {
        let manifest = r#"
            [package]
            name = "hdr"
            version = "0.1.0"

            [target.hdr]
            type = "cpp_header_only"
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
    fn omitted_optional_arrays_default_to_empty() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target.hello]
            type = "cpp_executable"
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
            type = "cpp_library"

            [target.b]
            type = "cpp_executable"

            [target.c]
            type = "rust_library"
            manifest_path = "rust/Cargo.toml"

            [target.d]
            type = "rust_executable"
            manifest_path = "rust-bin/Cargo.toml"

            [target.e]
            type = "cpp_test"
            sources = ["tests/e.cc"]

            [target.f]
            type = "cpp_example"
            sources = ["examples/f.cc"]
        "#;
        let package = parse_project(manifest);
        let kinds: Vec<TargetKind> = package.targets.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TargetKind::CppLibrary,
                TargetKind::CppExecutable,
                TargetKind::RustLibrary,
                TargetKind::RustExecutable,
                TargetKind::CppTest,
                TargetKind::CppExample,
            ]
        );
    }

    #[test]
    fn rust_library_defaults_crate_type_to_staticlib() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [target.rust_core]
            type = "rust_library"
            manifest_path = "rust/Cargo.toml"
        "#;
        let package = parse_project(manifest);
        let rust = package.targets[0].rust.as_ref().unwrap();
        assert_eq!(rust.crate_type, "staticlib");
        assert_eq!(rust.manifest_path, PathBuf::from("rust/Cargo.toml"));
        assert!(rust.crate_name.is_none());
        assert!(rust.features.is_empty());
        assert!(rust.default_features);
    }

    #[test]
    fn rust_library_records_explicit_fields() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [target.rust_core]
            type = "rust_library"
            manifest_path = "rust/Cargo.toml"
            crate_type = "staticlib"
            crate_name = "rust-core"
            features = ["ffi", "logging"]
            default_features = false
        "#;
        let package = parse_project(manifest);
        let rust = package.targets[0].rust.as_ref().unwrap();
        assert_eq!(rust.crate_type, "staticlib");
        assert_eq!(rust.crate_name.as_deref(), Some("rust-core"));
        assert_eq!(rust.features, vec!["ffi".to_string(), "logging".into()]);
        assert!(!rust.default_features);
    }

    #[test]
    fn rust_library_missing_manifest_path_errors() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [target.rust_core]
            type = "rust_library"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::RustMissingManifestPath { target } => {
                assert_eq!(target, "rust_core");
            }
            other => panic!("expected RustMissingManifestPath, got {other:?}"),
        }
    }

    #[test]
    fn rust_field_on_cpp_target_errors() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [target.app]
            type = "cpp_executable"
            sources = ["src/main.cc"]
            manifest_path = "rust/Cargo.toml"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::RustFieldOnNonRustTarget { target, kind } => {
                assert_eq!(target, "app");
                assert_eq!(kind, "cpp_executable");
            }
            other => panic!("expected RustFieldOnNonRustTarget, got {other:?}"),
        }
    }

    #[test]
    fn cpp_target_does_not_carry_rust_block() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [target.app]
            type = "cpp_executable"
            sources = ["src/main.cc"]
        "#;
        let package = parse_project(manifest);
        assert!(package.targets[0].rust.is_none());
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
            type = "cpp_executable"
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
            type = "cpp_executable"
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
            type = "cpp_executable"
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
    /// malicious package write artefacts outside `--build-dir`.
    #[test]
    fn path_unsafe_target_name_errors() {
        let manifest = r#"
            [package]
            name = "hello"
            version = "0.1.0"

            [target."/tmp/out"]
            type = "cpp_executable"
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
            type = "cpp_executable"
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
    fn dependency_table_with_git_source_errors() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [dependencies]
            fmt = { version = "1.0", git = "https://example.com" }
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        match err {
            ManifestError::UnsupportedDependencyField {
                name,
                section,
                field,
                ..
            } => {
                assert_eq!(name, "fmt");
                assert_eq!(section, "[dependencies]");
                assert_eq!(field, "git");
            }
            other => panic!("expected UnsupportedDependencyField for `git`, got {other:?}"),
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
    // features / options / variants
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

    #[test]
    fn options_bool_with_default() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [options]
            warnings_as_errors = { type = "bool", default = false }
        "#;
        let package = parse_project(manifest);
        match &package.options["warnings_as_errors"] {
            cabin_core::OptionDecl::Bool { default } => assert!(!*default),
            other => panic!("expected Bool, got {other:?}"),
        }
    }

    #[test]
    fn options_enum_validates_default() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [options]
            allocator = { type = "enum", values = ["system", "mimalloc"], default = "mimalloc" }
        "#;
        let package = parse_project(manifest);
        match &package.options["allocator"] {
            cabin_core::OptionDecl::Enum { values, default } => {
                assert_eq!(default, "mimalloc");
                assert_eq!(values, &vec!["system".to_string(), "mimalloc".into()]);
            }
            other => panic!("expected Enum, got {other:?}"),
        }
    }

    #[test]
    fn options_enum_default_must_be_in_values() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [options]
            allocator = { type = "enum", values = ["system", "mimalloc"], default = "jemalloc" }
        "#;
        match parse_manifest_str(manifest).unwrap_err() {
            ManifestError::Validation(ValidationError::OptionDefaultNotInValues {
                option, ..
            }) => assert_eq!(option, "allocator"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn options_unsupported_type_errors() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [options]
            x = { type = "uuid", default = "abc" }
        "#;
        match parse_manifest_str(manifest).unwrap_err() {
            ManifestError::UnsupportedOptionType { option, ty } => {
                assert_eq!(option, "x");
                assert_eq!(ty, "uuid");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn options_bool_wrong_default_type_errors() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [options]
            x = { type = "bool", default = "true" }
        "#;
        match parse_manifest_str(manifest).unwrap_err() {
            ManifestError::OptionDefaultWrongType { option, ty } => {
                assert_eq!(option, "x");
                assert_eq!(ty, "bool");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn variants_declaration() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [variants]
            linkage = { values = ["static", "shared"], default = "static" }
            stdlib = { values = ["default", "libstdc++", "libc++"], default = "default" }
        "#;
        let package = parse_project(manifest);
        assert_eq!(package.variants["linkage"].default, "static");
        assert_eq!(
            package.variants["linkage"].values,
            vec!["static".to_string(), "shared".into()]
        );
    }

    #[test]
    fn variant_default_must_be_in_values() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"

            [variants]
            linkage = { values = ["static", "shared"], default = "dynamic" }
        "#;
        match parse_manifest_str(manifest).unwrap_err() {
            ManifestError::Validation(ValidationError::VariantDefaultNotInValues {
                variant,
                default,
                ..
            }) => {
                assert_eq!(variant, "linkage");
                assert_eq!(default, "dynamic");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn no_features_options_variants_is_fine() {
        let manifest = r#"
            [package]
            name = "demo"
            version = "0.1.0"
        "#;
        let package = parse_project(manifest);
        assert!(package.features.default.is_empty());
        assert!(package.features.features.is_empty());
        assert!(package.options.is_empty());
        assert!(package.variants.is_empty());
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

            [build-dependencies]
            codegen = "^1"

            [dev-dependencies]
            gtest = "^1.14"
        "#,
        );
        for (kind, expected_name) in [
            (DependencyKind::Normal, "fmt"),
            (DependencyKind::Build, "codegen"),
            (DependencyKind::Dev, "gtest"),
        ] {
            let deps = deps_of_kind(&package, kind);
            assert_eq!(deps.len(), 1, "{kind:?} should have one dep");
            assert_eq!(deps[0].name.as_str(), expected_name);
            assert_eq!(deps[0].kind, kind);
        }
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
        let by_name: std::collections::BTreeMap<&str, &cabin_core::SystemDependency> = package
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

            [build-dependencies]
            cmake = { version = ">=3", system = true }

            [dev-dependencies]
            gtest = { version = "^1.14", system = true }
        "#,
        );
        let by_name: std::collections::BTreeMap<&str, &cabin_core::SystemDependency> = package
            .system_dependencies
            .iter()
            .map(|sd| (sd.name.as_str(), sd))
            .collect();
        assert_eq!(by_name["zlib"].kind, DependencyKind::Normal);
        assert_eq!(by_name["cmake"].kind, DependencyKind::Build);
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

            [build-dependencies]
            fmt = ">=10"

            [dev-dependencies]
            fmt = ">=10"
        "#,
        );
        // The duplicate-policy spec: same name across kinds is allowed.
        assert_eq!(deps_of_kind(&package, DependencyKind::Normal).len(), 1);
        assert_eq!(deps_of_kind(&package, DependencyKind::Build).len(), 1);
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
    fn build_dep_features_field_is_parsed() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [build-dependencies]
            codegen = { version = "^1", features = ["log"], default-features = false }
        "#;
        let package = parse_project(manifest);
        let dep = package
            .dependencies
            .iter()
            .find(|d| d.name.as_str() == "codegen")
            .unwrap();
        assert_eq!(dep.kind, DependencyKind::Build);
        assert_eq!(dep.features, vec!["log".to_string()]);
        assert!(!dep.default_features);
        assert!(!dep.optional);
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
        // `[test-dependencies]` is not a recognised top-level
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

            [target.'cfg(any(os = "macos", os = "linux"))'.build-dependencies]
            codegen = "^1"

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
        let build = deps_of_kind(&package, DependencyKind::Build);
        assert_eq!(build.len(), 1);
        assert!(matches!(
            build[0].condition.as_ref().map(ToString::to_string),
            Some(ref s) if s.starts_with("any(") && s.contains("os = \"linux\"")
        ));
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
    fn unsupported_profile_field_compiler_is_rejected() {
        let manifest = r#"
            [package]
            name = "app"
            version = "0.1.0"

            [profile.release]
            compiler = "gcc"
        "#;
        let err = parse_manifest_str(manifest).unwrap_err();
        assert!(matches!(err, ManifestError::Toml(_)));
        let msg = err.to_string();
        assert!(msg.contains("compiler"), "{msg}");
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

            [build-dependencies]
            codegen = { workspace = true }
        "#,
        );
        let deps = deps_of_kind(&package, DependencyKind::Normal);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].source, DependencySource::Workspace);
        let deps = deps_of_kind(&package, DependencyKind::Build);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].source, DependencySource::Workspace);
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
        assert!(package.compiler_wrapper.profile_overrides.is_empty());
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
    fn patch_table_rejects_git_source_with_dedicated_message() {
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [patch]
            fmt = { git = "https://example.com/fmt.git" }
        "#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("`fmt`")
                && message.contains("`git`")
                && message.contains("not supported"),
            "expected git rejection, got: {message}",
        );
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

    // -----------------------------------------------------------------
    // [lint.cpplint] table.
    // -----------------------------------------------------------------

    #[test]
    fn lint_cpplint_table_parses_filter_list() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [lint.cpplint]
            filters = ["-build/c++11", "-whitespace/braces"]
        "#,
        );
        assert_eq!(
            package.lint.cpplint.filters,
            vec!["-build/c++11", "-whitespace/braces"]
        );
    }

    #[test]
    fn lint_cpplint_preserves_declaration_order() {
        // cpplint filter precedence is position-sensitive: a
        // later `+foo` re-enables what an earlier `-foo`
        // disabled. The parser must not sort or dedupe.
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [lint.cpplint]
            filters = ["-build", "+build/c++11", "-build/c++14"]
        "#,
        );
        assert_eq!(
            package.lint.cpplint.filters,
            vec!["-build", "+build/c++11", "-build/c++14"]
        );
    }

    #[test]
    fn lint_cpplint_empty_filter_list_is_accepted() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [lint.cpplint]
            filters = []
        "#,
        );
        assert!(package.lint.cpplint.filters.is_empty());
        assert!(package.lint.is_empty());
    }

    #[test]
    fn lint_cpplint_missing_table_yields_empty_settings() {
        let package = parse_project(
            r#"
            [package]
            name = "app"
            version = "0.1.0"
        "#,
        );
        assert!(package.lint.is_empty());
    }

    #[test]
    fn lint_cpplint_empty_filter_entry_is_rejected() {
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [lint.cpplint]
            filters = ["-build/c++11", ""]
        "#,
        )
        .unwrap_err();
        match err {
            ManifestError::InvalidLintFilter { section, .. } => {
                assert_eq!(section, "lint.cpplint.filters");
            }
            other => panic!("expected InvalidLintFilter, got {other:?}"),
        }
    }

    #[test]
    fn lint_cpplint_unknown_key_is_rejected() {
        // `deny_unknown_fields` on RawCpplintLint catches this.
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [lint.cpplint]
            filters = []
            mystery = true
        "#,
        )
        .unwrap_err();
        assert!(matches!(err, ManifestError::Toml(_)));
    }

    #[test]
    fn lint_unknown_subtable_is_rejected() {
        // `deny_unknown_fields` on RawLint catches an unknown
        // sub-table.
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [lint.unknown_tool]
            filters = []
        "#,
        )
        .unwrap_err();
        assert!(matches!(err, ManifestError::Toml(_)));
    }

    #[test]
    fn lint_filters_wrong_type_is_rejected() {
        // `filters = "not a list"` must not silently become a
        // single-element list — serde rejects the type.
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"

            [lint.cpplint]
            filters = "all"
        "#,
        )
        .unwrap_err();
        assert!(matches!(err, ManifestError::Toml(_)));
    }

    // -----------------------------------------------------------------
    // `[package].language` rejection — Cabin's language semantics are
    // target-level (target kinds, source classification, toolchain
    // selection), not package-level. A package-level `language` field
    // would be informational at best and misleading for mixed-language
    // packages. The manifest grammar refuses the field outright so a
    // user who writes one gets the standard unknown-field diagnostic
    // instead of having Cabin silently accept and ignore it.
    // -----------------------------------------------------------------

    #[test]
    fn package_language_field_is_rejected() {
        let err = parse_manifest_str(
            r#"
            [package]
            name = "app"
            version = "0.1.0"
            language = "c++"
        "#,
        )
        .unwrap_err();
        match err {
            ManifestError::Toml(source) => {
                let message = source.to_string();
                assert!(
                    message.contains("unknown field `language`"),
                    "unexpected error: {message}"
                );
            }
            other => panic!("expected TOML parse error, got {other:?}"),
        }
    }
}
