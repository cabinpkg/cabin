//! Cabin feature resolver.
//!
//! Given a [`PackageGraph`], a set of
//! selected root packages, and a [`RootFeatureRequest`], computes
//! the *additive* closure of:
//!
//! - which features are enabled on each reachable package;
//! - which optional dependencies are enabled on each reachable
//!   package;
//! - which features each dependency edge requests on the
//!   depended-on package (defaults requested by default, plus
//!   per-edge `features = [...]` requests, plus
//!   `<dep>/<feature>` requests from `[features]`).
//!
//! Resolution is deterministic (sorted iteration, fixed-point
//! worklist) and never touches the network. It only operates on
//! the typed package graph that `cabin-workspace` already loaded.
//!
//! Feature entry syntax is validated generically by
//! `cabin-core`'s [`FeatureEntry::parse`]; characters and
//! separators outside the documented alphabet are rejected with
//! a uniform error.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::default_trait_access
)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cabin_core::{
    DEFAULT_FEATURE_KEY, DependencyKind, FeatureEntry, InvalidFeatureEntryKind, TargetPlatform,
};
use cabin_workspace::{PackageGraph, WorkspacePackage};
use thiserror::Error;

/// What the user (typically through CLI flags) is asking for on
/// each *selected root* package. Non-root packages always inherit
/// requests through dependency edges, never directly from this
/// struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootFeatureRequest {
    /// Whether the root packages' `default` feature should be
    /// enabled. `--no-default-features` flips this to `false`.
    pub include_defaults: bool,
    /// Explicitly requested feature names from `--features` /
    /// `--all-features`.
    ///
    /// If `all_features` is `true`, every declared feature is
    /// enabled in addition to `default` (when included). If
    /// `all_features` is `false`, only the names in
    /// `explicit_features` are enabled.
    pub all_features: bool,
    pub explicit_features: BTreeSet<String>,
}

impl Default for RootFeatureRequest {
    fn default() -> Self {
        Self {
            include_defaults: true,
            all_features: false,
            explicit_features: BTreeSet::new(),
        }
    }
}

impl From<&cabin_core::SelectionRequest> for RootFeatureRequest {
    /// Map a CLI [`cabin_core::SelectionRequest`] into a root feature
    /// request, flipping the `no_default_features` polarity into
    /// `include_defaults`. Keeping the conversion next to the type
    /// removes the hand-written converter the CLI used to carry.
    fn from(request: &cabin_core::SelectionRequest) -> Self {
        Self {
            include_defaults: !request.no_default_features,
            all_features: request.all_features,
            explicit_features: request.features.clone(),
        }
    }
}

/// Per-package feature resolution outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedPackageFeatures {
    /// Features enabled on this package (always includes
    /// `default` when it was requested by some path).
    pub enabled_features: BTreeSet<String>,
    /// Names of optional dependencies declared on this package
    /// that are now enabled.
    pub enabled_optional_deps: BTreeSet<String>,
}

/// Whole-graph resolution result keyed by package index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureResolution {
    pub per_package: BTreeMap<usize, ResolvedPackageFeatures>,
}

impl FeatureResolution {
    /// Lookup helper. Returns an empty resolution for packages
    /// outside the resolution scope so callers can iterate any
    /// graph index uniformly.
    pub fn for_package(&self, idx: usize) -> std::borrow::Cow<'_, ResolvedPackageFeatures> {
        match self.per_package.get(&idx) {
            Some(r) => std::borrow::Cow::Borrowed(r),
            None => std::borrow::Cow::Owned(ResolvedPackageFeatures::default()),
        }
    }

    /// Whether the named optional dependency is enabled on the
    /// given package. Returns `false` if the package is outside
    /// the resolution scope.
    pub fn is_optional_dep_enabled(&self, package: usize, dep_name: &str) -> bool {
        self.per_package
            .get(&package)
            .is_some_and(|r| r.enabled_optional_deps.contains(dep_name))
    }
}

/// Errors produced by [`resolve_features`].
#[derive(Debug, Error)]
pub enum FeatureResolverError {
    #[error("unknown feature {feature:?} for package {package:?}")]
    UnknownRootFeature { package: String, feature: String },

    #[error("unknown feature {feature:?} for package {package:?} (requested by {referrer})")]
    UnknownFeature {
        package: String,
        feature: String,
        referrer: FeatureRequestSource,
    },

    #[error(
        "feature {feature:?} in package {package:?} references unknown dependency {dependency:?}"
    )]
    UnknownDependency {
        package: String,
        feature: String,
        dependency: String,
    },

    #[error(
        "feature {feature:?} in package {package:?} enables dependency {dependency:?}, but {dependency:?} is not optional"
    )]
    DepIsNotOptional {
        package: String,
        feature: String,
        dependency: String,
    },

    #[error(
        "dependency {dependency:?} of package {package:?} requests feature {feature:?}, but {dependency:?} does not declare that feature"
    )]
    DepFeatureRequestUnknown {
        package: String,
        dependency: String,
        feature: String,
    },

    #[error(
        "invalid feature entry {entry:?} in feature {feature:?} of package {package:?}: {message}"
    )]
    InvalidFeatureEntry {
        package: String,
        feature: String,
        entry: String,
        message: &'static str,
    },
}

/// Where a feature request originated from. Used in error
/// messages so the user can find the chain that asked for a
/// missing feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureRequestSource {
    Root,
    LocalImplication { from_feature: String },
    DependencyEdge { from_package: String },
}

impl std::fmt::Display for FeatureRequestSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeatureRequestSource::Root => f.write_str("root request"),
            FeatureRequestSource::LocalImplication { from_feature } => {
                write!(f, "feature {from_feature:?}")
            }
            FeatureRequestSource::DependencyEdge { from_package } => {
                write!(f, "dependency edge from package {from_package:?}")
            }
        }
    }
}

/// Resolve feature requests across the package graph.
///
/// `selected_roots` are the package indices that receive the
/// `request`. Their reachable closure (over normal edges plus
/// enabled optional edges) is then walked iteratively until no
/// new features or optional dependencies become enabled.
///
/// `platform` is used to filter target-conditional dependency
/// declarations: `[target.'cfg(...)'.<kind>]` entries whose
/// condition does not evaluate to `true` against `platform` are
/// not visible to the resolver, so non-matching optional
/// dependencies cannot be enabled, and dependency feature
/// requests against non-matching declarations do not propagate.
///
/// The resolver is deterministic: feature names sort, dependency
/// names sort, and the worklist is drained FIFO so identical
/// inputs always yield identical outputs.
pub fn resolve_features(
    graph: &PackageGraph,
    selected_roots: &[usize],
    request: &RootFeatureRequest,
    platform: &TargetPlatform,
) -> Result<FeatureResolution, FeatureResolverError> {
    let mut state = ResolverState::new(graph, platform);

    // Seed: every selected root is *included* and receives the
    // root request. `IncludePackage` triggers expansion of its
    // non-optional package-dep edges (which carry `features` /
    // `default-features` requests), so this seed alone is enough
    // to set up the whole closure.
    for &root in selected_roots {
        state
            .work
            .push_back(WorkItem::IncludePackage { package: root });
        let pkg = &graph.packages[root];
        let package = &pkg.package;
        let declared = &package.features;

        if request.include_defaults {
            // Even when the package has no `default` group declared
            // we record `default` so consumers see a uniform shape.
            state.queue_feature(
                root,
                DEFAULT_FEATURE_KEY.to_owned(),
                FeatureRequestSource::Root,
            );
        }
        if request.all_features {
            for name in declared.features.keys() {
                state.queue_feature(root, name.clone(), FeatureRequestSource::Root);
            }
        }
        for name in &request.explicit_features {
            if !declared.features.contains_key(name) && name != DEFAULT_FEATURE_KEY {
                return Err(FeatureResolverError::UnknownRootFeature {
                    package: package.name.as_str().to_owned(),
                    feature: name.clone(),
                });
            }
            state.queue_feature(root, name.clone(), FeatureRequestSource::Root);
        }
    }

    // Drain the worklist to a fixed point. Each work item either
    // includes a package, enables a feature, enables an optional
    // dependency, or applies a per-edge feature request;
    // bookkeeping prevents revisits.
    while let Some(work) = state.pop() {
        match work {
            WorkItem::IncludePackage { package } => state.apply_include_package(graph, package),
            WorkItem::EnableFeature {
                package,
                feature,
                source,
            } => state.apply_feature(graph, package, &feature, source)?,
            WorkItem::EnableOptionalDep { package, dep_name } => {
                state.apply_optional_dep(graph, package, &dep_name);
            }
            WorkItem::DepFeatureRequest {
                from_package,
                dep_name,
                feature,
            } => state.apply_dep_feature_request(graph, from_package, &dep_name, &feature)?,
        }
    }

    Ok(state.finalize())
}

#[derive(Debug)]
struct ResolverState {
    per_package: BTreeMap<usize, ResolvedPackageFeatures>,
    /// Packages whose non-optional resolvable-kind edges have
    /// been expanded. The same package never produces those
    /// requests twice, regardless of how many features get
    /// enabled on it.
    included: BTreeSet<usize>,
    /// Tracks dependency edges already expanded (per declared
    /// kind) so we do not re-emit `default-features` /
    /// `features = [...]` requests on every fixed-point
    /// iteration.
    edges_expanded: BTreeSet<(usize, usize, DependencyKind)>,
    work: VecDeque<WorkItem>,
    /// Evaluation context for `[target.'cfg(...)']` filtering.
    platform: TargetPlatform,
}

#[derive(Debug)]
enum WorkItem {
    /// Mark a package as included in resolution and expand its
    /// non-optional package-dep edges (queuing `default-features`
    /// and per-edge feature requests onto each target).
    IncludePackage {
        package: usize,
    },
    EnableFeature {
        package: usize,
        feature: String,
        source: FeatureRequestSource,
    },
    EnableOptionalDep {
        package: usize,
        dep_name: String,
    },
    DepFeatureRequest {
        from_package: usize,
        dep_name: String,
        feature: String,
    },
}

impl ResolverState {
    fn new(graph: &PackageGraph, platform: &TargetPlatform) -> Self {
        let per_package: BTreeMap<usize, ResolvedPackageFeatures> = (0..graph.packages.len())
            .map(|i| (i, ResolvedPackageFeatures::default()))
            .collect();
        Self {
            per_package,
            included: BTreeSet::new(),
            edges_expanded: BTreeSet::new(),
            work: VecDeque::new(),
            platform: platform.clone(),
        }
    }

    fn queue_feature(&mut self, package: usize, feature: String, source: FeatureRequestSource) {
        self.work.push_back(WorkItem::EnableFeature {
            package,
            feature,
            source,
        });
    }

    fn pop(&mut self) -> Option<WorkItem> {
        self.work.pop_front()
    }

    fn apply_feature(
        &mut self,
        graph: &PackageGraph,
        package: usize,
        feature: &str,
        source: FeatureRequestSource,
    ) -> Result<(), FeatureResolverError> {
        let pkg = &graph.packages[package];
        let manifest = &pkg.package;
        let features = &manifest.features;

        // The reserved `default` key expands to the package's
        // declared default features.
        if feature == DEFAULT_FEATURE_KEY {
            for name in &features.default {
                self.queue_feature(
                    package,
                    name.clone(),
                    FeatureRequestSource::LocalImplication {
                        from_feature: DEFAULT_FEATURE_KEY.to_owned(),
                    },
                );
            }
            // Record so consumers can see that defaults flowed through.
            self.per_package
                .entry(package)
                .or_default()
                .enabled_features
                .insert(DEFAULT_FEATURE_KEY.to_owned());
            return Ok(());
        }

        if !features.features.contains_key(feature) {
            return Err(FeatureResolverError::UnknownFeature {
                package: manifest.name.as_str().to_owned(),
                feature: feature.to_owned(),
                referrer: source,
            });
        }
        let resolved = self.per_package.entry(package).or_default();
        if !resolved.enabled_features.insert(feature.to_owned()) {
            return Ok(());
        }

        // Walk the right-hand side of the now-enabled feature.
        let entries = features
            .features
            .get(feature)
            .expect("checked above")
            .clone();
        for raw in entries {
            let entry = FeatureEntry::parse(&raw)
                .map_err(|kind| map_invalid_entry(manifest.name.as_str(), feature, &raw, kind))?;
            match entry {
                FeatureEntry::Local(local) => {
                    self.queue_feature(
                        package,
                        local,
                        FeatureRequestSource::LocalImplication {
                            from_feature: feature.to_owned(),
                        },
                    );
                }
                FeatureEntry::OptionalDep(dep_name) => {
                    // Inactive on this host: a `dep:foo` entry
                    // becomes a silent no-op rather than enabling
                    // a dep that does not apply. Active
                    // declarations still must be optional.
                    if self.assert_optional_dep_active_or_skip(pkg, feature, &dep_name)? {
                        self.work
                            .push_back(WorkItem::EnableOptionalDep { package, dep_name });
                    }
                }
                FeatureEntry::DepFeature {
                    dep,
                    feature: requested_feature,
                } => {
                    let Some(dep_entry) = self.lookup_declared_dep(pkg, feature, &dep)? else {
                        // Dep is declared only under a
                        // non-matching `cfg(...)` on this host —
                        // no work to enqueue for this evaluation.
                        continue;
                    };
                    if dep_entry.optional {
                        self.work.push_back(WorkItem::EnableOptionalDep {
                            package,
                            dep_name: dep.clone(),
                        });
                    }
                    self.work.push_back(WorkItem::DepFeatureRequest {
                        from_package: package,
                        dep_name: dep,
                        feature: requested_feature,
                    });
                }
            }
        }
        Ok(())
    }

    fn apply_include_package(&mut self, graph: &PackageGraph, package: usize) {
        if !self.included.insert(package) {
            return;
        }
        let pkg = &graph.packages[package];
        // Expand every non-optional resolvable-kind edge so the
        // target package receives the per-edge `default-features`
        // / `features = [...]` requests (and inherits its own
        // included status through the queued `IncludePackage`).
        // Conditional declarations whose target condition does
        // not match the evaluation platform are skipped here so
        // their feature requests do not propagate.
        for declared in &pkg.package.dependencies {
            if !declared.kind.is_resolved_by_default() {
                continue;
            }
            if declared.optional {
                continue;
            }
            if !declared.matches_platform(&self.platform) {
                continue;
            }
            self.expand_edge_for_dep(graph, package, declared.name.as_str());
        }
    }

    fn apply_optional_dep(&mut self, graph: &PackageGraph, package: usize, dep_name: &str) {
        let resolved = self.per_package.entry(package).or_default();
        if !resolved.enabled_optional_deps.insert(dep_name.to_owned()) {
            return;
        }
        // Now expand the corresponding edge (the dep is in the
        // graph; we just gated it). The edge_expanded set
        // prevents re-emit if the edge expands to the same target.
        self.expand_edge_for_dep(graph, package, dep_name);
    }

    fn apply_dep_feature_request(
        &mut self,
        graph: &PackageGraph,
        from_package: usize,
        dep_name: &str,
        feature: &str,
    ) -> Result<(), FeatureResolverError> {
        // Only propagate when the dep is *included*: non-optional
        // deps of a resolvable kind always include; optional deps
        // include only after `apply_optional_dep` ran.
        let pkg = &graph.packages[from_package];
        let Some(dep_entry) = self.lookup_declared_dep(pkg, "<dep-feature-request>", dep_name)?
        else {
            // Inactive on this host — nothing to propagate.
            return Ok(());
        };
        if dep_entry.optional
            && !self
                .per_package
                .get(&from_package)
                .is_some_and(|r| r.enabled_optional_deps.contains(dep_name))
        {
            // Optional dep not enabled: defer (not an error).
            // The request will be re-emitted when the dep is later
            // enabled.
            return Ok(());
        }
        // Resolve the target index in the package graph.
        let Some(edge) = pkg.deps.iter().find(|e| {
            graph.packages[e.index].package.name.as_str() == dep_name && e.kind == dep_entry.kind
        }) else {
            // Dep declared but not in the graph (registry not
            // materialized, or path-dep skipped). Silently skip;
            // the resolver layer surfaces unresolved registry
            // dependencies on its own.
            return Ok(());
        };
        let target_idx = edge.index;
        // Validate that the requested feature exists on the dep.
        let target_pkg = &graph.packages[target_idx];
        if feature != DEFAULT_FEATURE_KEY
            && !target_pkg.package.features.features.contains_key(feature)
        {
            return Err(FeatureResolverError::DepFeatureRequestUnknown {
                package: pkg.package.name.as_str().to_owned(),
                dependency: dep_name.to_owned(),
                feature: feature.to_owned(),
            });
        }
        self.queue_feature(
            target_idx,
            feature.to_owned(),
            FeatureRequestSource::DependencyEdge {
                from_package: pkg.package.name.as_str().to_owned(),
            },
        );
        Ok(())
    }

    /// Once a dependency edge is *included* (non-optional, or
    /// just-enabled optional), apply its `default-features` and
    /// per-edge `features = [...]` requests onto the target
    /// package, and mark the target itself for inclusion (so its
    /// own non-optional edges are expanded too). Idempotent via
    /// `edges_expanded`.
    fn expand_edge_for_dep(&mut self, graph: &PackageGraph, from_package: usize, dep_name: &str) {
        let pkg = &graph.packages[from_package];
        for declared in &pkg.package.dependencies {
            if declared.name.as_str() != dep_name {
                continue;
            }
            if !declared.kind.is_resolved_by_default() {
                continue;
            }
            if !declared.matches_platform(&self.platform) {
                continue;
            }
            // Find the matching graph edge by `(name, kind)`.
            let Some(edge) = pkg.deps.iter().find(|e| {
                graph.packages[e.index].package.name.as_str() == dep_name && e.kind == declared.kind
            }) else {
                continue;
            };
            if !self
                .edges_expanded
                .insert((from_package, edge.index, declared.kind))
            {
                continue;
            }
            // The target package is now part of the resolution.
            self.work.push_back(WorkItem::IncludePackage {
                package: edge.index,
            });
            if declared.default_features {
                self.queue_feature(
                    edge.index,
                    DEFAULT_FEATURE_KEY.to_owned(),
                    FeatureRequestSource::DependencyEdge {
                        from_package: pkg.package.name.as_str().to_owned(),
                    },
                );
            }
            for f in &declared.features {
                self.work.push_back(WorkItem::DepFeatureRequest {
                    from_package,
                    dep_name: dep_name.to_owned(),
                    feature: f.clone(),
                });
            }
        }
    }

    /// Look up a dependency by name. Returns:
    ///
    /// - `Ok(Some(dep))` for an active declaration of a
    ///   resolvable kind;
    /// - `Ok(None)` when every declaration matching `dep_name`
    ///   is gated by a non-matching `cfg(...)` on this host —
    ///   feature entries that reference such a dep become a
    ///   no-op for this evaluation, mirroring Cargo's behavior
    ///   for inactive target-conditional optional deps;
    /// - `Err(UnknownDependency)` when no declaration of any
    ///   resolvable kind names `dep_name`.
    fn lookup_declared_dep<'a>(
        &self,
        pkg: &'a WorkspacePackage,
        referring_feature: &str,
        dep_name: &str,
    ) -> Result<Option<&'a cabin_core::Dependency>, FeatureResolverError> {
        let mut declared_anywhere = false;
        for dep in &pkg.package.dependencies {
            if dep.name.as_str() != dep_name || !dep.kind.is_resolved_by_default() {
                continue;
            }
            declared_anywhere = true;
            if dep.matches_platform(&self.platform) {
                return Ok(Some(dep));
            }
        }
        if declared_anywhere {
            return Ok(None);
        }
        Err(FeatureResolverError::UnknownDependency {
            package: pkg.package.name.as_str().to_owned(),
            feature: referring_feature.to_owned(),
            dependency: dep_name.to_owned(),
        })
    }

    /// Returns `Ok(true)` when the dep is active for the host
    /// platform and is optional (the caller should enable it),
    /// `Ok(false)` when it is declared only under non-matching
    /// `cfg(...)` (the caller should skip), or
    /// `Err(DepIsNotOptional)` when the dep is active but not
    /// declared with `optional = true`.
    fn assert_optional_dep_active_or_skip(
        &self,
        pkg: &WorkspacePackage,
        referring_feature: &str,
        dep_name: &str,
    ) -> Result<bool, FeatureResolverError> {
        let Some(dep) = self.lookup_declared_dep(pkg, referring_feature, dep_name)? else {
            return Ok(false);
        };
        if !dep.optional {
            return Err(FeatureResolverError::DepIsNotOptional {
                package: pkg.package.name.as_str().to_owned(),
                feature: referring_feature.to_owned(),
                dependency: dep_name.to_owned(),
            });
        }
        Ok(true)
    }

    fn finalize(self) -> FeatureResolution {
        FeatureResolution {
            per_package: self.per_package,
        }
    }
}

fn map_invalid_entry(
    package: &str,
    feature: &str,
    raw: &str,
    kind: InvalidFeatureEntryKind,
) -> FeatureResolverError {
    FeatureResolverError::InvalidFeatureEntry {
        package: package.to_owned(),
        feature: feature.to_owned(),
        entry: raw.to_owned(),
        message: kind.message(),
    }
}

#[cfg(test)]
mod tests;
