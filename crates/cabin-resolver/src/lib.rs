//! Local dependency resolver for Cabin.
//!
//! Wraps `PubGrub` as the solving engine over a local
//! [`cabin_index::PackageIndex`].  The public surface is intentionally
//! Cabin-native ([`ResolveInput`], [`ResolveOutput`], [`ResolveError`]);
//! `PubGrub` is an implementation detail and does not appear in the
//! crate's public types.
//!
//! ## Internal modules
//!
//! * `preflight` - root-dependency checks that emit Cabin's
//!   targeted error variants before `PubGrub` runs.
//! * `provider` - the `PubGrub` `DependencyProvider`
//!   implementation, candidate selection, and dependency-edge
//!   filtering.
//! * `locked` - shared locked-version metadata validation
//!   plus the locked-mode-only constraint recorder.
//! * `explanation` - `PubGrub` no-solution → Cabin
//!   `ResolveError::Conflict` conversion.
//! * `output` - `PubGrub` `SelectedDependencies` →
//!   [`ResolveOutput`] assembly.
//! * `range` - `semver::VersionReq` → `PubGrub`
//!   `Ranges<semver::Version>` translation.

pub mod error;
mod explanation;
pub mod input;
mod locked;
pub mod output;
mod preflight;
mod provider;
mod range;

use std::collections::BTreeMap;

use cabin_core::standard_compatibility::{ConsumerStandards, EffectiveRequirements};
use cabin_core::{
    DEFAULT_FEATURE_KEY, FeatureEntry, IncompatibleStandards, PackageName, Requirement,
    SourceLanguage,
};
use cabin_index::PackageIndex;
use pubgrub::{PubGrubError, Ranges};
use semver::Version;

pub use error::{ResolveError, ResolverConstraint};
pub use input::{LinkDomain, LinksClaim, LockedVersion, ResolveInput, ResolveMode};
pub use output::{BlockedRequirement, HeldBack, ResolveOutput, ResolvedPackage, ResolvedSource};

use crate::explanation::explain_no_solution;
use crate::output::selected_dependencies_to_output;
use crate::preflight::{effective_locked, preflight_root_dependencies};
use crate::provider::{Node, Provider, version_metadata, without_rejected_prereleases};
use crate::range::req_to_range;

/// Give every version `input`'s lockfile keeps under its mode the
/// resolution metadata of the pinned packaging revision, so an old pin
/// resolves the graph its own archive declares rather than the current
/// revision's.  Idempotent; call it again after loading more of a lazy
/// index.
#[tracing::instrument(level = "debug", skip_all)]
pub fn select_locked_revisions(input: &ResolveInput, index: &mut PackageIndex) {
    for (name, locked) in effective_locked(input) {
        if let Some(checksum) = &locked.checksum
            && let Some(meta) = index
                .packages
                .get_mut(&name)
                .and_then(|entry| entry.versions.get_mut(&locked.version))
        {
            meta.select_revision(checksum);
        }
    }
}

/// Resolve `input`'s versioned dependencies against `index`.
///
/// Returns a [`ResolveOutput`] whose `packages` list contains the root
/// package plus every transitively-resolved registry package, sorted
/// with the root first and then alphabetical by name.
///
/// # Errors
/// Returns [`ResolveError::UnsupportedVersionRequirement`] when a root
/// requirement uses a `semver::Op` this release cannot translate, and
/// propagates the targeted preflight variants (`UnknownPackage`,
/// `NoMatchingVersion`, `AllMatchingVersionsYanked`, `LockfileMissingPackage`,
/// and the `Locked*` variants) from `preflight_root_dependencies`.  When
/// `PubGrub` reports no solution it returns [`ResolveError::Conflict`]; any
/// provider-side [`ResolveError`] surfaced while choosing versions,
/// retrieving dependencies, or cancelling is bubbled back unchanged.
/// Returns [`ResolveError::MetadataNeeded`] when a lazy index lacks
/// metadata the solve reached, and [`ResolveError::LinksCollision`]
/// when two packages claim the same `links` identity: anywhere in the
/// resolved graph, or within one link domain when the input declares
/// domains.
#[tracing::instrument(level = "info", skip_all, fields(roots = input.root_dependencies.len(), index_packages = index.packages.len()))]
pub fn resolve(input: &ResolveInput, index: &PackageIndex) -> Result<ResolveOutput, ResolveError> {
    let mut output = resolve_once(input, index)?;
    output.held_back = held_back_report(input, index, &output.packages)?;
    Ok(output)
}

/// Resolve `input` without computing the standard hold-back report.
///
/// Identical selection to [`resolve`] - the `Fallback` tiering still
/// orders candidates - but the returned `held_back` is always empty.
/// [`resolve`] populates that report by running a second `Allow`-mode
/// solve to diff against; callers that only consume `packages`
/// (build/run/test/vendor, which read the resolved graph into a
/// lockfile and never render the report) call this to skip that extra
/// solve.
///
/// # Errors
/// Same as [`resolve`].
#[tracing::instrument(level = "info", skip_all, fields(roots = input.root_dependencies.len(), index_packages = index.packages.len()))]
pub fn resolve_packages(
    input: &ResolveInput,
    index: &PackageIndex,
) -> Result<ResolveOutput, ResolveError> {
    resolve_once(input, index)
}

/// Resolve `input` once under its own mode, without computing the
/// standard hold-back report (which needs a second, `Allow`-mode
/// resolution to diff against - see [`held_back_report`]).
fn resolve_once(input: &ResolveInput, index: &PackageIndex) -> Result<ResolveOutput, ResolveError> {
    let locked = effective_locked(input);

    // Convert root requirements up front so an unsupported
    // `semver::Op` surfaces as `UnsupportedVersionRequirement`
    // before preflight collapses it into a less specific
    // `NoMatchingVersion`.
    let root_dependencies = convert_root_dependencies(input, index)?;

    // Preflight surfaces Cabin's targeted error variants for
    // root dependencies before `PubGrub` is invoked; the
    // returned constraints seed the locked-mode recorder when
    // it is constructed.
    let root_constraints = preflight_root_dependencies(input, index, &locked)?;

    let provider = Provider::new(input, index, locked, root_constraints, root_dependencies);

    let result = pubgrub::resolve(
        &provider,
        Node::Root(input.root_name.clone()),
        input.root_version.clone(),
    );
    // A round decides a package missing from the index last, so it can
    // still find a solution, or none, after backtracking away from it.
    // Stopping for every one it reached keeps the outcome the one the
    // full index gives.
    let mut unfetched = provider.unfetched.take();
    if let Err(
        PubGrubError::ErrorChoosingVersion {
            source: ResolveError::MetadataNeeded(names),
            ..
        }
        | PubGrubError::ErrorRetrievingDependencies {
            source: ResolveError::MetadataNeeded(names),
            ..
        }
        | PubGrubError::ErrorInShouldCancel(ResolveError::MetadataNeeded(names)),
    ) = &result
    {
        unfetched.extend(names.iter().cloned());
    }
    if !unfetched.is_empty() {
        return Err(ResolveError::MetadataNeeded(unfetched));
    }
    let solution = match result {
        Ok(solution) => solution,
        Err(PubGrubError::NoSolution(tree)) => {
            return Err(explain_no_solution(tree, input));
        }
        Err(
            PubGrubError::ErrorChoosingVersion { source, .. }
            | PubGrubError::ErrorRetrievingDependencies { source, .. }
            | PubGrubError::ErrorInShouldCancel(source),
        ) => return Err(source),
    };

    let output = selected_dependencies_to_output(input, index, solution);
    // Checked against the locked version, not the selected one: an
    // index that drops a pinned revision can also rewrite that
    // version's metadata so the solver moves elsewhere.  `Locked`
    // already validated every pin it used.
    if !matches!(input.mode, ResolveMode::Locked) {
        let locked = effective_locked(input);
        for pkg in &output.packages {
            if pkg.source == ResolvedSource::Index
                && let Some(pin) = locked.get(&pkg.name)
                && let Some(entry) = index.package(&pkg.name)
            {
                locked::check_locked_pin_listed(&pkg.name, pin, entry)?;
            }
        }
    }
    // Post-resolution `links` uniqueness, over the whole graph or
    // per link domain.  Runs here - not in `resolve` - so
    // `resolve_packages` (build/run/test/vendor) enforces it too,
    // and before any lockfile is written from this output.
    if input.link_domains.is_empty() {
        detect_links_collision(&links_claims_of_solution(input, index, &output.packages))?;
    }
    for domain in &input.link_domains {
        detect_links_collision(&links_claims_of_domain(
            input,
            index,
            &output.packages,
            domain,
        ))?;
    }
    Ok(output)
}

/// Every `links` claim the resolved graph carries: the caller's
/// claims for the local packages it includes without resolution,
/// plus the claims of every resolved package - from its index
/// metadata, or from its local entry when the workspace supplies
/// it.  Only selected packages claim: a `[patch]`ed name resolves to
/// its fork, so the upstream it replaces never claims, and the
/// solution holds exactly the packages active edges reach.
fn links_claims_of_solution(
    input: &ResolveInput,
    index: &PackageIndex,
    packages: &[ResolvedPackage],
) -> Vec<LinksClaim> {
    let mut claims = input.local_links.clone();
    claims.extend(resolved_claims(input, index, packages));
    claims
}

/// The claims of one link/runtime domain: its local packages' plus
/// those of every resolved package its roots reach.
fn links_claims_of_domain(
    input: &ResolveInput,
    index: &PackageIndex,
    packages: &[ResolvedPackage],
    domain: &LinkDomain,
) -> Vec<LinksClaim> {
    let by_name: BTreeMap<&PackageName, &ResolvedPackage> = packages
        .iter()
        .filter(|package| package.source != ResolvedSource::Root)
        .map(|package| (&package.name, package))
        .collect();
    let mut reached: BTreeMap<&PackageName, &ResolvedPackage> = BTreeMap::new();
    let mut pending: Vec<&PackageName> = domain.roots.iter().collect();
    while let Some(name) = pending.pop() {
        if let Some(package) = by_name.get(name)
            && reached.insert(name, package).is_none()
        {
            pending.extend(&package.dependencies);
        }
    }
    let mut claims = domain.local_links.clone();
    claims.extend(resolved_claims(input, index, reached.into_values()));
    claims
}

fn resolved_claims<'a>(
    input: &ResolveInput,
    index: &PackageIndex,
    packages: impl IntoIterator<Item = &'a ResolvedPackage>,
) -> Vec<LinksClaim> {
    let mut claims = Vec::new();
    for package in packages {
        // A same-named index entry is a stranger to the local root
        // and must not attribute its claims to it.
        if package.source == ResolvedSource::Root {
            continue;
        }
        let Some(meta) = version_metadata(input, index, &package.name, &package.version) else {
            continue;
        };
        for (target, links) in &meta.links {
            claims.push(LinksClaim {
                package: package.name.clone(),
                version: package.version.clone(),
                target: target.clone(),
                links: links.clone(),
            });
        }
    }
    claims
}

/// Refuse a claim set in which one native-library identity is
/// claimed by two or more distinct packages.  Symbol collisions at
/// the final link ignore dependency visibility, so every claim in
/// the graph participates regardless of the edges that reached it.
///
/// Reports the lexicographically first colliding identity with its
/// full sorted claimant list, so the diagnostic is deterministic.
/// Exposed for the CLI's no-versioned-deps fast path, which skips
/// resolution entirely but must still validate the local claims
/// against each other.
///
/// # Errors
/// Returns [`ResolveError::LinksCollision`] naming the shared
/// identity and every claimant.
pub fn detect_links_collision(claims: &[LinksClaim]) -> Result<(), ResolveError> {
    let mut by_identity: BTreeMap<&str, Vec<&LinksClaim>> = BTreeMap::new();
    for claim in claims {
        by_identity.entry(&claim.links).or_default().push(claim);
    }
    for (links, claimants) in by_identity {
        let mut claimants: Vec<&LinksClaim> = claimants;
        claimants.sort();
        // The same package can legitimately appear twice - once from
        // `local_links` and once from the index - when a caller
        // over-supplies; identical (package, target) claims are one
        // claim, not a collision.
        claimants.dedup_by(|a, b| a.package == b.package && a.target == b.target);
        let distinct_packages = claimants
            .iter()
            .map(|c| &c.package)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        if distinct_packages >= 2 {
            return Err(ResolveError::LinksCollision {
                links: links.to_owned(),
                claimants: claimants.into_iter().cloned().collect(),
            });
        }
    }
    Ok(())
}

/// The standard-caused hold-backs of a `Fallback` resolution: the diff
/// against the `Allow` solution.
///
/// `Allow` is the newest-first selection standards never touch, so the
/// diff never reports a newer version `Allow` also passed over (out of
/// range, unsolvable dependencies, yanked): such a version never
/// appears in the `Allow` solution.  The diff is not exact, though.
/// `Allow` may pick a newer version only because it chose a different
/// parent ([`admissible_under`] drops those rows).  Conversely, a hold
/// `Allow` does not reproduce goes unreported: when `Allow`'s own parent
/// choice confines a package to the version `Fallback` selected, a newer
/// incompatible version the fallback graph admits is never named.
///
/// Empty under `Allow`, and whenever the workspace declares no consumer
/// standard (nothing can be incompatible).  Solvability is identical
/// under both modes, but the `Allow` selection can still be refused -
/// a `links` collision among its newer picks - and then `allow`
/// selects nothing to diff against: any such error drops the newer-pick
/// rows rather than aborting, and a selected version that is itself
/// incompatible is still reported.  The exception is
/// [`ResolveError::MetadataNeeded`]: the `Allow` solve can reach a
/// package the `Fallback` solve never needed, and the caller fetches it
/// and resolves again.
#[tracing::instrument(level = "debug", skip_all)]
fn held_back_report(
    input: &ResolveInput,
    index: &PackageIndex,
    fallback_packages: &[ResolvedPackage],
) -> Result<Vec<HeldBack>, ResolveError> {
    if input.incompatible_standards != IncompatibleStandards::Fallback {
        return Ok(Vec::new());
    }
    let consumer = input.consumer_standards;
    if consumer.c.is_none() && consumer.cxx.is_none() {
        return Ok(Vec::new());
    }

    let mut allow_input = input.clone();
    allow_input.incompatible_standards = IncompatibleStandards::Allow;
    let allow_packages = match resolve_once(&allow_input, index) {
        Ok(allow) => allow.packages,
        Err(err @ ResolveError::MetadataNeeded(_)) => return Err(err),
        Err(_) => Vec::new(),
    };
    let allow_versions: BTreeMap<&PackageName, &Version> = allow_packages
        .iter()
        .map(|package| (&package.name, &package.version))
        .collect();

    // A version kept because the lockfile pinned it is never flagged:
    // lockfile stability wins and metadata alone never churns a lock,
    // so an incompatible *locked* version is not a "no compatible
    // version" case (a compatible one may well be in range).
    let locked = effective_locked(input);

    let mut held_back = Vec::new();
    for package in fallback_packages {
        // The root and local packages offer one version each: nothing
        // to hold back.
        if package.source != ResolvedSource::Index {
            continue;
        }
        if locked
            .get(&package.name)
            .is_some_and(|entry| entry.version == package.version)
        {
            continue;
        }
        // The selected version is *itself* standard-incompatible
        // because nothing in range satisfies the consumer: report its
        // own unmet requirement, with no compatible alternative to
        // name.
        let selected_advertised = index
            .package(&package.name)
            .and_then(|entry| entry.versions.get(&package.version))
            .map(|meta| meta.standards.version_wide_join());
        if let Some(advertised) = selected_advertised {
            let blocked_by = blocking_requirements(consumer, advertised);
            if !blocked_by.is_empty() {
                held_back.push(HeldBack {
                    name: package.name.clone(),
                    selected: package.version.clone(),
                    newest: None,
                    blocked_by,
                });
                continue;
            }
        }

        // Otherwise the selected version is compatible; was a strictly
        // newer version passed over because it is incompatible?
        let Some(&newest) = allow_versions.get(&package.name) else {
            continue;
        };
        if *newest <= package.version {
            continue;
        }
        // `Allow` may have selected `newest` in a *different* dependency
        // context - a divergent parent version pulling a different range
        // for this package.  A hold-back is real only if `newest` is
        // actually reachable under the fallback solution's own
        // constraints; otherwise the difference is a consequence of the
        // parent choice, not a standard hold on this package.
        if !admissible_under(&package.name, newest, input, index, fallback_packages) {
            continue;
        }
        let Some(standards) = index
            .package(&package.name)
            .and_then(|entry| entry.versions.get(newest))
            .map(|meta| &meta.standards)
        else {
            continue;
        };
        // Empty when the newer version is compatible: fallback passed it
        // over only because it is undeclared and a declared-compatible
        // older version outranks it (tier 1 over tier 2).
        held_back.push(HeldBack {
            name: package.name.clone(),
            selected: package.version.clone(),
            newest: Some(newest.clone()),
            blocked_by: blocking_requirements(consumer, standards.version_wide_join()),
        });
    }
    held_back.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(held_back)
}

/// Whether `version` of `package` is admissible under the fallback
/// `solution`'s effective constraints - the root requirement plus every
/// active dependency edge the resolved packages place on `package` -
/// under the same numeric-range *and* pre-release rules the provider
/// applies when selecting.
fn admissible_under(
    package: &PackageName,
    version: &Version,
    input: &ResolveInput,
    index: &PackageIndex,
    solution: &[ResolvedPackage],
) -> bool {
    let mut range = Ranges::full();
    if let Some(req) = input.root_dependencies.get(package)
        && let Ok(root_range) = req_to_range(req)
    {
        let root_range = without_rejected_prereleases(input, index, package, req, root_range);
        range = range.intersection(&root_range);
    }
    // Features other packages request on `package` hold it to versions
    // declaring them, which no standard explains.  Its own defaults and
    // implications are not requests: they may change between versions.
    let mut requested = input
        .root_features
        .get(package)
        .cloned()
        .unwrap_or_default();
    for resolved in solution {
        // The root's own requirements are the `root_dependencies` folded
        // in above; a registry entry that coincidentally shares the
        // root's name and version is unrelated and must not contribute
        // its constraints.
        // Only edges that participated in resolution bind the range.
        if resolved.source == ResolvedSource::Root || !resolved.dependencies.contains(package) {
            continue;
        }
        let Some(meta) = version_metadata(input, index, &resolved.name, &resolved.version) else {
            continue;
        };
        let Some(dep) = meta.dependencies.get(package) else {
            continue;
        };
        if let Ok(dep_range) = req_to_range(&dep.req) {
            let dep_range =
                without_rejected_prereleases(input, index, package, &dep.req, dep_range);
            range = range.intersection(&dep_range);
        }
        requested.extend(dep.features.iter().cloned());
        for enabled in &resolved.features {
            for raw in meta.features.features.get(enabled).into_iter().flatten() {
                if let Ok(FeatureEntry::DepFeature { dep, feature }) = FeatureEntry::parse(raw)
                    && dep == package.as_str()
                {
                    requested.insert(feature);
                }
            }
        }
    }
    let declares_features = version_metadata(input, index, package, version).is_none_or(|meta| {
        requested.iter().all(|feature| {
            feature == DEFAULT_FEATURE_KEY || meta.features.features.contains_key(feature)
        })
    });
    // Match the provider's own admissibility, pre-release rule included,
    // so a pre-release the fallback constraints would reject is not
    // reported as a standard hold.
    declares_features
        && range.contains(version)
        && crate::provider::candidate_admits_prerelease(&range, version)
}

/// The interface requirements of a version the consumer fails to
/// satisfy, in fixed C-before-C++ order for deterministic output.
fn blocking_requirements(
    consumer: ConsumerStandards,
    advertised: EffectiveRequirements,
) -> Vec<BlockedRequirement> {
    let mut blocked = Vec::new();
    if let Some(level) = consumer.c
        && !advertised.c.satisfied_by(level)
    {
        blocked.push(BlockedRequirement {
            language: SourceLanguage::C,
            accepted: accepted_display(advertised.c),
        });
    }
    if let Some(level) = consumer.cxx
        && !advertised.cxx.satisfied_by(level)
    {
        blocked.push(BlockedRequirement {
            language: SourceLanguage::Cxx,
            accepted: accepted_display(advertised.cxx),
        });
    }
    blocked
}

/// The accepted consumer range of a blocking requirement for the
/// message, or `None` for a forbidden requirement (a declared
/// `"none"` or an empty intersection - nothing to name).
/// `Unconstrained` never reaches here - it is satisfied at every
/// consumer level.
fn accepted_display<S: cabin_core::StandardLevel>(requirement: Requirement<S>) -> Option<String> {
    match requirement {
        Requirement::Min(_) | Requirement::Bounded(_) => Some(requirement.to_string()),
        Requirement::Forbidden | Requirement::Unconstrained => None,
    }
}

/// Translate every root requirement to its `PubGrub` range, trimmed
/// like an edge's ([`without_rejected_prereleases`]), mapping the
/// crate-internal [`range::RangeConversionError`] to
/// [`ResolveError::UnsupportedVersionRequirement`] with the
/// package the requirement was attached to.
fn convert_root_dependencies(
    input: &ResolveInput,
    index: &PackageIndex,
) -> Result<Vec<(PackageName, Ranges<Version>)>, ResolveError> {
    input
        .root_dependencies
        .iter()
        .map(|(name, req)| {
            req_to_range(req)
                .map(|range| {
                    let range = without_rejected_prereleases(input, index, name, req, range);
                    (name.clone(), range)
                })
                .map_err(|err| ResolveError::UnsupportedVersionRequirement {
                    package: name.as_str().to_owned(),
                    requirement: err.requirement,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::PackageName;
    use cabin_index::{IndexEntry, PackageIndex, VersionMetadata};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    fn pkg_name(s: &str) -> PackageName {
        PackageName::new(s).unwrap()
    }

    fn req(s: &str) -> semver::VersionReq {
        // Tests use the comma-separated form the semver crate accepts
        // natively to keep this helper trivial.
        semver::VersionReq::parse(s).unwrap()
    }

    fn version(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    fn build_index(entries: Vec<IndexEntry>) -> PackageIndex {
        let mut packages = BTreeMap::new();
        for entry in entries {
            packages.insert(entry.name.clone(), entry);
        }
        PackageIndex {
            root: PathBuf::from("/abs/index"),
            packages,
        }
    }

    type VersionSpec<'a> = (&'a str, Vec<(&'a str, &'a str)>, bool);

    fn entry(name: &str, versions: Vec<VersionSpec<'_>>) -> IndexEntry {
        use cabin_index::IndexPackageDependency;
        let mut vmap = BTreeMap::new();
        for (ver, deps, yanked) in versions {
            let mut depmap = BTreeMap::new();
            for (dep, dep_req) in deps {
                depmap.insert(
                    pkg_name(dep),
                    IndexPackageDependency {
                        req: req(dep_req),
                        optional: false,
                        features: Vec::new(),
                        default_features: true,
                        condition: None,
                    },
                );
            }
            vmap.insert(
                version(ver),
                VersionMetadata {
                    dependencies: depmap,
                    dev_dependencies: BTreeMap::new(),
                    system_dependencies: BTreeMap::new(),
                    yanked,
                    revision: None,
                    revisions: BTreeMap::new(),
                    checksum: None,
                    source: None,
                    features: cabin_core::Features::default(),
                    profiles: None,
                    toolchain: None,
                    build: None,
                    compiler_wrapper: None,
                    language: None,
                    standards: cabin_index::StandardsMetadata::default(),
                    links: BTreeMap::new(),
                    upstream: None,
                },
            );
        }
        IndexEntry {
            name: pkg_name(name),
            versions: vmap,
        }
    }

    fn make_input(root_deps: Vec<(&str, &str)>) -> ResolveInput {
        let mut deps = BTreeMap::new();
        for (n, r) in root_deps {
            deps.insert(pkg_name(n), req(r));
        }
        ResolveInput::new(pkg_name("app"), version("0.1.0"), deps)
    }

    fn input_with_locked(
        root_deps: Vec<(&str, &str)>,
        locked: Vec<(&str, &str)>,
        mode: ResolveMode,
    ) -> ResolveInput {
        let mut input = make_input(root_deps);
        for (name, ver_str) in locked {
            input.locked.insert(
                pkg_name(name),
                LockedVersion {
                    version: version(ver_str),
                    checksum: None,
                },
            );
        }
        input.mode = mode;
        input
    }

    #[test]
    fn resolves_direct_dependency() {
        let index = build_index(vec![entry("fmt", vec![("10.2.1", vec![], false)])]);
        let out = resolve(&make_input(vec![("fmt", ">=10.0.0, <11.0.0")]), &index).unwrap();
        assert_eq!(out.packages[0].source, ResolvedSource::Root);
        assert_eq!(out.packages[0].name.as_str(), "app");
        assert_eq!(out.packages[1].source, ResolvedSource::Index);
        assert_eq!(out.packages[1].name.as_str(), "fmt");
        assert_eq!(out.packages[1].version, version("10.2.1"));
    }

    /// A scoped name and a bare name sharing the base part are
    /// simply distinct packages: the resolver keys on the full
    /// `PackageName` and never splits or normalizes the scope, so
    /// both resolve side by side with their own versions.
    #[test]
    fn scoped_and_bare_names_are_distinct_packages() {
        let index = build_index(vec![
            entry("fmtlib/fmt", vec![("10.2.1", vec![], false)]),
            entry("fmt", vec![("1.0.0", vec![], false)]),
        ]);
        let out = resolve(
            &make_input(vec![("fmtlib/fmt", ">=10.0.0"), ("fmt", ">=1.0.0")]),
            &index,
        )
        .unwrap();
        let mut resolved: Vec<(&str, String)> = out
            .packages
            .iter()
            .filter(|p| p.source == ResolvedSource::Index)
            .map(|p| (p.name.as_str(), p.version.to_string()))
            .collect();
        resolved.sort();
        assert_eq!(
            resolved,
            vec![
                ("fmt", "1.0.0".to_owned()),
                ("fmtlib/fmt", "10.2.1".to_owned()),
            ]
        );
    }

    /// With no consumer standard (the `ResolveInput::new` default),
    /// standard-compatibility metadata on candidate versions does not
    /// change resolution: every candidate ranks alike, so the output
    /// is identical with and without it.
    #[test]
    fn standard_metadata_does_not_change_resolution() {
        use cabin_core::{CxxStandard, Requirement, StandardsMetadata, TargetStandards};
        // A small graph with real selection: app -> spdlog -> fmt, with
        // two fmt candidates so a filter would have something to drop.
        let make = || {
            build_index(vec![
                entry(
                    "spdlog",
                    vec![("1.13.0", vec![("fmt", ">=10.0.0, <11.0.0")], false)],
                ),
                entry(
                    "fmt",
                    vec![("10.2.1", vec![], false), ("10.1.0", vec![], false)],
                ),
            ])
        };
        let input = make_input(vec![("spdlog", ">=1.0.0")]);
        let baseline = resolve(&input, &make()).unwrap();

        let mut targets = BTreeMap::new();
        targets.insert(
            "fmt".to_owned(),
            TargetStandards {
                header_only: false,
                gnu_extensions: true,
                interface_c: Requirement::Forbidden,
                interface_cxx: Requirement::Min(CxxStandard::Cxx20),
            },
        );
        let table = StandardsMetadata { targets };
        let mut index = make();
        for entry in index.packages.values_mut() {
            for meta in entry.versions.values_mut() {
                meta.standards = table.clone();
            }
        }
        assert!(
            !index.packages[&pkg_name("fmt")].versions[&version("10.2.1")]
                .standards
                .is_empty(),
            "the table must actually be present for this to prove anything"
        );

        let with_standards = resolve(&input, &index).unwrap();
        assert_eq!(baseline, with_standards);
    }

    #[test]
    fn resolves_transitive_dependency() {
        let index = build_index(vec![
            entry(
                "fmt",
                vec![("10.2.1", vec![], false), ("10.0.0", vec![], false)],
            ),
            entry(
                "spdlog",
                vec![("1.13.0", vec![("fmt", ">=10.0.0, <11.0.0")], false)],
            ),
        ]);
        let out = resolve(&make_input(vec![("spdlog", "^1.13.0")]), &index).unwrap();
        let names: Vec<&str> = out.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["app", "fmt", "spdlog"]);
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("10.2.1"));
    }

    #[test]
    fn picks_newest_compatible_version() {
        let index = build_index(vec![entry(
            "fmt",
            vec![
                ("10.2.1", vec![], false),
                ("10.1.0", vec![], false),
                ("10.0.0", vec![], false),
                ("11.0.0", vec![], false),
            ],
        )]);
        let out = resolve(&make_input(vec![("fmt", ">=10, <11")]), &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("10.2.1"));
    }

    #[test]
    fn skips_yanked_versions() {
        let index = build_index(vec![entry(
            "fmt",
            vec![
                ("10.2.1", vec![], true), // yanked
                ("10.1.0", vec![], false),
            ],
        )]);
        let out = resolve(&make_input(vec![("fmt", ">=10, <11")]), &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("10.1.0"));
    }

    #[test]
    fn all_yanked_errors() {
        let index = build_index(vec![entry("fmt", vec![("10.2.1", vec![], true)])]);
        let err = resolve(&make_input(vec![("fmt", ">=10, <11")]), &index).unwrap_err();
        assert!(matches!(err, ResolveError::AllMatchingVersionsYanked(name) if name == "fmt"));
    }

    #[test]
    fn missing_package_errors() {
        let index = build_index(vec![]);
        let err = resolve(&make_input(vec![("fmt", "*")]), &index).unwrap_err();
        assert!(matches!(err, ResolveError::UnknownPackage(name) if name == "fmt"));
    }

    #[test]
    fn no_matching_version_errors_with_constraints() {
        let index = build_index(vec![entry("fmt", vec![("9.0.0", vec![], false)])]);
        let err = resolve(&make_input(vec![("fmt", ">=10, <11")]), &index).unwrap_err();
        match err {
            ResolveError::NoMatchingVersion {
                package,
                constraints,
            } => {
                assert_eq!(package, "fmt");
                assert_eq!(constraints.len(), 1);
                assert_eq!(constraints[0].origin.as_str(), "app");
            }
            other => panic!("expected NoMatchingVersion, got {other:?}"),
        }
    }

    #[test]
    fn conflict_between_two_callers_errors() {
        // root depends on a >=1, and on b >=1.
        // b 1.0.0 depends on a >=2.
        // a only has 1.0.0 → conflict.
        let index = build_index(vec![
            entry("a", vec![("1.0.0", vec![], false)]),
            entry("b", vec![("1.0.0", vec![("a", ">=2, <3")], false)]),
        ]);
        let err = resolve(
            &make_input(vec![("a", ">=1, <2"), ("b", ">=1, <2")]),
            &index,
        )
        .unwrap_err();
        // The first failure encountered is reported.  The exact variant
        // varies depending on visit order; we assert that
        // resolution failed with a useful error.
        match err {
            ResolveError::NoMatchingVersion { package, .. } => {
                assert!(package == "a" || package == "b");
            }
            ResolveError::Conflict { .. } => {}
            other => panic!("expected NoMatchingVersion or Conflict, got {other:?}"),
        }
    }

    #[test]
    fn a_deep_conflict_is_explained_without_expanding_its_shared_derivations() {
        // Every version of p<i> needs a lower version of p<i+1>, one
        // more level than there are versions.  The derivation shares
        // its subproofs, and expanded as a tree it doubles with each
        // level.
        let depth = 30;
        let names: Vec<String> = (0..=depth).map(|i| format!("p{i}")).collect();
        let versions: Vec<String> = (1..depth).map(|j| format!("{j}.0.0")).collect();
        let below: Vec<String> = (1..depth).map(|j| format!("<{j}.0.0")).collect();
        let index = build_index(
            (0..=depth)
                .map(|i| {
                    let specs = (0..depth - 1)
                        .map(|j| {
                            let deps = names
                                .get(i + 1)
                                .map(|next| vec![(next.as_str(), below[j].as_str())])
                                .unwrap_or_default();
                            (versions[j].as_str(), deps, false)
                        })
                        .collect();
                    entry(&names[i], specs)
                })
                .collect(),
        );
        let err = resolve(&make_input(vec![("p0", "*")]), &index).unwrap_err();
        let ResolveError::Conflict { package, .. } = err else {
            panic!("expected a conflict, got {err:?}");
        };
        assert_eq!(package, "p0");
    }

    #[test]
    fn deterministic_output_order() {
        // Two unrelated direct deps should always come out alphabetical.
        let index = build_index(vec![
            entry("alpha", vec![("1.0.0", vec![], false)]),
            entry("beta", vec![("2.0.0", vec![], false)]),
        ]);
        let out = resolve(&make_input(vec![("beta", "*"), ("alpha", "*")]), &index).unwrap();
        let names: Vec<&str> = out.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["app", "alpha", "beta"]);
    }

    #[test]
    fn backtracks_when_newest_is_incompatible() {
        // root requires fmt ^10. spdlog 1.0 requires fmt >=11.
        // spdlog 0.9 requires fmt >=10.
        // Resolver should pick spdlog 0.9, then fmt 10.x.
        let index = build_index(vec![
            entry(
                "fmt",
                vec![("10.2.1", vec![], false), ("11.0.0", vec![], false)],
            ),
            entry(
                "spdlog",
                vec![
                    ("1.0.0", vec![("fmt", ">=11, <12")], false),
                    ("0.9.0", vec![("fmt", ">=10, <11")], false),
                ],
            ),
        ]);
        let out = resolve(&make_input(vec![("fmt", "^10"), ("spdlog", "<2")]), &index).unwrap();
        let spdlog = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "spdlog")
            .unwrap();
        assert_eq!(spdlog.version, version("0.9.0"));
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("10.2.1"));
    }

    // -----------------------------------------------------------------
    // lockfile preferences
    // -----------------------------------------------------------------

    #[test]
    fn prefer_locked_keeps_locked_version_when_compatible() {
        // Index has 10.2.1, 10.1.0; lockfile pins 10.1.0; constraint *
        // accepts both.  PreferLocked must keep the older locked version.
        let index = build_index(vec![entry(
            "fmt",
            vec![("10.2.1", vec![], false), ("10.1.0", vec![], false)],
        )]);
        let input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "10.1.0")],
            ResolveMode::PreferLocked,
        );
        let out = resolve(&input, &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("10.1.0"));
    }

    #[test]
    fn prefer_locked_falls_back_when_lockfile_violates_constraint() {
        // Locked 10.0.0 but constraint requires >=10.2.  Resolver should
        // pick 10.2.0 instead (the newest compatible).
        let index = build_index(vec![entry(
            "fmt",
            vec![("10.2.0", vec![], false), ("10.0.0", vec![], false)],
        )]);
        let input = input_with_locked(
            vec![("fmt", ">=10.2")],
            vec![("fmt", "10.0.0")],
            ResolveMode::PreferLocked,
        );
        let out = resolve(&input, &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("10.2.0"));
    }

    #[test]
    fn locked_mode_succeeds_when_lockfile_is_current() {
        let index = build_index(vec![entry(
            "fmt",
            vec![("10.2.1", vec![], false), ("10.1.0", vec![], false)],
        )]);
        let input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "10.1.0")],
            ResolveMode::Locked,
        );
        let out = resolve(&input, &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("10.1.0"));
    }

    #[test]
    fn locked_mode_fails_when_lockfile_is_missing_required_package() {
        let index = build_index(vec![entry("fmt", vec![("10.2.1", vec![], false)])]);
        let input = input_with_locked(
            vec![("fmt", "*")],
            vec![], // empty lockfile
            ResolveMode::Locked,
        );
        let err = resolve(&input, &index).unwrap_err();
        assert!(matches!(err, ResolveError::LockfileMissingPackage(name) if name == "fmt"));
    }

    #[test]
    fn locked_mode_fails_when_locked_version_violates_constraint() {
        let index = build_index(vec![entry(
            "fmt",
            vec![("10.2.0", vec![], false), ("10.0.0", vec![], false)],
        )]);
        let input = input_with_locked(
            vec![("fmt", ">=10.2")],
            vec![("fmt", "10.0.0")],
            ResolveMode::Locked,
        );
        let err = resolve(&input, &index).unwrap_err();
        match err {
            ResolveError::LockedVersionViolatesConstraint {
                name,
                version,
                constraints,
            } => {
                assert_eq!(name, "fmt");
                assert_eq!(version, "10.0.0");
                // The recorded constraint must name the actual
                // root package ("app") so the rendered message
                // points at the dependency that imposed the
                // requirement, not the literal string "root".
                assert_eq!(constraints.len(), 1);
                assert_eq!(constraints[0].origin.as_str(), "app");
                assert_eq!(constraints[0].requirement.to_string(), ">=10.2");
            }
            other => panic!("expected LockedVersionViolatesConstraint, got {other:?}"),
        }
    }

    #[test]
    fn locked_mode_fails_when_locked_version_missing_from_index() {
        let index = build_index(vec![entry("fmt", vec![("10.2.1", vec![], false)])]);
        let input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "9.9.9")],
            ResolveMode::Locked,
        );
        let err = resolve(&input, &index).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::LockedVersionMissing { name, version }
            if name == "fmt" && version == "9.9.9"
        ));
    }

    #[test]
    fn locked_mode_fails_when_locked_version_yanked() {
        let index = build_index(vec![entry(
            "fmt",
            vec![("10.2.1", vec![], true), ("10.1.0", vec![], false)],
        )]);
        let input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "10.2.1")],
            ResolveMode::Locked,
        );
        let err = resolve(&input, &index).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::LockedVersionYanked { name, version }
            if name == "fmt" && version == "10.2.1"
        ));
    }

    #[test]
    fn update_all_ignores_locked_preferences() {
        let index = build_index(vec![entry(
            "fmt",
            vec![("10.2.1", vec![], false), ("10.1.0", vec![], false)],
        )]);
        let input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "10.1.0")],
            ResolveMode::UpdateAll,
        );
        let out = resolve(&input, &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("10.2.1"));
    }

    #[test]
    fn update_package_drops_only_named_lock() {
        // Both packages locked to older versions; update spdlog.
        let index = build_index(vec![
            entry(
                "fmt",
                vec![("10.2.1", vec![], false), ("10.1.0", vec![], false)],
            ),
            entry(
                "spdlog",
                vec![
                    ("1.13.0", vec![("fmt", ">=10.0, <11")], false),
                    ("1.12.0", vec![("fmt", ">=10.0, <11")], false),
                ],
            ),
        ]);
        let input = input_with_locked(
            vec![("spdlog", "*"), ("fmt", "*")],
            vec![("fmt", "10.1.0"), ("spdlog", "1.12.0")],
            ResolveMode::UpdatePackage(pkg_name("spdlog")),
        );
        let out = resolve(&input, &index).unwrap();
        let spdlog = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "spdlog")
            .unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(spdlog.version, version("1.13.0"));
        assert_eq!(fmt.version, version("10.1.0"));
    }

    /// Build an [`IndexEntry`] with a single version whose normal
    /// dependencies are `(name, req, optional)` tuples, without
    /// `cfg(...)` conditions.
    fn entry_with_kinded(name: &str, ver_str: &str, normal: Vec<(&str, &str, bool)>) -> IndexEntry {
        use cabin_index::IndexPackageDependency;
        let to_deps = |list: Vec<(&str, &str, bool)>| -> BTreeMap<_, _> {
            list.into_iter()
                .map(|(dep, dep_req, optional)| {
                    (
                        pkg_name(dep),
                        IndexPackageDependency {
                            req: req(dep_req),
                            optional,
                            features: Vec::new(),
                            default_features: true,
                            condition: None,
                        },
                    )
                })
                .collect()
        };
        let mut vmap = BTreeMap::new();
        vmap.insert(
            version(ver_str),
            VersionMetadata {
                dependencies: to_deps(normal),
                dev_dependencies: BTreeMap::new(),
                system_dependencies: BTreeMap::new(),
                yanked: false,
                revision: None,
                revisions: BTreeMap::new(),
                checksum: None,
                source: None,
                features: cabin_core::Features::default(),
                profiles: None,
                toolchain: None,
                build: None,
                compiler_wrapper: None,
                language: None,
                standards: cabin_index::StandardsMetadata::default(),
                links: BTreeMap::new(),
                upstream: None,
            },
        );
        IndexEntry {
            name: pkg_name(name),
            versions: vmap,
        }
    }

    #[test]
    fn skips_disabled_optional_registry_dependencies() {
        // Optional registry deps stay out of resolution until a
        // feature enables them - matching the documented model
        // and the conservative pattern in
        // cabin-workspace::patch::collect_version_requirements.
        // If the resolver greedily queued them, the missing
        // `forbidden` package below would surface as
        // UnknownPackage instead of resolving cleanly.
        let parent = entry_with_kinded("parent", "1.0.0", vec![("forbidden", ">=1", true)]);
        let index = build_index(vec![parent]);
        let out = resolve(&make_input(vec![("parent", "*")]), &index).unwrap();
        assert!(!out.packages.iter().any(|p| p.name.as_str() == "forbidden"));
    }

    /// Give one version of an already-built index entry a packaging
    /// revision per checksum, with the last one current - the shape
    /// the loader produces for a published version.
    fn inject_revisions(index: &mut cabin_index::PackageIndex, name: &str, checksums: &[&str]) {
        let entry = index.packages.get_mut(&pkg_name(name)).unwrap();
        let meta = entry.versions.values_mut().next().unwrap();
        for hex in checksums {
            meta.revisions.insert(
                hex[..16].to_owned(),
                cabin_index::RevisionMetadata {
                    checksum: cabin_core::Checksum::parse(&format!("sha256:{hex}")).unwrap(),
                    published_at: "2026-07-28T00:00:00Z".to_owned(),
                    source: cabin_index::SourceLocation::LocalPath(std::path::PathBuf::from(
                        "unused.zip",
                    )),
                    resolution: None,
                },
            );
        }
        let current = checksums.last().unwrap();
        meta.revision = Some(current[..16].to_owned());
        meta.checksum = Some(cabin_core::Checksum::parse(&format!("sha256:{current}")).unwrap());
    }

    const REV_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const REV_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// `PreferLocked` refuses a pin the index no longer lists instead
    /// of re-pinning, even when the solution moves to another version.
    #[test]
    fn prefer_locked_fails_on_a_pin_the_index_no_longer_lists() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("10.2.1", vec![], true), ("10.2.2", vec![], false)],
        )]);
        inject_revisions(&mut index, "fmt", &[REV_A]);
        let mut input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "10.2.1")],
            ResolveMode::PreferLocked,
        );
        input.locked.get_mut(&pkg_name("fmt")).unwrap().checksum =
            Some(cabin_core::Checksum::parse(&format!("sha256:{REV_B}")).unwrap());
        assert!(matches!(
            resolve_packages(&input, &index).unwrap_err(),
            ResolveError::LockedChecksumMismatch { name, .. } if name == "fmt"
        ));

        input.locked.get_mut(&pkg_name("fmt")).unwrap().version = version("10.2.0");
        assert!(matches!(
            resolve_packages(&input, &index).unwrap_err(),
            ResolveError::LockedVersionMissing { name, .. } if name == "fmt"
        ));

        // Updating the package lets it move.
        input.mode = ResolveMode::UpdatePackage(pkg_name("fmt"));
        assert!(resolve_packages(&input, &index).is_ok());
    }

    #[test]
    fn locked_mode_fails_on_checksum_mismatch() {
        let mut index = build_index(vec![entry("fmt", vec![("10.2.1", vec![], false)])]);
        inject_revisions(&mut index, "fmt", &[REV_A]);

        let mut input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "10.2.1")],
            ResolveMode::Locked,
        );
        // The lockfile pins a checksum no published revision carries.
        input.locked.get_mut(&pkg_name("fmt")).unwrap().checksum =
            Some(cabin_core::Checksum::parse(&format!("sha256:{REV_B}")).unwrap());

        let err = resolve(&input, &index).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::LockedChecksumMismatch { name, .. } if name == "fmt"
        ));
    }

    /// A locked entry without a checksum cannot pin a revision of a
    /// version that has them; `--locked` refuses rather than falling
    /// back to the current revision.
    #[test]
    fn locked_mode_requires_a_checksum_when_the_version_has_revisions() {
        let mut index = build_index(vec![entry("fmt", vec![("10.2.1", vec![], false)])]);
        inject_revisions(&mut index, "fmt", &[REV_A]);
        let input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "10.2.1")],
            ResolveMode::Locked,
        );
        let err = resolve(&input, &index).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::LockedChecksumMissing { name, .. } if name == "fmt"
        ));
    }

    /// The lockfile checksum pins a packaging revision, and a
    /// superseded revision stays valid: published revisions remain
    /// fetchable, so a respin must never break `--locked` builds
    /// that pin the older bytes.
    #[test]
    fn locked_checksum_matching_a_superseded_revision_resolves() {
        let mut index = build_index(vec![entry("fmt", vec![("10.2.1", vec![], false)])]);
        // Two revisions; REV_B is current, REV_A superseded.
        inject_revisions(&mut index, "fmt", &[REV_A, REV_B]);

        let mut input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "10.2.1")],
            ResolveMode::Locked,
        );
        input.locked.get_mut(&pkg_name("fmt")).unwrap().checksum =
            Some(cabin_core::Checksum::parse(&format!("sha256:{REV_A}")).unwrap());

        let out = resolve(&input, &index).unwrap();
        assert_eq!(out.packages[1].version, version("10.2.1"));
    }

    // -----------------------------------------------------------------
    // Pre-release boundary
    //
    // Pre-release versions are excluded from candidate selection
    // unless the requirement opts in: a comparator names the
    // candidate's `major.minor.patch` with a pre-release tag, or the
    // range is the singleton containing it.  This pins the boundary
    // so future resolver work cannot silently expand the supported
    // syntax.
    // -----------------------------------------------------------------

    #[test]
    fn prerelease_version_excluded_under_wide_range() {
        let index = build_index(vec![entry(
            "fmt",
            vec![
                ("1.0.0-alpha", vec![], false),
                ("1.0.0", vec![], false),
                ("1.5.0", vec![], false),
            ],
        )]);
        let out = resolve(&make_input(vec![("fmt", ">=1.0.0, <2.0.0")]), &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("1.5.0"));
    }

    /// `PreferLocked` must fall back to a compatible release
    /// when the lockfile pins a pre-release the manifest
    /// constraint no longer admits - carrying the lock
    /// forward would violate the user-declared requirement.
    #[test]
    fn prerelease_lock_falls_back_when_constraint_no_longer_admits_it() {
        let index = build_index(vec![entry(
            "fmt",
            vec![("1.5.0-alpha", vec![], false), ("1.5.0", vec![], false)],
        )]);
        let input = input_with_locked(
            // The manifest does not open a pre-release window
            // for any `1.x.y`, so semver rejects `1.5.0-alpha`
            // even though it sits numerically inside the
            // range.
            vec![("fmt", ">=1.0.0, <2.0.0")],
            vec![("fmt", "1.5.0-alpha")],
            ResolveMode::PreferLocked,
        );
        let out = resolve(&input, &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("1.5.0"));
    }

    /// A pre-release version that was already pinned by the
    /// previous lockfile keeps being selected under
    /// `PreferLocked`, even if the user's requirement is a
    /// wide range - otherwise `cabin resolve` against an
    /// existing lockfile would silently churn an opt-in
    /// pre-release back to a release.
    #[test]
    fn prerelease_version_kept_when_locked() {
        let index = build_index(vec![entry(
            "fmt",
            vec![("1.0.0-alpha", vec![], false), ("1.0.0", vec![], false)],
        )]);
        let input = input_with_locked(
            vec![("fmt", ">=1.0.0-alpha, <2")],
            vec![("fmt", "1.0.0-alpha")],
            ResolveMode::PreferLocked,
        );
        let out = resolve(&input, &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("1.0.0-alpha"));
    }

    #[test]
    fn prerelease_version_selected_under_exact_match() {
        let index = build_index(vec![entry(
            "fmt",
            vec![("1.0.0-alpha", vec![], false), ("1.0.0", vec![], false)],
        )]);
        let out = resolve(&make_input(vec![("fmt", "=1.0.0-alpha")]), &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        assert_eq!(fmt.version, version("1.0.0-alpha"));
    }

    /// A non-singleton requirement that explicitly opts in to
    /// a pre-release of one `major.minor.patch` (semver's
    /// `pre_is_compatible` rule) must still admit other
    /// pre-releases of that same triple - `>=1.0.0-alpha, <1.0.0`
    /// is meant to mean "any 1.0.0-pre".
    #[test]
    fn prerelease_admitted_under_explicit_opt_in_range() {
        let index = build_index(vec![entry(
            "fmt",
            vec![
                ("1.0.0-alpha", vec![], false),
                ("1.0.0-beta", vec![], false),
                ("1.0.0", vec![], false),
            ],
        )]);
        let out = resolve(&make_input(vec![("fmt", ">=1.0.0-alpha, <1.0.0")]), &index).unwrap();
        let fmt = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap();
        // `1.0.0-beta` is the newest pre-release strictly less
        // than `1.0.0`.  Without the bound-aware pre-release
        // filter, the resolver would reject every candidate.
        assert_eq!(fmt.version, version("1.0.0-beta"));
    }

    /// An unknown transitive dependency must be a backtrackable
    /// miss rather than a fatal error: pubgrub can then fall
    /// back to an older version of the parent whose dependency
    /// list does not reach the missing package.
    #[test]
    fn unknown_transitive_dependency_lets_resolver_backtrack() {
        let index = build_index(vec![entry(
            "spdlog",
            vec![
                // The newest version pulls a package
                // that does not exist in the index.  The
                // older version is dependency-free and
                // satisfies the same root requirement.
                ("1.1.0", vec![("vanished", "^1")], false),
                ("1.0.0", vec![], false),
            ],
        )]);
        let out = resolve(&make_input(vec![("spdlog", "^1")]), &index).unwrap();
        let spdlog = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "spdlog")
            .unwrap();
        assert_eq!(spdlog.version, version("1.0.0"));
        assert!(
            !out.packages.iter().any(|p| p.name.as_str() == "vanished"),
            "missing transitive dep must not appear in output"
        );
    }

    /// `--locked` against a transitive pre-release lockfile
    /// entry must reject the entry when the parent's
    /// `VersionReq` does not name the same `major.minor.patch`
    /// with a pre tag - even though the numeric range from
    /// `req_to_range` would happily contain the pre-release.
    #[test]
    fn locked_mode_rejects_transitive_prerelease_outside_semver_rule() {
        // spdlog 1.0.0 requires `fmt >=1.0.0, <2.0.0` (a wide
        // numeric range).  The lockfile pins fmt to
        // `1.5.0-alpha`. semver's `matches` says no; the
        // resolver must too.
        let index = build_index(vec![
            entry(
                "spdlog",
                vec![("1.0.0", vec![("fmt", ">=1.0.0, <2.0.0")], false)],
            ),
            entry(
                "fmt",
                vec![("1.5.0-alpha", vec![], false), ("1.5.0", vec![], false)],
            ),
        ]);
        let mut input = input_with_locked(
            vec![("spdlog", "^1")],
            vec![("spdlog", "1.0.0"), ("fmt", "1.5.0-alpha")],
            ResolveMode::Locked,
        );
        // The root constraint on `spdlog` is `^1`, which
        // matches `1.0.0` - pre-flight passes.  The locked
        // `fmt 1.5.0-alpha` enters via the transitive path.
        input.locked.get_mut(&pkg_name("fmt")).unwrap().checksum = None;
        let err = resolve(&input, &index).unwrap_err();
        match err {
            ResolveError::LockedVersionViolatesConstraint { name, version, .. } => {
                assert_eq!(name, "fmt");
                assert_eq!(version, "1.5.0-alpha");
            }
            other => panic!("expected LockedVersionViolatesConstraint, got {other:?}"),
        }
    }

    /// Requirements opting in and out of `1.2.0-beta.3`, each pair in
    /// both decision orders (see [`split_edges_index`]).
    const SPLIT_REQUIREMENTS: [(&str, &str); 2] =
        [("^1.0", "^1.2.0-beta.1"), ("^1.2.0-beta.1", "^1.0")];

    /// `aaa` requires `fmt` at `direct` and `ccc` (under `bbb`) at
    /// `nested`, one level deeper, so `PubGrub` decides `fmt` between
    /// the two edges: swapping the requirements swaps which it sees
    /// first.  The roots are `aaa` and `bbb`.
    fn split_edges_index(direct: &str, nested: &str, fmt: &[&'static str]) -> PackageIndex {
        build_index(vec![
            entry("aaa", vec![("1.0.0", vec![("fmt", direct)], false)]),
            entry("bbb", vec![("1.0.0", vec![("ccc", "*")], false)]),
            entry("ccc", vec![("1.0.0", vec![("fmt", nested)], false)]),
            entry("fmt", fmt.iter().map(|v| (*v, vec![], false)).collect()),
        ])
    }

    /// A pre-release one requirement admits and another rejects is
    /// never selected, whichever requirement `PubGrub` sees first -
    /// with no release left (`1.0.0` misses `^1.2.0-beta.1`), nothing
    /// satisfies both.
    #[test]
    fn prerelease_must_satisfy_every_requirement_in_either_order() {
        for fmt in [&["1.2.0-beta.3"][..], &["1.0.0", "1.2.0-beta.3"]] {
            for (direct, nested) in SPLIT_REQUIREMENTS {
                let index = split_edges_index(direct, nested, fmt);
                let result = resolve(&make_input(vec![("aaa", "*"), ("bbb", "*")]), &index);
                assert!(
                    matches!(result, Err(ResolveError::Conflict { .. })),
                    "{fmt:?}, direct {direct}, nested {nested}: {result:?}"
                );
            }
        }
    }

    /// Trimming the newer `1.3.0-beta.1`, which only the second
    /// requirement admits, keeps the `1.2.0-beta.3` both admit.
    #[test]
    fn prerelease_every_requirement_admits_is_selected() {
        let both = ("^1.2.0-beta.1", ">=1.2.0-beta.1, <=1.3.0-beta.1");
        for (direct, nested) in [both, (both.1, both.0)] {
            let index =
                split_edges_index(direct, nested, &["1.0.0", "1.2.0-beta.3", "1.3.0-beta.1"]);
            let out = resolve(&make_input(vec![("aaa", "*"), ("bbb", "*")]), &index).unwrap();
            let fmt = out.packages.iter().find(|p| p.name.as_str() == "fmt");
            assert_eq!(
                fmt.unwrap().version,
                version("1.2.0-beta.3"),
                "{direct} / {nested}"
            );
        }
    }

    /// A root requirement's range is trimmed like an edge's: the root's
    /// `^1.0` rejects the beta `aaa` opts into.
    #[test]
    fn root_requirement_rejects_a_prerelease_an_edge_opts_into() {
        let index = build_index(vec![
            entry(
                "aaa",
                vec![("1.0.0", vec![("fmt", "^1.2.0-beta.1")], false)],
            ),
            entry(
                "fmt",
                vec![("1.0.0", vec![], false), ("1.2.0-beta.3", vec![], false)],
            ),
        ]);
        let result = resolve(&make_input(vec![("aaa", "*"), ("fmt", "^1.0")]), &index);
        assert!(
            matches!(result, Err(ResolveError::Conflict { .. })),
            "{result:?}"
        );
    }

    /// `--locked` accepts every selection the default mode makes.
    #[test]
    fn locked_mode_replays_every_default_resolution() {
        let mut replayed = 0;
        for fmt in [
            &["1.2.0-beta.3"][..],
            &["1.0.0", "1.2.0-beta.3"],
            &["1.2.0-beta.3", "1.2.0"],
        ] {
            for (direct, nested) in SPLIT_REQUIREMENTS
                .into_iter()
                .chain([("^1.2.0-beta.1", "^1.2.0-beta.1")])
            {
                let index = split_edges_index(direct, nested, fmt);
                let roots = vec![("aaa", "*"), ("bbb", "*")];
                let Ok(out) = resolve(&make_input(roots.clone()), &index) else {
                    continue;
                };
                let pins: Vec<(&str, String)> = out
                    .packages
                    .iter()
                    .filter(|p| p.source == ResolvedSource::Index)
                    .map(|p| (p.name.as_str(), p.version.to_string()))
                    .collect();
                let pins = pins.iter().map(|(n, v)| (*n, v.as_str())).collect();
                let input = input_with_locked(roots, pins, ResolveMode::Locked);
                let replay = resolve(&input, &index);
                assert!(
                    replay.as_ref().is_ok_and(|r| r.packages == out.packages),
                    "{fmt:?}, direct {direct}, nested {nested}: {replay:?}"
                );
                replayed += 1;
            }
        }
        // Both opt-in pairs resolve on each index, and `1.2.0` serves
        // either mixed pair.
        assert_eq!(replayed, 5);
    }

    /// `--locked` rejects a pre-release one dependent opts into and
    /// another does not, whichever edge reaches the package first.
    #[test]
    fn locked_mode_checks_every_edge_against_a_locked_prerelease() {
        for (direct, nested) in SPLIT_REQUIREMENTS {
            let index = split_edges_index(direct, nested, &["1.2.0-beta.3"]);
            let input = input_with_locked(
                vec![("aaa", "*"), ("bbb", "*")],
                vec![
                    ("aaa", "1.0.0"),
                    ("bbb", "1.0.0"),
                    ("ccc", "1.0.0"),
                    ("fmt", "1.2.0-beta.3"),
                ],
                ResolveMode::Locked,
            );
            let result = resolve(&input, &index);
            let Err(ResolveError::LockedVersionViolatesConstraint { name, .. }) = &result else {
                panic!("direct {direct}, nested {nested}: expected a violation, got {result:?}");
            };
            assert_eq!(name, "fmt");
        }
    }

    // -----------------------------------------------------------------
    // Conditional registry dependencies
    //
    // Resolution ignores index `cfg(...)` conditions: the lockfile
    // serves every platform, and builds filter per platform.
    // -----------------------------------------------------------------

    fn entry_with_conditional(
        name: &str,
        ver_str: &str,
        normal: Vec<(&str, &str, Option<cabin_core::Condition>)>,
    ) -> IndexEntry {
        use cabin_index::IndexPackageDependency;
        let mut depmap = BTreeMap::new();
        for (dep, dep_req, cond) in normal {
            depmap.insert(
                pkg_name(dep),
                IndexPackageDependency {
                    req: req(dep_req),
                    optional: false,
                    features: Vec::new(),
                    default_features: true,
                    condition: cond,
                },
            );
        }
        let mut vmap = BTreeMap::new();
        vmap.insert(
            version(ver_str),
            VersionMetadata {
                dependencies: depmap,
                dev_dependencies: BTreeMap::new(),
                system_dependencies: BTreeMap::new(),
                yanked: false,
                revision: None,
                revisions: BTreeMap::new(),
                checksum: None,
                source: None,
                features: cabin_core::Features::default(),
                profiles: None,
                toolchain: None,
                build: None,
                compiler_wrapper: None,
                language: None,
                standards: cabin_index::StandardsMetadata::default(),
                links: BTreeMap::new(),
                upstream: None,
            },
        );
        IndexEntry {
            name: pkg_name(name),
            versions: vmap,
        }
    }

    /// Both sides of a platform split resolve and stay listed as the
    /// parent's dependencies, so the lockfile is the same wherever it
    /// is written.
    #[test]
    fn conditional_registry_dependencies_resolve_for_every_platform() {
        let linux = cabin_core::Condition::parse_cfg(r#"cfg(os = "linux")"#).unwrap();
        let not_linux = cabin_core::Condition::parse_cfg(r#"cfg(not(os = "linux"))"#).unwrap();
        let parent = entry_with_conditional(
            "parent",
            "1.0.0",
            vec![
                ("epoll", ">=1", Some(linux)),
                ("kqueue", ">=1", Some(not_linux)),
            ],
        );
        let epoll = entry("epoll", vec![("1.0.0", vec![], false)]);
        let kqueue = entry("kqueue", vec![("1.0.0", vec![], false)]);
        let index = build_index(vec![parent, epoll, kqueue]);
        let out = resolve(&make_input(vec![("parent", "*")]), &index).unwrap();
        assert_eq!(names(&out), ["epoll", "kqueue", "parent"]);
        assert_eq!(
            selected(&out, "parent")
                .dependencies
                .iter()
                .map(PackageName::as_str)
                .collect::<Vec<_>>(),
            ["epoll", "kqueue"]
        );
    }

    // -----------------------------------------------------------------
    // miette diagnostic rendering
    //
    // Verifies that rendered `ResolveError` diagnostics carry the
    // stable `cabin::resolver::error` code and actionable help text.
    // -----------------------------------------------------------------

    fn render_diagnostic(diag: &dyn miette::Diagnostic) -> String {
        let mut out = String::new();
        let handler =
            miette::GraphicalReportHandler::new_themed(miette::GraphicalTheme::unicode_nocolor())
                .without_cause_chain();
        handler.render_report(&mut out, diag).unwrap();
        out
    }

    #[test]
    fn diagnostic_carries_stable_code_for_unknown_package() {
        let err = ResolveError::UnknownPackage("fmt".into());
        let rendered = render_diagnostic(&err);
        assert!(
            rendered.contains("cabin::resolver::error"),
            "expected diagnostic code, got: {rendered}"
        );
        assert!(
            rendered.contains("fmt"),
            "expected package name in: {rendered}"
        );
    }

    #[test]
    fn diagnostic_help_text_for_lockfile_violations() {
        let err = ResolveError::LockedVersionViolatesConstraint {
            name: "fmt".into(),
            version: "10.0.0".into(),
            constraints: Vec::new(),
        };
        let rendered = render_diagnostic(&err);
        assert!(
            rendered.contains("cabin update"),
            "expected `cabin update` hint in: {rendered}"
        );
    }

    #[test]
    fn conflict_diagnostic_includes_packages_and_explanation() {
        let index = build_index(vec![
            entry("a", vec![("1.0.0", vec![], false)]),
            entry("b", vec![("1.0.0", vec![("a", ">=2, <3")], false)]),
        ]);
        let err = resolve(
            &make_input(vec![("a", ">=1, <2"), ("b", ">=1, <2")]),
            &index,
        )
        .unwrap_err();
        let rendered = render_diagnostic(&err);
        assert!(
            rendered.contains("cabin::resolver::error"),
            "expected diagnostic code in: {rendered}"
        );
        assert!(
            rendered.contains('a') && rendered.contains('b'),
            "expected both packages in: {rendered}"
        );
        assert!(
            rendered.contains(">=2") || rendered.contains(">= 2"),
            "expected the conflicting version requirement in: {rendered}"
        );
        // Collapsed: the unused "no versions" branches are gone.
        assert!(
            !rendered.contains("there is no version"),
            "expected a collapsed explanation in: {rendered}"
        );
    }

    #[test]
    fn rendered_diagnostic_is_color_free() {
        let err = ResolveError::UnknownPackage("fmt".into());
        let rendered = render_diagnostic(&err);
        assert!(
            !rendered.contains('\x1b'),
            "expected no ANSI escape, got: {rendered:?}"
        );
    }

    /// Pins the user-facing rendering of the fail-closed
    /// conversion path: the diagnostic carries the stable
    /// resolver code, names the package and the offending
    /// requirement, and offers the actionable hint.  No mention
    /// of `Ranges`, `PubGrub`, or other implementation details
    /// is allowed to leak through.
    #[test]
    fn unsupported_version_requirement_renders_actionable_diagnostic() {
        let err = ResolveError::UnsupportedVersionRequirement {
            package: "fmt".into(),
            requirement: ">=1.0.0".into(),
        };
        let rendered = render_diagnostic(&err);
        assert!(
            rendered.contains("cabin::resolver::error"),
            "expected stable diagnostic code in: {rendered}"
        );
        assert!(
            rendered.contains("fmt"),
            "expected package name in: {rendered}"
        );
        assert!(
            rendered.contains(">=1.0.0"),
            "expected requirement text in: {rendered}"
        );
        assert!(
            rendered.contains("update Cabin"),
            "expected actionable help in: {rendered}"
        );
        for forbidden in ["Ranges", "PubGrub", "pubgrub"] {
            assert!(
                !rendered.contains(forbidden),
                "diagnostic must not leak {forbidden:?}: {rendered}"
            );
        }
    }

    // -----------------------------------------------------------------
    // Standard-aware version preference (`[resolver]
    // incompatible-standards`).  The post-resolution validation stays
    // the correctness authority; these tests pin the *ordering* and
    // its hold-back reporting.
    // -----------------------------------------------------------------

    use cabin_core::standard_compatibility::ConsumerStandards;
    use cabin_core::{
        CStandard, CxxStandard, IncompatibleStandards, Requirement, StandardsMetadata,
        TargetStandards,
    };

    /// A one-target `standards` table declaring a C++ interface
    /// requirement (the C side left unconstrained).
    fn cxx_table(interface_cxx: Requirement<CxxStandard>) -> StandardsMetadata {
        let mut targets = BTreeMap::new();
        targets.insert(
            "lib".to_owned(),
            TargetStandards {
                interface_cxx,
                ..Default::default()
            },
        );
        StandardsMetadata { targets }
    }

    /// The C sibling of [`cxx_table`].
    fn c_table(interface_c: Requirement<CStandard>) -> StandardsMetadata {
        let mut targets = BTreeMap::new();
        targets.insert(
            "lib".to_owned(),
            TargetStandards {
                interface_c,
                ..Default::default()
            },
        );
        StandardsMetadata { targets }
    }

    fn set_standards(index: &mut PackageIndex, pkg: &str, ver: &str, standards: StandardsMetadata) {
        index
            .packages
            .get_mut(&pkg_name(pkg))
            .unwrap()
            .versions
            .get_mut(&version(ver))
            .unwrap()
            .standards = standards;
    }

    fn set_links(index: &mut PackageIndex, pkg: &str, ver: &str, links: &[(&str, &str)]) {
        index
            .packages
            .get_mut(&pkg_name(pkg))
            .unwrap()
            .versions
            .get_mut(&version(ver))
            .unwrap()
            .links = links
            .iter()
            .map(|(target, id)| ((*target).to_owned(), (*id).to_owned()))
            .collect();
    }

    fn local_claim(package: &str, ver_str: &str, target: &str, links: &str) -> LinksClaim {
        LinksClaim {
            package: pkg_name(package),
            version: version(ver_str),
            target: target.to_owned(),
            links: links.to_owned(),
        }
    }

    // -----------------------------------------------------------------
    // links uniqueness
    // -----------------------------------------------------------------

    /// Two direct dependencies claiming the same identity collide;
    /// the error names the identity and both claimants sorted.
    #[test]
    fn links_collision_between_direct_dependencies_errors() {
        let mut index = build_index(vec![
            entry("zlib", vec![("1.3.1", vec![], false)]),
            entry("zlib-ng", vec![("2.2.0", vec![], false)]),
        ]);
        set_links(&mut index, "zlib", "1.3.1", &[("z", "z")]);
        set_links(&mut index, "zlib-ng", "2.2.0", &[("zng", "z")]);
        let err = resolve(&make_input(vec![("zlib", "*"), ("zlib-ng", "*")]), &index).unwrap_err();
        match err {
            ResolveError::LinksCollision { links, claimants } => {
                assert_eq!(links, "z");
                let rendered: Vec<(String, String)> = claimants
                    .iter()
                    .map(|c| (c.package.as_str().to_owned(), c.target.clone()))
                    .collect();
                assert_eq!(
                    rendered,
                    vec![
                        ("zlib".to_owned(), "z".to_owned()),
                        ("zlib-ng".to_owned(), "zng".to_owned()),
                    ]
                );
            }
            other => panic!("expected LinksCollision, got {other:?}"),
        }
    }

    /// A collision through transitive dependencies errors even when
    /// neither claimant is a direct dependency of the root.
    #[test]
    fn links_collision_between_transitive_dependencies_errors() {
        let mut index = build_index(vec![
            entry("a", vec![("1.0.0", vec![("x", ">=1, <2")], false)]),
            entry("b", vec![("1.0.0", vec![("y", ">=1, <2")], false)]),
            entry("x", vec![("1.0.0", vec![], false)]),
            entry("y", vec![("1.0.0", vec![], false)]),
        ]);
        set_links(&mut index, "x", "1.0.0", &[("x", "sqlite3")]);
        set_links(&mut index, "y", "1.0.0", &[("y", "sqlite3")]);
        let err = resolve(&make_input(vec![("a", "*"), ("b", "*")]), &index).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::LinksCollision { links, .. } if links == "sqlite3"
        ));
    }

    /// With link domains, a build tool's closure and the target
    /// program's closure may each claim an identity once, but one
    /// domain still may not claim it twice - through a transitive
    /// package or against a local claim.
    #[test]
    fn links_uniqueness_applies_per_link_domain() {
        let mut index = build_index(vec![
            entry("gen-support", vec![("1.0.0", vec![("miniz", "*")], false)]),
            entry("miniz", vec![("3.1.2", vec![], false)]),
            entry("zlib", vec![("1.3.1", vec![], false)]),
        ]);
        set_links(&mut index, "zlib", "1.3.1", &[("z", "z")]);
        set_links(&mut index, "miniz", "3.1.2", &[("miniz", "z")]);
        let domain = |roots: &[&str], local_links: Vec<LinksClaim>| LinkDomain {
            roots: roots.iter().map(|name| pkg_name(name)).collect(),
            local_links,
        };
        let mut input = make_input(vec![("zlib", "*"), ("gen-support", "*")]);
        input.link_domains = vec![domain(&["zlib"], vec![]), domain(&["gen-support"], vec![])];
        resolve_packages(&input, &index).expect("separate domains may share an identity");

        input.link_domains = vec![
            domain(&["zlib", "gen-support"], vec![]),
            domain(&["gen-support"], vec![]),
        ];
        let err = resolve_packages(&input, &index).unwrap_err();
        assert!(matches!(err, ResolveError::LinksCollision { links, .. } if links == "z"));

        input.link_domains = vec![
            domain(&["zlib"], vec![]),
            domain(
                &["gen-support"],
                vec![local_claim("gen", "0.1.0", "vendored-z", "z")],
            ),
        ];
        let err = resolve_packages(&input, &index).unwrap_err();
        assert!(matches!(err, ResolveError::LinksCollision { links, .. } if links == "z"));
    }

    /// The check runs on the lean `resolve_packages` path too - the
    /// one build/run/test/vendor use.
    #[test]
    fn links_collision_fires_on_resolve_packages() {
        let mut index = build_index(vec![
            entry("zlib", vec![("1.3.1", vec![], false)]),
            entry("miniz", vec![("3.1.2", vec![], false)]),
        ]);
        set_links(&mut index, "zlib", "1.3.1", &[("z", "z")]);
        set_links(&mut index, "miniz", "3.1.2", &[("miniz", "z")]);
        let err =
            resolve_packages(&make_input(vec![("zlib", "*"), ("miniz", "*")]), &index).unwrap_err();
        assert!(matches!(err, ResolveError::LinksCollision { .. }));
    }

    /// A caller-supplied local claim (workspace member, path dep,
    /// patch) collides with a resolved registry package's claim.
    #[test]
    fn links_collision_between_local_and_registry_claims_errors() {
        let mut index = build_index(vec![entry("zlib", vec![("1.3.1", vec![], false)])]);
        set_links(&mut index, "zlib", "1.3.1", &[("z", "z")]);
        let mut input = make_input(vec![("zlib", "*")]);
        input.local_links = vec![local_claim("app", "0.1.0", "vendored-z", "z")];
        let err = resolve(&input, &index).unwrap_err();
        match err {
            ResolveError::LinksCollision { links, claimants } => {
                assert_eq!(links, "z");
                assert_eq!(claimants.len(), 2);
                assert_eq!(claimants[0].package.as_str(), "app");
                assert_eq!(claimants[1].package.as_str(), "zlib");
            }
            other => panic!("expected LinksCollision, got {other:?}"),
        }
    }

    /// Distinct identities never collide, a sole claimant never
    /// collides, and packages without claims are inert - no false
    /// positives from the mere presence of links metadata.
    #[test]
    fn distinct_links_identities_resolve_cleanly() {
        let mut index = build_index(vec![
            entry("zlib", vec![("1.3.1", vec![], false)]),
            entry("libpng", vec![("1.6.50", vec![("zlib", ">=1, <2")], false)]),
            entry("fmt", vec![("10.2.1", vec![], false)]),
        ]);
        set_links(&mut index, "zlib", "1.3.1", &[("z", "z")]);
        set_links(&mut index, "libpng", "1.6.50", &[("png", "png")]);
        let mut input = make_input(vec![("zlib", "*"), ("libpng", "*"), ("fmt", "*")]);
        input.local_links = vec![local_claim("app", "0.1.0", "app-lib", "app-native")];
        let out = resolve(&input, &index).unwrap();
        assert_eq!(out.packages.len(), 4);
    }

    /// A locally supplied name replaces its index entry outright: the
    /// upstream's claims never count, while the local package's own
    /// claims do.
    #[test]
    fn local_packages_replace_index_claims() {
        let mut index = build_index(vec![
            entry("zlib", vec![("1.3.1", vec![], false)]),
            entry("miniz", vec![("3.1.2", vec![], false)]),
        ]);
        set_links(&mut index, "zlib", "1.3.1", &[("z", "z")]);
        set_links(&mut index, "miniz", "3.1.2", &[("miniz", "z")]);
        let mut input = make_input(vec![("zlib", "*"), ("miniz", "*")]);
        input.local_packages.insert(
            pkg_name("zlib"),
            entry("zlib", vec![("1.3.1", vec![], false)]),
        );
        let out = resolve(&input, &index).expect("the replaced upstream must not claim");
        assert_eq!(out.packages[2].source, ResolvedSource::Local);

        // A claiming local package still collides.
        let mut claiming = entry("zlib", vec![("1.3.1", vec![], false)]);
        claiming.versions.get_mut(&version("1.3.1")).unwrap().links =
            BTreeMap::from([("z".to_owned(), "z".to_owned())]);
        input.local_packages.insert(pkg_name("zlib"), claiming);
        let err = resolve(&input, &index).unwrap_err();
        assert!(matches!(err, ResolveError::LinksCollision { .. }));
    }

    /// A transitively reached local package (a `[patch]` fork) uses its
    /// own edges, so a dependency only the upstream declared is never
    /// selected; and its one version must satisfy every dependent, or
    /// the conflict names it as locally supplied.
    #[test]
    fn transitively_reached_local_packages_use_their_own_edges() {
        let mut index = build_index(vec![
            entry("aaa", vec![("1.0.0", vec![("bbb", ">=1")], false)]),
            entry("bbb", vec![("1.0.0", vec![("ccc", ">=1")], false)]),
            entry("ccc", vec![("1.0.0", vec![], false)]),
            entry("ddd", vec![("1.0.0", vec![], false)]),
        ]);
        set_links(&mut index, "ccc", "1.0.0", &[("ccc", "z")]);
        set_links(&mut index, "ddd", "1.0.0", &[("ddd", "z")]);
        let mut input = make_input(vec![("aaa", "*"), ("ddd", "*")]);
        input.local_packages.insert(
            pkg_name("bbb"),
            entry("bbb", vec![("1.0.0", vec![], false)]),
        );
        let out = resolve(&input, &index).unwrap();
        let names: Vec<(&str, ResolvedSource)> = out.packages[1..]
            .iter()
            .map(|p| (p.name.as_str(), p.source))
            .collect();
        assert_eq!(
            names,
            [
                ("aaa", ResolvedSource::Index),
                ("bbb", ResolvedSource::Local),
                ("ddd", ResolvedSource::Index),
            ]
        );

        let index = build_index(vec![entry(
            "aaa",
            vec![("1.0.0", vec![("bbb", ">=2")], false)],
        )]);
        let mut input = make_input(vec![("aaa", "*")]);
        input.local_packages.insert(
            pkg_name("bbb"),
            entry("bbb", vec![("1.0.0", vec![], false)]),
        );
        let err = resolve(&input, &index).unwrap_err();
        let ResolveError::Conflict { detail, .. } = err else {
            panic!("expected a conflict, got {err:?}");
        };
        assert!(
            detail.contains("so no other version can be selected: bbb 1.0.0"),
            "{detail}"
        );
    }

    /// A local pre-release satisfies only the edges that opt into it,
    /// even when another edge pins it exactly.
    #[test]
    fn local_prerelease_must_match_every_dependent_requirement() {
        let index = build_index(vec![entry(
            "aaa",
            vec![("1.0.0", vec![("bbb", "^1")], false)],
        )]);
        let mut input = make_input(vec![("aaa", "*"), ("ccc", "*")]);
        input.local_packages.insert(
            pkg_name("bbb"),
            entry("bbb", vec![("1.1.0-alpha", vec![], false)]),
        );
        input.local_packages.insert(
            pkg_name("ccc"),
            entry("ccc", vec![("1.0.0", vec![("bbb", "=1.1.0-alpha")], false)]),
        );
        let err = resolve(&input, &index).unwrap_err();
        let ResolveError::Conflict { detail, .. } = err else {
            panic!("expected a conflict, got {err:?}");
        };
        assert!(detail.contains("local bbb 1.1.0-alpha"), "{detail}");
    }

    /// The same package reported through both `local_links` and the
    /// index counts once: over-supplying local claims for a package
    /// the resolver also sees must not self-collide.
    #[test]
    fn duplicate_claims_of_one_package_do_not_self_collide() {
        let mut index = build_index(vec![entry("zlib", vec![("1.3.1", vec![], false)])]);
        set_links(&mut index, "zlib", "1.3.1", &[("z", "z")]);
        let mut input = make_input(vec![("zlib", "*")]);
        input.local_links = vec![local_claim("zlib", "1.3.1", "z", "z")];
        assert!(resolve(&input, &index).is_ok());
    }

    /// The exposed fast-path helper: local-only claim sets are
    /// validated the same way (the CLI calls this when there is
    /// nothing to resolve).
    #[test]
    fn detect_links_collision_validates_local_only_claims() {
        assert!(
            detect_links_collision(&[
                local_claim("member-a", "0.1.0", "z", "z"),
                local_claim("member-b", "0.1.0", "png", "png"),
            ])
            .is_ok()
        );
        let err = detect_links_collision(&[
            local_claim("member-a", "0.1.0", "za", "z"),
            local_claim("member-b", "0.1.0", "zb", "z"),
        ])
        .unwrap_err();
        assert!(matches!(err, ResolveError::LinksCollision { .. }));
    }

    /// The rendered diagnostic carries the stable resolver code, the
    /// shared identity, and every claimant with package, version, and
    /// target - the user must be able to act without a backtrace.
    #[test]
    fn links_collision_diagnostic_names_identity_and_claimants() {
        let err = ResolveError::LinksCollision {
            links: "z".to_owned(),
            claimants: vec![
                local_claim("miniz", "3.1.2", "miniz", "z"),
                local_claim("zlib", "1.3.1", "z", "z"),
            ],
        };
        let rendered = render_diagnostic(&err);
        assert!(
            rendered.contains("cabin::resolver::error"),
            "expected stable code in: {rendered}"
        );
        for needle in ["\"z\"", "zlib v1.3.1", "miniz v3.1.2", "target `miniz`"] {
            assert!(
                rendered.contains(needle),
                "expected {needle:?} in: {rendered}"
            );
        }
    }

    /// A `PreferLocked` request (no lockfile → fresh selection, so the
    /// tier ordering applies) with the given C++ consumer level.
    fn cxx_consumer_input(root_deps: Vec<(&str, &str)>, cxx: CxxStandard) -> ResolveInput {
        let mut input = make_input(root_deps);
        input.consumer_standards = ConsumerStandards {
            c: None,
            cxx: Some(cxx),
        };
        input
    }

    fn fmt_version(out: &ResolveOutput) -> Version {
        out.packages
            .iter()
            .find(|p| p.name.as_str() == "fmt")
            .unwrap()
            .version
            .clone()
    }

    /// Fallback (the default) prefers a declared-compatible older
    /// version over a declared-incompatible newer one, and reports the
    /// hold-back naming the selected version, the newest available, and
    /// the requirement that held it back.
    #[test]
    fn fallback_prefers_declared_compatible_over_newer_incompatible() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.4.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.4.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );

        let out = resolve(
            &cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.4.0"));
        assert_eq!(out.held_back.len(), 1);
        assert_eq!(
            out.held_back[0].message(),
            "fmt v1.4.0 (available: v2.0.0, requires interface c++20 or newer)"
        );
    }

    /// `allow` ignores standards entirely: it selects the newest
    /// admissible version and never reports a hold-back.
    #[test]
    fn allow_ignores_standards() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.4.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.4.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );

        let mut input = cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17);
        input.incompatible_standards = IncompatibleStandards::Allow;
        let out = resolve(&input, &index).unwrap();
        assert_eq!(fmt_version(&out), version("2.0.0"));
        assert!(out.held_back.is_empty());
    }

    /// The `Allow` solve is checked like any resolve: when its newer
    /// pick collides on `links`, `allow` selects nothing, so no newer
    /// pick is reported; a selected version that is itself incompatible
    /// still is, and the valid fallback resolution still succeeds.
    #[test]
    fn held_back_keeps_only_incompatible_selections_when_allow_collides_on_links() {
        let mut index = build_index(vec![
            entry(
                "fmt",
                vec![("2.0.0", vec![], false), ("1.4.0", vec![], false)],
            ),
            entry("zlib", vec![("1.0.0", vec![], false)]),
            entry("a", vec![("1.0.0", vec![], false)]),
        ]);
        set_standards(
            &mut index,
            "a",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.4.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );
        set_links(&mut index, "fmt", "2.0.0", &[("fmt", "z")]);
        set_links(&mut index, "zlib", "1.0.0", &[("z", "z")]);
        let out = resolve(
            &cxx_consumer_input(
                vec![("fmt", "*"), ("zlib", "*"), ("a", "*")],
                CxxStandard::Cxx17,
            ),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.4.0"));
        let rows: Vec<_> = out
            .held_back
            .iter()
            .map(|row| (row.name.as_str(), row.newest.is_none()))
            .collect();
        assert_eq!(rows, [("a", true)], "held_back: {:?}", out.held_back);
    }

    /// A newer pre-release the `Allow` solve picks is no hold-back when
    /// an edge of the fallback solution rejects it: `p 1.0.0`'s `^1.0`
    /// rejects `fmt 1.2.0-beta.3`, though the root's requirement opts in.
    #[test]
    fn held_back_skips_a_prerelease_a_fallback_edge_rejects() {
        let mut index = build_index(vec![
            entry(
                "p",
                vec![
                    ("2.0.0", vec![], false),
                    ("1.0.0", vec![("fmt", "^1.0")], false),
                ],
            ),
            entry(
                "fmt",
                vec![("1.2.0-beta.3", vec![], false), ("1.1.0", vec![], false)],
            ),
        ]);
        for (pkg, ver, level) in [
            ("p", "2.0.0", CxxStandard::Cxx20),
            ("p", "1.0.0", CxxStandard::Cxx17),
            ("fmt", "1.2.0-beta.3", CxxStandard::Cxx20),
            ("fmt", "1.1.0", CxxStandard::Cxx17),
        ] {
            set_standards(&mut index, pkg, ver, cxx_table(Requirement::Min(level)));
        }
        let out = resolve(
            &cxx_consumer_input(
                vec![("p", "*"), ("fmt", ">=1.1.0, <=1.2.0-beta.9")],
                CxxStandard::Cxx17,
            ),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.1.0"));
        assert!(
            out.held_back.iter().all(|row| row.name.as_str() != "fmt"),
            "held_back: {:?}",
            out.held_back
        );
    }

    /// `resolve_packages` selects identically to `resolve` (same
    /// `Fallback` tiering) but skips the second `Allow`-mode solve, so
    /// `held_back` is empty even where `resolve` would report a hold.
    #[test]
    fn resolve_packages_skips_the_hold_back_report() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.4.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.4.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );

        let input = cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17);
        let reported = resolve(&input, &index).unwrap();
        assert_eq!(reported.held_back.len(), 1, "sanity: resolve reports it");

        let lean = resolve_packages(&input, &index).unwrap();
        assert_eq!(lean.packages, reported.packages, "same selection");
        assert!(lean.held_back.is_empty(), "held_back: {:?}", lean.held_back);
    }

    /// `ResolveInput::new` defaults to `fallback`, so a fresh request
    /// (no explicit knob) already orders by standard compatibility.
    #[test]
    fn default_mode_is_fallback() {
        assert_eq!(
            ResolveInput::new(pkg_name("app"), version("0.1.0"), BTreeMap::new())
                .incompatible_standards,
            IncompatibleStandards::Fallback
        );
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.4.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.4.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );
        let out = resolve(
            &cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.4.0"));
    }

    /// Tier ordering over a mixed set: declared-compatible (tier 1)
    /// beats undeclared (tier 2) beats declared-incompatible (tier 3),
    /// even when a lower tier holds a newer version.
    #[test]
    fn tier_ordering_over_mixed_candidate_set() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![
                ("3.0.0", vec![], false),
                ("2.0.0", vec![], false),
                ("1.0.0", vec![], false),
            ],
        )]);
        // 3.0.0 declared-incompatible, 2.0.0 undeclared (empty table),
        // 1.0.0 declared-compatible.
        set_standards(
            &mut index,
            "fmt",
            "3.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );

        let out = resolve(
            &cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.0.0"));
        // The newest (3.0.0) is declared-incompatible, so it is named.
        assert_eq!(
            out.held_back[0].message(),
            "fmt v1.0.0 (available: v3.0.0, requires interface c++20 or newer)"
        );
    }

    /// When no candidate is compatible, the newest is selected (the
    /// post-resolution validation refuses it) and the selected version's
    /// own unmet requirement is reported, with no compatible alternative
    /// to name.
    #[test]
    fn no_compatible_candidate_reports_selected_incompatibility() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx23)),
        );

        let out = resolve(
            &cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        // Both are declared-incompatible; newest-first within the worst
        // tier picks 2.0.0, which is also the semver newest.
        assert_eq!(fmt_version(&out), version("2.0.0"));
        assert_eq!(out.held_back.len(), 1);
        assert_eq!(out.held_back[0].newest, None);
        assert_eq!(
            out.held_back[0].message(),
            "fmt v2.0.0 (requires interface c++20 or newer; no compatible version in range)"
        );
    }

    /// A declared-compatible older version outranks an undeclared newer
    /// one (tier 1 over tier 2), and the resulting downgrade is reported
    /// with no requirement to cite, since the newer version is
    /// compatible, just undeclared.
    #[test]
    fn undeclared_newer_version_downgrade_is_reported() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
        )]);
        // 2.0.0 undeclared, 1.0.0 declared-compatible.
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );
        let out = resolve(
            &cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.0.0"));
        assert_eq!(out.held_back.len(), 1);
        assert_eq!(out.held_back[0].newest, Some(version("2.0.0")));
        assert!(out.held_back[0].blocked_by.is_empty());
        assert_eq!(
            out.held_back[0].message(),
            "fmt v1.0.0 (available: v2.0.0, preferred as declared-compatible over the undeclared newer version)"
        );
    }

    /// Solvability is identical under both knob values: preference mode
    /// never introduces a resolution failure `allow` would not also
    /// produce, and never solves a case `allow` cannot.  Here fallback
    /// changes the *selection* (to an older compatible version) but
    /// both values resolve successfully.
    #[test]
    fn solvability_identical_under_both_values() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );

        let fallback = cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17);
        let mut allow = fallback.clone();
        allow.incompatible_standards = IncompatibleStandards::Allow;

        let fallback_out = resolve(&fallback, &index);
        let allow_out = resolve(&allow, &index);
        assert_eq!(fallback_out.is_ok(), allow_out.is_ok());
        let (fallback_out, allow_out) = (fallback_out.unwrap(), allow_out.unwrap());
        // Same packages resolved (solvability + graph shape), different
        // versions (the whole point of fallback).
        let names = |o: &ResolveOutput| {
            o.packages
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&fallback_out), names(&allow_out));
        assert_eq!(fmt_version(&fallback_out), version("1.0.0"));
        assert_eq!(fmt_version(&allow_out), version("2.0.0"));
    }

    /// Under `allow`, selection is a pure function of semver: changing
    /// the consumer standard does not move the selection, so a lockfile
    /// written under one workspace standard stays valid under another.
    #[test]
    fn lockfile_stable_under_allow_when_consumer_standard_changes() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );

        let make_allow = |cxx: CxxStandard| {
            let mut input = cxx_consumer_input(vec![("fmt", "*")], cxx);
            input.incompatible_standards = IncompatibleStandards::Allow;
            input
        };
        let low = resolve(&make_allow(CxxStandard::Cxx17), &index).unwrap();
        let high = resolve(&make_allow(CxxStandard::Cxx23), &index).unwrap();
        assert_eq!(low, high);
        assert_eq!(fmt_version(&low), version("2.0.0"));
    }

    /// Lockfile stability wins even in `fallback`: a locked
    /// incompatible version is kept over a declared-compatible
    /// alternative (metadata alone never churns a lockfile), and it is
    /// not reported as a hold-back.
    #[test]
    fn fallback_keeps_locked_version_over_compatible_alternative() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );

        let mut input = cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17);
        input.locked.insert(
            pkg_name("fmt"),
            LockedVersion {
                version: version("2.0.0"),
                checksum: None,
            },
        );
        let out = resolve(&input, &index).unwrap();
        assert_eq!(fmt_version(&out), version("2.0.0"));
        assert!(out.held_back.is_empty());
    }

    /// Fallback is deterministic for fixed inputs.
    #[test]
    fn fallback_is_deterministic() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![
                ("3.0.0", vec![], false),
                ("2.0.0", vec![], false),
                ("1.0.0", vec![], false),
            ],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "3.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );
        let input = cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17);
        let first = resolve(&input, &index).unwrap();
        let second = resolve(&input, &index).unwrap();
        assert_eq!(first, second);
    }

    /// The C sibling of the primary fallback test: a C consumer holds
    /// back a version whose `interface-c-standard` it cannot satisfy,
    /// and the message names the C level.
    #[test]
    fn fallback_prefers_declared_compatible_c() {
        let mut index = build_index(vec![entry(
            "clib",
            vec![("2.0.0", vec![], false), ("1.4.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "clib",
            "2.0.0",
            c_table(Requirement::Min(CStandard::C17)),
        );
        set_standards(
            &mut index,
            "clib",
            "1.4.0",
            c_table(Requirement::Min(CStandard::C11)),
        );

        let mut input = make_input(vec![("clib", "*")]);
        input.consumer_standards = ConsumerStandards {
            c: Some(CStandard::C11),
            cxx: None,
        };
        let out = resolve(&input, &index).unwrap();
        let clib = out
            .packages
            .iter()
            .find(|p| p.name.as_str() == "clib")
            .unwrap();
        assert_eq!(clib.version, version("1.4.0"));
        assert_eq!(
            out.held_back[0].message(),
            "clib v1.4.0 (available: v2.0.0, requires interface c17 or newer)"
        );
    }

    /// A newer version that is out of range in the final solution is
    /// never reported as a standard-caused hold, even if it was
    /// standard-incompatible while transiently in range: `spd 1.0.0`
    /// constrains `fmt < 2`, so `fmt 2.0.0` is a semver exclusion, not
    /// a standard hold-back - regardless of the order the solver
    /// decided the two packages.
    #[test]
    fn out_of_range_newer_version_is_not_reported_as_standard_hold() {
        let mut index = build_index(vec![
            entry(
                "fmt",
                vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
            ),
            entry("spd", vec![("1.0.0", vec![("fmt", ">=1, <2")], false)]),
        ]);
        // fmt 2.0.0 would be standard-incompatible for a c++17 consumer,
        // but `spd`'s `< 2` constraint excludes it on semver grounds.
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );
        let out = resolve(
            &cxx_consumer_input(vec![("fmt", ">=1"), ("spd", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.0.0"));
        assert!(out.held_back.is_empty(), "held_back: {:?}", out.held_back);
    }

    /// A newer version that is standard-incompatible *and* cannot itself
    /// resolve (an unsatisfiable transitive dependency) is not reported
    /// as a standard hold-back: `allow` would also backtrack to the
    /// older version for dependency reasons, so the diff against the
    /// `allow` solution shows no standard-caused hold.
    #[test]
    fn unsolvable_newer_version_is_not_reported_as_standard_hold() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![
                // 2.0.0 is standard-incompatible for a c++17 consumer and
                // depends on a package that is absent from the index, so
                // it can never participate in a solution.
                ("2.0.0", vec![("vanished", ">=1")], false),
                ("1.0.0", vec![], false),
            ],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );
        let out = resolve(
            &cxx_consumer_input(vec![("fmt", ">=1")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.0.0"));
        assert!(out.held_back.is_empty(), "held_back: {:?}", out.held_back);
    }

    /// When `fallback` and `allow` diverge at a parent, the child's
    /// version difference is a consequence of the parent choice, not a
    /// standard hold on the child.  `foo 2.0.0` (c++20) needs
    /// `bar >= 2`; `foo 1.0.0` (c++17) needs `bar < 2`.  Fallback picks
    /// `foo 1.0.0`/`bar 1.0.0`; allow picks `foo 2.0.0`/`bar 2.0.0`.
    /// Only `foo` is held back for standards - `bar 2.0.0` is out of
    /// range under the selected `foo 1.0.0`, so `bar` must not be
    /// reported.
    #[test]
    fn divergent_parent_does_not_hold_back_the_child() {
        let mut index = build_index(vec![
            entry(
                "foo",
                vec![
                    ("2.0.0", vec![("bar", ">=2, <3")], false),
                    ("1.0.0", vec![("bar", ">=1, <2")], false),
                ],
            ),
            entry(
                "bar",
                vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
            ),
        ]);
        set_standards(
            &mut index,
            "foo",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "foo",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );
        set_standards(
            &mut index,
            "bar",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "bar",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );

        let out = resolve(
            &cxx_consumer_input(vec![("foo", ">=1")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        let versions: BTreeMap<&str, String> = out
            .packages
            .iter()
            .map(|p| (p.name.as_str(), p.version.to_string()))
            .collect();
        assert_eq!(versions["foo"], "1.0.0");
        assert_eq!(versions["bar"], "1.0.0");
        assert_eq!(out.held_back.len(), 1, "held_back: {:?}", out.held_back);
        assert_eq!(
            out.held_back[0].message(),
            "foo v1.0.0 (available: v2.0.0, requires interface c++20 or newer)"
        );
    }

    /// The root package is never reported as held back, even when the
    /// index coincidentally contains an entry with its name and version
    /// carrying standards metadata: the root is the local project, not a
    /// registry selection.
    #[test]
    fn root_package_is_never_reported_as_held_back() {
        let mut index = build_index(vec![
            entry("app", vec![("0.1.0", vec![], false)]),
            entry("fmt", vec![("1.0.0", vec![], false)]),
        ]);
        // The index coincidentally declares c++20 for `app 0.1.0`, which
        // is also the root's identity.
        set_standards(
            &mut index,
            "app",
            "0.1.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        let out = resolve(
            &cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert!(
            out.held_back.iter().all(|held| held.name.as_str() != "app"),
            "root must not be held back: {:?}",
            out.held_back
        );
    }

    /// A registry entry that coincidentally shares the root's identity
    /// must not inject its dependency constraints into the fallback
    /// admissibility check and suppress a real hold-back: the root's
    /// actual requirements come from the manifest, not the index.
    #[test]
    fn root_index_entry_does_not_suppress_a_real_holdback() {
        let mut index = build_index(vec![
            // Coincidental `app 0.1.0` in the index declaring a narrower
            // `fmt` requirement than the local manifest allows.
            entry("app", vec![("0.1.0", vec![("fmt", ">=1, <2")], false)]),
            entry(
                "fmt",
                vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
            ),
        ]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );
        // The local manifest allows `fmt *`, so `fmt 2.0.0` is in range
        // and its c++20 hold-back is real.
        let out = resolve(
            &cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.0.0"));
        assert_eq!(out.held_back.len(), 1, "held_back: {:?}", out.held_back);
        assert_eq!(out.held_back[0].name.as_str(), "fmt");
        assert_eq!(out.held_back[0].newest, Some(version("2.0.0")));
    }

    /// A `"none"` (forbidden) interface on the newer version is
    /// reported with the declared-`"none"` wording rather than a
    /// minimum level.
    #[test]
    fn holdback_message_for_forbidden_interface() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::Forbidden),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx17)),
        );
        let out = resolve(
            &cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.0.0"));
        assert_eq!(
            out.held_back[0].message(),
            "fmt v1.0.0 (available: v2.0.0, not consumable from c++)"
        );
    }

    /// A bounded interface on the newer version holds it back for a
    /// consumer above the cap, and the message renders the full
    /// accepted range - preference mode understands that raising the
    /// consumer cannot fix a capped candidate.
    #[test]
    fn holdback_message_for_bounded_interface_above_the_cap() {
        let mut index = build_index(vec![entry(
            "fmt",
            vec![("2.0.0", vec![], false), ("1.0.0", vec![], false)],
        )]);
        set_standards(
            &mut index,
            "fmt",
            "2.0.0",
            cxx_table(Requirement::bounded(CxxStandard::Cxx11, CxxStandard::Cxx14).unwrap()),
        );
        set_standards(
            &mut index,
            "fmt",
            "1.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx11)),
        );
        let out = resolve(
            &cxx_consumer_input(vec![("fmt", "*")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(fmt_version(&out), version("1.0.0"));
        assert_eq!(
            out.held_back[0].message(),
            "fmt v1.0.0 (available: v2.0.0, requires interface c++11..c++14)"
        );
    }

    // -----------------------------------------------------------------
    // Feature-aware resolution
    // -----------------------------------------------------------------

    fn edge(dep_req: &str) -> cabin_index::IndexPackageDependency {
        cabin_index::IndexPackageDependency {
            req: req(dep_req),
            optional: false,
            features: Vec::new(),
            default_features: true,
            condition: None,
        }
    }

    fn optional(dep_req: &str) -> cabin_index::IndexPackageDependency {
        cabin_index::IndexPackageDependency {
            optional: true,
            ..edge(dep_req)
        }
    }

    /// One version with its dependency edges, `default` list, and
    /// `[features]` table.
    fn featured_version(
        deps: Vec<(&str, cabin_index::IndexPackageDependency)>,
        default: &[&str],
        features: &[(&str, &[&str])],
    ) -> VersionMetadata {
        VersionMetadata {
            dependencies: deps
                .into_iter()
                .map(|(name, dep)| (pkg_name(name), dep))
                .collect(),
            features: cabin_core::Features {
                default: default.iter().map(|f| (*f).to_owned()).collect(),
                features: features
                    .iter()
                    .map(|(name, entries)| {
                        (
                            (*name).to_owned(),
                            entries.iter().map(|e| (*e).to_owned()).collect(),
                        )
                    })
                    .collect(),
            },
            ..VersionMetadata::default()
        }
    }

    fn featured(name: &str, versions: Vec<(&str, VersionMetadata)>) -> IndexEntry {
        IndexEntry {
            name: pkg_name(name),
            versions: versions
                .into_iter()
                .map(|(ver, meta)| (version(ver), meta))
                .collect(),
        }
    }

    fn plain(name: &str) -> IndexEntry {
        featured(name, vec![("1.0.0", featured_version(vec![], &[], &[]))])
    }

    fn request(input: &mut ResolveInput, name: &str, features: &[&str]) {
        input
            .root_features
            .insert(pkg_name(name), set(features.iter().copied()));
    }

    fn set<'a>(items: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
        items.into_iter().map(str::to_owned).collect()
    }

    fn names(out: &ResolveOutput) -> Vec<&str> {
        out.packages[1..].iter().map(|p| p.name.as_str()).collect()
    }

    fn selected<'a>(out: &'a ResolveOutput, name: &str) -> &'a ResolvedPackage {
        out.packages
            .iter()
            .find(|p| p.name.as_str() == name)
            .unwrap()
    }

    /// A respin whose default feature turns on a new optional
    /// dependency: a lockfile pinning the revision before it resolves
    /// that revision's graph in every mode that keeps the pin, and the
    /// update modes resolve the current revision's.
    #[test]
    fn kept_pins_resolve_their_own_revisions_metadata() {
        let mut index = build_index(vec![
            featured(
                "fmt",
                vec![(
                    "10.2.1",
                    featured_version(vec![("b", optional("^1"))], &["b"], &[("b", &["dep:b"])]),
                )],
            ),
            plain("b"),
        ]);
        inject_revisions(&mut index, "fmt", &[REV_A, REV_B]);
        let meta = index.packages.get_mut(&pkg_name("fmt")).unwrap();
        let meta = meta.versions.get_mut(&version("10.2.1")).unwrap();
        meta.revisions.get_mut(&REV_A[..16]).unwrap().resolution = Some(Box::default());
        for (mode, takes_b) in [
            (ResolveMode::PreferLocked, false),
            (ResolveMode::Locked, false),
            (ResolveMode::UpdatePackage(pkg_name("fmt")), true),
            (ResolveMode::UpdateAll, true),
        ] {
            let mut input =
                input_with_locked(vec![("fmt", "*")], vec![("fmt", "10.2.1")], mode.clone());
            input.locked.get_mut(&pkg_name("fmt")).unwrap().checksum =
                Some(cabin_core::Checksum::parse(&format!("sha256:{REV_A}")).unwrap());
            request(&mut input, "fmt", &["default"]);
            let mut index = index.clone();
            select_locked_revisions(&input, &mut index);
            let out = resolve(&input, &index).unwrap();
            assert_eq!(names(&out).contains(&"b"), takes_b, "{mode:?}");
        }
    }

    /// A -> B -> C across registry packages: a root request for `a`'s
    /// `x` requests `b/y`, whose `dep:c` turns on the optional `c`;
    /// the reported features and active edges follow.
    #[test]
    fn feature_chain_activates_optional_registry_edges() {
        let index = build_index(vec![
            featured(
                "a",
                vec![(
                    "1.0.0",
                    featured_version(vec![("b", edge("^1"))], &[], &[("x", &["b/y"])]),
                )],
            ),
            featured(
                "b",
                vec![(
                    "1.0.0",
                    featured_version(vec![("c", optional("^1"))], &[], &[("y", &["dep:c"])]),
                )],
            ),
            plain("c"),
        ]);
        let mut input = make_input(vec![("a", "^1")]);
        assert_eq!(names(&resolve(&input, &index).unwrap()), ["a", "b"]);

        request(&mut input, "a", &["x"]);
        let out = resolve(&input, &index).unwrap();
        assert_eq!(names(&out), ["a", "b", "c"]);
        assert_eq!(selected(&out, "a").features, set(["x"]));
        assert_eq!(selected(&out, "b").features, set(["y"]));
        assert_eq!(
            selected(&out, "b").dependencies,
            BTreeSet::from([pkg_name("c")])
        );
    }

    /// Default features are additive per edge: `left` turns them off
    /// on `shared`, `right` keeps them, and the union enables them.
    #[test]
    fn default_features_are_additive_across_a_diamond() {
        let no_defaults = cabin_index::IndexPackageDependency {
            default_features: false,
            ..edge("^1")
        };
        let index = build_index(vec![
            featured(
                "left",
                vec![(
                    "1.0.0",
                    featured_version(vec![("shared", no_defaults)], &[], &[]),
                )],
            ),
            featured(
                "right",
                vec![(
                    "1.0.0",
                    featured_version(vec![("shared", edge("^1"))], &[], &[]),
                )],
            ),
            featured(
                "shared",
                vec![(
                    "1.0.0",
                    featured_version(
                        vec![("extra", optional("^1"))],
                        &["full"],
                        &[("full", &["dep:extra"])],
                    ),
                )],
            ),
            plain("extra"),
        ]);
        let out = resolve(&make_input(vec![("left", "^1")]), &index).unwrap();
        assert_eq!(names(&out), ["left", "shared"]);

        let out = resolve(&make_input(vec![("left", "^1"), ("right", "^1")]), &index).unwrap();
        assert_eq!(names(&out), ["extra", "left", "right", "shared"]);
        assert_eq!(selected(&out, "shared").features, set(["full"]));
    }

    /// A requested feature selects the newest version declaring it,
    /// not the newest version.
    #[test]
    fn feature_request_selects_an_older_version_declaring_it() {
        let index = build_index(vec![featured(
            "d",
            vec![
                ("1.0.0", featured_version(vec![], &[], &[("f", &[])])),
                ("2.0.0", featured_version(vec![], &[], &[])),
            ],
        )]);
        let mut input = make_input(vec![("d", ">=1")]);
        let out = resolve(&input, &index).unwrap();
        assert_eq!(selected(&out, "d").version, version("2.0.0"));

        request(&mut input, "d", &["f"]);
        let out = resolve(&input, &index).unwrap();
        assert_eq!(selected(&out, "d").version, version("1.0.0"));
    }

    /// An optional edge constrains selection once a feature enables
    /// it: the newest `a` would enable an incompatible `c`, so
    /// resolution backtracks to the `a` whose edge fits.
    #[test]
    fn enabled_optional_edges_backtrack() {
        let index = build_index(vec![
            featured(
                "a",
                vec![
                    (
                        "1.0.0",
                        featured_version(vec![("c", optional("^1"))], &[], &[("x", &["dep:c"])]),
                    ),
                    (
                        "2.0.0",
                        featured_version(vec![("c", optional("^2"))], &[], &[("x", &["dep:c"])]),
                    ),
                ],
            ),
            featured(
                "c",
                vec![
                    ("1.0.0", featured_version(vec![], &[], &[])),
                    ("2.0.0", featured_version(vec![], &[], &[])),
                ],
            ),
        ]);
        let mut input = make_input(vec![("a", "*"), ("c", "^1")]);
        let out = resolve(&input, &index).unwrap();
        assert_eq!(selected(&out, "a").version, version("2.0.0"));

        request(&mut input, "a", &["x"]);
        let out = resolve(&input, &index).unwrap();
        assert_eq!(selected(&out, "a").version, version("1.0.0"));
        assert_eq!(selected(&out, "c").version, version("1.0.0"));
    }

    /// Feature entries naming a dependency gated to another platform
    /// resolve it like any other: the lockfile serves every platform.
    #[test]
    fn features_enable_dependencies_gated_to_any_platform() {
        let gated = cabin_index::IndexPackageDependency {
            condition: Some(cabin_core::Condition::parse_cfg(r#"cfg(os = "plan9")"#).unwrap()),
            ..optional("^1")
        };
        let index = build_index(vec![
            featured(
                "a",
                vec![(
                    "1.0.0",
                    featured_version(vec![("d", gated)], &[], &[("x", &["dep:d", "d/y"])]),
                )],
            ),
            featured(
                "d",
                vec![("1.0.0", featured_version(vec![], &[], &[("y", &[])]))],
            ),
        ]);
        let mut input = make_input(vec![("a", "^1")]);
        request(&mut input, "a", &["x"]);
        let out = resolve(&input, &index).unwrap();
        assert_eq!(names(&out), ["a", "d"]);
        assert_eq!(selected(&out, "d").features, set(["y"]));
    }

    /// A version the fallback graph's feature requests rule out is not
    /// a standard hold-back: `b` 2.0.0 lacks the `x` that `a` 1.0.0
    /// requests, so only `a`'s row is real.
    #[test]
    fn held_back_skips_versions_lacking_an_enabled_feature() {
        let with_x = cabin_index::IndexPackageDependency {
            features: vec!["x".to_owned()],
            ..edge(">=1")
        };
        let mut index = build_index(vec![
            featured(
                "a",
                vec![
                    ("1.0.0", featured_version(vec![("b", with_x)], &[], &[])),
                    (
                        "2.0.0",
                        featured_version(vec![("b", edge(">=1"))], &[], &[]),
                    ),
                ],
            ),
            featured(
                "b",
                vec![
                    ("1.0.0", featured_version(vec![], &[], &[("x", &[])])),
                    ("2.0.0", featured_version(vec![], &[], &[])),
                ],
            ),
        ]);
        set_standards(
            &mut index,
            "a",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        let out = resolve(
            &cxx_consumer_input(vec![("a", ">=1")], CxxStandard::Cxx17),
            &index,
        )
        .unwrap();
        assert_eq!(selected(&out, "b").version, version("1.0.0"));
        let rows: Vec<&str> = out.held_back.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(rows, ["a"]);
    }

    /// A `dep/feature` entry in an enabled feature is a request too:
    /// `a` 1.0.0's `f` asks for `b/x`, which `b` 2.0.0 lacks.
    #[test]
    fn held_back_skips_versions_lacking_a_feature_table_request() {
        let a =
            |entries: &[&str]| featured_version(vec![("b", edge(">=1"))], &[], &[("f", entries)]);
        let mut index = build_index(vec![
            featured("a", vec![("1.0.0", a(&["b/x"])), ("2.0.0", a(&[]))]),
            featured(
                "b",
                vec![
                    ("1.0.0", featured_version(vec![], &[], &[("x", &[])])),
                    ("2.0.0", featured_version(vec![], &[], &[])),
                ],
            ),
        ]);
        set_standards(
            &mut index,
            "a",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        let mut input = cxx_consumer_input(vec![("a", ">=1")], CxxStandard::Cxx17);
        request(&mut input, "a", &["f"]);
        let out = resolve(&input, &index).unwrap();
        assert_eq!(selected(&out, "b").version, version("1.0.0"));
        let rows: Vec<&str> = out.held_back.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(rows, ["a"]);
    }

    /// A hold-back survives a default feature the newer version drops:
    /// `b` 1.0.0 enables its `legacy` default, which nothing requests,
    /// so `b` 2.0.0 is held back by its standard alone.
    #[test]
    fn held_back_ignores_default_features_the_newer_version_drops() {
        let mut index = build_index(vec![featured(
            "b",
            vec![
                (
                    "1.0.0",
                    featured_version(vec![], &["legacy"], &[("legacy", &[])]),
                ),
                ("2.0.0", featured_version(vec![], &[], &[])),
            ],
        )]);
        set_standards(
            &mut index,
            "b",
            "2.0.0",
            cxx_table(Requirement::Min(CxxStandard::Cxx20)),
        );
        let mut input = cxx_consumer_input(vec![("b", ">=1")], CxxStandard::Cxx17);
        request(&mut input, "b", &["default"]);
        let out = resolve(&input, &index).unwrap();
        assert_eq!(selected(&out, "b").features, set(["legacy"]));
        let rows: Vec<&str> = out.held_back.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(rows, ["b"]);
    }

    /// Feature strings on index dependency edges reach conflict
    /// diagnostics escaped.
    #[test]
    fn conflict_detail_escapes_feature_names() {
        let hostile = cabin_index::IndexPackageDependency {
            features: vec!["no\u{1b}[2Jpe".to_owned()],
            ..edge("^1")
        };
        let index = build_index(vec![
            featured(
                "a",
                vec![("1.0.0", featured_version(vec![("b", hostile)], &[], &[]))],
            ),
            plain("b"),
        ]);
        let err = resolve(&make_input(vec![("a", "^1")]), &index).unwrap_err();
        let ResolveError::Conflict { detail, .. } = err else {
            panic!("expected a conflict, got {err:?}");
        };
        assert!(detail.contains("b[no\\u{1b}[2Jpe]"), "{detail}");
        assert!(!detail.contains('\u{1b}'), "{detail}");
    }

    /// A root request no in-range version declares is a targeted error;
    /// a transitive one is a conflict naming the feature node.
    #[test]
    fn unknown_features_are_reported() {
        let index = build_index(vec![
            featured(
                "a",
                vec![(
                    "1.0.0",
                    featured_version(vec![("b", edge("^1"))], &[], &[("x", &["b/nope"])]),
                )],
            ),
            plain("b"),
        ]);
        let mut input = make_input(vec![("a", "^1")]);
        request(&mut input, "a", &["nope"]);
        let err = resolve(&input, &index).unwrap_err();
        assert!(
            matches!(&err, ResolveError::UnknownFeature { package, feature }
                if package == "a" && feature == "nope"),
            "{err:?}"
        );

        request(&mut input, "a", &["x"]);
        let err = resolve(&input, &index).unwrap_err();
        let ResolveError::Conflict { detail, .. } = err else {
            panic!("expected a conflict, got {err:?}");
        };
        assert!(detail.contains("b[nope]"), "{detail}");
    }

    /// `--locked` replays the locked version and refuses a feature
    /// request it cannot satisfy instead of moving the lock.
    #[test]
    fn locked_replay_checks_requested_features() {
        let index = build_index(vec![featured(
            "d",
            vec![
                ("1.0.0", featured_version(vec![], &[], &[("f", &[])])),
                ("2.0.0", featured_version(vec![], &[], &[])),
            ],
        )]);
        let mut input = input_with_locked(
            vec![("d", ">=1")],
            vec![("d", "1.0.0")],
            ResolveMode::Locked,
        );
        request(&mut input, "d", &["f"]);
        let out = resolve(&input, &index).unwrap();
        assert_eq!(selected(&out, "d").version, version("1.0.0"));

        let mut input = input_with_locked(
            vec![("d", ">=1")],
            vec![("d", "2.0.0")],
            ResolveMode::Locked,
        );
        request(&mut input, "d", &["f"]);
        let err = resolve(&input, &index).unwrap_err();
        assert!(
            matches!(err, ResolveError::LockedVersionLacksFeature { .. }),
            "{err:?}"
        );
    }

    /// `--locked` checks the edge a forwarded feature of a non-optional
    /// dependency travels, and cites it in the violation.
    #[test]
    fn locked_mode_names_the_dependency_a_forwarded_feature_reaches() {
        let x = featured_version(vec![], &[], &[("x", &[])]);
        let index = build_index(vec![
            featured(
                "b",
                vec![(
                    "1.0.0",
                    featured_version(vec![("fmt", edge("^1.0"))], &[], &[("f", &["fmt/x"])]),
                )],
            ),
            featured("fmt", vec![("1.0.0", x.clone()), ("1.2.0-beta.3", x)]),
        ]);
        let mut input = input_with_locked(
            vec![("b", "*")],
            vec![("b", "1.0.0"), ("fmt", "1.2.0-beta.3")],
            ResolveMode::Locked,
        );
        request(&mut input, "b", &["f"]);
        let err = resolve(&input, &index).unwrap_err();
        let ResolveError::LockedVersionViolatesConstraint {
            name, constraints, ..
        } = &err
        else {
            panic!("expected a violation, got {err:?}");
        };
        assert_eq!(name, "fmt");
        assert!(
            constraints
                .iter()
                .any(|c| c.origin.as_str() == "b" && c.requirement.to_string() == "^1.0"),
            "{err:?}"
        );
    }

    /// A lazily loaded index is asked for exactly the metadata the
    /// active graph needs: a disabled optional dependency never is.
    #[test]
    fn lazy_index_requests_only_enabled_optional_metadata() {
        let index = build_index(vec![featured(
            "a",
            vec![(
                "1.0.0",
                featured_version(vec![("d", optional("^1"))], &[], &[("x", &["dep:d"])]),
            )],
        )]);
        let mut input = make_input(vec![("a", "^1")]);
        input.lazy_index = true;
        assert_eq!(names(&resolve(&input, &index).unwrap()), ["a"]);

        request(&mut input, "a", &["x"]);
        let err = resolve(&input, &index).unwrap_err();
        assert!(
            matches!(&err, ResolveError::MetadataNeeded(names) if names.iter().map(PackageName::as_str).eq(["d"])),
            "{err:?}"
        );
    }

    /// A round that backtracks away from an unfetched package still
    /// stops for it.  With every document, `b 1.0.0` loses to `b 1.1.0`
    /// and its optional `u`; deciding `u` last, the first round would
    /// otherwise settle on `a 1.1.0` and `b 1.0.0` without ever reading `u`.
    #[test]
    fn lazy_index_stops_for_an_unfetched_package_the_solution_avoids() {
        let with_f = |deps| featured_version(deps, &[], &[("f", &[])]);
        let index = build_index(vec![
            featured(
                "a",
                vec![
                    ("1.0.0", with_f(vec![])),
                    ("1.1.0", with_f(vec![("b", edge("=1.0.0"))])),
                ],
            ),
            featured(
                "b",
                vec![
                    ("1.0.0", with_f(vec![])),
                    (
                        "1.1.0",
                        featured_version(vec![("u", optional("^1"))], &[], &[("f", &["dep:u"])]),
                    ),
                    (
                        "1.2.0",
                        featured_version(
                            vec![("z", optional("=1.2.0"))],
                            &[],
                            &[("f", &["dep:z"])],
                        ),
                    ),
                ],
            ),
        ]);
        let mut input = make_input(vec![("a", "^1"), ("b", "^1")]);
        input.lazy_index = true;
        request(&mut input, "a", &["f"]);
        request(&mut input, "b", &["f"]);
        let err = resolve(&input, &index).unwrap_err();
        assert!(
            matches!(&err, ResolveError::MetadataNeeded(names) if names.contains(&pkg_name("u"))),
            "{err:?}"
        );
    }

    /// A pure workspace's synthetic root name is never fetched, so a
    /// lazy round reports it as reserved rather than asking for it.
    #[test]
    fn lazy_index_reports_the_reserved_root_name() {
        let index = build_index(vec![featured(
            "a",
            vec![(
                "1.0.0",
                featured_version(vec![("app", edge("^0.1"))], &[], &[]),
            )],
        )]);
        let mut input = make_input(vec![("a", "^1")]);
        input.lazy_index = true;
        let err = resolve(&input, &index).unwrap_err();
        assert!(matches!(err, ResolveError::ReservedRootName(_)), "{err:?}");
    }

    /// A round stops for every unfetched package it reached, not one
    /// at a time: optional dependencies of two packages, each enabled
    /// by a feature, are both requested.
    #[test]
    fn lazy_index_requests_every_unfetched_package_a_round_reaches() {
        let index = build_index(vec![
            featured(
                "a",
                vec![(
                    "1.0.0",
                    featured_version(vec![("d", optional("^1"))], &[], &[("x", &["dep:d"])]),
                )],
            ),
            featured(
                "b",
                vec![(
                    "1.0.0",
                    featured_version(vec![("e", optional("^1"))], &[], &[("y", &["dep:e"])]),
                )],
            ),
        ]);
        let mut input = make_input(vec![("a", "^1"), ("b", "^1")]);
        input.lazy_index = true;
        request(&mut input, "a", &["x"]);
        request(&mut input, "b", &["y"]);
        let err = resolve(&input, &index).unwrap_err();
        assert!(
            matches!(&err, ResolveError::MetadataNeeded(names) if names.iter().map(PackageName::as_str).eq(["d", "e"])),
            "{err:?}"
        );
    }

    /// An index package's feature request reaches a locally supplied
    /// package (a `[patch]` fork) like any other: the fork's optional
    /// registry dependency resolves and its features are reported.
    #[test]
    fn index_requests_enable_local_optional_dependencies() {
        let index = build_index(vec![
            featured(
                "a",
                vec![(
                    "1.0.0",
                    featured_version(
                        vec![(
                            "p",
                            cabin_index::IndexPackageDependency {
                                features: vec!["with-q".to_owned()],
                                ..edge("^1")
                            },
                        )],
                        &[],
                        &[],
                    ),
                )],
            ),
            plain("q"),
        ]);
        let mut input = make_input(vec![("a", "^1")]);
        input.local_packages.insert(
            pkg_name("p"),
            featured(
                "p",
                vec![(
                    "1.0.0",
                    featured_version(vec![("q", optional("^1"))], &[], &[("with-q", &["dep:q"])]),
                )],
            ),
        );
        let out = resolve(&input, &index).unwrap();
        assert_eq!(names(&out), ["a", "p", "q"]);
        let p = selected(&out, "p");
        assert_eq!(p.source, ResolvedSource::Local);
        assert_eq!(p.features, set(["with-q"]));
    }
}
