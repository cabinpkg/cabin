use crate::error::ManifestError;
use crate::raw::{RawDependency, RawTarget};
use cabin_core::{Condition, Target, TargetKind, TargetName};
use serde::Deserialize;
use std::collections::BTreeMap;

pub(super) fn target_from_raw(name: String, raw: RawTarget) -> Result<Target, ManifestError> {
    let RawTarget {
        kind,
        sources,
        include_dirs,
        defines,
        deps,
        c_standard,
        cxx_standard,
        interface_c_standard,
        interface_cxx_standard,
    } = raw;

    let target_name = TargetName::new(name.clone())?;
    let kind = parse_target_kind(&name, &kind)?;

    if kind.is_header_only() && !sources.is_empty() {
        return Err(ManifestError::HeaderOnlyDeclaresSources { target: name });
    }

    let language = crate::parse::language_settings_from_raw(
        c_standard.as_deref(),
        cxx_standard.as_deref(),
        interface_c_standard.as_deref(),
        interface_cxx_standard.as_deref(),
    )?;
    // Interface standards describe what consumers of a library's
    // public headers need; executable-like targets have no
    // consumers, so a declared interface field there is a mistake.
    if kind.produces_executable() {
        let offending = if language.interface_c_standard.is_some() {
            Some("interface-c-standard")
        } else if language.interface_cxx_standard.is_some() {
            Some("interface-cxx-standard")
        } else {
            None
        };
        if let Some(field) = offending {
            return Err(ManifestError::InterfaceStandardOnNonLibrary {
                target: name,
                kind: kind.as_str().to_owned(),
                field,
            });
        }
    }

    Ok(Target {
        name: target_name,
        kind,
        sources,
        include_dirs,
        defines,
        deps,
        language,
    })
}

/// Raw shape of one `[target.'cfg(...)'.<...>]` entry, after we
/// have decided the entry is a target-conditional dep table
/// (i.e. the outer name is a `cfg(...)` expression). Captured
/// up-front so we can fold these into `Package::dependencies`
/// alongside the unconditional ones.
pub(super) struct RawConditionalTarget {
    pub(super) condition: Condition,
    pub(super) deps: BTreeMap<String, RawDependency>,
    pub(super) dev_deps: BTreeMap<String, RawDependency>,
    pub(super) toolchain: Option<crate::raw::RawToolchain>,
    pub(super) profile: Option<crate::raw::RawProfileFlags>,
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
pub(super) fn is_cfg_expression(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.starts_with("cfg(") && trimmed.ends_with(')')
}

pub(super) fn parse_conditional_target_entry(
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

pub(super) fn parse_target_table(
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

pub(super) fn parse_target_kind(
    target_name: &str,
    value: &str,
) -> Result<TargetKind, ManifestError> {
    match value {
        "library" => Ok(TargetKind::Library),
        "header-only" => Ok(TargetKind::HeaderOnly),
        "executable" => Ok(TargetKind::Executable),
        "test" => Ok(TargetKind::Test),
        "example" => Ok(TargetKind::Example),
        other => Err(ManifestError::UnknownTargetType {
            target: target_name.to_owned(),
            value: other.to_owned(),
        }),
    }
}
