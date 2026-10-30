use std::collections::{BTreeMap, BTreeSet};

use cabin_core::{DEFAULT_FEATURE_KEY, FeatureEntry, PackageName, SourceLanguage};
use cabin_index::{PackageIndex, VersionMetadata};
use pubgrub::SelectedDependencies;
use semver::Version;

use crate::input::ResolveInput;
use crate::provider::{Node, version_metadata};

/// Resolution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOutput {
    /// Resolved packages, including the root, sorted with the root first
    /// and the rest (index and local) alphabetical by name.
    pub packages: Vec<ResolvedPackage>,
    /// Packages whose newest in-range version was passed over because
    /// [`IncompatibleStandards::Fallback`](cabin_core::IncompatibleStandards::Fallback)
    /// found it standard-incompatible with the workspace.  Empty under
    /// `Allow`, and whenever selection matched the pure-semver newest
    /// (ordinary semver hold-backs are never reported).  Sorted by
    /// package name.
    pub held_back: Vec<HeldBack>,
}

/// A standard-compatibility note about a resolved package.  Three
/// shapes, distinguished by `newest` and `blocked_by`:
/// - a newer version was passed over because it is **incompatible**
///   (`newest` = `Some`, `blocked_by` non-empty);
/// - a newer version was passed over only because it is **undeclared**
///   and preference ranks a declared-compatible version above it
///   (`newest` = `Some`, `blocked_by` empty);
/// - the selected version is **itself incompatible** because nothing in
///   range satisfies the consumer (`newest` = `None`, the rule-2 case of
///   `preference-mode.md` section 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldBack {
    pub name: PackageName,
    /// The version preference mode selected.
    pub selected: Version,
    /// The newer in-range version passed over, or `None` when the
    /// selected version is the newest and is itself incompatible.
    pub newest: Option<Version>,
    /// The blocking interface requirement(s), in a fixed C-before-C++
    /// order for deterministic output - of `newest` when it is
    /// incompatible, of `selected` in the rule-2 case, and empty when
    /// the newer version was passed over only for being undeclared.
    pub blocked_by: Vec<BlockedRequirement>,
}

impl HeldBack {
    /// The update-facing one-line message, e.g.
    /// `foo v1.4.0 (available: v2.3.0, requires interface c++20)`;
    /// `foo v1.4.0 (available: v2.3.0, preferred as declared-compatible
    /// over the undeclared newer version)`; or
    /// `foo v2.0.0 (requires interface c++20; no compatible version in
    /// range)` when the selected version is itself incompatible.
    #[must_use]
    pub fn message(&self) -> String {
        let clauses = self
            .blocked_by
            .iter()
            .map(BlockedRequirement::clause)
            .collect::<Vec<_>>()
            .join("; ");
        match (&self.newest, self.blocked_by.is_empty()) {
            (Some(newest), false) => format!(
                "{} v{} (available: v{newest}, {clauses})",
                self.name.as_str(),
                self.selected,
            ),
            (Some(newest), true) => format!(
                "{} v{} (available: v{newest}, preferred as declared-compatible over the undeclared newer version)",
                self.name.as_str(),
                self.selected,
            ),
            (None, _) => format!(
                "{} v{} ({clauses}; no compatible version in range)",
                self.name.as_str(),
                self.selected,
            ),
        }
    }
}

/// One interface requirement of a held-back version that the
/// workspace consumer fails to satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedRequirement {
    /// The consumer language whose requirement is unmet.
    pub language: SourceLanguage,
    /// The accepted consumer range, rendered (`c++20 or newer`,
    /// `c++11..c++17`), or `None` when the version accepts no
    /// consumer of the language at all - a declared `"none"`, the
    /// strict cross-language default, or an empty intersection of
    /// its rows' ranges.
    pub accepted: Option<String>,
}

impl BlockedRequirement {
    /// The requirement clause used inside [`HeldBack::message`].
    #[must_use]
    fn clause(&self) -> String {
        match &self.accepted {
            Some(range) => format!("requires interface {range}"),
            None => format!("not consumable from {}", self.language_str()),
        }
    }

    fn language_str(&self) -> &'static str {
        match self.language {
            SourceLanguage::C => "c",
            SourceLanguage::Cxx => "c++",
        }
    }
}

/// One package selected by the resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub name: PackageName,
    pub version: semver::Version,
    pub source: ResolvedSource,
    /// Features enabled on this package, `default` expanded to the
    /// features it lists.
    pub features: BTreeSet<String>,
    /// Names of this package's active dependency edges under
    /// `features`: every non-optional dependency, whatever its
    /// `cfg(...)` condition, plus the optional ones an enabled feature
    /// turns on.  For the root,
    /// every root dependency and feature-request name.
    pub dependencies: BTreeSet<PackageName>,
}

/// Where a [`ResolvedPackage`] came from.
///
/// Deliberately not serde-serializable: every output path renders it
/// through [`ResolvedSource::as_str`], which owns the stable `"root"` /
/// `"index"` / `"local"` spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSource {
    /// The root package being resolved for.
    Root,
    /// Selected from the package index.
    Index,
    /// Supplied by the workspace
    /// ([`ResolveInput::local_packages`]).
    Local,
}

impl ResolvedSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ResolvedSource::Root => "root",
            ResolvedSource::Index => "index",
            ResolvedSource::Local => "local",
        }
    }
}

/// Assemble the public [`ResolveOutput`] from `PubGrub`'s
/// [`SelectedDependencies`] map.
///
/// Package nodes become [`ResolvedPackage`]s and feature nodes fold
/// into their package's `features`.  The output keeps the root first
/// and sorts the rest by name.  `PubGrub` types stay confined to this
/// boundary - callers see only Cabin-owned [`ResolvedPackage`] values.
pub(crate) fn selected_dependencies_to_output(
    input: &ResolveInput,
    index: &PackageIndex,
    solution: SelectedDependencies<Node, Version>,
) -> ResolveOutput {
    let mut selected: BTreeMap<PackageName, Version> = BTreeMap::new();
    let mut features: BTreeMap<PackageName, BTreeSet<String>> = BTreeMap::new();
    for (node, version) in solution {
        match node {
            Node::Root(_) => {}
            Node::Package(name) => {
                selected.insert(name, version);
            }
            Node::Feature(name, feature) => {
                if feature != DEFAULT_FEATURE_KEY {
                    features.entry(name).or_default().insert(feature);
                }
            }
        }
    }

    let mut packages = Vec::with_capacity(selected.len() + 1);
    packages.push(ResolvedPackage {
        name: input.root_name.clone(),
        version: input.root_version.clone(),
        source: ResolvedSource::Root,
        features: BTreeSet::new(),
        dependencies: input
            .root_dependencies
            .keys()
            .chain(input.root_features.keys())
            .cloned()
            .collect(),
    });
    for (name, version) in selected {
        let features = features.remove(&name).unwrap_or_default();
        let dependencies = version_metadata(input, index, &name, &version)
            .map(|meta| active_dependencies(meta, &features))
            .unwrap_or_default();
        let source = if input.local_packages.contains_key(&name) {
            ResolvedSource::Local
        } else {
            ResolvedSource::Index
        };
        packages.push(ResolvedPackage {
            name,
            version,
            source,
            features,
            dependencies,
        });
    }
    ResolveOutput {
        packages,
        held_back: Vec::new(),
    }
}

/// The dependency edges of `meta` that are active once `features`
/// are enabled - the same edges the provider required.
fn active_dependencies(
    meta: &VersionMetadata,
    features: &BTreeSet<String>,
) -> BTreeSet<PackageName> {
    let enabled: BTreeSet<String> = features
        .iter()
        .filter_map(|feature| meta.features.features.get(feature))
        .flatten()
        .filter_map(|raw| match FeatureEntry::parse(raw) {
            Ok(FeatureEntry::OptionalDep(dep) | FeatureEntry::DepFeature { dep, .. }) => Some(dep),
            _ => None,
        })
        .collect();
    meta.dependencies
        .iter()
        .filter(|(name, dep)| !dep.optional || enabled.contains(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect()
}
