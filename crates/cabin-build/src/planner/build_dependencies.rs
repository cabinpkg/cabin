//! The build machine's target selection for `[build-dependencies]`
//! entries that list `targets`.

use std::collections::{BTreeSet, HashMap};

use cabin_core::TargetName;
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

fn check_listed_target(
    graph: &PackageGraph,
    dependent: usize,
    dep: usize,
    name: &TargetName,
    enabled_features: &HashMap<usize, BTreeSet<String>>,
) -> Result<(), BuildError> {
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
    Ok(())
}
