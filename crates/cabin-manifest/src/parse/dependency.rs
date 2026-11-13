use crate::error::ManifestError;
use crate::raw::{RawDependency, RawDependencyTable};
use cabin_core::{
    Condition, Dependency, DependencyKind, DependencySource, PackageName, TargetName,
};
use camino::Utf8PathBuf;
use std::collections::BTreeSet;

/// Resolved dependency fields before assembling the final
/// `Dependency`.
struct ResolvedDep {
    source: DependencySource,
    optional: bool,
    features: Vec<String>,
    default_features: bool,
    ignore_interface_standard: bool,
    targets: BTreeSet<TargetName>,
}

pub(super) fn package_dependency_from_raw(
    name: String,
    raw: RawDependency,
    kind: DependencyKind,
    condition: Option<Condition>,
) -> Result<Dependency, ManifestError> {
    let raw_outcome: ResolvedDep = match raw {
        RawDependency::String(s) => ResolvedDep {
            source: DependencySource::Version(parse_version_req(&name, &s)?),
            optional: false,
            features: Vec::new(),
            default_features: true,
            ignore_interface_standard: false,
            targets: BTreeSet::new(),
        },
        RawDependency::Table(table) => {
            if table.system.is_some() {
                return Err(ManifestError::SystemDependencyRemoved {
                    name,
                    table: kind.manifest_table(condition.as_ref()),
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
        targets,
    } = raw_outcome;
    // `workspace = true` is rejected under a `cfg(...)` table:
    // there is no per-condition `[workspace.*]` table to inherit
    // from.
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
        targets,
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
        targets,
    } = table;
    // `optional = true` is not supported for dev dependencies.
    let optional_flag = optional.unwrap_or(false);
    if optional_flag && kind == DependencyKind::Dev {
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

    let targets = match targets {
        None => BTreeSet::new(),
        Some(_) if kind != DependencyKind::Build => {
            return Err(ManifestError::TargetsNotSupportedForKind {
                name: name.to_owned(),
                kind,
            });
        }
        Some(names) if names.is_empty() => {
            return Err(ManifestError::EmptyDependencyTargets {
                name: name.to_owned(),
            });
        }
        Some(names) => names
            .into_iter()
            .map(TargetName::new)
            .collect::<Result<_, _>>()?,
    };

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
        targets,
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
