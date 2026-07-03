use crate::error::ManifestError;
use crate::raw::{RawDependency, RawStandardField, RawTarget};
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
        required_features,
        c_standard,
        cxx_standard,
        interface_c_standard,
        interface_cxx_standard,
        gnu_extensions,
    } = raw;

    let target_name = TargetName::new(name.clone())?;
    let kind = parse_target_kind(&name, &kind)?;

    if kind.is_header_only() && !sources.is_empty() {
        return Err(ManifestError::HeaderOnlyDeclaresSources { target: name });
    }

    // `{ workspace = true }` is a `[package]`-level opt-in; a
    // target-level marker has no workspace tier to inherit from.
    // `{ workspace = false }` flows into the shared field
    // validation, which reports it as explicitly disabled.
    for (raw_field, field) in [
        (&c_standard, "c-standard"),
        (&cxx_standard, "cxx-standard"),
        (&interface_c_standard, "interface-c-standard"),
        (&interface_cxx_standard, "interface-cxx-standard"),
    ] {
        if matches!(raw_field, Some(RawStandardField::Marker(m)) if m.workspace) {
            return Err(ManifestError::WorkspaceStandardOnTarget {
                target: name.clone(),
                field,
            });
        }
    }

    let language = crate::parse::language_settings_from_raw(
        c_standard.as_ref(),
        cxx_standard.as_ref(),
        interface_c_standard.as_ref(),
        interface_cxx_standard.as_ref(),
        gnu_extensions,
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
        required_features,
        language,
    })
}

/// Raw shape of one `[target.'cfg(...)'.<...>]` entry, after we
/// have decided the entry is a target-conditional dep table
/// (i.e. the outer name is a `cfg(...)` expression).  Captured
/// up-front so we can fold these into `Package::dependencies`
/// alongside the unconditional ones.
pub(super) struct RawConditionalTarget {
    pub(super) condition: Condition,
    pub(super) deps: BTreeMap<String, RawDependency>,
    pub(super) dev_deps: BTreeMap<String, RawDependency>,
    pub(super) toolchain: Option<crate::raw::RawToolchain>,
    pub(super) profile: Option<crate::raw::RawProfileFlags>,
    pub(super) named_profiles: Vec<(cabin_core::ProfileName, crate::raw::RawProfileFlags)>,
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
    profile: Option<toml::Table>,
}

/// Whether a `[target.<NAME>]` entry name should be interpreted
/// as a `cfg(...)` expression.  Cabin's existing buildable-target
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
    let (profile, named_profiles) = parse_conditional_profile(raw_target_name, typed.profile)?;
    Ok(RawConditionalTarget {
        condition,
        deps: typed.dependencies,
        dev_deps: typed.dev_dependencies,
        toolchain: typed.toolchain,
        profile,
        named_profiles,
    })
}

type ParsedConditionalProfile = (
    Option<crate::raw::RawProfileFlags>,
    Vec<(cabin_core::ProfileName, crate::raw::RawProfileFlags)>,
);

fn parse_conditional_profile(
    raw_target_name: &str,
    raw: Option<toml::Table>,
) -> Result<ParsedConditionalProfile, ManifestError> {
    let Some(raw) = raw else {
        return Ok((None, Vec::new()));
    };
    let mut general = toml::Table::new();
    let mut named = Vec::new();
    for (key, value) in raw {
        if let toml::Value::Table(fields) = &value {
            let profile = cabin_core::ProfileName::new(key)
                .map_err(|err| ManifestError::InvalidProfileName { value: err.0 })?;
            validate_named_overlay_fields(raw_target_name, &profile, fields)?;
            let flags = value.try_into().map_err(|source| {
                ManifestError::InvalidConditionalTargetTable {
                    raw: raw_target_name.to_owned(),
                    source: Box::new(source),
                }
            })?;
            named.push((profile, flags));
        } else {
            general.insert(key, value);
        }
    }
    let general = if general.is_empty() {
        None
    } else {
        Some(toml::Value::Table(general).try_into().map_err(|source| {
            ManifestError::InvalidConditionalTargetTable {
                raw: raw_target_name.to_owned(),
                source: Box::new(source),
            }
        })?)
    };
    Ok((general, named))
}

fn validate_named_overlay_fields(
    raw_target_name: &str,
    profile: &cabin_core::ProfileName,
    fields: &toml::Table,
) -> Result<(), ManifestError> {
    let table = named_overlay_table(raw_target_name, profile);
    for field in fields.keys() {
        match field.as_str() {
            "defines" | "include-dirs" | "cflags" | "cxxflags" | "ldflags" | "link-libs" => {}
            "inherits" => {
                return Err(ManifestError::NamedTargetProfileInherits {
                    table,
                    profile: profile.as_str().to_owned(),
                });
            }
            "debug" | "opt-level" | "assertions" | "toolchain" => {
                return Err(ManifestError::NamedTargetProfileField {
                    table,
                    field: field.clone(),
                });
            }
            _ => {
                return Err(ManifestError::UnknownNamedTargetProfileField {
                    table,
                    field: field.clone(),
                });
            }
        }
    }
    Ok(())
}

fn named_overlay_table(raw_target_name: &str, profile: &cabin_core::ProfileName) -> String {
    let profile = if profile.as_str().contains('.') {
        format!("'{}'", profile.as_str())
    } else {
        profile.as_str().to_owned()
    };
    format!("`[target.'{raw_target_name}'.profile.{profile}]`")
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
