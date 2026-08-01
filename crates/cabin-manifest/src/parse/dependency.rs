use crate::error::ManifestError;
use crate::raw::{RawDependency, RawDependencyTable};
use cabin_core::{
    Condition, Dependency, DependencyKind, DependencySource, PackageName, SystemDependency,
};
use camino::Utf8PathBuf;

/// Inspect `raw` and route it onto either `dep_models`
/// (Cabin-package dependency) or `system_models` (system-sourced
/// dependency, probed via pkg-config at build time).  The
/// `system = true` flag on a `RawDependencyTable` is the only
/// signal that selects the system path; bare-string entries
/// (`name = "^1"`) always mean registry source.
pub(super) fn route_dependency_from_raw(
    name: String,
    raw: RawDependency,
    kind: DependencyKind,
    condition: Option<Condition>,
    dep_models: &mut Vec<Dependency>,
    system_models: &mut Vec<SystemDependency>,
) -> Result<(), ManifestError> {
    match raw {
        RawDependency::Table(table) if table.system => {
            system_models.push(system_dependency_from_raw_table(
                name, table, kind, condition,
            )?);
        }
        other => dep_models.push(package_dependency_from_raw(name, other, kind, condition)?),
    }
    Ok(())
}

/// Resolved dependency fields before assembling the final
/// `Dependency`.  A named struct (rather than a positional 4-tuple)
/// so the construction arms and the destructure in
/// [`package_dependency_from_raw`] name each field - a field
/// reorder can no longer silently swap two values.
struct ResolvedDep {
    source: DependencySource,
    optional: bool,
    features: Vec<String>,
    default_features: bool,
    ignore_interface_standard: bool,
}

pub(super) fn package_dependency_from_raw(
    name: String,
    raw: RawDependency,
    kind: DependencyKind,
    condition: Option<Condition>,
) -> Result<Dependency, ManifestError> {
    let section = kind.manifest_section();
    let raw_outcome: ResolvedDep = match raw {
        RawDependency::String(s) => ResolvedDep {
            source: DependencySource::Version(parse_version_req(&name, &s)?),
            optional: false,
            features: Vec::new(),
            default_features: true,
            ignore_interface_standard: false,
        },
        RawDependency::Table(table) => {
            // The router catches `system = true`.  Reaching this
            // arm with `system = true` is an internal invariant
            // violation; fail loudly so a future refactor cannot
            // silently drop the system path.
            debug_assert!(!table.system, "router should have routed system deps");
            if table.system {
                return Err(ManifestError::SystemConflictsWith {
                    name,
                    section,
                    field: "system",
                    detail: "system = true must be routed before package_dependency_from_raw",
                });
            }

            ordinary_dep_from_table(&name, table, kind)?
        }
    };
    let ResolvedDep {
        source,
        optional,
        features,
        default_features,
        ignore_interface_standard,
    } = raw_outcome;
    // `workspace = true` inside a target-conditional table is
    // not currently supported - workspace inheritance has no
    // per-condition table to look up against, and silently
    // pretending the lookup is unconditional would be
    // surprising.  Reject explicitly so users get a clear
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
        ignore_interface_standard,
    })
}

/// Resolve an ordinary dependency table whose source is one of
/// `path`, `version`, or `workspace = true`.
fn ordinary_dep_from_table(
    name: &str,
    table: RawDependencyTable,
    kind: DependencyKind,
) -> Result<ResolvedDep, ManifestError> {
    let RawDependencyTable {
        path,
        version,
        workspace,
        system: _,
        optional,
        features,
        default_features,
        ignore_interface_standard,
    } = table;
    // `optional = true` is supported only for normal
    // dependencies.  Dev declarations remain not-optional
    // in this step.
    let optional_flag = optional.unwrap_or(false);
    if optional_flag && !matches!(kind, DependencyKind::Normal) {
        return Err(ManifestError::OptionalNotSupportedForKind {
            name: name.to_owned(),
            kind,
        });
    }

    let features_vec = features.unwrap_or_default();
    if features_vec.iter().any(String::is_empty) {
        return Err(ManifestError::EmptyDependencyFeatureName {
            name: name.to_owned(),
        });
    }
    let default_features_flag = default_features.unwrap_or(true);

    // `workspace = false` is an explicit-disable error, distinct
    // from an absent field.
    let resolved_source = match (path, version, workspace) {
        (Some(_), Some(_), _) => {
            return Err(ManifestError::DependencyHasPathAndVersion {
                name: name.to_owned(),
            });
        }
        (Some(_), _, Some(true)) | (_, Some(_), Some(true)) => {
            return Err(ManifestError::WorkspaceDependencyHasOtherSource {
                name: name.to_owned(),
            });
        }
        (Some(path), None, _) => DependencySource::Path(Utf8PathBuf::from(path)),
        (None, Some(req), _) => DependencySource::Version(parse_version_req(name, &req)?),
        (None, None, Some(true)) => DependencySource::Workspace,
        (None, None, Some(false)) => {
            return Err(ManifestError::WorkspaceDependencyExplicitlyDisabled {
                name: name.to_owned(),
            });
        }
        (None, None, None) => {
            return Err(ManifestError::DependencyMissingSource {
                name: name.to_owned(),
            });
        }
    };
    Ok(ResolvedDep {
        source: resolved_source,
        optional: optional_flag,
        features: features_vec,
        default_features: default_features_flag,
        ignore_interface_standard: ignore_interface_standard.unwrap_or(false),
    })
}

/// Produce a `SystemDependency` from a `[dependencies]` /
/// `[dev-dependencies]` entry that
/// carries `system = true`.  Only `version` is permitted
/// alongside the flag; every other field is rejected with a
/// clear error so users learn the rule.
pub(super) fn system_dependency_from_raw_table(
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
        features,
        default_features,
        ignore_interface_standard,
    } = table;
    debug_assert!(system, "router only dispatches here when system = true");
    let _ = system;

    // Reject every field that has no meaning alongside
    // `system = true`.  The order matches the user-visible field
    // order so the first conflict reported is the one earliest
    // in the table.
    let forbidden: &[(&'static str, bool)] = &[
        ("path", path.is_some()),
        ("workspace", workspace.is_some()),
        ("features", features.is_some()),
        ("default-features", default_features.is_some()),
        ("optional", optional.is_some()),
        (
            "ignore-interface-standard",
            ignore_interface_standard.is_some(),
        ),
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

pub(super) fn parse_version_req(
    dep_name: &str,
    raw: &str,
) -> Result<semver::VersionReq, ManifestError> {
    cabin_core::version_req::parse_lenient(raw).map_err(|source| {
        ManifestError::InvalidDependencyRequirement {
            name: dep_name.to_owned(),
            requirement: raw.to_owned(),
            source,
        }
    })
}
