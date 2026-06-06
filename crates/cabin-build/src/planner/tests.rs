#[cfg(unix)]
use std::path::Path;

use camino::Utf8Path;

#[cfg(unix)]
use super::lowering::promote_dir;
use super::*;
use cabin_core::{
    Dependency, DependencySource, Package, PackageName, ProfileDefinition, ProfileName,
    ProfileSelection, ResolvedProfile, Target as CoreTarget, TargetName, resolve_profile,
};
use cabin_workspace::{PackageGraph, PackageKind, WorkspacePackage};
use camino::Utf8PathBuf;
use std::collections::BTreeMap;

use cabin_driver::{LoweredAction, LoweredActionKind, lower};

/// Lower a semantic action to inspect the concrete argv / backend
/// kind the Ninja writer will render. Lowering is infallible because
/// the semantic IR already carries UTF-8 paths. These tests anchor
/// on the GNU/Clang dialect, the historic default.
fn lowered(action: &BuildAction) -> LoweredAction {
    lower(Dialect::GnuLike, action)
}

/// The lowered (backend) kind of each action, in graph order.
fn lowered_kinds(bg: &BuildGraph) -> Vec<LoweredActionKind> {
    bg.actions.iter().map(|a| lowered(a).kind).collect()
}

/// Borrow every compile action in the graph, in order.
fn compile_actions(bg: &BuildGraph) -> Vec<&CompileAction> {
    bg.actions
        .iter()
        .filter_map(|a| match a {
            BuildAction::Compile(c) => Some(c),
            _ => None,
        })
        .collect()
}

/// The single link action in the graph.
fn link_action(bg: &BuildGraph) -> &LinkAction {
    bg.actions
        .iter()
        .find_map(|a| match a {
            BuildAction::Link(l) => Some(l),
            _ => None,
        })
        .expect("link action present")
}

/// Primary output of an action (object, library, or executable;
/// the stamp in syntax-only mode).
fn primary_output(action: &BuildAction) -> &Utf8Path {
    match action {
        BuildAction::Compile(c) => match &c.mode {
            CompileMode::Object => &c.object,
            CompileMode::SyntaxOnly { stamp } => stamp,
        },
        BuildAction::Archive(a) => &a.output,
        BuildAction::Link(l) => &l.output,
    }
}

fn dev_profile() -> ResolvedProfile {
    resolve_profile(
        &ProfileSelection::default_dev(),
        &BTreeMap::<ProfileName, ProfileDefinition>::new(),
    )
    .expect("built-in dev resolves")
}

fn release_profile() -> ResolvedProfile {
    resolve_profile(
        &ProfileSelection::release_alias(),
        &BTreeMap::<ProfileName, ProfileDefinition>::new(),
    )
    .expect("built-in release resolves")
}

fn version() -> semver::Version {
    semver::Version::parse("0.1.0").unwrap()
}

fn pkg_name(name: &str) -> PackageName {
    PackageName::new(name).unwrap()
}

fn target_name(name: &str) -> TargetName {
    TargetName::new(name).unwrap()
}

fn target(name: &str, kind: TargetKind, sources: &[&str], deps: &[&str]) -> CoreTarget {
    CoreTarget {
        name: target_name(name),
        kind,
        sources: sources.iter().map(Utf8PathBuf::from).collect(),
        include_dirs: Vec::new(),
        defines: Vec::new(),
        deps: deps.iter().map(|d| (*d).to_owned()).collect(),
    }
}

fn target_with_includes(
    name: &str,
    kind: TargetKind,
    sources: &[&str],
    includes: &[&str],
    deps: &[&str],
) -> CoreTarget {
    CoreTarget {
        name: target_name(name),
        kind,
        sources: sources.iter().map(Utf8PathBuf::from).collect(),
        include_dirs: includes.iter().map(Utf8PathBuf::from).collect(),
        defines: Vec::new(),
        deps: deps.iter().map(|d| (*d).to_owned()).collect(),
    }
}

fn dep(name: &str, path: &str) -> Dependency {
    Dependency {
        name: pkg_name(name),
        source: DependencySource::Path(Utf8PathBuf::from(path)),
        kind: cabin_core::DependencyKind::Normal,
        optional: false,
        features: Vec::new(),
        default_features: true,
        condition: None,
    }
}

fn toolchain() -> ResolvedToolchain {
    use cabin_core::{ResolvedTool, ToolKind, ToolSource, ToolSpec};
    ResolvedToolchain {
        cxx: ResolvedTool {
            kind: ToolKind::CxxCompiler,
            path: Utf8PathBuf::from("/usr/bin/g++"),
            spec: ToolSpec::Name("g++".into()),
            source: ToolSource::Default,
        },
        ar: ResolvedTool {
            kind: ToolKind::Archiver,
            path: Utf8PathBuf::from("/usr/bin/ar"),
            spec: ToolSpec::Name("ar".into()),
            source: ToolSource::Default,
        },
        cc: None,
    }
}

/// Toolchain with both compilers resolved. Used by tests that
/// exercise the C compile path or the link-driver pick.
fn toolchain_with_cc() -> ResolvedToolchain {
    use cabin_core::{ResolvedTool, ToolKind, ToolSource, ToolSpec};
    let mut tc = toolchain();
    tc.cc = Some(ResolvedTool {
        kind: ToolKind::CCompiler,
        path: Utf8PathBuf::from("/usr/bin/cc"),
        spec: ToolSpec::Name("cc".into()),
        source: ToolSource::Default,
    });
    tc
}

fn empty_build_flags() -> HashMap<usize, ResolvedProfileFlags> {
    HashMap::new()
}

fn make_pkg(
    _name: &str,
    manifest_dir: &str,
    package: Package,
    deps: Vec<usize>,
) -> WorkspacePackage {
    let manifest_dir = PathBuf::from(manifest_dir);
    let manifest_path = manifest_dir.join("cabin.toml");
    WorkspacePackage {
        package,
        manifest_path,
        manifest_dir,
        deps: deps
            .into_iter()
            .map(|index| cabin_workspace::DependencyEdge {
                index,
                kind: cabin_core::DependencyKind::Normal,
                condition: None,
            })
            .collect(),
        kind: PackageKind::Local,
    }
}

fn graph_with(
    packages: Vec<WorkspacePackage>,
    primaries: Vec<usize>,
    root: Option<usize>,
) -> PackageGraph {
    let root_dir = packages
        .first()
        .map_or_else(|| PathBuf::from("/abs"), |p| p.manifest_dir.clone());
    let root_manifest = root_dir.join("cabin.toml");
    PackageGraph {
        root_manifest_path: root_manifest,
        root_dir,
        is_workspace_root: false,
        root_package: root,
        root_settings: Default::default(),
        primary_packages: primaries,
        default_members: Vec::new(),
        excluded_members: Vec::new(),
        packages,
    }
}

fn single_package_graph(package: Package, manifest_dir: &str) -> PackageGraph {
    let name = package.name.as_str().to_owned();
    let pkg = make_pkg(&name, manifest_dir, package, vec![]);
    graph_with(vec![pkg], vec![0], Some(0))
}

#[test]
fn plans_single_executable() {
    let package = Package::new(
        pkg_name("hello"),
        version(),
        vec![target(
            "hello",
            TargetKind::Executable,
            &["src/main.cc"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let req = PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    };
    let bg = plan(&req).unwrap();
    assert_eq!(bg.actions.len(), 2);
    assert_eq!(
        lowered_kinds(&bg),
        vec![
            LoweredActionKind::CompileCpp,
            LoweredActionKind::LinkExecutable
        ]
    );
    assert_eq!(
        bg.default_outputs,
        vec![Utf8PathBuf::from(
            "/abs/proj/build/dev/packages/hello/hello"
        )]
    );
    let cc = &bg.compile_commands[0];
    assert_eq!(
        cc.output,
        Utf8PathBuf::from("/abs/proj/build/dev/packages/hello/obj/hello/src/main.cc.o")
    );
    assert!(cc.arguments.iter().any(|a| a == "-std=c++17"));
}

#[test]
fn compiler_wrapper_prefixes_only_the_ninja_command() {
    let package = Package::new(
        pkg_name("hello"),
        version(),
        vec![target(
            "hello",
            TargetKind::Executable,
            &["src/main.cc"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let wrapper = ResolvedCompilerWrapper {
        kind: cabin_core::CompilerWrapperKind::Ccache,
        path: Utf8PathBuf::from("/usr/local/bin/ccache"),
        spec: "ccache".into(),
        source: cabin_core::CompilerWrapperSource::Cli,
        identity: None,
    };
    let req = PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: Some(&wrapper),
        dialect: Dialect::GnuLike,
    };
    let bg = plan(&req).unwrap();
    let compile = lowered(
        bg.actions
            .iter()
            .find(|a| matches!(a, BuildAction::Compile(_)))
            .expect("compile action present"),
    );
    assert_eq!(compile.command[0], "/usr/local/bin/ccache");
    assert_eq!(compile.command[1], "/usr/bin/g++");
    let cc = &bg.compile_commands[0];
    // compile_commands.json must keep the underlying compiler
    // first so clangd / IDE tooling continues to recognize the
    // command shape.
    assert_eq!(cc.arguments[0], "/usr/bin/g++");
    // Link / archive paths are never wrapped.
    let link = lowered(
        bg.actions
            .iter()
            .find(|a| matches!(a, BuildAction::Link(_)))
            .expect("link action present"),
    );
    assert_eq!(link.command[0], "/usr/bin/g++");
    assert!(
        !link.command.iter().any(|a| a == "/usr/local/bin/ccache"),
        "wrapper must not appear in link command"
    );
}

#[test]
fn compiler_wrapper_does_not_prefix_c_compile_commands() {
    let package = Package::new(
        pkg_name("hello"),
        version(),
        vec![target(
            "hello",
            TargetKind::Executable,
            &["src/main.c"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain_with_cc();
    let wrapper = ResolvedCompilerWrapper {
        kind: cabin_core::CompilerWrapperKind::Ccache,
        path: Utf8PathBuf::from("/usr/local/bin/ccache"),
        spec: "ccache".into(),
        source: cabin_core::CompilerWrapperSource::Cli,
        identity: None,
    };
    let req = PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: Some(&wrapper),
        dialect: Dialect::GnuLike,
    };
    let bg = plan(&req).unwrap();
    let compile = lowered(
        bg.actions
            .iter()
            .find(|a| matches!(a, BuildAction::Compile(c) if c.language == SourceLanguage::C))
            .expect("C compile action present"),
    );
    assert_eq!(compile.command[0], "/usr/bin/cc");
    assert!(
        !compile.command.iter().any(|a| a == "/usr/local/bin/ccache"),
        "wrapper must not appear in C compile command"
    );
}

#[test]
fn release_profile_uses_release_flags() {
    let package = Package::new(
        pkg_name("hello"),
        version(),
        vec![target(
            "hello",
            TargetKind::Executable,
            &["src/main.cc"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: release_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();
    let cc = &bg.compile_commands[0];
    assert!(cc.arguments.iter().any(|a| a == "-O3"));
    assert!(cc.arguments.iter().any(|a| a == "-DNDEBUG"));
    assert!(!cc.arguments.iter().any(|a| a == "-O0"));
}

#[test]
fn plans_library_then_executable_within_one_package() {
    let package = Package::new(
        pkg_name("multi"),
        version(),
        vec![
            target_with_includes(
                "greet",
                TargetKind::Library,
                &["src/greet.cc"],
                &["include"],
                &[],
            ),
            target(
                "hello",
                TargetKind::Executable,
                &["src/main.cc"],
                &["greet"],
            ),
        ],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();
    assert_eq!(
        lowered_kinds(&bg),
        vec![
            LoweredActionKind::CompileCpp,
            LoweredActionKind::ArchiveStaticLibrary,
            LoweredActionKind::CompileCpp,
            LoweredActionKind::LinkExecutable,
        ]
    );
    let BuildAction::Link(link) = &bg.actions[3] else {
        panic!("expected a link action at index 3");
    };
    assert!(link.inputs.contains(&Utf8PathBuf::from(
        "/abs/proj/build/dev/packages/multi/libgreet.a"
    )));
    let BuildAction::Compile(hello_compile) = &bg.actions[2] else {
        panic!("expected a compile action at index 2");
    };
    // greet's include dir is carried semantically, not yet as argv.
    assert!(
        hello_compile
            .arguments
            .include_dirs
            .contains(&Utf8PathBuf::from("/abs/proj/include"))
    );
}

#[test]
fn cross_package_path_dep_links_library() {
    // greet at /abs/greet, app at /abs/app depending on greet.
    let greet_proj = Package::new(
        pkg_name("greet"),
        version(),
        vec![target_with_includes(
            "greet",
            TargetKind::Library,
            &["src/greet.cc"],
            &["include"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let app_proj = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            &["greet"],
        )],
        vec![dep("greet", "../greet")],
    )
    .unwrap();
    let greet_pkg = make_pkg("greet", "/abs/greet", greet_proj, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
    let graph = graph_with(vec![greet_pkg, app_pkg], vec![1], Some(1));
    let tc = toolchain();
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();

    // Outputs should be namespaced by package.
    let greet_lib = Utf8PathBuf::from("/abs/build/dev/packages/greet/libgreet.a");
    let app_exe = Utf8PathBuf::from("/abs/build/dev/packages/app/app");
    // app's link action must include greet's static archive.
    let link = link_action(&bg);
    assert!(link.inputs.contains(&greet_lib));
    assert_eq!(link.output, app_exe);

    // Default outputs are only the primary package's targets (app).
    assert_eq!(bg.default_outputs, vec![app_exe]);

    // greet's include dir should propagate into app's compile action.
    let app_compile = compile_actions(&bg)
        .into_iter()
        // Normalize separators: object paths join with `\` on Windows.
        .find(|c| c.object.as_str().replace('\\', "/").contains("/app/"))
        .expect("app compile action present");
    assert!(
        app_compile
            .arguments
            .include_dirs
            .contains(&Utf8PathBuf::from("/abs/greet/include"))
    );
}

#[test]
fn qualified_target_selector_picks_specific_target() {
    let greet_proj = Package::new(
        pkg_name("greet"),
        version(),
        vec![target("greet", TargetKind::Library, &["src/greet.cc"], &[])],
        Vec::new(),
    )
    .unwrap();
    let app_proj = Package::new(
        pkg_name("app"),
        version(),
        vec![
            target("app", TargetKind::Executable, &["src/main.cc"], &["greet"]),
            target("other", TargetKind::Executable, &["src/other.cc"], &[]),
        ],
        vec![dep("greet", "../greet")],
    )
    .unwrap();
    let greet_pkg = make_pkg("greet", "/abs/greet", greet_proj, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
    let graph = graph_with(vec![greet_pkg, app_pkg], vec![1], Some(1));
    let tc = toolchain();
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("app:app")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();
    // Only app:app and greet:greet should appear; not app:other.
    let outs: Vec<String> = bg
        .actions
        .iter()
        // Normalize separators: output paths join with `\` on Windows.
        .map(|a| primary_output(a).to_string().replace('\\', "/"))
        .collect();
    assert!(outs.iter().any(|o| o.ends_with("/packages/app/app")));
    assert!(!outs.iter().any(|o| o.contains("/packages/app/other")));
}

#[test]
fn ambiguous_unqualified_target_errors() {
    // Workspace with two member packages each having a target "build".
    let a = Package::new(
        pkg_name("a"),
        version(),
        vec![target("build", TargetKind::Executable, &["a.cc"], &[])],
        Vec::new(),
    )
    .unwrap();
    let b = Package::new(
        pkg_name("b"),
        version(),
        vec![target("build", TargetKind::Executable, &["b.cc"], &[])],
        Vec::new(),
    )
    .unwrap();
    let pkg_a = make_pkg("a", "/abs/a", a, vec![]);
    let pkg_b = make_pkg("b", "/abs/b", b, vec![]);
    let mut graph = graph_with(vec![pkg_a, pkg_b], vec![0, 1], None);
    graph.is_workspace_root = true;
    let tc = toolchain();
    let err = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("build")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap_err();
    assert!(matches!(err, BuildError::AmbiguousTarget(_, _)));
}

#[test]
fn unknown_package_in_qualified_selector_errors() {
    let package = Package::new(
        pkg_name("hello"),
        version(),
        vec![target(
            "hello",
            TargetKind::Executable,
            &["src/main.cc"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let err = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("nope:thing")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap_err();
    assert!(matches!(
        err,
        BuildError::UnknownPackageInTargetSelector { .. }
    ));
}

#[test]
fn target_dep_cycle_within_package_is_reported() {
    let package = Package {
        name: pkg_name("cyc"),
        version: version(),
        targets: vec![
            target("a", TargetKind::Library, &["a.cc"], &["b"]),
            target("b", TargetKind::Library, &["b.cc"], &["a"]),
        ],
        dependencies: Vec::new(),
        system_dependencies: Vec::new(),
        features: Default::default(),
        profiles: Default::default(),
        toolchain: Default::default(),
        build: Default::default(),
        compiler_wrapper: Default::default(),
        patches: Default::default(),
    };
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let err = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap_err();
    match err {
        BuildError::DependencyCycle(cycle) => {
            assert_eq!(cycle.first(), cycle.last());
            assert!(cycle.iter().any(|s| s == "cyc:a"));
            assert!(cycle.iter().any(|s| s == "cyc:b"));
        }
        other => panic!("expected DependencyCycle, got {other:?}"),
    }
}

#[test]
fn unknown_target_in_qualified_selector_errors() {
    let package = Package::new(
        pkg_name("hello"),
        version(),
        vec![target(
            "hello",
            TargetKind::Executable,
            &["src/main.cc"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let err = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("hello:missing")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap_err();
    assert!(matches!(err, BuildError::UnknownTargetInPackage { .. }));
}

/// Helper: the lowered link-action argv of a planned graph, so
/// tests can assert on `command[0]` (the chosen driver). Panics if
/// no link action is present.
fn link_command(bg: &BuildGraph) -> Vec<String> {
    let action = bg
        .actions
        .iter()
        .find(|a| matches!(a, BuildAction::Link(_)))
        .expect("link action present");
    lowered(action).command
}

#[test]
fn link_driver_is_c_when_target_has_only_c_sources() {
    // A pure-C executable must link through the C driver so
    // the C++ runtime is not pulled in.
    let package = Package::new(
        pkg_name("cdemo"),
        version(),
        vec![target(
            "cdemo_exe",
            TargetKind::Executable,
            &["src/main.c"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/cdemo");
    let tc = toolchain_with_cc();
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/cdemo/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();
    let link = link_command(&bg);
    assert_eq!(link[0], "/usr/bin/cc");
}

#[test]
fn link_driver_is_cxx_when_target_has_any_cpp_source() {
    // Mixed C/C++ executable in a single target must link
    // through the C++ driver because the closure has C++
    // objects.
    let package = Package::new(
        pkg_name("mixed"),
        version(),
        vec![target(
            "mixed_exe",
            TargetKind::Executable,
            &["src/c_part.c", "src/cpp_part.cc"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/mixed");
    let tc = toolchain_with_cc();
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/mixed/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();
    let link = link_command(&bg);
    assert_eq!(link[0], "/usr/bin/g++");
}

#[test]
fn link_driver_is_cxx_when_dependency_has_cpp_objects() {
    // Pure-C executable that links a C++ static library
    // must use the C++ driver — the runtime is required
    // because the library carries C++ objects.
    let cpp_lib = target("cppcore", TargetKind::Library, &["src/cpp_part.cc"], &[]);
    let c_exe = target(
        "c_runner",
        TargetKind::Executable,
        &["src/main.c"],
        &["cppcore"],
    );
    let package = Package::new(
        pkg_name("interop"),
        version(),
        vec![cpp_lib, c_exe],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/interop");
    let tc = toolchain_with_cc();
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/interop/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("c_runner")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();
    let link = link_command(&bg);
    assert_eq!(link[0], "/usr/bin/g++");
}

#[test]
fn link_driver_stays_c_when_dependency_is_also_pure_c() {
    // C executable + C library: still link through the C
    // driver because the closure has no C++ objects.
    let c_lib = target("ccore", TargetKind::Library, &["src/util.c"], &[]);
    let c_exe = target(
        "c_runner",
        TargetKind::Executable,
        &["src/main.c"],
        &["ccore"],
    );
    let package = Package::new(
        pkg_name("clib_only"),
        version(),
        vec![c_lib, c_exe],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/clib_only");
    let tc = toolchain_with_cc();
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/clib_only/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("c_runner")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();
    let link = link_command(&bg);
    assert_eq!(link[0], "/usr/bin/cc");
}

#[test]
fn missing_c_compiler_yields_actionable_error_with_target_id() {
    // C source + no `cc` resolved → MissingCCompiler error
    // that names both the package and the target so a
    // monorepo user can map the failure to the right
    // manifest.
    let package = Package::new(
        pkg_name("cdemo"),
        version(),
        vec![target(
            "cdemo_exe",
            TargetKind::Executable,
            &["src/main.c"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/cdemo");
    let tc = toolchain(); // no cc populated
    let err = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/cdemo/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap_err();
    let rendered = err.to_string();
    assert!(
        rendered.contains("cdemo:cdemo_exe"),
        "error should name the package:target, got: {rendered}"
    );
    assert!(
        rendered.contains("CC") || rendered.contains("`--cc"),
        "error should suggest how to set the C compiler, got: {rendered}"
    );
}

#[test]
fn unrecognized_source_extension_yields_actionable_error() {
    let package = Package::new(
        pkg_name("broken"),
        version(),
        vec![target(
            "broken",
            TargetKind::Library,
            &["src/file.txt"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/broken");
    let tc = toolchain_with_cc();
    let err = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        build_dir: PathBuf::from("/abs/broken/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap_err();
    let rendered = err.to_string();
    assert!(
        rendered.contains("broken:broken"),
        "error should name package:target, got: {rendered}"
    );
    assert!(
        rendered.contains(".c") && rendered.contains(".cc"),
        "error should enumerate the supported extensions, got: {rendered}"
    );
}

// The standard-flag-per-language and profile-flag ordering is now
// owned and tested by `cabin-driver`'s GNU/Clang lowering; the
// planner tests below assert it end-to-end through the lowered
// `compile_commands` argv instead.

/// Build a single-package graph with a `mixed` library
/// target carrying one C source and one C++ source. Used by
/// the per-language flag-routing tests below.
fn graph_with_mixed_sources() -> PackageGraph {
    let package = Package::new(
        pkg_name("mixed"),
        version(),
        vec![target(
            "mixed",
            TargetKind::Library,
            &["src/c_part.c", "src/cpp_part.cc"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    single_package_graph(package, "/abs/mixed")
}

/// Build per-package flag map carrying a single
/// `ResolvedProfileFlags` for the mixed package.
fn build_flags_map(flags: ResolvedProfileFlags) -> HashMap<usize, ResolvedProfileFlags> {
    let mut out = HashMap::new();
    out.insert(0usize, flags);
    out
}

/// Plan and return the compile actions for the mixed
/// fixture under the supplied build flags. Used by every
/// per-language flag-routing test below to keep the
/// boilerplate to one place.
fn plan_compile_actions(flags: ResolvedProfileFlags) -> Vec<CompileAction> {
    let graph = graph_with_mixed_sources();
    let tc = toolchain_with_cc();
    let map = build_flags_map(flags);
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &map,
        build_dir: PathBuf::from("/abs/mixed/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();
    bg.actions
        .into_iter()
        .filter_map(|a| match a {
            BuildAction::Compile(c) => Some(c),
            _ => None,
        })
        .collect()
}

fn compile_action_for(actions: &[CompileAction], language: SourceLanguage) -> &CompileAction {
    actions
        .iter()
        .find(|c| c.language == language)
        .unwrap_or_else(|| panic!("expected a {language:?} compile action"))
}

#[test]
fn cflags_route_to_c_compile_only() {
    // The C-only escape-hatch reaches every C compile
    // command and never reaches a C++ compile. Without this
    // routing, a flag that is invalid for C++ (`-std=c99`,
    // `-Wno-pointer-sign`) would break C++ builds.
    let flags = ResolvedProfileFlags {
        cflags: vec!["-DC_ONLY_FLAG=1".to_owned()],
        ..ResolvedProfileFlags::default()
    };
    let actions = plan_compile_actions(flags);
    let c = compile_action_for(&actions, SourceLanguage::C);
    let cxx = compile_action_for(&actions, SourceLanguage::Cxx);
    assert!(
        c.arguments
            .extra_flags
            .iter()
            .any(|a| a == "-DC_ONLY_FLAG=1"),
        "C compile must include the C-only define, got: {:?}",
        c.arguments.extra_flags
    );
    assert!(
        !cxx.arguments
            .extra_flags
            .iter()
            .any(|a| a == "-DC_ONLY_FLAG=1"),
        "C-only define must NOT leak into the C++ compile, got: {:?}",
        cxx.arguments.extra_flags
    );
}

#[test]
fn cxxflags_route_to_cxx_compile_only() {
    // Mirror of the C-only test: a C++-only flag never
    // reaches the C compile command. Required so a flag
    // that is invalid for C (`-fno-rtti`, `-std=c++20`)
    // does not break C builds.
    let flags = ResolvedProfileFlags {
        cxxflags: vec!["-DCXX_ONLY_FLAG=1".to_owned()],
        ..ResolvedProfileFlags::default()
    };
    let actions = plan_compile_actions(flags);
    let c = compile_action_for(&actions, SourceLanguage::C);
    let cxx = compile_action_for(&actions, SourceLanguage::Cxx);
    assert!(
        cxx.arguments
            .extra_flags
            .iter()
            .any(|a| a == "-DCXX_ONLY_FLAG=1"),
        "C++ compile must include the C++-only define, got: {:?}",
        cxx.arguments.extra_flags
    );
    assert!(
        !c.arguments
            .extra_flags
            .iter()
            .any(|a| a == "-DCXX_ONLY_FLAG=1"),
        "C++-only define must NOT leak into the C compile, got: {:?}",
        c.arguments.extra_flags
    );
}

#[test]
fn language_neutral_extra_compile_args_reach_both_compile_kinds() {
    // The language-neutral slot is the documented home for
    // flags that are valid for both C/C++. It must
    // appear on every compile command.
    let flags = ResolvedProfileFlags {
        extra_compile_args: vec!["-Wall".to_owned()],
        ..ResolvedProfileFlags::default()
    };
    let actions = plan_compile_actions(flags);
    let c = compile_action_for(&actions, SourceLanguage::C);
    let cxx = compile_action_for(&actions, SourceLanguage::Cxx);
    assert!(
        c.arguments.extra_flags.iter().any(|a| a == "-Wall"),
        "C compile must include the language-neutral flag, got: {:?}",
        c.arguments.extra_flags
    );
    assert!(
        cxx.arguments.extra_flags.iter().any(|a| a == "-Wall"),
        "C++ compile must include the language-neutral flag, got: {:?}",
        cxx.arguments.extra_flags
    );
}

#[test]
fn ldflags_appear_on_link_command_only() {
    // Link-only flags must reach every link command and
    // never appear on a compile command.
    let package = Package::new(
        pkg_name("mixed"),
        version(),
        vec![
            target(
                "mixedlib",
                TargetKind::Library,
                &["src/c_part.c", "src/cpp_part.cc"],
                &[],
            ),
            target(
                "app",
                TargetKind::Executable,
                &["src/main.cc"],
                &["mixedlib"],
            ),
        ],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/mixed");
    let tc = toolchain_with_cc();
    let mut map = HashMap::new();
    let flags = ResolvedProfileFlags {
        ldflags: vec!["-Wl,--as-needed".to_owned()],
        ..ResolvedProfileFlags::default()
    };
    map.insert(0usize, flags);
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &map,
        build_dir: PathBuf::from("/abs/mixed/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
    })
    .unwrap();
    let link = link_action(&bg);
    assert!(
        link.arguments.iter().any(|a| a == "-Wl,--as-needed"),
        "link command must include the link-only flag, got: {:?}",
        link.arguments
    );
    for compile in compile_actions(&bg) {
        // The link-only flag must not leak anywhere into the
        // lowered compile argv.
        let command = lowered(&BuildAction::Compile(compile.clone())).command;
        assert!(
            !command.iter().any(|a| a == "-Wl,--as-needed"),
            "link-only flag must NOT appear on compile, got: {command:?}",
        );
    }
}

#[cfg(unix)]
#[test]
fn promote_dir_rejects_non_utf8() {
    use std::os::unix::ffi::OsStrExt;
    // A non-UTF-8 build or manifest directory cannot anchor the
    // UTF-8 build model, so the planner rejects it with a typed
    // error rather than lossily promoting it.
    let p = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff/build"));
    assert!(matches!(promote_dir(p), Err(BuildError::NonUtf8Path(_))));
}
