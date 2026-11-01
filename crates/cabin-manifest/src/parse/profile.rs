use crate::error::ManifestError;
use cabin_core::{Features, PackageName};
use std::collections::BTreeMap;

/// Validate every `[profile.<name>]` table and convert it into a
/// typed [`cabin_core::ProfileDefinition`].
pub(super) fn profiles_from_raw(
    raw: BTreeMap<String, crate::raw::RawProfile>,
) -> Result<BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>, ManifestError> {
    let mut out = BTreeMap::new();
    for (name, raw_profile) in raw {
        let pname = cabin_core::ProfileName::new(name.clone())
            .map_err(ManifestError::InvalidProfileName)?;
        let inherits = raw_profile
            .inherits
            .clone()
            .map(|v| {
                cabin_core::ProfileName::new(v).map_err(|source| {
                    ManifestError::InvalidInheritedProfileName {
                        profile: pname.as_str().to_owned(),
                        source,
                    }
                })
            })
            .transpose()?;
        let build = profile_flags_from_overrides(&raw_profile)?;
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
/// [`cabin_core::ToolchainDecl`].  Unknown keys are rejected at the
/// TOML layer by `deny_unknown_fields` on
/// [`crate::raw::RawToolchain`].
pub(super) fn toolchain_decl_from_raw_ref(
    raw: &crate::raw::RawToolchain,
) -> Result<cabin_core::ToolchainDecl, ManifestError> {
    let cc = raw.cc.as_deref().map(parse_tool_spec).transpose()?;
    let cxx = raw.cxx.as_deref().map(parse_tool_spec).transpose()?;
    let ar = raw.ar.as_deref().map(parse_tool_spec).transpose()?;
    Ok(cabin_core::ToolchainDecl { cc, cxx, ar })
}

pub(super) fn parse_tool_spec(raw: &str) -> Result<cabin_core::ToolSpec, ManifestError> {
    cabin_core::ToolSpec::parse_non_empty(raw).ok_or(ManifestError::EmptyToolSpec)
}

/// Build a per-profile [`cabin_core::ProfileFlags`] from the flag
/// fields declared directly on a `[profile.<name>]` table, or `None`
/// when it declares none.  Undeclared fields become empty lists:
/// `cabin_core::resolve_build_flags` appends declared lists after
/// the inherited layers, so an empty list and an absent field have
/// the same effect.
pub(super) fn profile_flags_from_overrides(
    raw: &crate::raw::RawProfile,
) -> Result<Option<cabin_core::ProfileFlags>, ManifestError> {
    if raw.defines.is_none()
        && raw.include_dirs.is_none()
        && raw.cflags.is_none()
        && raw.cxxflags.is_none()
        && raw.ldflags.is_none()
        && raw.link_libs.is_none()
        && raw.frameworks.is_none()
    {
        return Ok(None);
    }
    let decl = cabin_core::ProfileFlags {
        defines: raw.defines.clone().unwrap_or_default(),
        include_dirs: raw.include_dirs.clone().unwrap_or_default(),
        cflags: raw.cflags.clone().unwrap_or_default(),
        cxxflags: raw.cxxflags.clone().unwrap_or_default(),
        ldflags: raw.ldflags.clone().unwrap_or_default(),
        link_libs: raw.link_libs.clone().unwrap_or_default(),
        frameworks: raw.frameworks.clone().unwrap_or_default(),
    };
    decl.validate().map_err(ManifestError::InvalidBuildFlags)?;
    Ok(Some(decl))
}

pub(super) fn build_flags_decl_from_raw_ref(
    raw: &crate::raw::RawProfileFlags,
) -> Result<cabin_core::ProfileFlags, ManifestError> {
    let decl = cabin_core::ProfileFlags {
        defines: raw.defines.clone(),
        include_dirs: raw.include_dirs.clone(),
        cflags: raw.cflags.clone(),
        cxxflags: raw.cxxflags.clone(),
        ldflags: raw.ldflags.clone(),
        link_libs: raw.link_libs.clone(),
        frameworks: raw.frameworks.clone(),
    };
    decl.validate().map_err(ManifestError::InvalidBuildFlags)?;
    Ok(decl)
}

/// Convert a raw `[patch]` table into typed
/// [`cabin_core::PatchManifestSettings`].  The only supported
/// source kind is `path = "..."`; every other key is rejected
/// by `deny_unknown_fields` on [`crate::raw::RawPatch`].
pub(super) fn patch_settings_from_raw(
    raw: BTreeMap<String, crate::raw::RawPatch>,
) -> Result<cabin_core::PatchManifestSettings, ManifestError> {
    use cabin_core::PatchSource;

    let mut entries = BTreeMap::new();
    for (name, row) in raw {
        let package = PackageName::new(name).map_err(ManifestError::Validation)?;
        let crate::raw::RawPatch { path } = row;
        let source = PatchSource::from_path_field(package.as_str(), path).map_err(|source| {
            ManifestError::InvalidPatch {
                package: package.as_str().to_owned(),
                source,
            }
        })?;
        entries.insert(package, source);
    }
    Ok(cabin_core::PatchManifestSettings { entries })
}

/// Convert a raw `[system-provided]` table into typed
/// [`cabin_core::SystemProvidedSettings`].
pub(super) fn system_provided_from_raw(
    raw: BTreeMap<String, crate::raw::RawSystemProvider>,
) -> Result<cabin_core::SystemProvidedSettings, ManifestError> {
    let mut entries = BTreeMap::new();
    for (name, row) in raw {
        let package = PackageName::new(name).map_err(ManifestError::Validation)?;
        let targets = row.targets.map(|rows| {
            rows.into_iter()
                .map(|(target, row)| (target, row.pkg_config))
                .collect()
        });
        let provider =
            cabin_core::SystemProvider::new(row.pkg_config, targets).map_err(|source| {
                ManifestError::InvalidSystemProvider {
                    package: package.as_str().to_owned(),
                    source,
                }
            })?;
        entries.insert(package, provider);
    }
    Ok(cabin_core::SystemProvidedSettings { entries })
}

pub(super) fn features_from_raw(mut raw: BTreeMap<String, Vec<String>>) -> Features {
    let default = raw
        .remove(cabin_core::DEFAULT_FEATURE_KEY)
        .unwrap_or_default();
    Features {
        default,
        features: raw,
    }
}
