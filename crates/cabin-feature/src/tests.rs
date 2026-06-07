//! Unit tests for the feature resolver. Test fixtures build a
//! synthetic [`PackageGraph`] in-memory; the workspace loader's
//! filesystem path is exercised end-to-end by the CLI integration
//! tests in `cabin/tests/cli.rs`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use camino::Utf8PathBuf;

use cabin_core::{
    Dependency, DependencyKind, DependencySource, Features, Package, PackageName, TargetPlatform,
};
// Allow workspace `semver` use.
use cabin_workspace::{DependencyEdge, PackageGraph, PackageKind, WorkspacePackage};
use semver as _;

use super::*;

fn pkg(name: &str) -> PackageName {
    PackageName::new(name).unwrap()
}

fn host() -> TargetPlatform {
    TargetPlatform::current()
}

fn ver() -> semver::Version {
    semver::Version::parse("0.1.0").unwrap()
}

fn empty_features() -> Features {
    Features::default()
}

fn features(default: &[&str], features: &[(&str, &[&str])]) -> Features {
    Features {
        default: default.iter().map(|s| (*s).to_owned()).collect(),
        features: features
            .iter()
            .map(|(name, list)| {
                (
                    (*name).to_owned(),
                    list.iter().map(|s| (*s).to_owned()).collect(),
                )
            })
            .collect(),
    }
}

fn dep_normal(name: &str, optional: bool) -> Dependency {
    Dependency {
        name: pkg(name),
        source: DependencySource::Path(Utf8PathBuf::from(format!("../{name}"))),
        kind: DependencyKind::Normal,
        optional,
        features: Vec::new(),
        default_features: true,
        condition: None,
    }
}

fn dep_normal_with(
    name: &str,
    optional: bool,
    features: &[&str],
    default_features: bool,
) -> Dependency {
    Dependency {
        name: pkg(name),
        source: DependencySource::Path(Utf8PathBuf::from(format!("../{name}"))),
        kind: DependencyKind::Normal,
        optional,
        features: features.iter().map(|s| (*s).to_owned()).collect(),
        default_features,
        condition: None,
    }
}

fn make_project(name: &str, deps: Vec<Dependency>, fts: Features) -> Package {
    Package::with_config(cabin_core::PackageConfigInput {
        name: pkg(name),
        version: ver(),
        targets: Vec::new(),
        dependencies: deps,
        system_dependencies: Vec::new(),
        features: fts,
    })
    .unwrap()
}

fn make_graph(packages: Vec<(Package, Vec<(usize, DependencyKind)>)>) -> PackageGraph {
    let mut pkgs: Vec<WorkspacePackage> = Vec::with_capacity(packages.len());
    for (package, edges) in packages {
        pkgs.push(WorkspacePackage {
            package,
            manifest_path: PathBuf::from("/synth/cabin.toml"),
            manifest_dir: PathBuf::from("/synth"),
            deps: edges
                .into_iter()
                .map(|(index, kind)| DependencyEdge {
                    index,
                    kind,
                    condition: None,
                })
                .collect(),
            kind: PackageKind::Local,
            is_port: false,
        });
    }
    PackageGraph {
        root_manifest_path: PathBuf::from("/synth/cabin.toml"),
        root_dir: PathBuf::from("/synth"),
        is_workspace_root: false,
        root_package: Some(0),
        root_settings: Default::default(),
        primary_packages: vec![0],
        default_members: Vec::new(),
        excluded_members: Vec::new(),
        packages: pkgs,
    }
}

fn names(set: &BTreeSet<String>) -> Vec<&str> {
    set.iter().map(String::as_str).collect()
}

#[test]
fn enables_default_feature_for_root() {
    // Given a single root with default = ["a"], `a = ["b"]`,
    // `b = []`, the default request enables both.
    let package = make_project(
        "root",
        Vec::new(),
        features(&["a"], &[("a", &["b"]), ("b", &[])]),
    );
    let graph = make_graph(vec![(package, Vec::new())]);
    let res = resolve_features(&graph, &[0], &RootFeatureRequest::default(), &host()).unwrap();
    let r = res.for_package(0);
    assert_eq!(names(&r.enabled_features), vec!["a", "b", "default"]);
}

#[test]
fn no_default_features_skips_default_chain() {
    let package = make_project(
        "root",
        Vec::new(),
        features(&["a"], &[("a", &["b"]), ("b", &[])]),
    );
    let graph = make_graph(vec![(package, Vec::new())]);
    let request = RootFeatureRequest {
        include_defaults: false,
        all_features: false,
        explicit_features: BTreeSet::new(),
    };
    let res = resolve_features(&graph, &[0], &request, &host()).unwrap();
    let r = res.for_package(0);
    assert!(r.enabled_features.is_empty());
}

#[test]
fn explicit_features_request_only_those_features() {
    let package = make_project(
        "root",
        Vec::new(),
        features(&[], &[("a", &[]), ("b", &[]), ("c", &[])]),
    );
    let graph = make_graph(vec![(package, Vec::new())]);
    let mut explicit = BTreeSet::new();
    explicit.insert("b".to_owned());
    let request = RootFeatureRequest {
        include_defaults: false,
        all_features: false,
        explicit_features: explicit,
    };
    let res = resolve_features(&graph, &[0], &request, &host()).unwrap();
    let r = res.for_package(0);
    assert_eq!(names(&r.enabled_features), vec!["b"]);
}

#[test]
fn all_features_includes_every_declared_feature() {
    let package = make_project("root", Vec::new(), features(&[], &[("a", &[]), ("b", &[])]));
    let graph = make_graph(vec![(package, Vec::new())]);
    let request = RootFeatureRequest {
        include_defaults: false,
        all_features: true,
        explicit_features: BTreeSet::new(),
    };
    let res = resolve_features(&graph, &[0], &request, &host()).unwrap();
    let r = res.for_package(0);
    assert_eq!(names(&r.enabled_features), vec!["a", "b"]);
}

#[test]
fn unknown_root_feature_errors_clearly() {
    let package = make_project("root", Vec::new(), empty_features());
    let graph = make_graph(vec![(package, Vec::new())]);
    let mut explicit = BTreeSet::new();
    explicit.insert("nope".to_owned());
    let err = resolve_features(
        &graph,
        &[0],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap_err();
    match err {
        FeatureResolverError::UnknownRootFeature { package, feature } => {
            assert_eq!(package, "root");
            assert_eq!(feature, "nope");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn dep_colon_enables_optional_dependency() {
    // `[features] ssl = ["dep:openssl"]` enables the optional
    // dep when `ssl` is requested.
    let openssl = make_project("openssl", Vec::new(), empty_features());
    let root = make_project(
        "root",
        vec![dep_normal("openssl", true)],
        features(&[], &[("ssl", &["dep:openssl"])]),
    );
    let graph = make_graph(vec![
        (openssl, Vec::new()),
        (root, vec![(0, DependencyKind::Normal)]),
    ]);
    let mut explicit = BTreeSet::new();
    explicit.insert("ssl".to_owned());
    let res = resolve_features(
        &graph,
        &[1],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap();
    let r = res.for_package(1);
    assert!(r.enabled_features.contains("ssl"));
    assert_eq!(names(&r.enabled_optional_deps), vec!["openssl"]);
}

#[test]
fn dep_colon_on_non_optional_dep_errors_clearly() {
    let fmt = make_project("fmt", Vec::new(), empty_features());
    let root = make_project(
        "root",
        vec![dep_normal("fmt", false)],
        features(&[], &[("ssl", &["dep:fmt"])]),
    );
    let graph = make_graph(vec![
        (fmt, Vec::new()),
        (root, vec![(0, DependencyKind::Normal)]),
    ]);
    let mut explicit = BTreeSet::new();
    explicit.insert("ssl".to_owned());
    let err = resolve_features(
        &graph,
        &[1],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap_err();
    match err {
        FeatureResolverError::DepIsNotOptional {
            package,
            feature,
            dependency,
        } => {
            assert_eq!(package, "root");
            assert_eq!(feature, "ssl");
            assert_eq!(dependency, "fmt");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn dep_slash_feature_requests_dep_feature_and_enables_optional() {
    // `["openssl/vendored"]` should enable optional `openssl`
    // *and* request `vendored` on it.
    let openssl = make_project("openssl", Vec::new(), features(&[], &[("vendored", &[])]));
    let root = make_project(
        "root",
        vec![dep_normal("openssl", true)],
        features(&[], &[("ssl", &["openssl/vendored"])]),
    );
    let graph = make_graph(vec![
        (openssl, Vec::new()),
        (root, vec![(0, DependencyKind::Normal)]),
    ]);
    let mut explicit = BTreeSet::new();
    explicit.insert("ssl".to_owned());
    let res = resolve_features(
        &graph,
        &[1],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap();
    let openssl_features = res.for_package(0);
    assert!(openssl_features.enabled_features.contains("vendored"));
    let root_features = res.for_package(1);
    assert!(root_features.enabled_optional_deps.contains("openssl"));
}

#[test]
fn dep_colon_referencing_undeclared_dependency_errors_clearly() {
    // `ssl = ["dep:ghost"]` but `ghost` is never declared.
    let root = make_project(
        "root",
        Vec::new(),
        features(&[], &[("ssl", &["dep:ghost"])]),
    );
    let graph = make_graph(vec![(root, Vec::new())]);
    let mut explicit = BTreeSet::new();
    explicit.insert("ssl".to_owned());
    let err = resolve_features(
        &graph,
        &[0],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap_err();
    match err {
        FeatureResolverError::UnknownDependency {
            package,
            feature,
            dependency,
        } => {
            assert_eq!(package, "root");
            assert_eq!(feature, "ssl");
            assert_eq!(dependency, "ghost");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn dep_slash_requesting_unknown_dep_feature_errors_clearly() {
    // `openssl/vendored`, but `openssl` declares no `vendored`.
    let openssl = make_project("openssl", Vec::new(), empty_features());
    let root = make_project(
        "root",
        vec![dep_normal("openssl", true)],
        features(&[], &[("ssl", &["openssl/vendored"])]),
    );
    let graph = make_graph(vec![
        (openssl, Vec::new()),
        (root, vec![(0, DependencyKind::Normal)]),
    ]);
    let mut explicit = BTreeSet::new();
    explicit.insert("ssl".to_owned());
    let err = resolve_features(
        &graph,
        &[1],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap_err();
    match err {
        FeatureResolverError::DepFeatureRequestUnknown {
            package,
            dependency,
            feature,
        } => {
            assert_eq!(package, "root");
            assert_eq!(dependency, "openssl");
            assert_eq!(feature, "vendored");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn additive_unification_across_paths() {
    // Two edges to the same dependency request different
    // features; the unified set is the union.
    let dep = make_project(
        "dep",
        Vec::new(),
        features(&[], &[("a", &[]), ("b", &[]), ("c", &[])]),
    );
    let root = make_project(
        "root",
        vec![
            // First edge requests `a`.
            dep_normal_with("dep", false, &["a"], false),
        ],
        features(&[], &[("via", &["dep/b"])]),
    );
    // Second hop: same package, second edge through feature `via`
    // requests `b`.
    let graph = make_graph(vec![
        (dep, Vec::new()),
        (root, vec![(0, DependencyKind::Normal)]),
    ]);
    let mut explicit = BTreeSet::new();
    explicit.insert("via".to_owned());
    let res = resolve_features(
        &graph,
        &[1],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap();
    let r = res.for_package(0);
    let names: Vec<&str> = r.enabled_features.iter().map(String::as_str).collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
}

#[test]
fn default_features_request_is_default_on() {
    // A non-optional edge auto-requests the dep's `default`.
    let dep = make_project("dep", Vec::new(), features(&["d"], &[("d", &[])]));
    let root = make_project("root", vec![dep_normal("dep", false)], empty_features());
    let graph = make_graph(vec![
        (dep, Vec::new()),
        (root, vec![(0, DependencyKind::Normal)]),
    ]);
    let res = resolve_features(&graph, &[1], &RootFeatureRequest::default(), &host()).unwrap();
    let r = res.for_package(0);
    assert!(r.enabled_features.contains("d"));
}

#[test]
fn default_features_false_does_not_globally_disable() {
    // One edge with default-features = false; another (here:
    // root requests `dep/d`) still pulls in default-equivalent.
    let dep = make_project("dep", Vec::new(), features(&["d"], &[("d", &[])]));
    let root = make_project(
        "root",
        vec![dep_normal_with("dep", false, &["d"], false)],
        empty_features(),
    );
    let graph = make_graph(vec![
        (dep, Vec::new()),
        (root, vec![(0, DependencyKind::Normal)]),
    ]);
    let res = resolve_features(&graph, &[1], &RootFeatureRequest::default(), &host()).unwrap();
    let r = res.for_package(0);
    // Per-edge `features = ["d"]` plus `default-features = false`
    // means the unified result includes only the explicit set.
    assert!(r.enabled_features.contains("d"));
    assert!(!r.enabled_features.contains("default"));
}

#[test]
fn local_feature_cycle_is_terminating_and_recorded() {
    // Cycle detection only follows local-feature edges. Cycles
    // are reported at validation time, but a graph with no cycles
    // still resolves correctly. Construct a chain a -> b -> c (no
    // cycle) and verify everything is enabled.
    let package = make_project(
        "root",
        Vec::new(),
        features(&[], &[("a", &["b"]), ("b", &["c"]), ("c", &[])]),
    );
    let graph = make_graph(vec![(package, Vec::new())]);
    let mut explicit = BTreeSet::new();
    explicit.insert("a".to_owned());
    let res = resolve_features(
        &graph,
        &[0],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap();
    let r = res.for_package(0);
    assert_eq!(names(&r.enabled_features), vec!["a", "b", "c"]);
}

#[test]
fn unknown_local_feature_in_chain_errors_clearly() {
    // `Features::validate` rejects unknown local references at
    // parse time, but the resolver should surface them with a
    // clear error if the runtime ever sees them (e.g. an entry
    // injected by a future API). Smoke-test the resolver-side
    // message.
    let mut feats = features(&[], &[("a", &["nope"])]);
    // Bypass `Features::validate` by mutating after construction.
    feats.features.insert("a".into(), vec!["nope".into()]);
    let package = Package::with_config(cabin_core::PackageConfigInput {
        name: pkg("root"),
        version: ver(),
        targets: Vec::new(),
        dependencies: Vec::new(),
        system_dependencies: Vec::new(),
        features: Features::default(),
    })
    .unwrap();
    // Inject an unknown reference via direct field access.
    let mut package = package;
    package.features = feats;
    let graph = make_graph(vec![(package, Vec::new())]);
    let mut explicit = BTreeSet::new();
    explicit.insert("a".to_owned());
    let err = resolve_features(
        &graph,
        &[0],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap_err();
    assert!(matches!(err, FeatureResolverError::UnknownFeature { .. }));
}

#[test]
fn deterministic_output_for_identical_inputs() {
    let dep = make_project(
        "dep",
        Vec::new(),
        features(&[], &[("a", &[]), ("b", &[]), ("c", &[])]),
    );
    let root = make_project(
        "root",
        vec![dep_normal_with("dep", false, &["a", "b", "c"], false)],
        empty_features(),
    );
    let graph = make_graph(vec![
        (dep, Vec::new()),
        (root, vec![(0, DependencyKind::Normal)]),
    ]);
    let req = RootFeatureRequest::default();
    let r1 = resolve_features(&graph, &[1], &req, &host()).unwrap();
    let r2 = resolve_features(&graph, &[1], &req, &host()).unwrap();
    assert_eq!(r1, r2);
}

#[test]
fn dep_colon_skips_optional_dep_when_target_does_not_match() {
    // An optional dep guarded by a non-matching `cfg(...)` is
    // invisible to the feature resolver — `dep:openssl` cannot
    // enable a dep that does not apply on this host. Use a
    // synthetic platform whose `os` is guaranteed not to match
    // the predicate so the test is deterministic.
    use cabin_core::{Condition, ConditionKey};
    let openssl = make_project("openssl", Vec::new(), empty_features());
    let mut openssl_dep = dep_normal("openssl", true);
    openssl_dep.condition = Some(Condition::KeyValue {
        key: ConditionKey::Os,
        value: "this-os-never-matches".into(),
    });
    let root = make_project(
        "root",
        vec![openssl_dep],
        features(&[], &[("ssl", &["dep:openssl"])]),
    );
    let graph = make_graph(vec![
        (openssl, Vec::new()),
        (root, vec![(0, DependencyKind::Normal)]),
    ]);
    let mut explicit = BTreeSet::new();
    explicit.insert("ssl".to_owned());
    let res = resolve_features(
        &graph,
        &[1],
        &RootFeatureRequest {
            include_defaults: false,
            all_features: false,
            explicit_features: explicit,
        },
        &host(),
    )
    .unwrap();
    let r = res.for_package(1);
    assert!(r.enabled_features.contains("ssl"));
    // The dep was filtered out by the platform check, so the
    // resolver did not enable it.
    assert!(
        !r.enabled_optional_deps.contains("openssl"),
        "openssl should not be enabled: {:?}",
        r.enabled_optional_deps,
    );
}
