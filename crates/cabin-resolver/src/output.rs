use cabin_core::PackageName;
use serde::Serialize;

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
