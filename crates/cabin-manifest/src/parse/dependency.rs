use crate::error::ManifestError;
use crate::raw::{RawDependency, RawDependencyTable};
use cabin_core::{
    Condition, Dependency, DependencyKind, DependencySource, PackageName, PortDepSource,
    SystemDependency,
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
    if let RawDependency::Table(ref table) = raw
        && table.system
    {
        // Route to the system path.  Take ownership for clean
        // destructuring without aliasing the borrow.
        let RawDependency::Table(table) = raw else {
            unreachable!("guarded by the `if let ... && table.system` above");
        };
        system_models.push(system_dependency_from_raw_table(
            name, table, kind, condition,
        )?);
        return Ok(());
    }
    dep_models.push(package_dependency_from_raw(name, raw, kind, condition)?);
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
        },
        RawDependency::Table(mut table) => {
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

            // `port` / `port-path` are mutually exclusive with every
            // other source form and do not support feature gating
            // for this milestone.  Check both conditions before
            // routing through the path/version/workspace
            // selector so a port dep cannot silently shadow a
            // mistakenly-set field.
            let port_builtin = table.port.unwrap_or(false);
            match (port_builtin, table.port_path.take()) {
                (true, Some(_)) => {
                    return Err(ManifestError::PortDependencyHasOtherSource {
                        name,
                        conflicting: "port-path",
                    });
                }
                (true, None) => builtin_port_dep_from_table(&name, table)?,
                (false, Some(port_path)) => path_port_dep_from_table(&name, port_path, table)?,
                (false, None) => ordinary_dep_from_table(&name, table, kind)?,
            }
        }
    };
    let ResolvedDep {
        source,
        optional,
        features,
        default_features,
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
    })
}

/// Resolve a `port = true` (bundled foundation-port) dependency
/// table.  The caller's dispatch already rejected `port-path`.
fn builtin_port_dep_from_table(
    name: &str,
    table: RawDependencyTable,
) -> Result<ResolvedDep, ManifestError> {
    let RawDependencyTable {
        path,
        version,
        port: _,
        port_path: _,
        workspace,
        system: _,
        optional,
        features,
        default_features,
    } = table;
    if path.is_some() {
        return Err(ManifestError::PortDependencyHasOtherSource {
            name: name.to_owned(),
            conflicting: "path",
        });
    }
    if workspace.is_some() {
        return Err(ManifestError::PortDependencyHasOtherSource {
            name: name.to_owned(),
            conflicting: "workspace",
        });
    }
    // `optional` ports are still unsupported (the port
    // forms never enter the version resolver), but
    // `features` / `default-features` are honored: a
    // port's overlay can declare `[features]`, and the
    // feature resolver threads per-edge requests onto
    // the prepared port package like a path dep.
    if optional.is_some() {
        return Err(ManifestError::PortDependencyUnsupportedOption {
            name: name.to_owned(),
            conflicting: "optional",
        });
    }
    let (features_vec, default_features_flag) =
        port_feature_selection(name, features, default_features)?;
    let req_str = version.ok_or_else(|| ManifestError::PortDependencyMissingVersion {
        name: name.to_owned(),
    })?;
    let version_req = parse_version_req(name, &req_str)?;
    Ok(ResolvedDep {
        source: DependencySource::Port(PortDepSource::Builtin {
            name: PackageName::new(name.to_owned())?,
            version_req,
        }),
        optional: false,
        features: features_vec,
        default_features: default_features_flag,
    })
}

/// Resolve a `port-path = "..."` (local port overlay) dependency
/// table. `port_path` is the value the caller's dispatch took out
/// of the table.
fn path_port_dep_from_table(
    name: &str,
    port_path: String,
    table: RawDependencyTable,
) -> Result<ResolvedDep, ManifestError> {
    let RawDependencyTable {
        path,
        version,
        port: _,
        port_path: _,
        workspace,
        system: _,
        optional,
        features,
        default_features,
    } = table;
    if path.is_some() {
        return Err(ManifestError::PortDependencyHasOtherSource {
            name: name.to_owned(),
            conflicting: "path",
        });
    }
    if version.is_some() {
        return Err(ManifestError::PortDependencyHasOtherSource {
            name: name.to_owned(),
            conflicting: "version",
        });
    }
    if workspace.is_some() {
        return Err(ManifestError::PortDependencyHasOtherSource {
            name: name.to_owned(),
            conflicting: "workspace",
        });
    }
    if optional.is_some() {
        return Err(ManifestError::PortDependencyUnsupportedOption {
            name: name.to_owned(),
            conflicting: "optional",
        });
    }
    let (features_vec, default_features_flag) =
        port_feature_selection(name, features, default_features)?;
    Ok(ResolvedDep {
        source: DependencySource::Port(PortDepSource::Path(Utf8PathBuf::from(port_path))),
        optional: false,
        features: features_vec,
        default_features: default_features_flag,
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
        port: _,
        port_path: _,
        workspace,
        system: _,
        optional,
        features,
        default_features,
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

    let workspace_flag = workspace.unwrap_or(false);
    // `workspace = false` is treated as if the field were
    // absent so it never collides with a path/version source.
    let workspace_set = workspace.is_some();
    let resolved_source = match (path, version, workspace_flag, workspace_set) {
        (Some(_), Some(_), _, _) => {
            return Err(ManifestError::DependencyHasPathAndVersion {
                name: name.to_owned(),
            });
        }
        (Some(_), _, true, _) | (_, Some(_), true, _) => {
            return Err(ManifestError::WorkspaceDependencyHasOtherSource {
                name: name.to_owned(),
            });
        }
        (Some(path), None, false, _) => DependencySource::Path(Utf8PathBuf::from(path)),
        (None, Some(req), false, _) => DependencySource::Version(parse_version_req(name, &req)?),
        (None, None, true, _) => DependencySource::Workspace,
        (None, None, false, true) => {
            return Err(ManifestError::WorkspaceDependencyExplicitlyDisabled {
                name: name.to_owned(),
            });
        }
        (None, None, false, false) => {
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
    })
}

/// Validate and normalize the `features` / `default-features`
/// selection on a foundation-port dependency.  Mirrors the
/// validation the normal package-dependency path applies: feature
/// names must be non-empty, and an omitted `default-features`
/// defaults to `true`.
fn port_feature_selection(
    name: &str,
    features: Option<Vec<String>>,
    default_features: Option<bool>,
) -> Result<(Vec<String>, bool), ManifestError> {
    let features_vec = features.unwrap_or_default();
    if features_vec.iter().any(String::is_empty) {
        return Err(ManifestError::EmptyDependencyFeatureName {
            name: name.to_owned(),
        });
    }
    Ok((features_vec, default_features.unwrap_or(true)))
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
    // `system = true`.  The order matches the user-visible field
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
