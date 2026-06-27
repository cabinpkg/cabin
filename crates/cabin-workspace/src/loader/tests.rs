use super::*;
use assert_fs::TempDir;
use assert_fs::prelude::*;

#[test]
fn loads_single_package_with_no_deps() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "solo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    assert!(!graph.is_workspace_root);
    assert_eq!(graph.packages.len(), 1);
    assert_eq!(graph.packages[0].package.name.as_str(), "solo");
    assert_eq!(graph.packages[0].deps.len(), 0);
    assert_eq!(graph.primary_packages, vec![0]);
    assert_eq!(graph.root_package, Some(0));
}

#[test]
fn loads_package_with_local_path_dep() {
    let dir = TempDir::new().unwrap();
    dir.child("greet/cabin.toml")
        .write_str(
            r#"[package]
name = "greet"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("app/cabin.toml")).unwrap();
    assert_eq!(graph.packages.len(), 2);
    // greet must come before app in topological order.
    assert_eq!(graph.packages[0].package.name.as_str(), "greet");
    assert_eq!(graph.packages[1].package.name.as_str(), "app");
    assert_eq!(
        graph.packages[1]
            .deps
            .iter()
            .map(|e| (e.index, e.kind))
            .collect::<Vec<_>>(),
        vec![(0, DependencyKind::Normal)]
    );
    assert_eq!(graph.primary_packages, vec![1]);
}

#[test]
fn loads_transitive_local_path_deps() {
    let dir = TempDir::new().unwrap();
    dir.child("c/cabin.toml")
        .write_str(
            r#"[package]
name = "c"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("b/cabin.toml")
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
c = { path = "../c" }
"#,
        )
        .unwrap();
    dir.child("a/cabin.toml")
        .write_str(
            r#"[package]
name = "a"
version = "0.1.0"

[dependencies]
b = { path = "../b" }
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("a/cabin.toml")).unwrap();
    assert_eq!(graph.packages.len(), 3);
    let names: Vec<&str> = graph
        .packages
        .iter()
        .map(|p| p.package.name.as_str())
        .collect();
    // Topo order: c before b before a.
    let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
    assert!(pos("c") < pos("b"));
    assert!(pos("b") < pos("a"));
}

#[test]
fn detects_package_cycle() {
    let dir = TempDir::new().unwrap();
    dir.child("a/cabin.toml")
        .write_str(
            r#"[package]
name = "a"
version = "0.1.0"

[dependencies]
b = { path = "../b" }
"#,
        )
        .unwrap();
    dir.child("b/cabin.toml")
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
a = { path = "../a" }
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("a/cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::PackageDependencyCycle(cycle) => {
            assert_eq!(cycle.first(), cycle.last());
            assert!(cycle.contains(&"a".to_owned()));
            assert!(cycle.contains(&"b".to_owned()));
        }
        other => panic!("expected PackageDependencyCycle, got {other:?}"),
    }
}

#[test]
fn loads_workspace_with_exact_member_path() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/greet"]
"#,
        )
        .unwrap();
    dir.child("packages/greet/cabin.toml")
        .write_str(
            r#"[package]
name = "greet"
version = "0.1.0"
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    assert!(graph.is_workspace_root);
    assert!(graph.root_package.is_none());
    assert_eq!(graph.packages.len(), 1);
    assert_eq!(graph.packages[0].package.name.as_str(), "greet");
}

#[test]
fn pure_workspace_root_policy_is_available_on_graph() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/greet"]

[profile.release]
opt-level = 0

[toolchain]
cxx = "clang++"

[build]
compiler-wrapper = "ccache"

[patch]
fmt = { path = "../fmt" }
"#,
        )
        .unwrap();
    dir.child("packages/greet/cabin.toml")
        .write_str(
            r#"[package]
name = "greet"
version = "0.1.0"
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    assert!(graph.is_workspace_root);
    assert!(graph.root_package.is_none());

    let release = cabin_core::ProfileName::new("release").unwrap();
    assert_eq!(
        graph
            .root_settings
            .profiles
            .get(&release)
            .and_then(|p| p.opt_level),
        Some(cabin_core::OptLevel::O0)
    );
    assert_eq!(
        graph
            .root_settings
            .toolchain
            .general
            .get(cabin_core::ToolKind::CxxCompiler)
            .map(cabin_core::ToolSpec::display)
            .as_deref(),
        Some("clang++")
    );
    assert_eq!(
        graph.root_settings.compiler_wrapper,
        Some(cabin_core::CompilerWrapperRequest::Use {
            wrapper: cabin_core::ToolSpec::Name("ccache".into()),
        })
    );
    assert_eq!(graph.root_settings.patches.entries.len(), 1);
}

#[test]
fn loads_workspace_with_glob_member_pattern() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    dir.child("packages/a/cabin.toml")
        .write_str(
            r#"[package]
name = "a"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("packages/b/cabin.toml")
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    assert_eq!(graph.packages.len(), 2);
    let names: Vec<&str> = graph
        .packages
        .iter()
        .map(|p| p.package.name.as_str())
        .collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
}

#[test]
fn rejects_duplicate_package_names_in_workspace() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    dir.child("packages/a/cabin.toml")
        .write_str(
            r#"[package]
name = "shared"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("packages/b/cabin.toml")
        .write_str(
            r#"[package]
name = "shared"
version = "0.2.0"
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::DuplicatePackageName { name, .. } => assert_eq!(name, "shared"),
        other => panic!("expected DuplicatePackageName, got {other:?}"),
    }
}

#[test]
fn missing_local_dependency_manifest_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("app/cabin.toml")).unwrap_err();
    assert!(matches!(
        err,
        WorkspaceError::LocalDependencyManifestMissing { .. }
    ));
}

#[test]
fn dependency_name_mismatch_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("greet/cabin.toml")
        .write_str(
            r#"[package]
name = "actually-hello"
version = "0.1.0"
"#,
        )
        .unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
greet = { path = "../greet" }
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("app/cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::DependencyNameMismatch {
            dep_name,
            actual_name,
            ..
        } => {
            assert_eq!(dep_name, "greet");
            assert_eq!(actual_name, "actually-hello");
        }
        other => panic!("expected DependencyNameMismatch, got {other:?}"),
    }
}

#[test]
fn versioned_dependencies_are_preserved_but_not_traversed() {
    let dir = TempDir::new().unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("app/cabin.toml")).unwrap();
    // Only the root package is loaded - versioned deps don't pull in
    // any local manifests.
    assert_eq!(graph.packages.len(), 1);
    let app = &graph.packages[0];
    assert!(app.deps.is_empty());
    // But the Package still records the declared dependency.
    assert_eq!(app.package.dependencies.len(), 1);
    assert_eq!(app.package.dependencies[0].name.as_str(), "fmt");
    assert!(matches!(
        &app.package.dependencies[0].source,
        cabin_core::DependencySource::Version(_)
    ));
}

#[test]
fn unsupported_glob_pattern_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*/foo"]
"#,
        )
        .unwrap();
    dir.child("packages/a/foo/cabin.toml")
        .write_str(
            r#"[package]
name = "a"
version = "0.1.0"
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    assert!(matches!(
        err,
        WorkspaceError::UnsupportedWorkspacePattern { .. }
    ));
}

#[test]
fn missing_workspace_member_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/missing"]
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    assert!(matches!(err, WorkspaceError::WorkspaceMemberMissing { .. }));
}

// -------------------------------------------------------------------
// registry package integration
// -------------------------------------------------------------------

fn pkg(name: &str) -> PackageName {
    PackageName::new(name).unwrap()
}

fn ver(s: &str) -> semver::Version {
    semver::Version::parse(s).unwrap()
}

#[test]
fn loads_registry_package_via_versioned_dep() {
    let dir = TempDir::new().unwrap();
    // Root depends on `fmt` versionally.
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    // Registry "extracted source" lives in a sibling directory.
    dir.child("registry/fmt/cabin.toml")
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"
"#,
        )
        .unwrap();
    let registry = vec![RegistryPackageSource {
        name: pkg("fmt"),
        version: ver("10.2.1"),
        manifest_path: dir.path().join("registry/fmt/cabin.toml"),
    }];
    let graph = load_workspace_with_options(
        dir.path().join("app/cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap();
    assert_eq!(graph.packages.len(), 2);
    // Topological order: fmt before app.
    assert_eq!(graph.packages[0].package.name.as_str(), "fmt");
    assert_eq!(graph.packages[0].kind, PackageKind::Registry);
    assert_eq!(graph.packages[1].package.name.as_str(), "app");
    assert_eq!(graph.packages[1].kind, PackageKind::Local);
    // Only `app` is primary.
    assert_eq!(graph.primary_packages, vec![1]);
    // The dep edge is recorded so cabin-build can resolve target deps.
    let edges: Vec<(usize, DependencyKind)> = graph.packages[1]
        .deps
        .iter()
        .map(|e| (e.index, e.kind))
        .collect();
    assert_eq!(edges, vec![(0, DependencyKind::Normal)]);
}

#[test]
fn registry_package_declaring_path_dependency_is_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
evil = ">=1.0.0 <2.0.0"
"#,
        )
        .unwrap();
    // A malicious registry archive ships a nested `path` sub-package
    // whose `[profile]` smuggles a build-time code-execution flag.  The
    // loader must refuse the path dependency rather than load the
    // sub-package as a trusted `PackageKind::Local`.
    dir.child("registry/evil/cabin.toml")
        .write_str(
            r#"[package]
name = "evil"
version = "1.0.0"

[dependencies]
inner = { path = "inner" }
"#,
        )
        .unwrap();
    dir.child("registry/evil/inner/cabin.toml")
        .write_str(
            r#"[package]
name = "inner"
version = "1.0.0"

[profile]
cxxflags = ["-fplugin=evil.so"]
"#,
        )
        .unwrap();
    let registry = vec![RegistryPackageSource {
        name: pkg("evil"),
        version: ver("1.0.0"),
        manifest_path: dir.path().join("registry/evil/cabin.toml"),
    }];
    let err = load_workspace_with_options(
        dir.path().join("app/cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            WorkspaceError::RegistryPackageDeclaresPathDependency { .. }
        ),
        "expected RegistryPackageDeclaresPathDependency, got {err:?}"
    );
}

#[test]
fn registry_package_declaring_port_dependency_is_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
evil = ">=1.0.0 <2.0.0"
"#,
        )
        .unwrap();
    // The same invariant covers port dependencies: a downloaded registry
    // archive may not pull in a port (a port is prepared as a trusted
    // `PackageKind::Local` package).
    dir.child("registry/evil/cabin.toml")
        .write_str(
            r#"[package]
name = "evil"
version = "1.0.0"

[dependencies]
inner = { port-path = "ports/inner" }
"#,
        )
        .unwrap();
    let registry = vec![RegistryPackageSource {
        name: pkg("evil"),
        version: ver("1.0.0"),
        manifest_path: dir.path().join("registry/evil/cabin.toml"),
    }];
    let err = load_workspace_with_options(
        dir.path().join("app/cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            WorkspaceError::RegistryPackageDeclaresPortDependency { .. }
        ),
        "expected RegistryPackageDeclaresPortDependency, got {err:?}"
    );
}

#[test]
fn unresolved_registry_dep_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"
spdlog = ">=1"
"#,
        )
        .unwrap();
    dir.child("registry/fmt/cabin.toml")
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"
"#,
        )
        .unwrap();
    // Only `fmt` is in the registry; `spdlog` is missing.
    let registry = vec![RegistryPackageSource {
        name: pkg("fmt"),
        version: ver("10.2.1"),
        manifest_path: dir.path().join("registry/fmt/cabin.toml"),
    }];
    let err = load_workspace_with_options(
        dir.path().join("app/cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    match err {
        WorkspaceError::UnresolvedRegistryDependency { dep_name, parent } => {
            assert_eq!(dep_name, "spdlog");
            assert_eq!(parent, "app");
        }
        other => panic!("expected UnresolvedRegistryDependency, got {other:?}"),
    }
}

#[test]
fn registry_dep_chained_through_extracted_manifest() {
    let dir = TempDir::new().unwrap();
    // Root -> spdlog -> fmt.
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
spdlog = ">=1"
"#,
        )
        .unwrap();
    dir.child("registry/spdlog/cabin.toml")
        .write_str(
            r#"[package]
name = "spdlog"
version = "1.13.0"

[dependencies]
fmt = ">=10"
"#,
        )
        .unwrap();
    dir.child("registry/fmt/cabin.toml")
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"
"#,
        )
        .unwrap();
    let registry = vec![
        RegistryPackageSource {
            name: pkg("fmt"),
            version: ver("10.2.1"),
            manifest_path: dir.path().join("registry/fmt/cabin.toml"),
        },
        RegistryPackageSource {
            name: pkg("spdlog"),
            version: ver("1.13.0"),
            manifest_path: dir.path().join("registry/spdlog/cabin.toml"),
        },
    ];
    let graph = load_workspace_with_options(
        dir.path().join("app/cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap();
    assert_eq!(graph.packages.len(), 3);
    // Topological order: fmt before spdlog before app.
    let names: Vec<&str> = graph
        .packages
        .iter()
        .map(|p| p.package.name.as_str())
        .collect();
    let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
    assert!(pos("fmt") < pos("spdlog"));
    assert!(pos("spdlog") < pos("app"));
}

#[test]
fn registry_package_version_mismatch_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"
"#,
        )
        .unwrap();
    dir.child("registry/fmt/cabin.toml")
        .write_str(
            r#"[package]
name = "fmt"
version = "10.1.0"
"#,
        )
        .unwrap();
    let registry = vec![RegistryPackageSource {
        name: pkg("fmt"),
        version: ver("10.2.1"),
        manifest_path: dir.path().join("registry/fmt/cabin.toml"),
    }];
    let err = load_workspace_with_options(
        dir.path().join("app/cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        WorkspaceError::RegistryPackageMismatch { .. }
    ));
}

// -----------------------------------------------------------------
// workspace.exclude / default-members / dependency
// inheritance / nested workspaces.
// -----------------------------------------------------------------

#[test]
fn exclude_drops_member_from_primary_set() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
exclude = ["packages/skipme"]
"#,
        )
        .unwrap();
    dir.child("packages/keep/cabin.toml")
        .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
        .unwrap();
    dir.child("packages/skipme/cabin.toml")
        .write_str("[package]\nname = \"skipme\"\nversion = \"0.1.0\"\n")
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    let names: Vec<&str> = graph
        .primary_packages
        .iter()
        .map(|i| graph.packages[*i].package.name.as_str())
        .collect();
    assert_eq!(names, vec!["keep"]);
    assert_eq!(graph.excluded_members.len(), 1);
    assert!(
        graph.excluded_members[0]
            .to_string_lossy()
            .ends_with("skipme")
    );
}

#[test]
fn unused_exclude_pattern_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/keep"]
exclude = ["packages/missing"]
"#,
        )
        .unwrap();
    dir.child("packages/keep/cabin.toml")
        .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::UnusedExcludePattern { pattern, .. } => {
            assert_eq!(pattern, "packages/missing");
        }
        other => panic!("expected UnusedExcludePattern, got {other:?}"),
    }
}

#[test]
fn default_members_must_be_workspace_members() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/keep"]
default-members = ["packages/missing"]
"#,
        )
        .unwrap();
    dir.child("packages/keep/cabin.toml")
        .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::DefaultMemberNotInMembers { member } => {
            assert_eq!(member, "packages/missing");
        }
        other => panic!("expected DefaultMemberNotInMembers, got {other:?}"),
    }
}

#[test]
fn default_members_resolved_to_indices() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
default-members = ["packages/a"]
"#,
        )
        .unwrap();
    dir.child("packages/a/cabin.toml")
        .write_str("[package]\nname = \"a\"\nversion = \"0.1.0\"\n")
        .unwrap();
    dir.child("packages/b/cabin.toml")
        .write_str("[package]\nname = \"b\"\nversion = \"0.1.0\"\n")
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    assert_eq!(graph.default_members.len(), 1);
    let name = graph.packages[graph.default_members[0]]
        .package
        .name
        .as_str();
    assert_eq!(name, "a");
}

#[test]
fn workspace_dependency_inheritance() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10 <11"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    let app = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "app")
        .unwrap();
    assert_eq!(app.package.dependencies.len(), 1);
    match &app.package.dependencies[0].source {
        cabin_core::DependencySource::Version(req) => {
            assert!(req.to_string().contains(">=10"));
        }
        other => panic!("expected resolved Version, got {other:?}"),
    }
}

#[test]
fn workspace_dependency_inheritance_per_kind() {
    // Each `dep = { workspace = true }` looks up the matching
    // `[workspace.<kind>-dependencies]` table - never a sibling
    // table.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10"

[workspace.dev-dependencies]
gtest = "^1.14"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }

[dev-dependencies]
gtest = { workspace = true }
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    let app = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "app")
        .unwrap();
    for (name, kind) in [
        ("fmt", DependencyKind::Normal),
        ("gtest", DependencyKind::Dev),
    ] {
        let dep = app
            .package
            .dependencies
            .iter()
            .find(|d| d.name.as_str() == name && d.kind == kind)
            .unwrap_or_else(|| panic!("expected {name} in {kind:?}"));
        assert!(
            matches!(dep.source, cabin_core::DependencySource::Version(_)),
            "workspace inheritance should rewrite {name} into a Version source"
        );
    }
}

#[test]
fn workspace_dependency_kind_does_not_cross_tables() {
    // `[dev-dependencies] foo = { workspace = true }` must
    // *not* fall back to `[workspace.dependencies]` - the
    // lookup is strictly kind-specific.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dev-dependencies]
fmt = { workspace = true }
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::UnresolvedWorkspaceDependency {
            dep_name,
            parent,
            kind,
        } => {
            assert_eq!(dep_name, "fmt");
            assert_eq!(parent, "app");
            assert_eq!(kind, DependencyKind::Dev);
        }
        other => panic!("expected UnresolvedWorkspaceDependency for dev, got {other:?}"),
    }
}

#[test]
fn dev_path_dependency_is_not_loaded_into_graph() {
    // Dev path-deps are declaration-only: they appear on
    // `package.dependencies` but never become a graph node, so
    // a missing dev-dep target directory is *not* an error
    // for ordinary commands.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dev-dependencies]
harness = { path = "../harness-that-does-not-exist" }
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml"))
        .expect("dev path-dep should not be traversed by ordinary load");
    // Only the root package is loaded.
    assert_eq!(graph.packages.len(), 1);
    // But the package still records the declaration.
    let app = &graph.packages[0];
    assert_eq!(app.package.dependencies.len(), 1);
    assert_eq!(app.package.dependencies[0].kind, DependencyKind::Dev);
}

#[test]
fn unresolved_workspace_dependency_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::UnresolvedWorkspaceDependency {
            dep_name,
            parent,
            kind,
        } => {
            assert_eq!(dep_name, "fmt");
            assert_eq!(parent, "app");
            assert_eq!(kind, DependencyKind::Normal);
        }
        other => panic!("expected UnresolvedWorkspaceDependency, got {other:?}"),
    }
}

#[test]
fn workspace_standard_inheritance() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]
cxx-standard = "c++20"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = { workspace = true }
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    let app = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "app")
        .unwrap();
    assert_eq!(
        app.package.language.cxx_standard,
        Some(cabin_core::StandardDeclaration::Inherited(
            cabin_core::CxxStandard::Cxx20
        ))
    );
    assert_eq!(app.package.language.c_standard, None);
}

#[test]
fn workspace_standard_opt_in_without_declaration_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]
cxx-standard = "c++20"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
interface-cxx-standard = { workspace = true }
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::UnresolvedWorkspaceStandard { package, field, .. } => {
            assert_eq!(package, "app");
            assert_eq!(field, "interface-cxx-standard");
        }
        other => panic!("expected UnresolvedWorkspaceStandard, got {other:?}"),
    }
}

#[test]
fn root_package_may_opt_into_its_own_workspace_standards() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]
c-standard = "c17"

[package]
name = "root"
version = "0.1.0"
c-standard = { workspace = true }
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    let root = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "root")
        .unwrap();
    assert_eq!(
        root.package.language.c_standard,
        Some(cabin_core::StandardDeclaration::Inherited(
            cabin_core::CStandard::C17
        ))
    );
    // A member that did not opt in is untouched.
    let app = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "app")
        .unwrap();
    assert!(app.package.language.is_empty());
}

#[test]
fn standalone_package_standard_marker_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "solo"
version = "0.1.0"
cxx-standard = { workspace = true }
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    assert!(matches!(
        err,
        WorkspaceError::UnresolvedWorkspaceStandard {
            field: "cxx-standard",
            ..
        }
    ));
}

#[test]
fn member_literal_standard_stays_declared() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]
cxx-standard = "c++20"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++14"
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    let app = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "app")
        .unwrap();
    assert_eq!(
        app.package.language.cxx_standard,
        Some(cabin_core::StandardDeclaration::Declared(
            cabin_core::CxxStandard::Cxx14
        ))
    );
}

#[test]
fn multi_marker_member_errors_on_first_undeclared_field() {
    // Markers resolve in field order c -> cxx -> interface-c ->
    // interface-cxx, so when several fields opt in and the root
    // declares only a later one, the first undeclared field is the
    // one the error names.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]
cxx-standard = "c++20"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = { workspace = true }
cxx-standard = { workspace = true }
"#,
        )
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::UnresolvedWorkspaceStandard { package, field, .. } => {
            assert_eq!(package, "app");
            assert_eq!(field, "c-standard");
        }
        other => panic!("expected UnresolvedWorkspaceStandard, got {other:?}"),
    }
}

#[test]
fn non_member_path_dep_marker_resolves_against_consuming_workspace() {
    // A plain path dependency outside the member set still resolves
    // `{ workspace = true }` markers against the *consuming*
    // workspace's `[workspace]` defaults.  Intentional divergence
    // from Cargo, where inheritance resolves against the dep's own
    // workspace.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]
cxx-standard = "c++20"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
helper = { path = "../../helper" }
"#,
        )
        .unwrap();
    dir.child("helper/cabin.toml")
        .write_str(
            r#"[package]
name = "helper"
version = "0.1.0"
cxx-standard = { workspace = true }
"#,
        )
        .unwrap();
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    let helper = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "helper")
        .unwrap();
    assert_eq!(
        helper.package.language.cxx_standard,
        Some(cabin_core::StandardDeclaration::Inherited(
            cabin_core::CxxStandard::Cxx20
        ))
    );
}

#[test]
fn registry_package_with_workspace_standard_marker_is_rejected() {
    let dir = TempDir::new().unwrap();
    // The consuming workspace declares the very default a tampered
    // archive's marker would otherwise silently adopt.
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = []
cxx-standard = "c++20"

[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    // A hand-crafted registry archive carries an unresolved marker;
    // publish-side validation keeps legitimate archives marker-free.
    dir.child("registry/fmt/cabin.toml")
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"
cxx-standard = { workspace = true }
"#,
        )
        .unwrap();
    let registry = vec![RegistryPackageSource {
        name: pkg("fmt"),
        version: ver("10.2.1"),
        manifest_path: dir.path().join("registry/fmt/cabin.toml"),
    }];
    let err = load_workspace_with_options(
        dir.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    match err {
        WorkspaceError::ExternalPackageDeclaresWorkspaceStandard {
            origin,
            package,
            field,
            ..
        } => {
            assert_eq!(origin, "registry");
            assert_eq!(package, "fmt");
            assert_eq!(field, "cxx-standard");
        }
        other => panic!("expected ExternalPackageDeclaresWorkspaceStandard, got {other:?}"),
    }
}

#[test]
fn prepared_port_with_workspace_standard_marker_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let prepared = tmp.child("cache/sources/sha256/abc");
    prepared
        .child("cabin.toml")
        .write_str(
            r#"[package]
name = "zlib"
version = "1.3.1"
c-standard = { workspace = true }
"#,
        )
        .unwrap();
    let consumer = tmp.child("consumer");
    consumer
        .child("cabin.toml")
        .write_str(
            r#"[workspace]
members = []
c-standard = "c17"

[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^1.3" }
"#,
        )
        .unwrap();
    let port_sources = vec![PortPackageSource {
        name: PackageName::new("zlib").unwrap(),
        version: semver::Version::new(1, 3, 1),
        manifest_path: prepared.path().join("cabin.toml"),
        origin: cabin_port::PortOrigin::Builtin("zlib"),
    }];
    let err = load_workspace_with_options(
        consumer.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &[],
            patches: &[],
            ports: &port_sources,
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    match err {
        WorkspaceError::ExternalPackageDeclaresWorkspaceStandard {
            origin,
            package,
            field,
            ..
        } => {
            assert_eq!(origin, "foundation-port");
            assert_eq!(package, "zlib");
            assert_eq!(field, "c-standard");
        }
        other => panic!("expected ExternalPackageDeclaresWorkspaceStandard, got {other:?}"),
    }
}

#[test]
fn registry_package_with_workspace_dep_marker_is_rejected() {
    let dir = TempDir::new().unwrap();
    // The consuming workspace declares the very name a tampered
    // archive's marker would otherwise silently resolve against.
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = []

[workspace.dependencies]
fmt = "^1"

[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    // A hand-crafted registry archive carries an unresolved marker;
    // publish-side normalization keeps legitimate archives marker-free.
    dir.child("registry/fmt/cabin.toml")
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"

[dependencies]
fmt = { workspace = true }
"#,
        )
        .unwrap();
    let registry = vec![RegistryPackageSource {
        name: pkg("fmt"),
        version: ver("10.2.1"),
        manifest_path: dir.path().join("registry/fmt/cabin.toml"),
    }];
    let err = load_workspace_with_options(
        dir.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    match err {
        WorkspaceError::ExternalPackageDeclaresWorkspaceDependency {
            origin,
            package,
            dep_name,
            ..
        } => {
            assert_eq!(origin, "registry");
            assert_eq!(package, "fmt");
            assert_eq!(dep_name, "fmt");
        }
        other => panic!("expected ExternalPackageDeclaresWorkspaceDependency, got {other:?}"),
    }
}

#[test]
fn prepared_port_with_workspace_dep_marker_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let prepared = tmp.child("cache/sources/sha256/abc");
    prepared
        .child("cabin.toml")
        .write_str(
            r#"[package]
name = "zlib"
version = "1.3.1"

[dependencies]
fmt = { workspace = true }
"#,
        )
        .unwrap();
    let consumer = tmp.child("consumer");
    consumer
        .child("cabin.toml")
        .write_str(
            r#"[workspace]
members = []

[workspace.dependencies]
fmt = "^1"

[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^1.3" }
"#,
        )
        .unwrap();
    let port_sources = vec![PortPackageSource {
        name: PackageName::new("zlib").unwrap(),
        version: semver::Version::new(1, 3, 1),
        manifest_path: prepared.path().join("cabin.toml"),
        origin: cabin_port::PortOrigin::Builtin("zlib"),
    }];
    let err = load_workspace_with_options(
        consumer.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &[],
            patches: &[],
            ports: &port_sources,
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    match err {
        WorkspaceError::ExternalPackageDeclaresWorkspaceDependency {
            origin,
            package,
            dep_name,
            ..
        } => {
            assert_eq!(origin, "foundation-port");
            assert_eq!(package, "zlib");
            assert_eq!(dep_name, "fmt");
        }
        other => panic!("expected ExternalPackageDeclaresWorkspaceDependency, got {other:?}"),
    }
}

#[test]
fn nested_workspace_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["nested"]
"#,
        )
        .unwrap();
    dir.child("nested/cabin.toml")
        .write_str(
            r"[workspace]
members = []
",
        )
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::NestedWorkspace { path } => {
            assert!(path.to_string_lossy().contains("nested"));
        }
        other => panic!("expected NestedWorkspace, got {other:?}"),
    }
}

#[test]
fn member_expansion_is_deterministic() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    for name in ["zeta", "alpha", "mu", "kappa"] {
        dir.child(format!("packages/{name}/cabin.toml"))
            .write_str(&format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"
            ))
            .unwrap();
    }
    let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
    let names: Vec<&str> = graph
        .primary_packages
        .iter()
        .map(|i| graph.packages[*i].package.name.as_str())
        .collect();
    assert_eq!(names, vec!["alpha", "kappa", "mu", "zeta"]);
}

// -----------------------------------------------------------------
// Workspace pattern paths must be relative to the workspace root.
// Absolute and `..` patterns are rejected.
// -----------------------------------------------------------------

fn workspace_with_outside_member(pattern: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(&format!("[workspace]\nmembers = [\"{pattern}\"]\n"))
        .unwrap();
    dir
}

#[test]
fn member_pattern_with_absolute_path_rejected() {
    // The pattern must be *absolute on the host*: `/tmp/outside` is
    // absolute on Unix but not on Windows (which needs a drive), so
    // there a drive-rooted, forward-slash (TOML-safe) spelling is
    // used.  The manifest never reaches the FS in the failing branch.
    let outside = if cfg!(windows) {
        "C:/tmp/outside"
    } else {
        "/tmp/outside"
    };
    let dir = workspace_with_outside_member(outside);
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::WorkspacePatternEscapesRoot { field, pattern } => {
            assert_eq!(field, "workspace.members");
            assert_eq!(pattern, outside);
        }
        other => panic!("expected WorkspacePatternEscapesRoot, got {other:?}"),
    }
}

#[test]
fn member_pattern_with_parent_dir_rejected() {
    // Set up a sibling directory the pattern would pull in,
    // proving the validator stops the load *before* expansion.
    let dir = TempDir::new().unwrap();
    let workspace_dir = dir.child("ws");
    let outside_dir = dir.child("outside");
    workspace_dir
        .child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["../outside"]
"#,
        )
        .unwrap();
    outside_dir
        .child("cabin.toml")
        .write_str("[package]\nname = \"sneaky\"\nversion = \"0.1.0\"\n")
        .unwrap();
    let err = load_workspace(workspace_dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::WorkspacePatternEscapesRoot { field, pattern } => {
            assert_eq!(field, "workspace.members");
            assert_eq!(pattern, "../outside");
        }
        other => panic!("expected WorkspacePatternEscapesRoot, got {other:?}"),
    }
}

#[test]
fn exclude_pattern_with_parent_dir_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/keep"]
exclude = ["../outside"]
"#,
        )
        .unwrap();
    dir.child("packages/keep/cabin.toml")
        .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::WorkspacePatternEscapesRoot { field, pattern } => {
            assert_eq!(field, "workspace.exclude");
            assert_eq!(pattern, "../outside");
        }
        other => panic!("expected WorkspacePatternEscapesRoot, got {other:?}"),
    }
}

#[test]
fn default_member_with_parent_dir_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/keep"]
default-members = ["../outside"]
"#,
        )
        .unwrap();
    dir.child("packages/keep/cabin.toml")
        .write_str("[package]\nname = \"keep\"\nversion = \"0.1.0\"\n")
        .unwrap();
    let err = load_workspace(dir.path().join("cabin.toml")).unwrap_err();
    match err {
        WorkspaceError::WorkspacePatternEscapesRoot { field, pattern } => {
            assert_eq!(field, "workspace.default-members");
            assert_eq!(pattern, "../outside");
        }
        other => panic!("expected WorkspacePatternEscapesRoot, got {other:?}"),
    }
}

// -----------------------------------------------------------------
// selection-aware registry materialization.
// -----------------------------------------------------------------

#[test]
fn for_selection_skips_versioned_deps_outside_strict_set() {
    // app needs fmt; unrelated `b` declares spdlog.  The
    // strict set is {app}; the registry only has fmt.  Loading
    // must succeed because b's spdlog dep is skipped.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10 <11"
"#,
        )
        .unwrap();
    dir.child("packages/b/cabin.toml")
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
spdlog = "^1"
"#,
        )
        .unwrap();
    // Pretend we already extracted fmt 10.2.1 somewhere on disk.
    dir.child("registry/fmt/cabin.toml")
        .write_str("[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n")
        .unwrap();
    let registry = vec![RegistryPackageSource {
        name: PackageName::new("fmt").unwrap(),
        version: ver("10.2.1"),
        manifest_path: dir.path().join("registry/fmt/cabin.toml"),
    }];
    let mut strict: BTreeSet<String> = BTreeSet::new();
    strict.insert("app".into());
    let graph = load_workspace_with_options(
        dir.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::StrictFor(&strict),
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .expect("selection-aware load should not require spdlog");
    // app, b, and fmt all loaded; no `spdlog` was added.
    let names: BTreeSet<&str> = graph
        .packages
        .iter()
        .map(|p| p.package.name.as_str())
        .collect();
    assert!(names.contains("app"));
    assert!(names.contains("b"));
    assert!(names.contains("fmt"));
    assert!(!names.contains("spdlog"));
}

#[test]
fn for_selection_still_errors_when_strict_dep_missing() {
    // app is strict and depends on fmt, but the registry is
    // empty.  The selection-aware loader must still error on
    // app's missing fmt.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10 <11"
"#,
        )
        .unwrap();
    // A non-empty registry shifts the loader out of the
    // legacy "skip versioned deps" mode.  Build a sham entry
    // for some other package so registry_by_name is
    // populated but does not contain `fmt`.
    dir.child("registry/other/cabin.toml")
        .write_str("[package]\nname = \"other\"\nversion = \"1.0.0\"\n")
        .unwrap();
    let registry = vec![RegistryPackageSource {
        name: PackageName::new("other").unwrap(),
        version: ver("1.0.0"),
        manifest_path: dir.path().join("registry/other/cabin.toml"),
    }];
    let mut strict: BTreeSet<String> = BTreeSet::new();
    strict.insert("app".into());
    let err = load_workspace_with_options(
        dir.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &registry,
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::StrictFor(&strict),
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .expect_err("expected UnresolvedRegistryDependency for selected closure dep");
    match err {
        WorkspaceError::UnresolvedRegistryDependency { dep_name, parent } => {
            assert_eq!(dep_name, "fmt");
            assert_eq!(parent, "app");
        }
        other => panic!("expected UnresolvedRegistryDependency, got {other:?}"),
    }
}

// ---------------------------------------------------------------
// Foundation-port resolution
// ---------------------------------------------------------------

#[test]
fn resolves_port_dep_via_supplied_source() {
    let tmp = TempDir::new().unwrap();

    // Port directory (contains port.toml in real life, but
    // the workspace loader only cares about the canonical
    // path).
    let port_dir = tmp.child("ports/zlib/1.3.1");
    port_dir.create_dir_all().unwrap();

    // Prepared overlay manifest directory (the CLI
    // orchestration step writes the upstream sources here
    // before the loader runs).
    let prepared = tmp.child("cache/sources/sha256/abc");
    prepared
            .child("cabin.toml")
            .write_str(
                "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[target.zlib]\ntype = \"library\"\nsources = [\"zlib.c\"]\n",
            )
            .unwrap();
    prepared
        .child("zlib.c")
        .write_str("int zlib_dummy(void){return 0;}\n")
        .unwrap();

    // Consumer manifest that references the port by
    // relative path.
    let consumer = tmp.child("consumer");
    consumer
        .child("cabin.toml")
        .write_str(
            r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }

[target.consumer]
type = "executable"
sources = ["src/main.c"]
deps = ["zlib"]
"#,
        )
        .unwrap();
    consumer
        .child("src/main.c")
        .write_str("int main(void){return 0;}\n")
        .unwrap();

    let port_sources = vec![PortPackageSource {
        name: PackageName::new("zlib").unwrap(),
        version: semver::Version::new(1, 3, 1),
        manifest_path: prepared.path().join("cabin.toml"),
        origin: cabin_port::PortOrigin::PortDir(port_dir.to_path_buf()),
    }];
    let graph = load_workspace_with_options(
        consumer.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &[],
            patches: &[],
            ports: &port_sources,
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap();
    // Two packages: the consumer and the zlib port.
    assert_eq!(graph.packages.len(), 2);
    let zlib = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "zlib")
        .unwrap();
    assert_eq!(
        zlib.manifest_dir,
        // Match the loader's own verbatim-stripped spelling so the
        // expectation holds on Windows, where `std::fs::canonicalize`
        // would add a `\\?\` prefix the loader does not carry.
        cabin_fs::canonicalize(prepared.path()).unwrap()
    );
    // Foundation ports are local development policy, so the
    // package kind is Local.
    assert_eq!(zlib.kind, PackageKind::Local);
}

#[test]
fn resolves_builtin_port_dep_by_name() {
    let tmp = TempDir::new().unwrap();

    // The "prepared" overlay (in a real build this is in the
    // cabin cache).  The loader only needs the [package] block
    // to match the dep, plus a source file for the target.
    let prepared = tmp.child("cache/sources/sha256/abc");
    prepared
            .child("cabin.toml")
            .write_str(
                "[package]\nname = \"zlib\"\nversion = \"1.3.1\"\n\n[target.zlib]\ntype = \"library\"\nsources = [\"zlib.c\"]\n",
            )
            .unwrap();
    prepared
        .child("zlib.c")
        .write_str("int zlib_dummy(void){return 0;}\n")
        .unwrap();

    let consumer = tmp.child("consumer");
    consumer
        .child("cabin.toml")
        .write_str(
            r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^1.3" }

[target.consumer]
type = "executable"
sources = ["src/main.c"]
deps = ["zlib"]
"#,
        )
        .unwrap();
    consumer
        .child("src/main.c")
        .write_str("int main(void){return 0;}\n")
        .unwrap();

    let port_sources = vec![PortPackageSource {
        name: PackageName::new("zlib").unwrap(),
        version: semver::Version::new(1, 3, 1),
        manifest_path: prepared.path().join("cabin.toml"),
        origin: cabin_port::PortOrigin::Builtin("zlib"),
    }];
    let graph = load_workspace_with_options(
        consumer.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &[],
            patches: &[],
            ports: &port_sources,
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap();
    assert_eq!(graph.packages.len(), 2);
    let zlib = graph
        .packages
        .iter()
        .find(|p| p.package.name.as_str() == "zlib")
        .unwrap();
    assert_eq!(zlib.kind, PackageKind::Local);
}

#[test]
fn rejects_port_dep_without_prepared_source() {
    let tmp = TempDir::new().unwrap();
    let port_dir = tmp.child("ports/zlib/1.3.1");
    port_dir.create_dir_all().unwrap();

    let consumer = tmp.child("consumer");
    consumer
        .child("cabin.toml")
        .write_str(
            r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../ports/zlib/1.3.1" }
"#,
        )
        .unwrap();

    let err = load_workspace_with_options(
        consumer.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &[],
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    assert!(
        matches!(err, WorkspaceError::PortDependencyNotPrepared { .. }),
        "{err:?}"
    );
}

#[test]
fn rejects_port_dep_with_missing_port_directory() {
    let tmp = TempDir::new().unwrap();

    let consumer = tmp.child("consumer");
    consumer
        .child("cabin.toml")
        .write_str(
            r#"
[package]
name = "consumer"
version = "0.1.0"

[dependencies]
zlib = { port-path = "../nonexistent/zlib" }
"#,
        )
        .unwrap();

    let err = load_workspace_with_options(
        consumer.path().join("cabin.toml"),
        &WorkspaceLoadOptions {
            registry: &[],
            patches: &[],
            ports: &[],
            registry_policy: RegistryPolicy::Strict,
            include_dev_for: &BTreeSet::new(),
            port_policy: PortPolicy::Strict,
        },
    )
    .unwrap_err();
    assert!(
        matches!(err, WorkspaceError::PortDirectoryMissing { .. }),
        "{err:?}"
    );
}
