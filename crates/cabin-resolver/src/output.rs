use cabin_core::PackageName;
use pubgrub::SelectedDependencies;
use semver::Version;
use serde::Serialize;

use crate::input::ResolveInput;

/// Resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOutput {
    /// Resolved packages, including the root, sorted with the root first
    /// and registry packages alphabetical by name.
    pub packages: Vec<ResolvedPackage>,
}

/// One package selected by the resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub name: PackageName,
    pub version: semver::Version,
    pub source: ResolvedSource,
}

/// Where a [`ResolvedPackage`] came from.
///
/// Kept as a closed enum so future steps can extend it (`Path`, `Git`,
/// etc.) without breaking the public API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedSource {
    /// The root package being resolved for.
    Root,
    /// Selected from the local JSON package index.
    Index,
}

impl ResolvedSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ResolvedSource::Root => "root",
            ResolvedSource::Index => "index",
        }
    }
}

/// Assemble the public [`ResolveOutput`] from `PubGrub`'s
/// [`SelectedDependencies`] map.
///
/// The output keeps the root package first and sorts the
/// remaining registry packages alphabetically by name (with
/// version as a secondary key for the deterministic-ordering
/// invariant). `PubGrub` types stay confined to this
/// boundary — callers see only Cabin-owned [`ResolvedPackage`]
/// values.
pub(crate) fn selected_dependencies_to_output(
    input: &ResolveInput,
    solution: SelectedDependencies<PackageName, Version>,
) -> ResolveOutput {
    let mut others: Vec<ResolvedPackage> = solution
        .into_iter()
        .filter(|(name, _)| name != &input.root_name)
        .map(|(name, version)| ResolvedPackage {
            name,
            version,
            source: ResolvedSource::Index,
        })
        .collect();
    others.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));

    let mut packages = Vec::with_capacity(others.len() + 1);
    packages.push(ResolvedPackage {
        name: input.root_name.clone(),
        version: input.root_version.clone(),
        source: ResolvedSource::Root,
    });
    packages.extend(others);
    ResolveOutput { packages }
}
