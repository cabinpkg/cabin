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

use cabin_core::standard_compatibility::{
    ConsumerStandards, EffectiveRequirements, edge_compatible,
};
use cabin_core::{IncompatibleStandards, PackageName, Requirement, SourceLanguage, TargetPlatform};
use cabin_index::PackageIndex;
use pubgrub::{PubGrubError, Ranges};
use semver::{Version, VersionReq};

pub use error::{ResolveError, ResolverConstraint};
pub use input::{LockedVersion, ResolveInput, ResolveMode};
pub use output::{BlockedRequirement, HeldBack, ResolveOutput, ResolvedPackage, ResolvedSource};

use crate::explanation::explain_no_solution;
use crate::output::selected_dependencies_to_output;
use crate::preflight::{effective_locked, preflight_root_dependencies};
use crate::provider::Provider;
use crate::range::req_to_range;

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
pub fn resolve(input: &ResolveInput, index: &PackageIndex) -> Result<ResolveOutput, ResolveError> {
    let mut output = resolve_once(input, index)?;
    output.held_back = held_back_report(input, index, &output.packages);
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
    let platform = TargetPlatform::current();
    let locked = effective_locked(input);

    // Convert root requirements up front so an unsupported
    // `semver::Op` surfaces as `UnsupportedVersionRequirement`
    // before preflight collapses it into a less specific
    // `NoMatchingVersion`.
    let root_dependencies = convert_root_dependencies(&input.root_dependencies)?;

    // Preflight surfaces Cabin's targeted error variants for
    // root dependencies before `PubGrub` is invoked; the
    // returned constraints seed the locked-mode recorder when
    // it is constructed.
    let root_constraints = preflight_root_dependencies(input, index, &locked)?;

    let provider = Provider::new(
        input,
        index,
        locked,
        platform,
        root_constraints,
        root_dependencies,
    );

    let solution = match pubgrub::resolve(
        &provider,
        input.root_name.clone(),
        input.root_version.clone(),
    ) {
        Ok(solution) => solution,
        Err(PubGrubError::NoSolution(tree)) => {
            return Err(explain_no_solution(tree, &input.root_name));
        }
        // `ErrorChoosingVersion`, `ErrorRetrievingDependencies`,
        // and `ErrorInShouldCancel` all bubble a provider-side
        // `ResolveError` back out - surface the original
        // variant.
        Err(
            PubGrubError::ErrorChoosingVersion { source, .. }
            | PubGrubError::ErrorRetrievingDependencies { source, .. }
            | PubGrubError::ErrorInShouldCancel(source),
        ) => return Err(source),
    };

    Ok(selected_dependencies_to_output(input, solution))
}

/// The standard-caused hold-backs of a `Fallback` resolution: the diff
/// against the `Allow` solution.
///
/// `Allow` is the newest-first selection standards never touch, so any
/// package `Fallback` resolved to an *older* version than `Allow` did -
/// where that newer `Allow` version is declared-incompatible with the
/// consumer - was held back for a standard reason.  Computing the
/// report as this diff is exact by construction: a newer version that
/// `Allow` also passed over (out of range, unsolvable dependencies,
/// yanked) never appears in the `Allow` solution, so it is never
/// misreported as a standard hold.
///
/// Empty under `Allow`, and whenever the workspace declares no consumer
/// standard (nothing can be incompatible).  On the `Fallback`-success
/// path the `Allow` resolution cannot fail - solvability is identical -
/// but a defensive error yields an empty report rather than aborting.
fn held_back_report(
    input: &ResolveInput,
    index: &PackageIndex,
    fallback_packages: &[ResolvedPackage],
) -> Vec<HeldBack> {
    if input.incompatible_standards != IncompatibleStandards::Fallback {
        return Vec::new();
    }
    let consumer = input.consumer_standards;
    if consumer.c.is_none() && consumer.cxx.is_none() {
        return Vec::new();
    }

    let mut allow_input = input.clone();
    allow_input.incompatible_standards = IncompatibleStandards::Allow;
    let Ok(allow) = resolve_once(&allow_input, index) else {
        return Vec::new();
    };
    let allow_versions: BTreeMap<&PackageName, &Version> = allow
        .packages
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
        // The root is the local project, never selected from the index;
        // a same-named index entry must not be applied to it.
        if package.source == ResolvedSource::Root {
            continue;
        }
        if locked
            .get(&package.name)
            .is_some_and(|entry| entry.version == package.version)
        {
            continue;
        }
        // Rule-2 case (preference-mode.md section 2): the selected
        // version is *itself* standard-incompatible because nothing in
        // range satisfies the consumer.  Report its own unmet
        // requirement, with no compatible alternative to name.
        let selected_advertised = index
            .package(&package.name)
            .and_then(|entry| entry.versions.get(&package.version))
            .map(|meta| meta.standards.version_wide_join());
        if let Some(advertised) = selected_advertised
            && !edge_compatible(consumer, advertised)
        {
            let blocked_by = blocking_requirements(consumer, advertised);
            if !blocked_by.is_empty() {
                held_back.push(HeldBack {
                    name: package.name.clone(),
                    selected: package.version.clone(),
                    newest: None,
                    blocked_by,
                });
            }
            continue;
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
        let advertised = standards.version_wide_join();
        let blocked_by = if edge_compatible(consumer, advertised) {
            // The newer version is compatible, so the only reason
            // fallback passed it over is that it is undeclared and a
            // declared-compatible older version outranks it (tier 1 over
            // tier 2).  A selected version compatible with a compatible
            // newer one can only have been the older, declared pick - so
            // this is always the tier preference.  Report it with no
            // requirement to cite.
            Vec::new()
        } else {
            let requirements = blocking_requirements(consumer, advertised);
            // A declared-incompatible version always has a failing
            // clause; guard defensively.
            if requirements.is_empty() {
                continue;
            }
            requirements
        };
        held_back.push(HeldBack {
            name: package.name.clone(),
            selected: package.version.clone(),
            newest: Some(newest.clone()),
            blocked_by,
        });
    }
    held_back.sort_by(|a, b| a.name.cmp(&b.name));
    held_back
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
    let platform = TargetPlatform::current();
    let mut range = Ranges::full();
    if let Some(req) = input.root_dependencies.get(package)
        && let Ok(root_range) = req_to_range(req)
    {
        range = range.intersection(&root_range);
    }
    for resolved in solution {
        // The root's own requirements are the `root_dependencies` folded
        // in above; a registry entry that coincidentally shares the
        // root's name and version is unrelated and must not contribute
        // its constraints.
        if resolved.source == ResolvedSource::Root {
            continue;
        }
        let Some(meta) = index
            .package(&resolved.name)
            .and_then(|entry| entry.versions.get(&resolved.version))
        else {
            continue;
        };
        let Some(dep) = meta.dependencies.get(package) else {
            continue;
        };
        // Only edges that participated in resolution bind the range.
        if !dep.is_active_for(&platform) {
            continue;
        }
        if let Ok(dep_range) = req_to_range(&dep.req) {
            range = range.intersection(&dep_range);
        }
    }
    // Match the provider's own admissibility, pre-release rule included,
    // so a pre-release the fallback constraints would reject is not
    // reported as a standard hold.
    range.contains(version) && crate::provider::candidate_admits_prerelease(&range, version)
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

/// Translate every root requirement to its `PubGrub` range,
/// mapping the crate-internal [`range::RangeConversionError`] to
/// [`ResolveError::UnsupportedVersionRequirement`] with the
/// package the requirement was attached to.
fn convert_root_dependencies(
    root_dependencies: &BTreeMap<PackageName, VersionReq>,
) -> Result<Vec<(PackageName, Ranges<Version>)>, ResolveError> {
    root_dependencies
        .iter()
        .map(|(name, req)| {
            req_to_range(req)
                .map(|range| (name.clone(), range))
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
    use std::collections::BTreeMap;
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
                    checksum: None,
                    source: None,
                    features: None,
                    profiles: None,
                    toolchain: None,
                    build: None,
                    compiler_wrapper: None,
                    language: None,
                    standards: cabin_index::StandardsMetadata::default(),
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

    /// Standard-compatibility metadata present on candidate versions
    /// does not change resolution: this step plumbs the table through
    /// but never consults it for version selection, so the output is
    /// byte-for-byte identical with and without it.
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

        // Populate a per-target table on every candidate version.
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
        // UpdateAll picks the newest available compatible version.
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
        // spdlog updated to newest; fmt kept at locked.
        assert_eq!(spdlog.version, version("1.13.0"));
        assert_eq!(fmt.version, version("10.1.0"));
    }

    /// Build an [`IndexEntry`] with a single version whose
    /// per-kind dependency tables come from the caller.  Each
    /// caller-supplied dep tuple is `(name, req, optional)`;
    /// per-version `cfg(...)` predicates are not needed by the
    /// tests that consume this helper.
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
                checksum: None,
                source: None,
                features: None,
                profiles: None,
                toolchain: None,
                build: None,
                compiler_wrapper: None,
                language: None,
                standards: cabin_index::StandardsMetadata::default(),
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

    #[test]
    fn locked_mode_fails_on_checksum_mismatch() {
        let mut index = build_index(vec![entry("fmt", vec![("10.2.1", vec![], false)])]);
        // Inject a checksum on the index entry.
        let entry = index.packages.get_mut(&pkg_name("fmt")).unwrap();
        entry.versions.get_mut(&version("10.2.1")).unwrap().checksum =
            Some("sha256:newvalue".into());

        let mut input = input_with_locked(
            vec![("fmt", "*")],
            vec![("fmt", "10.2.1")],
            ResolveMode::Locked,
        );
        // Lockfile thinks the checksum is something else.
        input.locked.get_mut(&pkg_name("fmt")).unwrap().checksum = Some("sha256:oldvalue".into());

        let err = resolve(&input, &index).unwrap_err();
        assert!(matches!(
            err,
            ResolveError::LockedChecksumMismatch { name, .. } if name == "fmt"
        ));
    }

    // -----------------------------------------------------------------
    // Pre-release boundary
    //
    // Pre-release versions are excluded from candidate selection
    // unless the requirement is the singleton that contains the
    // exact pre-release version.  This pins the boundary so future
    // resolver work cannot silently expand the supported syntax.
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

    // -----------------------------------------------------------------
    // Conditional registry dependencies
    //
    // Index `cfg(...)` conditions are filtered against the host
    // target platform.  Tests parametrise on the runner's actual
    // `os` so they stay deterministic across CI machines.
    // -----------------------------------------------------------------

    fn host_os() -> String {
        cabin_core::TargetPlatform::current().os
    }

    fn other_os() -> String {
        if host_os() == "linux" {
            "macos".to_owned()
        } else {
            "linux".to_owned()
        }
    }

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
                checksum: None,
                source: None,
                features: None,
                profiles: None,
                toolchain: None,
                build: None,
                compiler_wrapper: None,
                language: None,
                standards: cabin_index::StandardsMetadata::default(),
            },
        );
        IndexEntry {
            name: pkg_name(name),
            versions: vmap,
        }
    }

    #[test]
    fn conditional_registry_dependency_included_when_predicate_matches() {
        let cond =
            cabin_core::Condition::parse_cfg(&format!(r#"cfg(os = "{}")"#, host_os())).unwrap();
        let parent = entry_with_conditional("parent", "1.0.0", vec![("fmt", ">=1", Some(cond))]);
        let fmt = entry("fmt", vec![("1.0.0", vec![], false)]);
        let index = build_index(vec![parent, fmt]);
        let out = resolve(&make_input(vec![("parent", "*")]), &index).unwrap();
        let names: Vec<&str> = out.packages.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"fmt"),
            "fmt should resolve when condition matches host: {names:?}"
        );
    }

    #[test]
    fn conditional_registry_dependency_skipped_when_predicate_fails() {
        let cond =
            cabin_core::Condition::parse_cfg(&format!(r#"cfg(os = "{}")"#, other_os())).unwrap();
        // The `fmt` package is absent from the index.  If the
        // conditional edge leaked through, resolution would fail
        // with `UnknownPackage`.
        let parent = entry_with_conditional("parent", "1.0.0", vec![("fmt", ">=1", Some(cond))]);
        let index = build_index(vec![parent]);
        let out = resolve(&make_input(vec![("parent", "*")]), &index).unwrap();
        assert!(!out.packages.iter().any(|p| p.name.as_str() == "fmt"));
    }

    // -----------------------------------------------------------------
    // miette diagnostic rendering
    //
    // Verifies that every `ResolveError` variant carries the
    // stable `cabin::resolver::error` code and the actionable
    // help text the rendered diagnostic exposes.
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
        // The conflict diagnostic must surface the stable code,
        // both package names involved, and the version
        // requirements that failed to align.
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
    /// admissible version (pre-preference-mode behavior) and never
    /// reports a hold-back.
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
    /// to name (preference-mode.md section 2, rule 2).
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
        // 2.0.0 is out of range in the final solution: a semver
        // exclusion, not a standard hold.
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
        // Exactly one standard hold: `foo`.  `bar` is a semver
        // consequence of the parent choice, not a standard hold.
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
}
