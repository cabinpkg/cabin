//! What `[build-dependencies]` entries select: the targets the build
//! machine builds, and of those, what a package's build script takes.

use std::collections::{BTreeSet, HashMap};

use cabin_core::{Target, TargetKind, TargetName, TargetPlatform};
use cabin_workspace::{DependencyEdge, PackageGraph};

use super::{ManifestTargetSelector, TargetId, default_selection};
use crate::error::BuildError;

/// Select what the build machine builds for the active build edges,
/// each given with the index of its dependent: every target an edge
/// lists in `targets`, plus the default targets of each dependency
/// some edge reaches without `targets`.  `None` when no edge lists
/// `targets`, so the planner's default selection over the
/// dependencies applies unchanged.  `enabled_features` are the build
/// context's.
///
/// # Errors
///
/// [`BuildError::UnknownBuildDependencyTarget`],
/// [`BuildError::BuildDependencyTargetKind`], or
/// [`BuildError::BuildDependencyTargetRequiresFeatures`] for a listed
/// target the build machine cannot build.
#[allow(clippy::implicit_hasher)]
pub fn select_build_dependency_targets(
    graph: &PackageGraph,
    edges: &[(usize, &DependencyEdge)],
    enabled_features: &HashMap<usize, BTreeSet<String>>,
) -> Result<Option<Vec<ManifestTargetSelector>>, BuildError> {
    let mut defaulted = BTreeSet::new();
    let mut listed: BTreeSet<TargetId> = BTreeSet::new();
    for &(dependent, edge) in edges {
        if edge.targets.is_empty() {
            defaulted.insert(edge.index);
        }
        for name in &edge.targets {
            check_listed_target(graph, dependent, edge.index, name, enabled_features)?;
            listed.insert((edge.index, name.as_str().to_owned()));
        }
    }
    if listed.is_empty() {
        return Ok(None);
    }
    let defaulted: Vec<usize> = defaulted.into_iter().collect();
    let (defaults, _gated) =
        default_selection(graph, Some(defaulted.as_slice()), Some(enabled_features));
    listed.extend(defaults);
    Ok(Some(
        listed
            .into_iter()
            .map(|(idx, name)| ManifestTargetSelector {
                package: Some(graph.packages[idx].package.name.as_str().to_owned()),
                name,
            })
            .collect(),
    ))
}

/// What a package's build script takes from the package's
/// `[build-dependencies]`, each target by `graph.packages` index and
/// name, in the entries' order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BuildScriptInputs {
    /// The library and header-only targets the script links.
    pub libraries: Vec<(usize, String)>,
    /// The executables the script may run.
    pub tools: Vec<(usize, String)>,
}

/// What the build script of `package` takes from the package's
/// `[build-dependencies]` entries active on `platform`, the platform of
/// the context the script prepares.  An entry's `targets` name them.
/// Without `targets` they are among what the build machine builds for
/// the entry by default: the dependency's executables, and its library
/// when it has exactly one.  `enabled_optional_deps` names the
/// `optional = true` entries that context enables on `package`;
/// `enabled_features` are the build context's.
///
/// # Errors
///
/// What [`select_build_dependency_targets`] returns for a listed
/// target, and [`BuildError::AmbiguousBuildDependencyLibrary`] for an
/// entry without `targets` whose dependency builds several libraries.
#[allow(clippy::implicit_hasher)]
pub fn build_script_inputs(
    graph: &PackageGraph,
    package: usize,
    platform: &TargetPlatform,
    enabled_optional_deps: &BTreeSet<String>,
    enabled_features: &HashMap<usize, BTreeSet<String>>,
) -> Result<BuildScriptInputs, BuildError> {
    let empty = BTreeSet::new();
    let mut inputs = BuildScriptInputs::default();
    for edge in graph.build_edges(package, platform, enabled_optional_deps) {
        let dependency = &graph.packages[edge.index].package;
        let mut selected: Vec<&Target> = Vec::new();
        for name in &edge.targets {
            selected.push(check_listed_target(
                graph,
                package,
                edge.index,
                name,
                enabled_features,
            )?);
        }
        if edge.targets.is_empty() {
            let enabled = enabled_features.get(&edge.index).unwrap_or(&empty);
            let built = |target: &&Target| target.missing_required_features(enabled).is_empty();
            let (libraries, tools): (Vec<&Target>, Vec<&Target>) = dependency
                .targets
                .iter()
                .filter(|target| {
                    target.kind.is_library_like() || target.kind == TargetKind::Executable
                })
                .filter(built)
                .partition(|target| target.kind.is_library_like());
            if libraries.len() > 1 {
                return Err(BuildError::AmbiguousBuildDependencyLibrary {
                    dependency: dependency.name.as_str().to_owned(),
                    dependent: graph.packages[package].package.name.as_str().to_owned(),
                    candidates: libraries
                        .iter()
                        .map(|target| target.name.as_str().to_owned())
                        .collect(),
                });
            }
            selected.extend(libraries);
            selected.extend(tools);
        }
        for target in selected {
            let id = (edge.index, target.name.as_str().to_owned());
            let set = if target.kind.is_library_like() {
                &mut inputs.libraries
            } else {
                &mut inputs.tools
            };
            if !set.contains(&id) {
                set.push(id);
            }
        }
    }
    Ok(inputs)
}

fn check_listed_target<'a>(
    graph: &'a PackageGraph,
    dependent: usize,
    dep: usize,
    name: &TargetName,
    enabled_features: &HashMap<usize, BTreeSet<String>>,
) -> Result<&'a Target, BuildError> {
    let dependent = graph.packages[dependent].package.name.as_str().to_owned();
    let package = &graph.packages[dep].package;
    let dependency = package.name.as_str().to_owned();
    let target_name = name.as_str().to_owned();
    let Some(target) = package.targets.iter().find(|target| target.name == *name) else {
        return Err(BuildError::UnknownBuildDependencyTarget {
            dependency,
            dependent,
            target: target_name,
            candidates: package
                .targets
                .iter()
                .filter(|target| target.kind.is_default_buildable())
                .map(|target| target.name.as_str().to_owned())
                .collect(),
        });
    };
    if !target.kind.is_default_buildable() {
        return Err(BuildError::BuildDependencyTargetKind {
            dependency,
            dependent,
            target: target_name,
            kind: target.kind.to_string(),
        });
    }
    let empty = BTreeSet::new();
    let missing = target.missing_required_features(enabled_features.get(&dep).unwrap_or(&empty));
    if !missing.is_empty() {
        return Err(BuildError::BuildDependencyTargetRequiresFeatures {
            dependency,
            dependent,
            target: target_name,
            missing,
        });
    }
    Ok(target)
}
