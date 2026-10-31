//! The pre-resolution seed every versioned-dependency command
//! computes before it decides whether a registry is needed at all:
//! `cabin resolve` / `cabin update`, `cabin fetch`, `cabin vendor`,
//! and the `build` / `run` / `test` preamble.
//!
//! Given the loaded graph and the command's selection, the seed fixes
//! which local packages are included and with which features, the
//! `links` claims those packages declare, and the versioned
//! dependencies the selected closure needs from an index, with the
//! features it requests on them - patched names excluded (they ship
//! from a local working copy) and the referenced forks' own versioned
//! dependencies folded back in.  The
//! local-vs-local `links` check runs here, unconditionally, because
//! every command has a no-versioned-deps fast path that never reaches
//! resolution: a workspace with no registry dependencies must reject
//! duplicate claims exactly like a resolved graph.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use cabin_core::{DependencyKind, DependencySource, PackageName};
use cabin_index::{IndexEntry, IndexPackageDependency, VersionMetadata};
use cabin_resolver::{LinkDomain, LinksClaim};
use cabin_workspace::{
    ActivePatchSet, ClosureVersionedDeps, PackageGraph, PackageKind, PackageSelection,
    PatchedVersionedDeps, ResolvedSelection, collect_patched_versioned_deps,
};

/// What a command hands the seed, borrowed from its prepared
/// workspace.
pub(crate) struct SeedInputs<'a> {
    pub graph: &'a PackageGraph,
    pub selection: &'a PackageSelection,
    pub request: &'a cabin_core::SelectionRequest,
    /// Names the active patch set supplies locally.
    pub patched_names: &'a BTreeSet<String>,
    pub active_patches: &'a ActivePatchSet,
    /// Primary packages whose dev dependencies are active.
    pub dev_for: &'a BTreeSet<String>,
}

pub(crate) struct ResolutionSeed {
    pub resolved_selection: ResolvedSelection,
    pub features: cabin_feature::FeatureResolution,
    /// The build context the selection's build dependencies reach,
    /// resolved apart from `features`.
    pub build_features: cabin_feature::FeatureResolution,
    /// Declared claims of the included local packages, sorted and
    /// deduplicated - the `local_links` a resolution starts from
    /// before any fork claims join.
    pub local_links: Vec<LinksClaim>,
    /// The target and build link domains when the build context is
    /// not empty (see `cabin_resolver::ResolveInput::link_domains`).
    pub link_domains: Vec<LinkDomain>,
    /// Closure-collected versioned deps with patched names excluded,
    /// plus the patched names the walk reached on active edges (the
    /// patches a direct local reference activates).
    pub closure_deps: ClosureVersionedDeps,
    /// The versioned deps of the activated forks and the activated
    /// set itself, chained patches included.
    pub patched_deps: PatchedVersionedDeps,
}

impl ResolutionSeed {
    /// Compute the seed.  Fails on an invalid selection, a feature
    /// resolution error (for the selection or an activated fork
    /// island), conflicting version requirements inside the closure
    /// or the activated forks, or a local-vs-local `links` collision
    /// - all before any caller consults an index.
    pub(crate) fn compute(inputs: &SeedInputs<'_>) -> Result<Self> {
        let resolved_selection =
            cabin_workspace::resolve_package_selection(inputs.graph, inputs.selection)?;
        let features = super::compute_feature_resolution(
            inputs.graph,
            &resolved_selection,
            inputs.request,
            None,
            inputs.dev_for,
        )?;
        let build_features =
            cabin_feature::resolve_build_context_features(inputs.graph, &features, None, None)?;
        let local_links = local_links_claims(inputs.graph, &features);
        let build_local_links = local_links_claims(inputs.graph, &build_features);
        let closure_deps = collect_closure_versioned_deps_excluding_patches(
            inputs.graph,
            &resolved_selection,
            &[&features, &build_features],
            inputs.patched_names,
            inputs.dev_for,
        )?;
        let patched_deps = collect_patched_versioned_deps(
            inputs.active_patches,
            inputs.patched_names,
            &closure_deps.referenced_excluded,
        )?;
        // The full pre-resolution seed claims, per link domain: the
        // included local packages' plus the activated forks' - a
        // purely-local patched chain still links its forks, so their
        // claims cannot wait for a resolution that never runs.  A fork
        // neither context includes is reached through a registry
        // edge, whose domain only resolution decides, so its claims
        // join both.
        let included_names: BTreeSet<&PackageName> = features
            .included
            .union(&build_features.included)
            .map(|&idx| &inputs.graph.packages[idx].package.name)
            .collect();
        let fork_claims: Vec<LinksClaim> =
            activated_fork_claims(inputs.graph, inputs.active_patches, &patched_deps.activated)?
                .into_iter()
                .filter(|claim| !included_names.contains(&claim.package))
                .collect();
        for domain_links in [&local_links, &build_local_links] {
            let mut claims = domain_links.clone();
            claims.extend(fork_claims.iter().cloned());
            claims.sort();
            claims.dedup();
            enforce_local_links_uniqueness(&claims)?;
        }
        let mut build_roots = domain_roots(inputs.graph, &features, |_, kind| {
            kind == DependencyKind::Build
        });
        build_roots.extend(domain_roots(inputs.graph, &build_features, |_, kind| {
            kind != DependencyKind::Dev
        }));
        // A registry-only build dependency leaves the build context
        // without local packages, but it still links apart.
        let link_domains = if build_features.included.is_empty() && build_roots.is_empty() {
            Vec::new()
        } else {
            vec![
                LinkDomain {
                    roots: domain_roots(inputs.graph, &features, |package, kind| {
                        kind == DependencyKind::Normal
                            || (kind == DependencyKind::Dev && inputs.dev_for.contains(package))
                    }),
                    local_links: local_links.clone(),
                },
                LinkDomain {
                    roots: build_roots,
                    local_links: build_local_links,
                },
            ]
        };
        Ok(Self {
            resolved_selection,
            features,
            build_features,
            local_links,
            link_domains,
            closure_deps,
            patched_deps,
        })
    }

    /// Every package either context includes.
    pub(crate) fn included(&self) -> BTreeSet<usize> {
        self.features
            .included
            .union(&self.build_features.included)
            .copied()
            .collect()
    }

    /// Both contexts' feature requests on versioned dependencies:
    /// resolution picks one version per name for both.
    pub(crate) fn root_features(&self) -> BTreeMap<PackageName, BTreeSet<String>> {
        let mut requests = self.features.versioned_requests.clone();
        for (name, features) in &self.build_features.versioned_requests {
            requests
                .entry(name.clone())
                .or_default()
                .extend(features.iter().cloned());
        }
        requests
    }

    /// Whether anything must come from an index: the gate on every
    /// command's local-only fast path.
    pub(crate) fn has_versioned_deps(&self) -> bool {
        !self.patched_deps.deps.is_empty() || !self.closure_deps.deps.is_empty()
    }

    /// The resolver's root requirement set: the closure deps joined
    /// with the activated forks' deps.  Joined on demand so a
    /// requirement-join failure surfaces where the pipelines raise
    /// it, after their own preflight checks.
    pub(crate) fn root_deps(&self) -> Result<BTreeMap<PackageName, semver::VersionReq>> {
        let mut root_deps = self.closure_deps.deps.clone();
        merge_versioned_deps(&mut root_deps, self.patched_deps.deps.clone())?;
        Ok(root_deps)
    }
}

/// Declared `links` claims of the local packages a command's
/// resolution actually includes - the feature resolver's exact
/// reachable set, so a disabled optional path dependency never
/// contributes a claim it could not collide with.  Patched packages
/// are graph packages like any local, so an included patch's claims
/// participate; what never participates is the patched *name*'s
/// index metadata - stripped from root deps, and replaced for
/// transitive selections by the fork's
/// `cabin_resolver::ResolveInput::local_packages` entry.
///
/// [`ResolutionSeed::compute`] validates these claims against each
/// other with [`enforce_local_links_uniqueness`] *before* any
/// no-versioned-deps fast path, then hands the same claims to
/// `cabin_resolver::ResolveInput::local_links` so the resolver's
/// post-solve check sees local and registry claims together.
pub(crate) fn local_links_claims(
    graph: &PackageGraph,
    features: &cabin_feature::FeatureResolution,
) -> Vec<LinksClaim> {
    let mut claims = links_claims_of(graph, &features.included);
    claims.sort();
    claims.dedup();
    claims
}

/// The declared `links` claims of the given graph packages, in
/// index order (callers sort/dedup the combined seed themselves).
fn links_claims_of(graph: &PackageGraph, indices: &BTreeSet<usize>) -> Vec<LinksClaim> {
    let mut claims = Vec::new();
    for &idx in indices {
        let package = &graph.packages[idx].package;
        for target in &package.targets {
            if let Some(links) = &target.links {
                claims.push(LinksClaim {
                    package: package.name.clone(),
                    version: package.version.clone(),
                    target: target.name.as_str().to_owned(),
                    links: links.clone(),
                });
            }
        }
    }
    claims
}

/// Local-vs-local `links` uniqueness, run unconditionally before a
/// command's fast paths: a workspace with no versioned dependencies
/// must reject duplicate claims exactly like a resolved graph.
pub(crate) fn enforce_local_links_uniqueness(claims: &[LinksClaim]) -> Result<()> {
    cabin_resolver::detect_links_collision(claims).context("dependency resolution failed")?;
    Ok(())
}

/// The declared `links` claims of the activated patches' forks and
/// of everything the forks' manifests pull into the graph (path
/// dependencies) - the local packages the final
/// reload will link in place of, or alongside, the upstream
/// packages the activation suppressed.  Feature resolution never
/// included any of them (nothing selected reaches a
/// transitively-activated fork locally), so
/// [`cabin_feature::resolve_fork_island_features`] runs a dedicated
/// under-approximating pass over the fork islands - see it for why
/// only mandatory members claim here; the exact, edge-aware check
/// runs in dependency resolution, which selects the forks and their
/// path dependencies under the registry edges' real feature
/// requests.  Scoping to the pass's `included` set - not the
/// loader-stitch closure - matches [`local_links_claims`]: a
/// feature-disabled optional path dependency is loaded into the
/// graph but never linked, so its claims must not participate.
pub(crate) fn activated_fork_claims(
    graph: &PackageGraph,
    active_patches: &ActivePatchSet,
    activated: &BTreeSet<String>,
) -> Result<Vec<LinksClaim>> {
    let fork_roots = cabin_workspace::activated_fork_indices(graph, active_patches, activated);
    if fork_roots.is_empty() {
        return Ok(Vec::new());
    }
    let resolution = cabin_feature::resolve_fork_island_features(graph, &fork_roots, None)?;
    Ok(links_claims_of(graph, &resolution.included))
}

/// Names of the versioned dependencies that the local packages
/// `features` includes declare under a kind `in_domain` accepts: the
/// roots of one link domain.
fn domain_roots(
    graph: &PackageGraph,
    features: &cabin_feature::FeatureResolution,
    in_domain: impl Fn(&str, DependencyKind) -> bool,
) -> BTreeSet<PackageName> {
    let mut roots = BTreeSet::new();
    for &idx in &features.included {
        let pkg = &graph.packages[idx];
        if pkg.kind != PackageKind::Local {
            continue;
        }
        for dep in &pkg.package.dependencies {
            if in_domain(pkg.package.name.as_str(), dep.kind)
                && matches!(dep.source, DependencySource::Version(_))
                && (!dep.optional || features.is_optional_dep_enabled(idx, dep.name.as_str()))
            {
                roots.insert(dep.name.clone());
            }
        }
    }
    roots
}

/// Build the selection's closure once and adapt the contexts'
/// [`cabin_feature::FeatureResolution`]s into the
/// `Fn(usize, &str) -> bool` optional-dep filter the workspace
/// versioned-dep helper consumes.
///
/// The edge closure is intersected with the feature resolver's
/// `included` set: the raw closure follows optional path edges even
/// when disabled, and a package reachable only through a disabled
/// optional dependency must contribute nothing to resolution - its
/// registry deps would otherwise resolve (over-fetching) and their
/// index `links` claims would hard-fail graphs the build never
/// links.
fn closure_and_optional_filter<'a>(
    graph: &PackageGraph,
    selection: &ResolvedSelection,
    contexts: &'a [&'a cabin_feature::FeatureResolution],
) -> (BTreeSet<usize>, impl Fn(usize, &str) -> bool + 'a) {
    let closure: BTreeSet<usize> = selection
        .closure(graph)
        .into_iter()
        .filter(|idx| {
            contexts
                .iter()
                .any(|features| features.included.contains(idx))
        })
        .collect();
    (closure, move |idx, name| {
        contexts
            .iter()
            .any(|features| features.is_optional_dep_enabled(idx, name))
    })
}

/// Collect every versioned dependency reachable from `selection`
/// after dropping patched names.  Thin shim around the typed API
/// in `cabin-workspace`.
fn collect_closure_versioned_deps_excluding_patches(
    graph: &PackageGraph,
    selection: &ResolvedSelection,
    contexts: &[&cabin_feature::FeatureResolution],
    patched_names: &BTreeSet<String>,
    dev_for: &BTreeSet<String>,
) -> Result<ClosureVersionedDeps> {
    let (closure, is_optional_dep_enabled) =
        closure_and_optional_filter(graph, selection, contexts);
    cabin_workspace::collect_closure_versioned_deps_excluding_with_dev(
        graph,
        &closure,
        is_optional_dep_enabled,
        patched_names,
        dev_for,
    )
    .map_err(Into::into)
}

/// Every local package of `graph` as the single-version entry the
/// resolver offers for its name, so a registry edge onto a `[patch]`ed
/// name resolves to the fork, with the fork's own dependencies and
/// features.  A path dependency becomes an exact requirement on the
/// package it loads.  Dev dependencies stay out: their activation
/// never propagates past the selected packages, whose dev edges are
/// root dependencies already.
pub(crate) fn local_packages(graph: &PackageGraph) -> BTreeMap<PackageName, IndexEntry> {
    graph
        .packages
        .iter()
        .filter(|pkg| pkg.kind == PackageKind::Local)
        .map(|pkg| {
            let package = &pkg.package;
            let dependencies = package
                .dependencies
                .iter()
                .filter(|dep| dep.kind.is_resolved_by_default())
                .filter_map(|dep| {
                    let req = match &dep.source {
                        DependencySource::Version(req) => req.clone(),
                        // The loader loads every path dependency,
                        // whatever its condition.
                        DependencySource::Path(_) => pkg
                            .deps
                            .iter()
                            .map(|edge| &graph.packages[edge.index].package)
                            .find(|target| target.name == dep.name)
                            .map_or(semver::VersionReq::STAR, |target| {
                                exact_req(&target.version)
                            }),
                        DependencySource::Workspace => return None,
                    };
                    Some((
                        dep.name.clone(),
                        IndexPackageDependency {
                            req,
                            optional: dep.optional,
                            features: dep.features.clone(),
                            default_features: dep.default_features,
                            condition: dep.condition.clone(),
                        },
                    ))
                })
                .fold(BTreeMap::new(), merge_same_name_dependency);
            let links = package
                .targets
                .iter()
                .filter_map(|target| {
                    let links = target.links.clone()?;
                    Some((target.name.as_str().to_owned(), links))
                })
                .collect();
            let meta = VersionMetadata {
                dependencies,
                features: package.features.clone(),
                links,
                ..VersionMetadata::default()
            };
            (
                package.name.clone(),
                IndexEntry {
                    name: package.name.clone(),
                    versions: BTreeMap::from([(package.version.clone(), meta)]),
                },
            )
        })
        .collect()
}

/// One name declared as both a normal and a build dependency resolves
/// to one version that must satisfy both declarations.
fn merge_same_name_dependency(
    mut deps: BTreeMap<PackageName, IndexPackageDependency>,
    (name, dep): (PackageName, IndexPackageDependency),
) -> BTreeMap<PackageName, IndexPackageDependency> {
    match deps.entry(name) {
        std::collections::btree_map::Entry::Vacant(slot) => {
            slot.insert(dep);
        }
        std::collections::btree_map::Entry::Occupied(mut slot) => {
            let merged = slot.get_mut();
            merged.req.comparators.extend(dep.req.comparators);
            merged.optional &= dep.optional;
            merged.features.extend(dep.features);
            merged.default_features |= dep.default_features;
        }
    }
    deps
}

fn exact_req(version: &semver::Version) -> semver::VersionReq {
    semver::VersionReq {
        comparators: vec![semver::Comparator {
            op: semver::Op::Exact,
            major: version.major,
            minor: Some(version.minor),
            patch: Some(version.patch),
            pre: version.pre.clone(),
        }],
    }
}

/// Merge `extra` into `into`, joining version requirements for
/// names that appear in both so the resolver sees a single
/// requirement per package.  Mirrors the join-and-reparse pattern
/// the workspace closure walker uses.
pub(crate) fn merge_versioned_deps(
    into: &mut BTreeMap<PackageName, semver::VersionReq>,
    extra: BTreeMap<PackageName, semver::VersionReq>,
) -> Result<()> {
    for (name, req) in extra {
        match into.entry(name.clone()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(req);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                // The manifest-level and island folds overlap on the
                // forks' own deps; an identical requirement adds
                // nothing and must not grow into ">=1, >=1".
                if slot.get() == &req {
                    continue;
                }
                let parsed = cabin_workspace::combine_version_reqs(&[
                    slot.get().to_string(),
                    req.to_string(),
                ])
                .map_err(|(joined, err)| {
                    anyhow::anyhow!(
                        "conflicting dependency requirements for {}: {}: {}",
                        name.as_str(),
                        joined,
                        err
                    )
                })?;
                slot.insert(parsed);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cabin_workspace::{RegistryPolicy, SelectionMode, WorkspaceLoadOptions};

    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Load `manifest` the way `workspace_prep` does, minus config
    /// discovery: manifest-declared patches only, so the test never
    /// reads the developer's own config files.
    fn load(manifest: &Path) -> (PackageGraph, ActivePatchSet) {
        let config = cabin_config::EffectiveConfig::default();
        let unpatched = cabin_workspace::load_workspace(manifest).unwrap();
        let patches = crate::cli::patch::load_active_patches(&unpatched, &config, false).unwrap();
        let patched_sources = patches.workspace_sources();
        let graph = if patched_sources.is_empty() {
            unpatched
        } else {
            cabin_workspace::load_workspace_with_options(
                manifest,
                &WorkspaceLoadOptions {
                    registry: &[],
                    patches: &patched_sources,
                    registry_policy: RegistryPolicy::StrictFor(&BTreeSet::new()),
                    include_dev_for: &BTreeSet::new(),
                },
            )
            .unwrap()
        };
        let active_patches =
            crate::cli::patch::load_active_patches(&graph, &config, false).unwrap();
        (graph, active_patches)
    }

    fn seed(
        graph: &PackageGraph,
        active_patches: &ActivePatchSet,
        mode: SelectionMode,
        features: &[&str],
    ) -> Result<ResolutionSeed> {
        let selection = PackageSelection {
            mode,
            exclude: Vec::new(),
        };
        let request = cabin_core::SelectionRequest {
            features: features.iter().map(|f| (*f).to_owned()).collect(),
            ..Default::default()
        };
        ResolutionSeed::compute(&SeedInputs {
            graph,
            selection: &selection,
            request: &request,
            patched_names: &active_patches.owned_patched_names(),
            active_patches,
            dev_for: &BTreeSet::new(),
        })
    }

    /// `app` depends on a patched `fmt` whose fork declares its own
    /// registry dependency.
    fn patched_app(dir: &Path) -> (PackageGraph, ActivePatchSet) {
        write(
            dir,
            "app/cabin.toml",
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt-fork" }
"#,
        );
        write(
            dir,
            "fmt-fork/cabin.toml",
            r#"[package]
name = "fmt"
version = "10.2.1"

[dependencies]
spdlog = ">=1.13.0 <2.0.0"
"#,
        );
        load(&dir.join("app/cabin.toml"))
    }

    /// A workspace whose `other` member is the only one with a
    /// registry dependency.
    fn two_member_workspace(dir: &Path) -> (PackageGraph, ActivePatchSet) {
        write(
            dir,
            "ws/cabin.toml",
            r#"[workspace]
members = ["app", "other"]
"#,
        );
        write(
            dir,
            "ws/app/cabin.toml",
            r#"[package]
name = "app"
version = "0.1.0"
"#,
        );
        write(
            dir,
            "ws/other/cabin.toml",
            r#"[package]
name = "other"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        );
        load(&dir.join("ws/cabin.toml"))
    }

    /// `app` with an optional path dependency that carries a
    /// registry dependency, enabled by the `bundled` feature.
    fn optional_path_dep_app(dir: &Path) -> (PackageGraph, ActivePatchSet) {
        write(
            dir,
            "app/cabin.toml",
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
extras = { path = "../extras", optional = true }

[features]
bundled = ["dep:extras"]
"#,
        );
        write(
            dir,
            "extras/cabin.toml",
            r#"[package]
name = "extras"
version = "0.1.0"

[dependencies]
zlib = ">=1"
"#,
        );
        load(&dir.join("app/cabin.toml"))
    }

    #[test]
    fn fork_deps_replace_the_patched_name_in_the_root_set() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (graph, patches) = patched_app(dir.path());
        let seed = seed(&graph, &patches, SelectionMode::CurrentPackage, &[]).unwrap();
        let root_deps = seed.root_deps().unwrap();
        let names: Vec<&str> = root_deps.keys().map(PackageName::as_str).collect();
        assert_eq!(names, ["spdlog"]);
    }

    #[test]
    fn a_closure_reference_activates_the_patch() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (graph, patches) = patched_app(dir.path());
        let seed = seed(&graph, &patches, SelectionMode::CurrentPackage, &[]).unwrap();
        assert_eq!(
            seed.patched_deps.activated,
            BTreeSet::from(["fmt".to_owned()])
        );
    }

    /// `app` depends on a patched `fmt` whose fork declares nothing
    /// versioned: activation alone must not force an index.
    #[test]
    fn an_activated_fork_without_registry_deps_needs_no_index() {
        let dir = assert_fs::TempDir::new().unwrap();
        write(
            dir.path(),
            "app/cabin.toml",
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt-fork" }
"#,
        );
        write(
            dir.path(),
            "fmt-fork/cabin.toml",
            r#"[package]
name = "fmt"
version = "10.2.1"
"#,
        );
        let (graph, patches) = load(&dir.path().join("app/cabin.toml"));
        let seed = seed(&graph, &patches, SelectionMode::CurrentPackage, &[]).unwrap();
        assert_eq!(
            seed.patched_deps.activated,
            BTreeSet::from(["fmt".to_owned()])
        );
        assert!(!seed.has_versioned_deps());
    }

    #[test]
    fn unselected_members_do_not_add_versioned_deps() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (graph, patches) = two_member_workspace(dir.path());
        let only_app = SelectionMode::ExplicitPackages(vec!["app".to_owned()]);
        let seed = seed(&graph, &patches, only_app, &[]).unwrap();
        assert!(!seed.has_versioned_deps());
    }

    #[test]
    fn the_whole_workspace_needs_an_index_when_any_member_does() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (graph, patches) = two_member_workspace(dir.path());
        let seed = seed(&graph, &patches, SelectionMode::WholeWorkspace, &[]).unwrap();
        assert!(seed.has_versioned_deps());
    }

    #[test]
    fn disabled_optional_path_deps_keep_their_registry_deps_out() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (graph, patches) = optional_path_dep_app(dir.path());
        let seed = seed(&graph, &patches, SelectionMode::CurrentPackage, &[]).unwrap();
        assert!(!seed.has_versioned_deps());
    }

    #[test]
    fn enabling_the_feature_pulls_the_optional_path_deps_registry_deps_in() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (graph, patches) = optional_path_dep_app(dir.path());
        let seed = seed(
            &graph,
            &patches,
            SelectionMode::CurrentPackage,
            &["bundled"],
        )
        .unwrap();
        assert!(seed.has_versioned_deps());
    }

    /// `app` and the fork it patches `fmt` with both claim `links =
    /// "z"`; nothing is versioned, so no resolution ever runs.
    #[test]
    fn a_purely_local_patched_chain_still_collides_on_links() {
        let dir = assert_fs::TempDir::new().unwrap();
        write(
            dir.path(),
            "app/cabin.toml",
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"

[patch]
fmt = { path = "../fmt-fork" }

[target.app]
type = "library"
sources = ["src/app.c"]
c-standard = "c11"
links = "z"
"#,
        );
        write(dir.path(), "app/src/app.c", "void app(void) {}\n");
        write(
            dir.path(),
            "fmt-fork/cabin.toml",
            r#"[package]
name = "fmt"
version = "10.2.1"

[target.fmt]
type = "library"
sources = ["src/fmt.c"]
c-standard = "c11"
links = "z"
"#,
        );
        write(dir.path(), "fmt-fork/src/fmt.c", "void fmt(void) {}\n");
        let (graph, patches) = load(&dir.path().join("app/cabin.toml"));
        let err = seed(&graph, &patches, SelectionMode::CurrentPackage, &[])
            .err()
            .expect("duplicate claims must fail the seed");
        assert!(
            format!("{err:#}").contains("is claimed by multiple packages in the dependency graph"),
            "expected the collision diagnostic, got: {err:#}"
        );
    }

    /// `app` uses `za` (patched to a fork), `zb` only under
    /// `[dev-dependencies]`, and the build tool `gen`, which links
    /// `miniz`; the fork and `miniz` both claim `z`.
    fn two_context_app(dir: &Path) -> (PackageGraph, ActivePatchSet) {
        write(
            dir,
            "app/cabin.toml",
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
za = ">=1"

[dev-dependencies]
zb = ">=1"

[build-dependencies]
gen = { path = "../gen" }

[patch]
za = { path = "../za-fork" }
"#,
        );
        write(
            dir,
            "gen/cabin.toml",
            r#"[package]
name = "gen"
version = "0.1.0"

[dependencies]
miniz = { path = "../miniz" }
"#,
        );
        for (rel, name, version) in [("za-fork", "za", "1.0.0"), ("miniz", "miniz", "0.1.0")] {
            write(
                dir,
                &format!("{rel}/cabin.toml"),
                &format!(
                    r#"[package]
name = "{name}"
version = "{version}"

[target.{name}]
type = "library"
sources = ["src/{name}.c"]
c-standard = "c11"
links = "z"
"#
                ),
            );
            write(dir, &format!("{rel}/src/{name}.c"), "void f(void) {}\n");
        }
        load(&dir.join("app/cabin.toml"))
    }

    #[test]
    fn a_fork_one_context_includes_claims_only_there() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (graph, patches) = two_context_app(dir.path());
        let seed = seed(&graph, &patches, SelectionMode::CurrentPackage, &[]).unwrap();
        let [target, build] = &seed.link_domains[..] else {
            panic!("expected two link domains: {:?}", seed.link_domains);
        };
        assert_eq!(target.local_links.len(), 1);
        assert_eq!(build.local_links.len(), 1);
        assert_ne!(target.local_links, build.local_links);
    }

    #[test]
    fn inactive_dev_dependencies_are_not_link_domain_roots() {
        let dir = assert_fs::TempDir::new().unwrap();
        let (graph, patches) = two_context_app(dir.path());
        let seed = seed(&graph, &patches, SelectionMode::CurrentPackage, &[]).unwrap();
        assert!(
            seed.link_domains
                .iter()
                .all(|domain| !domain.roots.contains(&PackageName::new("zb").unwrap())),
            "{:?}",
            seed.link_domains
        );
    }

    #[test]
    fn one_name_in_two_kinds_offers_both_declarations() {
        let dir = assert_fs::TempDir::new().unwrap();
        write(
            dir.path(),
            "app/cabin.toml",
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
util = { version = "^1", features = ["a"] }

[build-dependencies]
util = { version = ">=1.2", features = ["b"] }
"#,
        );
        let (graph, _) = load(&dir.path().join("app/cabin.toml"));
        let packages = local_packages(&graph);
        let app = packages[&PackageName::new("app").unwrap()]
            .versions
            .values()
            .next()
            .unwrap();
        let util = &app.dependencies[&PackageName::new("util").unwrap()];
        assert!(util.req.matches(&semver::Version::new(1, 2, 0)));
        assert!(!util.req.matches(&semver::Version::new(1, 1, 0)));
        assert!(!util.req.matches(&semver::Version::new(2, 0, 0)));
        assert_eq!(util.features, ["a", "b"]);
    }

    #[test]
    fn duplicate_local_links_claims_fail_before_any_index_is_consulted() {
        let dir = assert_fs::TempDir::new().unwrap();
        write(
            dir.path(),
            "ws/cabin.toml",
            r#"[workspace]
members = ["one", "two"]
"#,
        );
        for name in ["one", "two"] {
            write(
                dir.path(),
                &format!("ws/{name}/cabin.toml"),
                &format!(
                    r#"[package]
name = "{name}"
version = "0.1.0"

[target.{name}]
type = "library"
sources = ["src/{name}.c"]
c-standard = "c11"
links = "z"
"#
                ),
            );
            write(
                dir.path(),
                &format!("ws/{name}/src/{name}.c"),
                "void f(void) {}\n",
            );
        }
        let (graph, patches) = load(&dir.path().join("ws/cabin.toml"));
        let err = seed(&graph, &patches, SelectionMode::WholeWorkspace, &[])
            .err()
            .expect("duplicate claims must fail the seed");
        assert!(
            format!("{err:#}").contains("is claimed by multiple packages in the dependency graph"),
            "expected the collision diagnostic, got: {err:#}"
        );
    }
}
