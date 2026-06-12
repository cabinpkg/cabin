use crate::error::WorkspaceError;
use cabin_core::{DependencyKind, DependencySource};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::canonicalize;

/// Expansion result for `[workspace.members]` /
/// `[workspace.exclude]`. `included` is a sorted, deduplicated list
/// of canonical manifest paths. `excluded` is the list of relative
/// paths (under `workspace_dir`) the loader removed from the
/// candidate set, surfaced for metadata.
pub(super) struct WorkspaceMembers {
    pub(super) included: Vec<PathBuf>,
    pub(super) excluded: Vec<PathBuf>,
}

pub(super) fn expand_workspace_members(
    workspace_dir: &Path,
    members: &[String],
    exclude: &[String],
) -> Result<WorkspaceMembers, WorkspaceError> {
    // Expand member patterns. Membership is tracked by canonicalized
    // directory path so two patterns matching the same dir collapse
    // to one entry.
    let mut included: BTreeSet<PathBuf> = BTreeSet::new();
    for pattern in members {
        let dirs = expand_member_pattern(workspace_dir, pattern)?;
        for dir in dirs {
            let manifest = dir.join("cabin.toml");
            if !manifest.is_file() {
                return Err(WorkspaceError::WorkspaceMemberMissing {
                    pattern: pattern.clone(),
                    root: workspace_dir.to_path_buf(),
                });
            }
            let canonical_dir = canonicalize(&dir)?;
            included.insert(canonical_dir);
        }
    }

    // Expand exclude patterns. Globs are best-effort: an exclude
    // pattern need not match any directory that contains a cabin.toml
    // (a partial match such as `third_party/*` covering some
    // subdirectories without manifests is fine), but the pattern as a
    // whole must hit at least one entry already in the member set so
    // typos surface.
    let mut excluded: BTreeSet<PathBuf> = BTreeSet::new();
    let canonical_root = canonicalize(workspace_dir)?;
    for pattern in exclude {
        if pattern.is_empty() {
            return Err(WorkspaceError::UnsupportedWorkspacePattern {
                pattern: pattern.clone(),
            });
        }
        let dirs = expand_exclude_pattern(workspace_dir, pattern)?;
        let mut hit_any = false;
        for dir in dirs {
            // We only canonicalize existing dirs; missing exclude
            // dirs collapse to no-op without erroring (the pattern
            // itself may have legitimately hit non-package
            // directories).
            if !dir.is_dir() {
                continue;
            }
            let Ok(canonical_dir) = canonicalize(&dir) else {
                continue;
            };
            if included.remove(&canonical_dir) {
                hit_any = true;
                if let Ok(rel) = canonical_dir.strip_prefix(&canonical_root) {
                    excluded.insert(rel.to_path_buf());
                } else {
                    excluded.insert(canonical_dir.clone());
                }
            }
        }
        if !hit_any {
            return Err(WorkspaceError::UnusedExcludePattern {
                pattern: pattern.clone(),
                root: workspace_dir.to_path_buf(),
            });
        }
    }

    // Convert the surviving directories to canonical manifest paths.
    let mut out: Vec<PathBuf> = Vec::with_capacity(included.len());
    for dir in &included {
        let manifest = dir.join("cabin.toml");
        out.push(canonicalize(&manifest)?);
    }
    out.sort();
    let excluded_paths: Vec<PathBuf> = excluded.into_iter().collect();
    Ok(WorkspaceMembers {
        included: out,
        excluded: excluded_paths,
    })
}

/// Resolve every `DependencySource::Workspace` entry on
/// `package` by looking it up in the workspace table that matches
/// each entry's [`DependencyKind`]. Returns a `Package` whose
/// dependencies are entirely `Path` or `Version`. References that
/// have no matching workspace entry are surfaced as a clear
/// kind-aware error.
pub(super) fn resolve_workspace_dependencies(
    mut package: cabin_core::Package,
    workspace_deps: &BTreeMap<DependencyKind, BTreeMap<String, DependencySource>>,
) -> Result<cabin_core::Package, WorkspaceError> {
    for dep in &mut package.dependencies {
        if !matches!(dep.source, DependencySource::Workspace) {
            continue;
        }
        let table = workspace_deps.get(&dep.kind);
        let resolved = table
            .and_then(|t| t.get(dep.name.as_str()))
            .ok_or_else(|| WorkspaceError::UnresolvedWorkspaceDependency {
                dep_name: dep.name.as_str().to_owned(),
                parent: package.name.as_str().to_owned(),
                kind: dep.kind,
            })?;
        dep.source = resolved.clone();
    }
    Ok(package)
}

/// Resolve every `{ workspace = true }` standard-field marker on
/// `package` against the workspace root's `[workspace]` defaults.
/// Mirrors [`resolve_workspace_dependencies`]: runs on every
/// locally loaded manifest before any other consumer sees it, and
/// surfaces a clear field-naming error when the root declares no
/// matching value (including the no-workspace standalone case,
/// where the defaults are simply all `None`).
pub(super) fn resolve_workspace_standards(
    mut package: cabin_core::Package,
    defaults: cabin_core::WorkspaceStandardDefaults,
    manifest_path: &Path,
) -> Result<cabin_core::Package, WorkspaceError> {
    fn resolve_field<S: Copy>(
        field: &mut Option<cabin_core::StandardDeclaration<S>>,
        default: Option<S>,
        field_name: &'static str,
        package: &str,
        manifest_path: &Path,
    ) -> Result<(), WorkspaceError> {
        if !matches!(field, Some(cabin_core::StandardDeclaration::Workspace)) {
            return Ok(());
        }
        let value = default.ok_or_else(|| WorkspaceError::UnresolvedWorkspaceStandard {
            package: package.to_owned(),
            field: field_name,
            path: manifest_path.to_path_buf(),
        })?;
        *field = Some(cabin_core::StandardDeclaration::Inherited(value));
        Ok(())
    }
    resolve_field(
        &mut package.language.c_standard,
        defaults.c_standard,
        "c-standard",
        package.name.as_str(),
        manifest_path,
    )?;
    resolve_field(
        &mut package.language.cxx_standard,
        defaults.cxx_standard,
        "cxx-standard",
        package.name.as_str(),
        manifest_path,
    )?;
    resolve_field(
        &mut package.language.interface_c_standard,
        defaults.interface_c_standard,
        "interface-c-standard",
        package.name.as_str(),
        manifest_path,
    )?;
    resolve_field(
        &mut package.language.interface_cxx_standard,
        defaults.interface_cxx_standard,
        "interface-cxx-standard",
        package.name.as_str(),
        manifest_path,
    )?;
    Ok(package)
}

/// Parse a `[workspace.<kind>-dependencies]` value into a
/// `DependencySource`. Uses the existing manifest-side parser so
/// requirement-string handling stays a single source of truth.
pub(super) fn parse_workspace_dep_source(
    name: &str,
    req: &str,
) -> Result<DependencySource, WorkspaceError> {
    // Wrap the raw requirement in a tiny manifest to reuse the
    // existing dependency parser. We round-trip through the
    // manifest crate so error messages mention the dependency name
    // and the failing requirement consistently.
    let manifest = format!(
        "[package]\nname = \"__workspace_root__\"\nversion = \"0.0.0\"\n[dependencies]\n{name} = \"{}\"\n",
        req.replace('"', "\\\""),
    );
    let parsed = cabin_manifest::parse_manifest_str(&manifest).map_err(|source| {
        WorkspaceError::InvalidWorkspaceDependency {
            name: name.to_owned(),
            source: Box::new(source),
        }
    })?;
    let package = parsed
        .package
        .expect("inline manifest always has [package]");
    let dep = package
        .dependencies
        .into_iter()
        .next()
        .expect("inline manifest declared exactly one dependency");
    Ok(dep.source)
}

/// Reject workspace patterns that escape the workspace
/// root or that use absolute paths. Applied to every `members`,
/// `exclude`, and `default-members` entry so an unsafe pattern
/// fails fast with a clear error before any filesystem walk.
pub(super) fn validate_workspace_pattern(
    field: &'static str,
    pattern: &str,
) -> Result<(), WorkspaceError> {
    if pattern.is_empty() {
        return Err(WorkspaceError::UnsupportedWorkspacePattern {
            pattern: pattern.to_owned(),
        });
    }
    let p = std::path::Path::new(pattern);
    if p.is_absolute() {
        return Err(WorkspaceError::WorkspacePatternEscapesRoot {
            field,
            pattern: pattern.to_owned(),
        });
    }
    for component in p.components() {
        if matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        ) {
            return Err(WorkspaceError::WorkspacePatternEscapesRoot {
                field,
                pattern: pattern.to_owned(),
            });
        }
    }
    Ok(())
}

/// Resolve a `[workspace.members]` pattern to a list of directories
/// containing `cabin.toml`. The supported syntaxes are:
///
/// - exact relative path (`tools/hello`)
/// - single-`*` glob in the final component (`packages/*`)
pub(super) fn expand_member_pattern(
    workspace_dir: &Path,
    pattern: &str,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    validate_workspace_pattern("workspace.members", pattern)?;

    if !pattern.contains('*') {
        let dir = workspace_dir.join(pattern);
        return Ok(vec![dir]);
    }

    // Single trailing `/*` only.
    let Some(trimmed) = pattern.strip_suffix("/*") else {
        return Err(WorkspaceError::UnsupportedWorkspacePattern {
            pattern: pattern.to_owned(),
        });
    };
    if trimmed.contains('*') {
        return Err(WorkspaceError::UnsupportedWorkspacePattern {
            pattern: pattern.to_owned(),
        });
    }

    let prefix_dir = if trimmed.is_empty() {
        workspace_dir.to_path_buf()
    } else {
        workspace_dir.join(trimmed)
    };
    if !prefix_dir.is_dir() {
        return Err(WorkspaceError::WorkspaceMemberMissing {
            pattern: pattern.to_owned(),
            root: workspace_dir.to_path_buf(),
        });
    }

    let entries = std::fs::read_dir(&prefix_dir).map_err(|source| WorkspaceError::Io {
        path: prefix_dir.clone(),
        source,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| WorkspaceError::Io {
            path: prefix_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() && path.join("cabin.toml").is_file() {
            out.push(path);
        }
    }
    if out.is_empty() {
        return Err(WorkspaceError::WorkspaceMemberMissing {
            pattern: pattern.to_owned(),
            root: workspace_dir.to_path_buf(),
        });
    }
    out.sort();
    Ok(out)
}

/// Resolve a `[workspace.exclude]` pattern. Same grammar as
/// `expand_member_pattern`, but more lenient about empty matches:
/// The pattern may legitimately match directories that do not
/// contain a `cabin.toml`. The caller validates that the overall
/// pattern hit at least one declared member.
pub(super) fn expand_exclude_pattern(
    workspace_dir: &Path,
    pattern: &str,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    validate_workspace_pattern("workspace.exclude", pattern)?;

    if !pattern.contains('*') {
        return Ok(vec![workspace_dir.join(pattern)]);
    }

    let Some(trimmed) = pattern.strip_suffix("/*") else {
        return Err(WorkspaceError::UnsupportedWorkspacePattern {
            pattern: pattern.to_owned(),
        });
    };
    if trimmed.contains('*') {
        return Err(WorkspaceError::UnsupportedWorkspacePattern {
            pattern: pattern.to_owned(),
        });
    }

    let prefix_dir = if trimmed.is_empty() {
        workspace_dir.to_path_buf()
    } else {
        workspace_dir.join(trimmed)
    };
    if !prefix_dir.is_dir() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&prefix_dir).map_err(|source| WorkspaceError::Io {
        path: prefix_dir.clone(),
        source,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| WorkspaceError::Io {
            path: prefix_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}
