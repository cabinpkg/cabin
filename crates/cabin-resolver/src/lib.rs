//! Local dependency resolver for Cabin.
//!
//! Wraps `PubGrub` as the solving engine over a local
//! [`cabin_index::PackageIndex`]. The public surface is intentionally
//! Cabin-native ([`ResolveInput`], [`ResolveOutput`], [`ResolveError`]);
//! `PubGrub` is an implementation detail and does not appear in the
//! crate's public types.
//!
//! ## Internal modules
//!
//! * `preflight` — root-dependency checks that emit Cabin's
//!   targeted error variants before `PubGrub` runs.
//! * `provider` — the `PubGrub` `DependencyProvider`
//!   implementation, candidate selection, and dependency-edge
//!   filtering.
//! * `locked` — shared locked-version metadata validation
//!   plus the locked-mode-only constraint recorder.
//! * `explanation` — `PubGrub` no-solution → Cabin
//!   `ResolveError::Conflict` conversion.
//! * `output` — `PubGrub` `SelectedDependencies` →
//!   [`ResolveOutput`] assembly.
//! * `range` — `semver::VersionReq` → `PubGrub`
//!   `Ranges<semver::Version>` translation.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod error;
mod explanation;
pub mod input;
mod locked;
pub mod output;
mod preflight;
mod provider;
mod range;

use cabin_core::TargetPlatform;
use cabin_index::PackageIndex;
use pubgrub::PubGrubError;

pub use error::{ResolveError, ResolverConstraint};
pub use input::{LockedVersion, ResolveInput, ResolveMode};
pub use output::{ResolveOutput, ResolvedPackage, ResolvedSource};

use crate::explanation::explain_no_solution;
use crate::output::selected_dependencies_to_output;
use crate::preflight::{effective_locked, preflight_root_dependencies};
use crate::provider::Provider;

/// Resolve `input`'s versioned dependencies against `index`.
///
/// Returns a [`ResolveOutput`] whose `packages` list contains the root
/// package plus every transitively-resolved registry package, sorted
/// with the root first and then alphabetical by name.
pub fn resolve(input: &ResolveInput, index: &PackageIndex) -> Result<ResolveOutput, ResolveError> {
    let platform = TargetPlatform::current();
    let locked = effective_locked(input);

    // Preflight surfaces Cabin's targeted error variants for
    // root dependencies before `PubGrub` is invoked; the
    // returned constraints seed the locked-mode recorder when
    // it is constructed.
    let root_constraints = preflight_root_dependencies(input, index, &locked)?;

    let provider = Provider::new(input, index, locked, platform, root_constraints);

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
        // `ResolveError` back out — surface the original
        // variant.
        Err(
            PubGrubError::ErrorChoosingVersion { source, .. }
            | PubGrubError::ErrorRetrievingDependencies { source, .. }
            | PubGrubError::ErrorInShouldCancel(source),
        ) => return Err(source),
    };

    Ok(selected_dependencies_to_output(input, solution))
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
        // The first failure encountered is reported. The exact variant
        // varies depending on visit order; we just assert that
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
        // accepts both. PreferLocked must keep the older locked version.
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
        // Locked 10.0.0 but constraint requires >=10.2. Resolver should
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
    /// per-kind dependency tables come from the caller. Each
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
        // feature enables them — matching the documented model
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
    // exact pre-release version. This pins the boundary so future
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
    /// constraint no longer admits — carrying the lock
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
    /// wide range — otherwise `cabin resolve` against an
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
    /// pre-releases of that same triple — `>=1.0.0-alpha, <1.0.0`
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
        // than `1.0.0`. Without the bound-aware pre-release
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
                // that does not exist in the index. The
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
    /// with a pre tag — even though the numeric range from
    /// `req_to_range` would happily contain the pre-release.
    #[test]
    fn locked_mode_rejects_transitive_prerelease_outside_semver_rule() {
        // spdlog 1.0.0 requires `fmt >=1.0.0, <2.0.0` (a wide
        // numeric range). The lockfile pins fmt to
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
        // matches `1.0.0` — pre-flight passes. The locked
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
    // target platform. Tests parametrise on the runner's actual
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
        // The `fmt` package is absent from the index. If the
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
}
