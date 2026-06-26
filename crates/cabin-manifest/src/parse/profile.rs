use crate::error::ManifestError;
use cabin_core::{Features, PackageName};
use std::collections::BTreeMap;

/// Validate every `[profile.<name>]` table and convert it into a
/// typed [`cabin_core::ProfileDefinition`].  Errors short-circuit
/// the whole manifest because partial profile state would be
/// surprising downstream.
pub(super) fn profiles_from_raw(
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
/// [`cabin_core::ToolchainDecl`].  The future-feature `compiler-family`,
/// `compiler-version`, and similar capability-style fields are
/// rejected at the TOML layer because [`crate::raw::RawToolchain`]
/// uses `deny_unknown_fields`.
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

/// Build a per-profile [`cabin_core::ProfileFlags`] from the
/// flag-override fields declared directly on a `[profile.<name>]`
/// table.  Returns `None` when the user supplied no overrides at
/// all; the resolver will then fall back to the base
/// `[profile]` layer for this profile.
///
/// Flag fields are `Option<Vec<...>>` to preserve the distinction
/// between "the user did not override this field" and "the user
/// explicitly set this field to an empty list"; the conversion
/// below collapses both into the legacy `Vec<...>` shape because
/// the typed [`cabin_core::ProfileFlags`] cannot represent that
/// distinction yet.  Override-vs-append precedence is documented
/// at the resolver layer.
pub(super) fn profile_flags_from_overrides(
    raw: &crate::raw::RawProfile,
    pname: &cabin_core::ProfileName,
) -> Result<Option<cabin_core::ProfileFlags>, ManifestError> {
    if raw.defines.is_none()
        && raw.include_dirs.is_none()
        && raw.cflags.is_none()
        && raw.cxxflags.is_none()
        && raw.ldflags.is_none()
        && raw.link_libs.is_none()
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
    };
    decl.validate().map_err(|err| {
        let _ = pname;
        ManifestError::InvalidBuildFlags(err)
    })?;
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

/// Extract a `[profile.cache] compiler-wrapper = "..."` declaration
/// from a `[profile]` table (or any of the same shape: profile / target-
/// conditioned).  Returns `None` when neither `[profile.cache]` nor its
/// `compiler-wrapper` field is present. `section` is the TOML path
/// echoed back in the error message so the user sees exactly which
/// table they need to fix.
pub(super) fn compiler_wrapper_request_from_raw_build_ref(
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

pub(super) fn features_from_raw(mut raw: BTreeMap<String, Vec<String>>) -> Features {
    let default = raw
        .remove(cabin_core::DEFAULT_FEATURE_KEY)
        .unwrap_or_default();
    Features {
        default,
        features: raw,
    }
}
