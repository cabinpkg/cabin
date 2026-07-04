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

/// Wrap a single standard as the `{ min, max }` interface
/// requirement shape the interface fields carry.
fn interface_req<S>(min: S) -> cabin_core::InterfaceRequirement<S> {
    cabin_core::InterfaceRequirement::Requirement(cabin_core::StandardRequirement {
        min,
        max: None,
    })
}

/// Lower a semantic action to inspect the concrete argv / backend
/// kind the Ninja writer will render.  Lowering is infallible because
/// the semantic IR already carries UTF-8 paths.  These tests anchor
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

/// Target-level `c11` / `c++17` declarations for exactly the
/// languages `sources` compile.  Manifest loading requires every
/// compiled language to declare a standard (there is no built-in
/// default), so the fixture helpers mirror that invariant without
/// creating interface relevance for languages a target never
/// compiles.
fn language_for_sources(sources: &[&str]) -> cabin_core::LanguageStandardSettings {
    use cabin_core::{CStandard, CxxStandard, SourceLanguage, StandardDeclaration};
    let mut language = cabin_core::LanguageStandardSettings::default();
    for source in sources {
        match cabin_core::classify_source(Utf8Path::new(source)) {
            Some(SourceLanguage::C) => {
                language.c_standard = Some(StandardDeclaration::Declared(CStandard::C11));
            }
            Some(SourceLanguage::Cxx) => {
                language.cxx_standard = Some(StandardDeclaration::Declared(CxxStandard::Cxx17));
            }
            None => {}
        }
    }
    language
}

fn target(name: &str, kind: TargetKind, sources: &[&str], deps: &[&str]) -> CoreTarget {
    CoreTarget {
        name: target_name(name),
        kind,
        sources: sources.iter().map(Utf8PathBuf::from).collect(),
        include_dirs: Vec::new(),
        defines: Vec::new(),
        deps: deps
            .iter()
            .map(|d| cabin_core::TargetDep::from(*d))
            .collect(),
        required_features: Vec::new(),
        language: language_for_sources(sources),
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
        deps: deps
            .iter()
            .map(|d| cabin_core::TargetDep::from(*d))
            .collect(),
        required_features: Vec::new(),
        language: language_for_sources(sources),
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

/// Toolchain with both compilers resolved.  Used by tests that
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

/// A [`PlanRequest`] carrying the constant defaults nearly every
/// planner test uses: empty build flags, no language standards, no
/// flag conflicts, the dev profile, the default selection, and the
/// GNU dialect.  Tests override only the fields they exercise.
fn plan_request<'a>(
    graph: &'a PackageGraph,
    tc: &'a ResolvedToolchain,
    build_dir: &str,
) -> PlanRequest<'a> {
    use std::sync::LazyLock;
    static EMPTY_BUILD_FLAGS: LazyLock<HashMap<usize, ResolvedProfileFlags>> =
        LazyLock::new(HashMap::new);
    static NO_LANGUAGE_STANDARDS: LazyLock<HashMap<usize, cabin_core::ResolvedLanguageStandards>> =
        LazyLock::new(HashMap::new);
    static NO_FLAG_CONFLICTS: LazyLock<HashMap<usize, Vec<cabin_core::StandardFlagConflict>>> =
        LazyLock::new(HashMap::new);
    PlanRequest {
        graph,
        toolchain: tc,
        build_flags: &EMPTY_BUILD_FLAGS,
        language_standards: &NO_LANGUAGE_STANDARDS,
        standard_flag_conflicts: &NO_FLAG_CONFLICTS,
        build_dir: PathBuf::from(build_dir),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
        enabled_features: None,
        standard_compat: false,
    }
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
        is_port: false,
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
    let bg = plan(&plan_request(&graph, &tc, "/abs/proj/build")).unwrap();
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
fn default_selection_without_buildable_targets_errors() {
    // A package whose only target is a `test` (excluded from the
    // default build enumeration) yields no buildable default
    // selection, so `plan` reports `EmptySelectedPackages`.
    let package = Package::new(
        pkg_name("only_tests"),
        version(),
        vec![target("only_tests", TargetKind::Test, &["tests/t.cc"], &[])],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let req = plan_request(&graph, &tc, "/abs/proj/build");
    assert!(matches!(plan(&req), Err(BuildError::EmptySelectedPackages)));
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
        kind: cabin_core::CompilerWrapperKind::from_spec(&cabin_core::ToolSpec::parse("ccache")),
        path: Utf8PathBuf::from("/usr/local/bin/ccache"),
        spec: "ccache".into(),
        source: cabin_core::CompilerWrapperSource::Cli,
        identity: None,
    };
    let mut req = plan_request(&graph, &tc, "/abs/proj/build");
    req.compiler_wrapper = Some(&wrapper);
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
fn compiler_wrapper_prefixes_c_compile_commands() {
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
        kind: cabin_core::CompilerWrapperKind::from_spec(&cabin_core::ToolSpec::parse("ccache")),
        path: Utf8PathBuf::from("/usr/local/bin/ccache"),
        spec: "ccache".into(),
        source: cabin_core::CompilerWrapperSource::Cli,
        identity: None,
    };
    let mut req = plan_request(&graph, &tc, "/abs/proj/build");
    req.compiler_wrapper = Some(&wrapper);
    let bg = plan(&req).unwrap();
    let compile = lowered(
        bg.actions
            .iter()
            .find(|a| matches!(a, BuildAction::Compile(c) if c.standard.language() == SourceLanguage::C))
            .expect("C compile action present"),
    );
    assert_eq!(compile.command[0], "/usr/local/bin/ccache");
    assert_eq!(compile.command[1], "/usr/bin/cc");
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
    let mut req = plan_request(&graph, &tc, "/abs/proj/build");
    req.profile = release_profile();
    let bg = plan(&req).unwrap();
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
    let bg = plan(&plan_request(&graph, &tc, "/abs/proj/build")).unwrap();
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
    // `deps = ["greet"]` exercises the same-name shorthand
    // (`greet` -> `greet:greet`).
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
    let bg = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap();

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
    // A plain path dependency is the user's own code: nothing routes
    // to the system bucket.
    assert!(app_compile.arguments.system_include_dirs.is_empty());
}

#[test]
fn bare_dep_shorthand_requires_same_name_target() {
    // `deps = ["greet"]` is shorthand for `greet:greet` - pure name
    // matching, never a "default library" pick.  When the dependency
    // declares no target named like the package, the entry must fail
    // and spell out the qualified candidates.
    let greet_proj = Package::new(
        pkg_name("greet"),
        version(),
        vec![target("core", TargetKind::Library, &["src/core.cc"], &[])],
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
    let err = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap_err();
    match &err {
        BuildError::NoSameNameTargetInDependency {
            dep,
            package,
            candidates,
        } => {
            assert_eq!(dep, "greet");
            assert_eq!(package, "greet");
            assert_eq!(candidates, &["greet:core".to_owned()]);
        }
        other => panic!("expected NoSameNameTargetInDependency, got {other:?}"),
    }
    let rendered = err.to_string();
    assert!(
        rendered.contains("`greet:core`"),
        "error should suggest the qualified spelling, got: {rendered}"
    );
}

#[test]
fn bare_dep_shorthand_ignores_non_linkable_same_name_target() {
    // The dependency declares an *executable* named like the
    // package plus a differently named library.  The shorthand must
    // not silently pick the executable (it contributes no include
    // dirs or archives); the error lists the linkable candidates.
    let tool_proj = Package::new(
        pkg_name("tool"),
        version(),
        vec![
            target("tool", TargetKind::Executable, &["src/main.cc"], &[]),
            target("toollib", TargetKind::Library, &["src/lib.cc"], &[]),
        ],
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
            &["tool"],
        )],
        vec![dep("tool", "../tool")],
    )
    .unwrap();
    let tool_pkg = make_pkg("tool", "/abs/tool", tool_proj, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
    let graph = graph_with(vec![tool_pkg, app_pkg], vec![1], Some(1));
    let tc = toolchain();
    let err = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap_err();
    match &err {
        BuildError::NoSameNameTargetInDependency {
            dep,
            package,
            candidates,
        } => {
            assert_eq!(dep, "tool");
            assert_eq!(package, "tool");
            assert_eq!(candidates, &["tool:toollib".to_owned()]);
        }
        other => panic!("expected NoSameNameTargetInDependency, got {other:?}"),
    }
}

// -----------------------------------------------------------------
// per-edge dependency visibility.
// -----------------------------------------------------------------

/// greet at index 0 (library target `greet`), app at index 1
/// depending on greet.  The shared fixture for edge-visibility
/// resolution tests.
fn visibility_fixture() -> PackageGraph {
    let greet_proj = Package::new(
        pkg_name("greet"),
        version(),
        vec![
            target("greet", TargetKind::Library, &["src/greet.cc"], &[]),
            target("extras", TargetKind::Library, &["src/extras.cc"], &[]),
        ],
        Vec::new(),
    )
    .unwrap();
    let app_proj = Package::new(
        pkg_name("app"),
        version(),
        vec![
            target("core", TargetKind::Library, &["src/core.cc"], &[]),
            target("app", TargetKind::Executable, &["src/main.cc"], &[]),
        ],
        vec![dep("greet", "../greet")],
    )
    .unwrap();
    let greet_pkg = make_pkg("greet", "/abs/greet", greet_proj, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
    graph_with(vec![greet_pkg, app_pkg], vec![1], Some(1))
}

fn resolve_edge(decl: &cabin_core::TargetDep, graph: &PackageGraph) -> TargetDepEdge {
    lowering::resolve_target_dep_edge(decl, 1, false, graph).unwrap()
}

#[test]
fn dep_edge_visibility_defaults_private() {
    // The string shorthand is a private edge, across every
    // reference form: same-package bare name, same-name shorthand,
    // and qualified `package:target`.
    let graph = visibility_fixture();
    for reference in ["core", "greet", "greet:extras"] {
        let edge = resolve_edge(&cabin_core::TargetDep::private(reference), &graph);
        assert!(!edge.public, "edge for {reference:?} must default private");
    }
}

#[test]
fn dep_edge_visibility_survives_same_name_alias_resolution() {
    // `{ name = "greet", public = true }` resolves through the
    // same-name shorthand (`greet` -> `greet:greet`).  The recorded
    // edge names the concrete (package, target) - never the
    // pre-alias spelling - and still carries the declared
    // visibility.
    let graph = visibility_fixture();
    let decl = cabin_core::TargetDep {
        reference: "greet".to_owned(),
        public: true,
    };
    let edge = resolve_edge(&decl, &graph);
    assert_eq!(
        edge,
        TargetDepEdge {
            to: (0, "greet".to_owned()),
            public: true,
        }
    );
}

#[test]
fn dep_edge_visibility_attaches_to_qualified_and_local_references() {
    let graph = visibility_fixture();
    // Qualified reference to a dependency target named differently
    // from its package.
    let qualified = cabin_core::TargetDep {
        reference: "greet:extras".to_owned(),
        public: true,
    };
    let edge = resolve_edge(&qualified, &graph);
    assert_eq!(
        edge,
        TargetDepEdge {
            to: (0, "extras".to_owned()),
            public: true,
        }
    );
    // Same-package bare name.
    let local = cabin_core::TargetDep {
        reference: "core".to_owned(),
        public: true,
    };
    let edge = resolve_edge(&local, &graph);
    assert_eq!(
        edge,
        TargetDepEdge {
            to: (1, "core".to_owned()),
            public: true,
        }
    );
}

#[test]
fn public_dep_edge_plans_like_a_private_one() {
    // Visibility is declarative only today: a public edge must not
    // change planning output.  Same shape as
    // `cross_package_path_dep_links_library`, with the dep declared
    // public.
    let greet_proj = Package::new(
        pkg_name("greet"),
        version(),
        vec![target("greet", TargetKind::Library, &["src/greet.cc"], &[])],
        Vec::new(),
    )
    .unwrap();
    let mut app_target = target("app", TargetKind::Executable, &["src/main.cc"], &[]);
    app_target.deps = vec![cabin_core::TargetDep {
        reference: "greet".to_owned(),
        public: true,
    }];
    let app_proj = Package::new(
        pkg_name("app"),
        version(),
        vec![app_target],
        vec![dep("greet", "../greet")],
    )
    .unwrap();
    let greet_pkg = make_pkg("greet", "/abs/greet", greet_proj, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
    let graph = graph_with(vec![greet_pkg, app_pkg], vec![1], Some(1));
    let tc = toolchain();
    let bg = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap();
    let link = link_action(&bg);
    assert!(link.inputs.contains(&Utf8PathBuf::from(
        "/abs/build/dev/packages/greet/libgreet.a"
    )));
}

// -----------------------------------------------------------------
// required-features gating.
// -----------------------------------------------------------------

/// A package whose `[features]` table declares `features` (no
/// implications), for targets carrying `required-features`.
fn package_with_features(
    name: &str,
    targets: Vec<CoreTarget>,
    dependencies: Vec<Dependency>,
    features: &[&str],
) -> Package {
    let features = cabin_core::Features::new(
        Vec::new(),
        features
            .iter()
            .map(|f| ((*f).to_owned(), Vec::new()))
            .collect(),
    )
    .unwrap();
    Package::with_config(cabin_core::PackageConfigInput {
        name: pkg_name(name),
        version: version(),
        targets,
        dependencies,
        system_dependencies: Vec::new(),
        features,
    })
    .unwrap()
}

fn gated_target(
    name: &str,
    kind: TargetKind,
    sources: &[&str],
    deps: &[&str],
    required: &[&str],
) -> CoreTarget {
    let mut t = target(name, kind, sources, deps);
    t.required_features = required.iter().map(|f| (*f).to_owned()).collect();
    t
}

/// `demo` with an ungated `core` library and a `tls` library gated
/// on the `ssl` feature.
fn gated_single_package_graph() -> PackageGraph {
    let package = package_with_features(
        "demo",
        vec![
            target("core", TargetKind::Library, &["src/core.cc"], &[]),
            gated_target("tls", TargetKind::Library, &["src/tls.cc"], &[], &["ssl"]),
        ],
        Vec::new(),
        &["ssl"],
    );
    single_package_graph(package, "/abs/demo")
}

fn enabled(pairs: &[(usize, &[&str])]) -> HashMap<usize, BTreeSet<String>> {
    pairs
        .iter()
        .map(|(idx, names)| {
            (
                *idx,
                names
                    .iter()
                    .map(|n| (*n).to_owned())
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect()
}

#[test]
fn default_selection_skips_target_with_unmet_required_features() {
    let graph = gated_single_package_graph();
    let tc = toolchain();
    let bg = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap();
    let outputs: Vec<String> = bg.default_outputs.iter().map(ToString::to_string).collect();
    assert!(
        outputs.iter().any(|o| o.contains("libcore.a")),
        "ungated library must build: {outputs:?}"
    );
    assert!(
        !outputs.iter().any(|o| o.contains("libtls.a")),
        "feature-gated library must be skipped: {outputs:?}"
    );
}

#[test]
fn default_selection_builds_gated_target_when_feature_enabled() {
    let graph = gated_single_package_graph();
    let tc = toolchain();
    let features = enabled(&[(0, &["ssl"])]);
    let mut req = plan_request(&graph, &tc, "/abs/build");
    req.enabled_features = Some(&features);
    let bg = plan(&req).unwrap();
    assert!(
        bg.default_outputs
            .iter()
            .any(|o| o.as_str().contains("libtls.a")),
        "gated library must build once its required features are enabled"
    );
}

#[test]
fn explicit_selector_on_gated_target_is_a_hard_error() {
    let graph = gated_single_package_graph();
    let tc = toolchain();
    let mut req = plan_request(&graph, &tc, "/abs/build");
    req.selected = Some(vec![ManifestTargetSelector::parse("tls")]);
    let err = plan(&req).unwrap_err();
    match &err {
        BuildError::TargetRequiresFeatures {
            target,
            package,
            missing,
        } => {
            assert_eq!(target, "demo:tls");
            assert_eq!(package, "demo");
            assert_eq!(missing, &["ssl".to_owned()]);
        }
        other => panic!("expected TargetRequiresFeatures, got {other:?}"),
    }
    assert!(
        err.to_string().contains("--features ssl"),
        "help must name the flag: {err}"
    );
}

#[test]
fn same_package_dep_on_gated_target_errors_with_features_help() {
    let package = package_with_features(
        "demo",
        vec![
            gated_target("tls", TargetKind::Library, &["src/tls.cc"], &[], &["ssl"]),
            target("app", TargetKind::Executable, &["src/main.cc"], &["tls"]),
        ],
        Vec::new(),
        &["ssl"],
    );
    let graph = single_package_graph(package, "/abs/demo");
    let tc = toolchain();
    let err = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap_err();
    match &err {
        BuildError::TargetDepRequiresFeatures {
            consumer,
            dep_target,
            missing,
            fix,
            ..
        } => {
            assert_eq!(consumer, "demo:app");
            assert_eq!(dep_target, "demo:tls");
            assert_eq!(missing, &["ssl".to_owned()]);
            // `demo` is the selected root, so CLI selection applies.
            assert_eq!(*fix, crate::FeatureGateFix::RootSelection);
        }
        other => panic!("expected TargetDepRequiresFeatures, got {other:?}"),
    }
    assert!(
        err.to_string().contains("--features ssl"),
        "same-package help must name the flag: {err}"
    );
}

#[test]
fn cross_package_dep_on_gated_target_errors_with_edge_help() {
    let foo = package_with_features(
        "foo",
        vec![gated_target(
            "tls",
            TargetKind::Library,
            &["src/tls.cc"],
            &[],
            &["ssl"],
        )],
        Vec::new(),
        &["ssl"],
    );
    let app = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            &["foo:tls"],
        )],
        vec![dep("foo", "../foo")],
    )
    .unwrap();
    let foo_pkg = make_pkg("foo", "/abs/foo", foo, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app, vec![0]);
    let graph = graph_with(vec![foo_pkg, app_pkg], vec![1], Some(1));
    let tc = toolchain();
    let err = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap_err();
    match &err {
        BuildError::TargetDepRequiresFeatures {
            consumer,
            dep_target,
            dep_package,
            missing,
            fix,
        } => {
            assert_eq!(consumer, "app:app");
            assert_eq!(dep_target, "foo:tls");
            assert_eq!(dep_package, "foo");
            assert_eq!(missing, &["ssl".to_owned()]);
            assert_eq!(*fix, crate::FeatureGateFix::DependencyEdge);
        }
        other => panic!("expected TargetDepRequiresFeatures, got {other:?}"),
    }
    let rendered = err.to_string();
    assert!(
        rendered.contains("features = [\"ssl\"]"),
        "cross-package help must show the edge syntax: {rendered}"
    );
}

#[test]
fn dev_edge_dep_on_gated_target_points_help_at_dev_dependencies() {
    // A test target reaching a gated target through an activated
    // `[dev-dependencies]` edge must be told to add the feature
    // request on that edge - not to promote the dep to
    // `[dependencies]`.
    let kit = package_with_features(
        "kit",
        vec![gated_target(
            "tls",
            TargetKind::Library,
            &["src/tls.cc"],
            &[],
            &["ssl"],
        )],
        Vec::new(),
        &["ssl"],
    );
    let mut dev = dep("kit", "../kit");
    dev.kind = cabin_core::DependencyKind::Dev;
    let app = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "consumer",
            TargetKind::Test,
            &["src/consumer.cc"],
            &["kit:tls"],
        )],
        vec![dev],
    )
    .unwrap();
    let kit_pkg = make_pkg("kit", "/abs/kit", kit, vec![]);
    let mut app_pkg = make_pkg("app", "/abs/app", app, vec![0]);
    for edge in &mut app_pkg.deps {
        edge.kind = cabin_core::DependencyKind::Dev;
    }
    let graph = graph_with(vec![kit_pkg, app_pkg], vec![1], Some(1));
    let tc = toolchain();
    let mut req = plan_request(&graph, &tc, "/abs/build");
    req.selected = Some(vec![ManifestTargetSelector::parse("consumer")]);
    let err = plan(&req).unwrap_err();
    assert!(
        matches!(
            err,
            BuildError::TargetDepRequiresFeatures {
                fix: crate::FeatureGateFix::DevDependencyEdge,
                ..
            }
        ),
        "expected a dev-edge diagnostic, got {err:?}"
    );
    let rendered = err.to_string();
    assert!(
        rendered.contains("`[dev-dependencies]`"),
        "help must point at the dev edge: {rendered}"
    );
}

#[test]
fn cross_package_dep_on_gated_target_builds_when_feature_enabled() {
    let foo = package_with_features(
        "foo",
        vec![gated_target(
            "tls",
            TargetKind::Library,
            &["src/tls.cc"],
            &[],
            &["ssl"],
        )],
        Vec::new(),
        &["ssl"],
    );
    let app = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            &["foo:tls"],
        )],
        vec![dep("foo", "../foo")],
    )
    .unwrap();
    let foo_pkg = make_pkg("foo", "/abs/foo", foo, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app, vec![0]);
    let graph = graph_with(vec![foo_pkg, app_pkg], vec![1], Some(1));
    let tc = toolchain();
    let features = enabled(&[(0, &["ssl"])]);
    let mut req = plan_request(&graph, &tc, "/abs/build");
    req.enabled_features = Some(&features);
    let bg = plan(&req).unwrap();
    let link = link_action(&bg);
    assert!(
        link.inputs
            .contains(&Utf8PathBuf::from("/abs/build/dev/packages/foo/libtls.a")),
        "enabled feature must let the gated dep link: {:?}",
        link.inputs
    );
}

#[test]
fn transitive_gate_inside_dependency_points_help_upstream() {
    // app -> foo:api (ungated) -> foo:tls (gated).  `foo` is not a
    // selected root, so `--features` on the CLI cannot enable its
    // feature; the help must point at the dependency edge that
    // makes `foo` available instead.
    let foo = package_with_features(
        "foo",
        vec![
            {
                let mut api = target("api", TargetKind::Library, &["src/api.cc"], &["tls"]);
                api.deps = vec![cabin_core::TargetDep::private("tls")];
                api
            },
            gated_target("tls", TargetKind::Library, &["src/tls.cc"], &[], &["ssl"]),
        ],
        Vec::new(),
        &["ssl"],
    );
    let app = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            &["foo:api"],
        )],
        vec![dep("foo", "../foo")],
    )
    .unwrap();
    let foo_pkg = make_pkg("foo", "/abs/foo", foo, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app, vec![0]);
    let graph = graph_with(vec![foo_pkg, app_pkg], vec![1], Some(1));
    let tc = toolchain();
    let err = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap_err();
    assert!(
        matches!(
            &err,
            BuildError::TargetDepRequiresFeatures {
                fix: crate::FeatureGateFix::UpstreamEdge,
                ..
            }
        ),
        "expected an upstream-edge diagnostic, got {err:?}"
    );
    let rendered = err.to_string();
    assert!(
        !rendered.contains("--features"),
        "CLI selection cannot enable a non-root package's feature: {rendered}"
    );
    assert!(
        rendered.contains("features = [\"ssl\"]"),
        "help must show the edge request: {rendered}"
    );
}

#[test]
fn all_gated_default_selection_reports_actionable_error() {
    let package = package_with_features(
        "demo",
        vec![gated_target(
            "tls",
            TargetKind::Library,
            &["src/tls.cc"],
            &[],
            &["ssl"],
        )],
        Vec::new(),
        &["ssl"],
    );
    let graph = single_package_graph(package, "/abs/demo");
    let tc = toolchain();
    let err = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap_err();
    match &err {
        BuildError::AllDefaultTargetsRequireFeatures { gated } => {
            assert_eq!(gated.len(), 1);
            assert_eq!(gated[0].0, "demo:tls");
            assert_eq!(gated[0].1, vec!["ssl".to_owned()]);
        }
        other => panic!("expected AllDefaultTargetsRequireFeatures, got {other:?}"),
    }
    assert!(
        err.to_string().contains("--features"),
        "all-gated error must point at feature selection: {err}"
    );
}

#[test]
fn selector_required_features_met_matches_gating() {
    let graph = gated_single_package_graph();
    let sel = ManifestTargetSelector {
        package: Some("demo".to_owned()),
        name: "tls".to_owned(),
    };
    assert!(!selector_required_features_met(
        &sel,
        &graph,
        &HashMap::new()
    ));
    assert!(selector_required_features_met(
        &sel,
        &graph,
        &enabled(&[(0, &["ssl"])])
    ));
    // Unknown selectors stay `true`: resolution errors are plan()'s.
    let unknown = ManifestTargetSelector {
        package: Some("nope".to_owned()),
        name: "tls".to_owned(),
    };
    assert!(selector_required_features_met(
        &unknown,
        &graph,
        &HashMap::new()
    ));
}

// -----------------------------------------------------------------
// dev-dependency edge visibility.
// -----------------------------------------------------------------

/// Two-package fixture for dev-dependency resolution: `app`
/// declares `gtestish` under `[dev-dependencies]` and a `consumer`
/// target of the given kind referencing it via `dep_ref`.  The
/// graph edge is present only when `edge_active` - mirroring the
/// loader, which materializes dev edges only when the invocation
/// activates them (`cabin test` for the selected packages).
fn dev_dep_graph(consumer_kind: TargetKind, dep_ref: &str, edge_active: bool) -> PackageGraph {
    let gtestish = Package::new(
        pkg_name("gtestish"),
        version(),
        vec![target(
            "gtestish",
            TargetKind::Library,
            &["src/lib.cc"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap();
    let mut dev = dep("gtestish", "../gtestish");
    dev.kind = cabin_core::DependencyKind::Dev;
    let app = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "consumer",
            consumer_kind,
            &["src/consumer.cc"],
            &[dep_ref],
        )],
        vec![dev],
    )
    .unwrap();
    let gtestish_pkg = make_pkg("gtestish", "/abs/gtestish", gtestish, vec![]);
    let mut app_pkg = make_pkg(
        "app",
        "/abs/app",
        app,
        if edge_active { vec![0] } else { vec![] },
    );
    for edge in &mut app_pkg.deps {
        edge.kind = cabin_core::DependencyKind::Dev;
    }
    graph_with(vec![gtestish_pkg, app_pkg], vec![1], Some(1))
}

/// Plan the fixture's `consumer` target explicitly (dev-only kinds
/// are outside the default selection).
fn plan_dev_dep_consumer(graph: &PackageGraph) -> Result<BuildGraph, BuildError> {
    let tc = toolchain();
    let mut req = plan_request(graph, &tc, "/abs/build");
    req.selected = Some(vec![ManifestTargetSelector::parse("consumer")]);
    plan(&req)
}

#[test]
fn test_target_links_activated_dev_dependency_edge() {
    let graph = dev_dep_graph(TargetKind::Test, "gtestish", true);
    let bg = plan_dev_dep_consumer(&graph).unwrap();
    let link = link_action(&bg);
    assert!(
        link.inputs.contains(&Utf8PathBuf::from(
            "/abs/build/dev/packages/gtestish/libgtestish.a"
        )),
        "test executable must link the dev dependency's archive: {:?}",
        link.inputs
    );
}

#[test]
fn example_target_links_activated_dev_dependency_edge() {
    let graph = dev_dep_graph(TargetKind::Example, "gtestish", true);
    plan_dev_dep_consumer(&graph).expect("example targets share the dev-only dep policy");
}

#[test]
fn qualified_selector_resolves_activated_dev_dependency_edge() {
    let graph = dev_dep_graph(TargetKind::Test, "gtestish:gtestish", true);
    plan_dev_dep_consumer(&graph).expect("qualified selector must see the dev edge");
}

#[test]
fn ordinary_target_cannot_link_dev_dependency_edge() {
    // Even with the dev edge activated in the graph (as under
    // `cabin test`), a non-dev-only target must not resolve
    // through it: a library or executable linking a dev dep would
    // build differently under `cabin test` than under `cabin
    // build`.
    let graph = dev_dep_graph(TargetKind::Executable, "gtestish", true);
    let err = plan_dev_dep_consumer(&graph).unwrap_err();
    assert!(
        matches!(err, BuildError::DevDependencyNotActive { ref dep, ref package } if dep == "gtestish" && package == "app"),
        "expected DevDependencyNotActive, got {err:?}"
    );
}

#[test]
fn dev_dependency_without_activated_edge_diagnoses() {
    // The declaration exists but no edge was materialized - the
    // ordinary `cabin build` policy.  The failure must name the
    // dev-dependency policy instead of a generic unknown-target
    // reference.
    let graph = dev_dep_graph(TargetKind::Test, "gtestish", false);
    let err = plan_dev_dep_consumer(&graph).unwrap_err();
    assert!(
        matches!(err, BuildError::DevDependencyNotActive { ref dep, ref package } if dep == "gtestish" && package == "app"),
        "expected DevDependencyNotActive, got {err:?}"
    );
}

#[test]
fn qualified_dev_dependency_without_activated_edge_diagnoses() {
    let graph = dev_dep_graph(TargetKind::Test, "gtestish:gtestish", false);
    let err = plan_dev_dep_consumer(&graph).unwrap_err();
    assert!(
        matches!(err, BuildError::DevDependencyNotActive { ref dep, .. } if dep == "gtestish"),
        "expected DevDependencyNotActive, got {err:?}"
    );
}

/// Two-package fixture for the include-provenance tests: local `app`
/// (executable) depends on `greet` (library with an `include` dir),
/// with greet's provenance set by the caller.
fn provenance_graph(greet_kind: PackageKind, greet_is_port: bool) -> PackageGraph {
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
    let mut greet_pkg = make_pkg("greet", "/abs/greet", greet_proj, vec![]);
    greet_pkg.kind = greet_kind;
    greet_pkg.is_port = greet_is_port;
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
    graph_with(vec![greet_pkg, app_pkg], vec![1], Some(1))
}

/// The compile action whose object path contains `marker`
/// (separator-normalized for Windows object paths).
fn compile_for<'a>(bg: &'a BuildGraph, marker: &str) -> &'a CompileAction {
    compile_actions(bg)
        .into_iter()
        .find(|c| c.object.as_str().replace('\\', "/").contains(marker))
        .expect("compile action present")
}

fn plan_provenance_graph(
    graph: &PackageGraph,
    dialect: Dialect,
    msvc_external_includes: bool,
) -> BuildGraph {
    let tc = toolchain();
    let mut req = plan_request(graph, &tc, "/abs/build");
    req.dialect = dialect;
    req.msvc_external_includes = msvc_external_includes;
    plan(&req).unwrap()
}

#[test]
fn registry_dep_include_dirs_become_system_includes() {
    // greet is an extracted registry archive: third-party code whose
    // headers the user cannot fix, so its include dir routes to the
    // system bucket (`-isystem`) on the consumer's compiles.
    let graph = provenance_graph(PackageKind::Registry, false);
    let bg = plan_provenance_graph(&graph, Dialect::GnuLike, true);

    let app_compile = compile_for(&bg, "/app/");
    let greet_include = Utf8PathBuf::from("/abs/greet/include");
    assert!(
        app_compile
            .arguments
            .system_include_dirs
            .contains(&greet_include)
    );
    assert!(!app_compile.arguments.include_dirs.contains(&greet_include));

    // Building greet itself keeps its own headers as user includes:
    // a package always sees its own code under `-I`.
    let greet_compile = compile_for(&bg, "/greet/");
    assert!(
        greet_compile
            .arguments
            .include_dirs
            .contains(&greet_include)
    );
    assert!(greet_compile.arguments.system_include_dirs.is_empty());

    // The compile database spells the system bucket as `-isystem`.
    let cc = bg
        .compile_commands
        .iter()
        .find(|c| c.output.as_str().replace('\\', "/").contains("/app/"))
        .expect("app compile command present");
    let isystem = cc
        .arguments
        .iter()
        .position(|a| a == "-isystem")
        .expect("-isystem present");
    // Normalize separators: the planner joins include dirs with `\`
    // on Windows.
    assert_eq!(
        cc.arguments[isystem + 1].replace('\\', "/"),
        "/abs/greet/include"
    );
}

#[test]
fn port_dep_include_dirs_become_system_includes() {
    // Foundation ports are trust-local (their flags come from the
    // curated recipe) but code-wise third-party upstream sources, so
    // their headers take the system bucket like registry packages.
    let graph = provenance_graph(PackageKind::Local, true);
    let bg = plan_provenance_graph(&graph, Dialect::GnuLike, true);
    let app_compile = compile_for(&bg, "/app/");
    let greet_include = Utf8PathBuf::from("/abs/greet/include");
    assert!(
        app_compile
            .arguments
            .system_include_dirs
            .contains(&greet_include)
    );
    assert!(!app_compile.arguments.include_dirs.contains(&greet_include));
}

#[test]
fn msvc_with_external_support_keeps_system_includes() {
    let graph = provenance_graph(PackageKind::Registry, false);
    let bg = plan_provenance_graph(&graph, Dialect::Msvc, true);
    let app_compile = compile_for(&bg, "/app/");
    assert!(
        app_compile
            .arguments
            .system_include_dirs
            .contains(&Utf8PathBuf::from("/abs/greet/include"))
    );
    let cc = bg
        .compile_commands
        .iter()
        .find(|c| c.output.as_str().replace('\\', "/").contains("/app/"))
        .expect("app compile command present");
    assert!(cc.arguments.iter().any(|a| a == "/external:W0"));
    let ext = cc
        .arguments
        .iter()
        .position(|a| a == "/external:I")
        .expect("/external:I present");
    // Normalize separators: the planner joins include dirs with `\`
    // on Windows.
    assert_eq!(
        cc.arguments[ext + 1].replace('\\', "/"),
        "/abs/greet/include"
    );
}

#[test]
fn msvc_without_external_support_collapses_system_includes() {
    // A `cl` older than VS2019 16.10 rejects `/external:I`, so the
    // planner falls back to spelling every dependency include dir as
    // a plain `/I` - exactly the pre-system-include behavior.
    let graph = provenance_graph(PackageKind::Registry, false);
    let bg = plan_provenance_graph(&graph, Dialect::Msvc, false);
    let app_compile = compile_for(&bg, "/app/");
    let greet_include = Utf8PathBuf::from("/abs/greet/include");
    assert!(app_compile.arguments.system_include_dirs.is_empty());
    assert!(app_compile.arguments.include_dirs.contains(&greet_include));
    let cc = bg
        .compile_commands
        .iter()
        .find(|c| c.output.as_str().replace('\\', "/").contains("/app/"))
        .expect("app compile command present");
    assert!(!cc.arguments.iter().any(|a| a.starts_with("/external:")));
}

#[test]
fn flag_system_include_dirs_route_to_system_bucket() {
    // `ResolvedProfileFlags::system_include_dirs` (the pkg-config
    // contribution) lands in the compile's system bucket.
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
    let mut flags = HashMap::new();
    flags.insert(
        0,
        ResolvedProfileFlags {
            system_include_dirs: vec![Utf8PathBuf::from("/opt/zlib/include")],
            ..Default::default()
        },
    );
    let mut req = plan_request(&graph, &tc, "/abs/proj/build");
    req.build_flags = &flags;
    let bg = plan(&req).unwrap();
    let compile = compile_for(&bg, "/hello/");
    assert!(
        compile
            .arguments
            .system_include_dirs
            .contains(&Utf8PathBuf::from("/opt/zlib/include"))
    );
    assert!(
        !compile
            .arguments
            .include_dirs
            .contains(&Utf8PathBuf::from("/opt/zlib/include"))
    );
}

#[test]
fn link_libs_propagate_to_consumer_link_after_archives() {
    // A library package declares `link-libs` (resolved into its
    // per-package build flags); an executable in another package
    // depends on it.  The library's system libraries must appear on
    // the *executable's* link line, emitted as `-l<name>` AFTER the
    // library archive so GNU `ld`'s left-to-right resolution finds
    // them. macOS/libSystem resolves regardless of order, so this
    // plan-level assertion is what guards the ordering.
    let crypto_proj = Package::new(
        pkg_name("crypto"),
        version(),
        vec![target(
            "crypto",
            TargetKind::Library,
            &["src/crypto.c"],
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
            &["src/main.c"],
            &["crypto"],
        )],
        vec![dep("crypto", "../crypto")],
    )
    .unwrap();
    let crypto_pkg = make_pkg("crypto", "/abs/crypto", crypto_proj, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
    let graph = graph_with(vec![crypto_pkg, app_pkg], vec![1], Some(1));
    let tc = toolchain_with_cc();

    // crypto (package index 0) requires `-lpthread -lm`.
    let mut build_flags: HashMap<usize, ResolvedProfileFlags> = HashMap::new();
    build_flags.insert(
        0,
        ResolvedProfileFlags {
            link_libs: vec!["pthread".to_owned(), "m".to_owned()],
            ..Default::default()
        },
    );

    let mut req = plan_request(&graph, &tc, "/abs/build");
    req.build_flags = &build_flags;
    let bg = plan(&req).unwrap();

    let link = link_action(&bg);
    assert_eq!(
        link.link_libs,
        vec!["pthread".to_owned(), "m".to_owned()],
        "library link-libs propagate to the consumer link as bare names"
    );
    assert!(
        link.arguments.is_empty(),
        "`arguments` carries only ldflags; link-libs live in their own field, got {:?}",
        link.arguments
    );

    // Order check on the fully lowered GNU argv: the archive input
    // must precede the `-l` flags the dialect layer spells.
    let argv = lowered(&BuildAction::Link(link.clone())).command;
    let archive_pos = argv
        .iter()
        .position(|a| a.replace('\\', "/").ends_with("/libcrypto.a"))
        .expect("crypto archive on link line");
    let lpthread_pos = argv
        .iter()
        .position(|a| a == "-lpthread")
        .expect("-lpthread on link line");
    assert!(
        archive_pos < lpthread_pos,
        "archive must precede -lpthread for GNU ld; argv = {argv:?}"
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
    let mut req = plan_request(&graph, &tc, "/abs/build");
    req.selected = Some(vec![ManifestTargetSelector::parse("app:app")]);
    let bg = plan(&req).unwrap();
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
    let mut req = plan_request(&graph, &tc, "/abs/build");
    req.selected = Some(vec![ManifestTargetSelector::parse("build")]);
    let err = plan(&req).unwrap_err();
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
    let mut req = plan_request(&graph, &tc, "/abs/build");
    req.selected = Some(vec![ManifestTargetSelector::parse("nope:thing")]);
    let err = plan(&req).unwrap_err();
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
        language: Default::default(),
        compiler_wrapper: Default::default(),
        patches: Default::default(),
    };
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let err = plan(&plan_request(&graph, &tc, "/abs/build")).unwrap_err();
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
    let mut req = plan_request(&graph, &tc, "/abs/build");
    req.selected = Some(vec![ManifestTargetSelector::parse("hello:missing")]);
    let err = plan(&req).unwrap_err();
    assert!(matches!(err, BuildError::UnknownTargetInPackage { .. }));
}

/// Helper: the lowered link-action argv of a planned graph, so
/// tests can assert on `command[0]` (the chosen driver).  Panics if
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
    let bg = plan(&plan_request(&graph, &tc, "/abs/cdemo/build")).unwrap();
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
    let bg = plan(&plan_request(&graph, &tc, "/abs/mixed/build")).unwrap();
    let link = link_command(&bg);
    assert_eq!(link[0], "/usr/bin/g++");
}

#[test]
fn link_driver_is_cxx_when_dependency_has_cpp_objects() {
    // Pure-C executable that links a C++ static library
    // must use the C++ driver - the runtime is required
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
    let mut req = plan_request(&graph, &tc, "/abs/interop/build");
    req.selected = Some(vec![ManifestTargetSelector::parse("c_runner")]);
    let bg = plan(&req).unwrap();
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
    let mut req = plan_request(&graph, &tc, "/abs/clib_only/build");
    req.selected = Some(vec![ManifestTargetSelector::parse("c_runner")]);
    let bg = plan(&req).unwrap();
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
    let err = plan(&plan_request(&graph, &tc, "/abs/cdemo/build")).unwrap_err();
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
    let err = plan(&plan_request(&graph, &tc, "/abs/broken/build")).unwrap_err();
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
/// target carrying one C source and one C++ source.  Used by
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
/// fixture under the supplied build flags.  Used by every
/// per-language flag-routing test below to keep the
/// boilerplate to one place.
fn plan_compile_actions(flags: ResolvedProfileFlags) -> Vec<CompileAction> {
    let graph = graph_with_mixed_sources();
    let tc = toolchain_with_cc();
    let map = build_flags_map(flags);
    let mut req = plan_request(&graph, &tc, "/abs/mixed/build");
    req.build_flags = &map;
    let bg = plan(&req).unwrap();
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
        .find(|c| c.standard.language() == language)
        .unwrap_or_else(|| panic!("expected a {language:?} compile action"))
}

#[test]
fn cflags_route_to_c_compile_only() {
    // The C-only escape-hatch reaches every C compile
    // command and never reaches a C++ compile.  Without this
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
    // reaches the C compile command.  Required so a flag
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
    // flags that are valid for both C/C++.  It must
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
    let mut req = plan_request(&graph, &tc, "/abs/mixed/build");
    req.build_flags = &map;
    let bg = plan(&req).unwrap();
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

// ---------------------------------------------------------------------------
// language standards
// ---------------------------------------------------------------------------

/// Per-package effective standards for `graph`, mirroring the CLI's
/// `resolve_per_package_language_standards` loop.
fn standards_for(graph: &PackageGraph) -> HashMap<usize, cabin_core::ResolvedLanguageStandards> {
    graph
        .packages
        .iter()
        .enumerate()
        .map(|(idx, pkg)| {
            (
                idx,
                cabin_core::resolve_language_standards(&pkg.package.language),
            )
        })
        .collect()
}

fn plan_with_standards(graph: &PackageGraph, dialect: Dialect) -> Result<BuildGraph, BuildError> {
    let tc = toolchain_with_cc();
    let standards = standards_for(graph);
    let mut req = plan_request(graph, &tc, "/abs/build");
    req.language_standards = &standards;
    req.dialect = dialect;
    plan(&req)
}

fn target_with_language(
    name: &str,
    kind: TargetKind,
    sources: &[&str],
    deps: &[&str],
    language: cabin_core::LanguageStandardSettings,
) -> CoreTarget {
    let mut t = target(name, kind, sources, deps);
    t.language = language;
    t
}

#[test]
fn compile_actions_carry_per_target_effective_standards() {
    use cabin_core::{
        CStandard, CxxStandard, LanguageStandard, LanguageStandardSettings, StandardDeclaration,
    };
    let package = Package::new(
        pkg_name("demo"),
        version(),
        vec![
            target_with_language(
                "core",
                TargetKind::Library,
                &["src/core.cc"],
                &[],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                    // Keep the interface at the package default so the
                    // c++14 consumer below stays compatible.
                    interface_cxx_standard: Some(StandardDeclaration::Declared(interface_req(
                        CxxStandard::Cxx14,
                    ))),
                    ..Default::default()
                },
            ),
            // Declare only the C standard target-level so the C++
            // side exercises the package tier.
            target_with_language(
                "app",
                TargetKind::Executable,
                &["src/main.cc", "src/util.c"],
                &["core"],
                LanguageStandardSettings {
                    c_standard: Some(StandardDeclaration::Declared(CStandard::C11)),
                    ..Default::default()
                },
            ),
        ],
        Vec::new(),
    )
    .unwrap()
    .with_language(LanguageStandardSettings {
        cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx14)),
        ..Default::default()
    });
    let graph = single_package_graph(package, "/abs/proj");
    let bg = plan_with_standards(&graph, Dialect::GnuLike).unwrap();

    // `core` overrides the package default with c++20.
    let core_compile = compile_for(&bg, "/core/");
    assert_eq!(
        core_compile.standard,
        LanguageStandard::Cxx(CxxStandard::Cxx20)
    );
    // `app`'s C++ source inherits the package-level c++14; its C
    // source uses its target-level c11.
    let compiles = compile_actions(&bg);
    let app_cxx = compiles
        .iter()
        .find(|c| c.source.as_str().ends_with("main.cc"))
        .unwrap();
    assert_eq!(app_cxx.standard, LanguageStandard::Cxx(CxxStandard::Cxx14));
    let app_c = compiles
        .iter()
        .find(|c| c.source.as_str().ends_with("util.c"))
        .unwrap();
    assert_eq!(app_c.standard, LanguageStandard::C(CStandard::C11));
    // The lowered compile database spells both.
    let cc = bg
        .compile_commands
        .iter()
        .find(|c| c.file.as_str().ends_with("main.cc"))
        .unwrap();
    assert!(cc.arguments.iter().any(|a| a == "-std=c++14"));
}

#[test]
fn msvc_dialect_rejects_standards_without_stable_flag() {
    use cabin_core::{CxxStandard, LanguageStandardSettings, StandardDeclaration};
    let package = Package::new(
        pkg_name("demo"),
        version(),
        vec![target_with_language(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            &[],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx23)),
                ..Default::default()
            },
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    // Planning records the violation (deferred so `cabin check` can
    // prune dependency compiles first) and omits the un-lowerable
    // compile-commands entry; surfacing it is
    // `validate_planned_standards`' job.
    let bg = plan_with_standards(&graph, Dialect::Msvc).unwrap();
    assert_eq!(bg.standard_violations.len(), 1);
    assert!(matches!(
        &bg.standard_violations[0],
        crate::StandardViolation::MsvcSpelling {
            standard: "c++23",
            ..
        }
    ));
    assert!(
        bg.compile_commands.is_empty(),
        "a compile without a stable /std: flag has no lowerable argv"
    );
    let err = crate::validate_planned_standards(&bg).unwrap_err();
    match err {
        BuildError::StandardUnsupportedOnMsvcDialect { standard, .. } => {
            assert_eq!(standard, "c++23");
        }
        other => panic!("expected StandardUnsupportedOnMsvcDialect, got {other}"),
    }
    // The same plan succeeds on the GNU dialect, with no violations
    // and a normal compile-commands entry.
    let bg = plan_with_standards(&graph, Dialect::GnuLike).unwrap();
    assert!(bg.standard_violations.is_empty());
    assert_eq!(bg.compile_commands.len(), 1);
    crate::validate_planned_standards(&bg).unwrap();
}

#[test]
fn gnu_extensions_spell_the_gnu_std_flag_per_target() {
    use cabin_core::{CStandard, CxxStandard, LanguageStandardSettings, StandardDeclaration};
    let package = Package::new(
        pkg_name("demo"),
        version(),
        vec![
            // `core` opts in at target level (and overrides the
            // level); `app` stays on the ISO spelling.  Two targets
            // in one build differ in both level and gnu-extensions.
            target_with_language(
                "core",
                TargetKind::Library,
                &["src/core.cc"],
                &[],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                    gnu_extensions: Some(true),
                    ..Default::default()
                },
            ),
            target_with_language(
                "app",
                TargetKind::Executable,
                &["src/main.cc", "src/util.c"],
                &["core"],
                LanguageStandardSettings {
                    c_standard: Some(StandardDeclaration::Declared(CStandard::C17)),
                    ..Default::default()
                },
            ),
        ],
        Vec::new(),
    )
    .unwrap()
    .with_language(LanguageStandardSettings {
        cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
        ..Default::default()
    });
    let graph = single_package_graph(package, "/abs/proj");
    let bg = plan_with_standards(&graph, Dialect::GnuLike).unwrap();
    assert!(bg.standard_violations.is_empty());
    let flag_for = |suffix: &str| {
        let cc = bg
            .compile_commands
            .iter()
            .find(|c| c.file.as_str().ends_with(suffix))
            .unwrap();
        cc.arguments[1].clone()
    };
    assert_eq!(flag_for("core.cc"), "-std=gnu++20");
    assert_eq!(flag_for("main.cc"), "-std=c++20");
    assert_eq!(flag_for("util.c"), "-std=c17");
}

#[test]
fn msvc_dialect_rejects_gnu_extensions_at_planning_time() {
    use cabin_core::{CxxStandard, LanguageStandardSettings, StandardDeclaration};
    let package = Package::new(
        pkg_name("demo"),
        version(),
        vec![target_with_language(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            &[],
            LanguageStandardSettings {
                // c++20 has a stable /std: flag; only the
                // gnu-extensions request is unsatisfiable.
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                gnu_extensions: Some(true),
                ..Default::default()
            },
        )],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    // Recorded (not failed eagerly) for the same check-rewrite
    // reason as the missing-spelling case, and never silently
    // ignored: the un-honorable compile-commands entry is omitted.
    let bg = plan_with_standards(&graph, Dialect::Msvc).unwrap();
    assert_eq!(bg.standard_violations.len(), 1);
    assert!(matches!(
        &bg.standard_violations[0],
        crate::StandardViolation::MsvcGnuExtensions { .. }
    ));
    assert!(
        bg.compile_commands.is_empty(),
        "a gnu-extensions compile cannot be honored on the MSVC dialect"
    );
    let err = crate::validate_planned_standards(&bg).unwrap_err();
    match &err {
        BuildError::GnuExtensionsUnsupportedOnMsvcDialect { target } => {
            assert_eq!(target, "demo:app");
            let message = err.to_string();
            assert!(
                message.contains("no GNU dialect mode")
                    && message.contains("remove `gnu-extensions`")
                    && message.contains("GCC/Clang"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected GnuExtensionsUnsupportedOnMsvcDialect, got {other}"),
    }
    // The same plan succeeds on the GNU dialect with the GNU
    // spelling.
    let bg = plan_with_standards(&graph, Dialect::GnuLike).unwrap();
    assert!(bg.standard_violations.is_empty());
    assert!(
        bg.compile_commands[0]
            .arguments
            .iter()
            .any(|a| a == "-std=gnu++20")
    );
    crate::validate_planned_standards(&bg).unwrap();
}

#[test]
fn interface_requirement_blocks_lower_consumer() {
    use cabin_core::{CxxStandard, LanguageStandardSettings, StandardDeclaration};
    let package = Package::new(
        pkg_name("demo"),
        version(),
        vec![
            target_with_language(
                "core",
                TargetKind::Library,
                &["src/core.cc"],
                &[],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                    ..Default::default()
                },
            ),
            target_with_language(
                "app",
                TargetKind::Executable,
                &["src/main.cc"],
                &["core"],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                    ..Default::default()
                },
            ),
        ],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    // Planning records the incompatibility (deferred so the
    // `cabin check` rewrite can prune dependency compiles first);
    // surfacing it is `validate_planned_standards`' job.
    let bg = plan_with_standards(&graph, Dialect::GnuLike).unwrap();
    assert_eq!(bg.standard_violations.len(), 1);
    let err = crate::validate_planned_standards(&bg).unwrap_err();
    match err {
        BuildError::IncompatibleLanguageStandard {
            consumer,
            dependency,
            required,
            requirement_source,
            ..
        } => {
            assert_eq!(consumer, "demo:app");
            assert_eq!(dependency, "demo:core");
            assert_eq!(required, "c++20");
            assert!(
                requirement_source.contains("effective implementation standard"),
                "unexpected source: {requirement_source}"
            );
        }
        other => panic!("expected IncompatibleLanguageStandard, got {other}"),
    }
}

#[test]
fn interface_override_unblocks_consumer() {
    use cabin_core::{
        CxxStandard, LanguageStandard, LanguageStandardSettings, StandardDeclaration,
    };
    let package = Package::new(
        pkg_name("demo"),
        version(),
        vec![
            target_with_language(
                "core",
                TargetKind::Library,
                &["src/core.cc"],
                &[],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                    interface_cxx_standard: Some(StandardDeclaration::Declared(interface_req(
                        CxxStandard::Cxx17,
                    ))),
                    ..Default::default()
                },
            ),
            target_with_language(
                "app",
                TargetKind::Executable,
                &["src/main.cc"],
                &["core"],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                    ..Default::default()
                },
            ),
        ],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let bg = plan_with_standards(&graph, Dialect::GnuLike).unwrap();
    // The library's own objects still compile with its declared c++20
    // implementation standard.
    let core_compile = compile_for(&bg, "/core/");
    assert_eq!(
        core_compile.standard,
        LanguageStandard::Cxx(CxxStandard::Cxx20)
    );
}

#[test]
fn pure_c_dependency_imposes_no_cxx_requirement() {
    use cabin_core::{CxxStandard, LanguageStandardSettings, StandardDeclaration};
    let package = Package::new(
        pkg_name("demo"),
        version(),
        vec![
            target("clib", TargetKind::Library, &["src/clib.c"], &[]),
            target_with_language(
                "app",
                TargetKind::Executable,
                &["src/main.cc"],
                &["clib"],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx14)),
                    ..Default::default()
                },
            ),
        ],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    // The undeclared C library's built-in c11 is irrelevant to the
    // consumer's C++ side, and `app` compiles no C sources, so its
    // effective C standard never compares against the dependency.
    plan_with_standards(&graph, Dialect::GnuLike).unwrap();
}

#[test]
fn package_level_implementation_default_creates_no_relevance() {
    use cabin_core::{CxxStandard, LanguageStandardSettings, StandardDeclaration};
    // Dependency package declares a package-level cxx-standard, but
    // its library target carries only C sources: no C++ relevance,
    // so the c++17-default consumer plans fine.
    let dep_proj = Package::new(
        pkg_name("greet"),
        version(),
        vec![target("greet", TargetKind::Library, &["src/greet.c"], &[])],
        Vec::new(),
    )
    .unwrap()
    .with_language(LanguageStandardSettings {
        cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
        ..Default::default()
    });
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
    let greet_pkg = make_pkg("greet", "/abs/greet", dep_proj, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
    let graph = graph_with(vec![greet_pkg, app_pkg], vec![1], Some(1));
    plan_with_standards(&graph, Dialect::GnuLike).unwrap();
}

#[test]
fn header_only_package_interface_standard_binds_consumers() {
    use cabin_core::{CxxStandard, LanguageStandardSettings, StandardDeclaration};
    let dep_proj = Package::new(
        pkg_name("hdrs"),
        version(),
        vec![target_with_includes(
            "hdrs",
            TargetKind::HeaderOnly,
            &[],
            &["include"],
            &[],
        )],
        Vec::new(),
    )
    .unwrap()
    .with_language(LanguageStandardSettings {
        interface_cxx_standard: Some(StandardDeclaration::Declared(interface_req(
            CxxStandard::Cxx20,
        ))),
        ..Default::default()
    });
    let app_proj = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            &["hdrs"],
        )],
        vec![dep("hdrs", "../hdrs")],
    )
    .unwrap();
    let hdrs_pkg = make_pkg("hdrs", "/abs/hdrs", dep_proj, vec![]);
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
    let graph = graph_with(vec![hdrs_pkg, app_pkg], vec![1], Some(1));
    let bg = plan_with_standards(&graph, Dialect::GnuLike).unwrap();
    assert_eq!(bg.standard_violations.len(), 1);
    let err = crate::validate_planned_standards(&bg).unwrap_err();
    match err {
        BuildError::IncompatibleLanguageStandard {
            dependency,
            required,
            requirement_source,
            ..
        } => {
            assert_eq!(dependency, "hdrs:hdrs");
            assert_eq!(required, "c++20");
            assert!(
                requirement_source.contains("package-level interface standard"),
                "unexpected source: {requirement_source}"
            );
        }
        other => panic!("expected IncompatibleLanguageStandard, got {other}"),
    }
}

#[test]
fn dependency_internal_interface_violation_is_pruned_by_check() {
    use cabin_core::{CxxStandard, LanguageStandardSettings, StandardDeclaration};
    // app (c++20) -> liba (impl c++17) -> libb (interface c++20):
    // the incompatible pair is liba/libb, recorded on liba's compile.
    // `cabin build` of app surfaces it; `cabin check` of app prunes
    // liba's compiles and with them the violation.
    let leaf_proj = Package::new(
        pkg_name("libb"),
        version(),
        vec![target_with_language(
            "libb",
            TargetKind::Library,
            &["src/b.cc"],
            &[],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                ..Default::default()
            },
        )],
        Vec::new(),
    )
    .unwrap();
    let mid_proj = Package::new(
        pkg_name("liba"),
        version(),
        vec![target_with_language(
            "liba",
            TargetKind::Library,
            &["src/a.cc"],
            &["libb"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
                ..Default::default()
            },
        )],
        vec![dep("libb", "../libb")],
    )
    .unwrap();
    let app_proj = Package::new(
        pkg_name("app"),
        version(),
        vec![target_with_language(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            &["liba"],
            LanguageStandardSettings {
                cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                ..Default::default()
            },
        )],
        vec![dep("liba", "../liba")],
    )
    .unwrap();
    let leaf_pkg = make_pkg("libb", "/abs/libb", leaf_proj, vec![]);
    let mid_pkg = make_pkg("liba", "/abs/liba", mid_proj, vec![0]);
    let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![1]);
    let graph = graph_with(vec![leaf_pkg, mid_pkg, app_pkg], vec![2], Some(2));
    let bg = plan_with_standards(&graph, Dialect::GnuLike).unwrap();
    assert_eq!(bg.standard_violations.len(), 1);
    match &bg.standard_violations[0] {
        crate::StandardViolation::InterfaceIncompatibility {
            consumer,
            dependency,
            object,
            ..
        } => {
            assert_eq!(consumer, "liba:liba");
            assert_eq!(dependency, "libb:libb");
            assert!(
                object.starts_with("/abs/build/dev/packages/liba"),
                "violation must sit on the consumer's own compile, got {object}"
            );
        }
        other => panic!("expected InterfaceIncompatibility, got {other:?}"),
    }
    // A full build still surfaces the incompatibility...
    assert!(crate::validate_planned_standards(&bg).is_err());
    // ...while checking only `app` prunes liba's compiles and with
    // them the dependency-internal violation.
    let checked = crate::into_check_graph(bg, &[PathBuf::from("/abs/build/dev/packages/app")]);
    assert!(checked.standard_violations.is_empty());
    crate::validate_planned_standards(&checked).unwrap();
}

#[test]
fn requested_standards_follow_the_planned_selection() {
    use cabin_core::{CxxStandard, LanguageStandardSettings, StandardDeclaration};
    // Two executables; only `app` is selected.  The sibling's c++23
    // must not appear in the requested set - it is never planned, so
    // it must not gate toolchain validation (`cabin run --bin app`).
    let package = Package::new(
        pkg_name("demo"),
        version(),
        vec![
            target("app", TargetKind::Executable, &["src/main.cc"], &[]),
            target_with_language(
                "exotic",
                TargetKind::Executable,
                &["src/exotic.cc"],
                &[],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx23)),
                    ..Default::default()
                },
            ),
        ],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let standards = standards_for(&graph);
    let plan_for = |selected: Option<Vec<ManifestTargetSelector>>| {
        let mut req = plan_request(&graph, &tc, "/abs/build");
        req.language_standards = &standards;
        req.selected = selected;
        plan(&req).unwrap()
    };

    let narrowed = plan_for(Some(vec![ManifestTargetSelector::parse("app")]));
    let requested = crate::requested_standards_of(&narrowed);
    assert_eq!(
        requested.cxx,
        std::collections::BTreeSet::from([CxxStandard::Cxx17]),
        "the unbuilt sibling's c++23 must not gate validation"
    );
    assert!(requested.c.is_empty());

    // The default selection plans both executables, so both
    // standards are requested.
    let full = plan_for(None);
    let requested = crate::requested_standards_of(&full);
    assert_eq!(
        requested.cxx,
        std::collections::BTreeSet::from([CxxStandard::Cxx17, CxxStandard::Cxx23])
    );
}

#[test]
fn flag_conflicts_scope_to_planned_compiles() {
    use cabin_core::{
        CxxStandard, LanguageStandardSettings, SourceLanguage, StandardDeclaration,
        StandardFlagConflict,
    };
    // `exotic` declares a target-level cxx-standard while the package
    // flags pin one via `-std=`: the candidate covers only `exotic`'s
    // compiles, so selecting `app` must plan without a violation.
    let package = Package::new(
        pkg_name("demo"),
        version(),
        vec![
            target("app", TargetKind::Executable, &["src/main.cc"], &[]),
            target_with_language(
                "exotic",
                TargetKind::Executable,
                &["src/exotic.cc"],
                &[],
                LanguageStandardSettings {
                    cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
                    ..Default::default()
                },
            ),
        ],
        Vec::new(),
    )
    .unwrap();
    let graph = single_package_graph(package, "/abs/proj");
    let conflicts: HashMap<usize, Vec<StandardFlagConflict>> = HashMap::from([(
        0,
        vec![StandardFlagConflict {
            package: "demo".to_owned(),
            language: SourceLanguage::Cxx,
            field: "cxx-standard",
            flag_list: "cxxflags",
            flag: "-std=c++17".to_owned(),
            target: Some("exotic".to_owned()),
        }],
    )]);
    let tc = toolchain();
    let standards = standards_for(&graph);
    let plan_for = |selected: Option<Vec<ManifestTargetSelector>>| {
        let mut req = plan_request(&graph, &tc, "/abs/build");
        req.language_standards = &standards;
        req.standard_flag_conflicts = &conflicts;
        req.selected = selected;
        plan(&req).unwrap()
    };

    // Only `app` planned: the candidate's scope is never compiled.
    let narrowed = plan_for(Some(vec![ManifestTargetSelector::parse("app")]));
    assert!(narrowed.standard_violations.is_empty());
    crate::validate_planned_standards(&narrowed).unwrap();

    // Default selection plans `exotic` too: the conflict surfaces.
    let full = plan_for(None);
    assert!(matches!(
        full.standard_violations.first(),
        Some(crate::StandardViolation::FlagConflict { .. })
    ));
    let err = crate::validate_planned_standards(&full).unwrap_err();
    assert!(
        std::error::Error::source(&err).is_some_and(|s| s.to_string().contains("cxx-standard")),
        "the typed conflict must stay on the error chain: {err}"
    );
}

// ---------------------------------------------------------------------------
// experimental standard-compat pass (spec D13 over resolved edges)
// ---------------------------------------------------------------------------

use crate::standard_compat::{
    DeclScope, EdgeRequirement, RequirementOrigin, StandardCompatViolation,
};
use cabin_core::{CStandard, CxxStandard, StandardDeclaration};

/// A library package named `name` exposing one same-named target,
/// compiled from one C++ source at `impl_std`, with the given
/// interface declaration (target level).
fn cxx_lib_package(
    name: &str,
    impl_std: CxxStandard,
    interface: Option<cabin_core::InterfaceRequirement<CxxStandard>>,
    deps: &[cabin_core::TargetDep],
) -> Package {
    let mut lib = target(name, TargetKind::Library, &["src/lib.cc"], &[]);
    lib.language.cxx_standard = Some(StandardDeclaration::Declared(impl_std));
    lib.language.interface_cxx_standard = interface.map(StandardDeclaration::Declared);
    lib.deps = deps.to_vec();
    Package::new(pkg_name(name), version(), vec![lib], Vec::new()).unwrap()
}

/// An `app` package whose executable target depends on the given
/// references, compiled from one C++ source at c++17.
fn cxx_app_package(deps: &[&str], pkg_deps: Vec<Dependency>) -> Package {
    Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "app",
            TargetKind::Executable,
            &["src/main.cc"],
            deps,
        )],
        pkg_deps,
    )
    .unwrap()
}

/// Two-package graph: `app` (primary, index 0) depending on `dep`
/// (index 1).
fn app_dep_graph(app: Package, dep_pkg: Package, dep_name: &str) -> PackageGraph {
    graph_with(
        vec![
            make_pkg("app", "/abs/app", app, vec![1]),
            make_pkg(dep_name, &format!("/abs/{dep_name}"), dep_pkg, vec![]),
        ],
        vec![0],
        Some(0),
    )
}

fn standard_compat_warnings(graph: &PackageGraph) -> Vec<StandardCompatViolation> {
    let tc = toolchain_with_cc();
    let mut req = plan_request(graph, &tc, "/abs/app/build");
    req.standard_compat = true;
    plan(&req).unwrap().standard_compat_warnings
}

/// Feature-off no-op: with `standard_compat: false` (the default) a
/// graph carrying a clear violation records no warnings, so the
/// planner's output is exactly what it was before the pass existed.
#[test]
fn standard_compat_off_records_nothing() {
    let graph = app_dep_graph(
        cxx_app_package(&["lib"], vec![dep("lib", "../lib")]),
        cxx_lib_package(
            "lib",
            CxxStandard::Cxx20,
            Some(interface_req(CxxStandard::Cxx20)),
            &[],
        ),
        "lib",
    );
    let tc = toolchain();
    let bg = plan(&plan_request(&graph, &tc, "/abs/app/build")).unwrap();
    assert!(bg.standard_compat_warnings.is_empty());
}

/// Spec D9 row 2 / D13: an explicit interface minimum above the
/// consumer's effective level violates the edge, with the origin at
/// the dependency's own declaration.
#[test]
fn standard_compat_reports_direct_interface_minimum_violation() {
    let graph = app_dep_graph(
        cxx_app_package(&["lib"], vec![dep("lib", "../lib")]),
        cxx_lib_package(
            "lib",
            CxxStandard::Cxx20,
            Some(interface_req(CxxStandard::Cxx20)),
            &[],
        ),
        "lib",
    );
    let warnings = standard_compat_warnings(&graph);
    assert_eq!(warnings.len(), 1);
    let w = &warnings[0];
    assert_eq!(w.consumer, "app:app");
    assert_eq!(w.language, SourceLanguage::Cxx);
    assert_eq!(w.consumer_standard, "c++17");
    assert_eq!(w.consumer_site.field, "cxx-standard");
    assert_eq!(w.consumer_site.scope, DeclScope::Target("app".to_owned()));
    assert_eq!(
        w.consumer_site.manifest_path,
        PathBuf::from("/abs/app/cabin.toml")
    );
    assert_eq!(w.dependency, "lib:lib");
    assert_eq!(w.requirement, EdgeRequirement::Min("c++20"));
    assert_eq!(w.origin_target, "lib:lib");
    assert_eq!(w.chain, vec!["lib:lib".to_owned()]);
    let RequirementOrigin::Declared { site } = &w.origin else {
        panic!("expected a declared origin, got {:?}", w.origin);
    };
    assert_eq!(site.field, "interface-cxx-standard");
    assert_eq!(site.scope, DeclScope::Target("lib".to_owned()));
    assert_eq!(site.manifest_path, PathBuf::from("/abs/lib/cabin.toml"));
    assert!(!w.dependency_is_registry);
}

/// Spec D9 row 4: a compiled dependency with *no* interface
/// declaration imposes nothing at this layer - deliberately unlike
/// the build-time enforcement, whose implementation-standard
/// fallback still records its own violation for the same graph.
#[test]
fn standard_compat_imposes_nothing_for_compiled_without_declaration() {
    let graph = app_dep_graph(
        cxx_app_package(&["lib"], vec![dep("lib", "../lib")]),
        cxx_lib_package("lib", CxxStandard::Cxx20, None, &[]),
        "lib",
    );
    let tc = toolchain();
    let mut req = plan_request(&graph, &tc, "/abs/app/build");
    req.standard_compat = true;
    let bg = plan(&req).unwrap();
    assert!(bg.standard_compat_warnings.is_empty());
    // The build-time layer still catches it through its documented
    // implementation-standard fallback - the two layers differ here
    // by design (spec section 1, out-of-scope note).
    assert!(matches!(
        bg.standard_violations.first(),
        Some(crate::StandardViolation::InterfaceIncompatibility { .. })
    ));
}

/// Spec D9 row 1: `interface-cxx-standard = "none"` is forbidden -
/// unsatisfiable at every consumer level.
#[test]
fn standard_compat_reports_declared_none_as_forbidden() {
    let graph = app_dep_graph(
        cxx_app_package(&["lib"], vec![dep("lib", "../lib")]),
        cxx_lib_package(
            "lib",
            CxxStandard::Cxx20,
            Some(cabin_core::InterfaceRequirement::None),
            &[],
        ),
        "lib",
    );
    let warnings = standard_compat_warnings(&graph);
    assert_eq!(warnings.len(), 1);
    let w = &warnings[0];
    assert_eq!(w.requirement, EdgeRequirement::Forbidden);
    let RequirementOrigin::DeclaredNone { site } = &w.origin else {
        panic!("expected a declared-none origin, got {:?}", w.origin);
    };
    assert_eq!(site.field, "interface-cxx-standard");
}

/// Spec D10 / Example 3's mechanism: a requirement declared two
/// levels down reaches the consumer through a public edge, and the
/// provenance chain names every hop down to the origin declaration.
#[test]
fn standard_compat_propagates_through_public_edges_with_provenance() {
    let bottom = cxx_lib_package(
        "libb",
        CxxStandard::Cxx20,
        Some(interface_req(CxxStandard::Cxx20)),
        &[],
    );
    let middle = cxx_lib_package(
        "liba",
        CxxStandard::Cxx20,
        None,
        &[cabin_core::TargetDep {
            reference: "libb".to_owned(),
            public: true,
        }],
    );
    let app = cxx_app_package(&["liba"], vec![dep("liba", "../liba")]);
    let graph = graph_with(
        vec![
            make_pkg("app", "/abs/app", app, vec![1]),
            make_pkg("liba", "/abs/liba", middle, vec![2]),
            make_pkg("libb", "/abs/libb", bottom, vec![]),
        ],
        vec![0],
        Some(0),
    );
    let warnings = standard_compat_warnings(&graph);
    // `liba` itself compiles c++20, so only the app edge violates.
    assert_eq!(warnings.len(), 1);
    let w = &warnings[0];
    assert_eq!(w.consumer, "app:app");
    assert_eq!(w.dependency, "liba:liba");
    assert_eq!(w.origin_target, "libb:libb");
    assert_eq!(
        w.chain,
        vec!["liba:liba".to_owned(), "libb:libb".to_owned()]
    );
    assert_eq!(w.requirement, EdgeRequirement::Min("c++20"));
    assert!(matches!(w.origin, RequirementOrigin::Declared { .. }));
}

/// Spec D10: private edges do not propagate - the same graph with a
/// private `liba -> libb` edge is clean at every consumer.
#[test]
fn standard_compat_does_not_propagate_through_private_edges() {
    let bottom = cxx_lib_package(
        "libb",
        CxxStandard::Cxx20,
        Some(interface_req(CxxStandard::Cxx20)),
        &[],
    );
    let middle = cxx_lib_package(
        "liba",
        CxxStandard::Cxx20,
        None,
        &[cabin_core::TargetDep::private("libb")],
    );
    let app = cxx_app_package(&["liba"], vec![dep("liba", "../liba")]);
    let graph = graph_with(
        vec![
            make_pkg("app", "/abs/app", app, vec![1]),
            make_pkg("liba", "/abs/liba", middle, vec![2]),
            make_pkg("libb", "/abs/libb", bottom, vec![]),
        ],
        vec![0],
        Some(0),
    );
    assert!(standard_compat_warnings(&graph).is_empty());
}

/// Spec D9 row 3: a header-only dependency without an interface
/// declaration infers its minimum from its (target-level)
/// implementation standard, and the origin says so.
#[test]
fn standard_compat_reports_header_only_inference() {
    let mut hdr = target("hdr", TargetKind::HeaderOnly, &[], &[]);
    hdr.language.cxx_standard = Some(StandardDeclaration::Declared(CxxStandard::Cxx20));
    let hdr_pkg = Package::new(pkg_name("hdr"), version(), vec![hdr], Vec::new()).unwrap();
    let graph = app_dep_graph(
        cxx_app_package(&["hdr"], vec![dep("hdr", "../hdr")]),
        hdr_pkg,
        "hdr",
    );
    let warnings = standard_compat_warnings(&graph);
    assert_eq!(warnings.len(), 1);
    let w = &warnings[0];
    assert_eq!(w.dependency, "hdr:hdr");
    assert_eq!(w.requirement, EdgeRequirement::Min("c++20"));
    let RequirementOrigin::HeaderOnlyInference { site } = &w.origin else {
        panic!("expected header-only inference, got {:?}", w.origin);
    };
    assert_eq!(site.field, "cxx-standard");
    assert_eq!(site.scope, DeclScope::Target("hdr".to_owned()));
}

/// Spec D13: a mixed-language consumer checks every language it
/// compiles on the same edge, and each violated language reports
/// separately (sorted C before C++).
#[test]
fn standard_compat_reports_each_violated_language_separately() {
    let mut w_lib = target("w", TargetKind::Library, &["src/w.c"], &[]);
    w_lib.language.c_standard = Some(StandardDeclaration::Declared(CStandard::C17));
    w_lib.language.interface_c_standard =
        Some(StandardDeclaration::Declared(interface_req(CStandard::C17)));
    w_lib.language.interface_cxx_standard = Some(StandardDeclaration::Declared(interface_req(
        CxxStandard::Cxx23,
    )));
    let w_pkg = Package::new(pkg_name("w"), version(), vec![w_lib], Vec::new()).unwrap();
    let app = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "app",
            TargetKind::Executable,
            &["src/main.c", "src/main.cc"],
            &["w"],
        )],
        vec![dep("w", "../w")],
    )
    .unwrap();
    let graph = app_dep_graph(app, w_pkg, "w");
    let warnings = standard_compat_warnings(&graph);
    assert_eq!(warnings.len(), 2);
    assert_eq!(warnings[0].language, SourceLanguage::C);
    assert_eq!(warnings[0].consumer_standard, "c11");
    assert_eq!(warnings[0].requirement, EdgeRequirement::Min("c17"));
    assert_eq!(warnings[1].language, SourceLanguage::Cxx);
    assert_eq!(warnings[1].consumer_standard, "c++17");
    assert_eq!(warnings[1].requirement, EdgeRequirement::Min("c++23"));
    assert_eq!(warnings[0].dependency, warnings[1].dependency);
}

/// Spec D9 row 6: a C consumer of a C++-only dependency with no
/// declared C interface hits the strict cross-language default.
#[test]
fn standard_compat_reports_strict_cxx_to_c_default() {
    let cxxlib = cxx_lib_package("cxxlib", CxxStandard::Cxx17, None, &[]);
    let app = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "app",
            TargetKind::Executable,
            &["src/main.c"],
            &["cxxlib"],
        )],
        vec![dep("cxxlib", "../cxxlib")],
    )
    .unwrap();
    let graph = app_dep_graph(app, cxxlib, "cxxlib");
    let warnings = standard_compat_warnings(&graph);
    assert_eq!(warnings.len(), 1);
    let w = &warnings[0];
    assert_eq!(w.language, SourceLanguage::C);
    assert_eq!(w.requirement, EdgeRequirement::Forbidden);
    assert_eq!(w.origin, RequirementOrigin::CrossLanguageDefault);
}

/// Spec D13's vacuous case plus D10 propagation: a header-only
/// consumer's own edges never violate, but requirements flow
/// through it to the compiling consumer above, chain included.
#[test]
fn standard_compat_chains_through_header_only_dependencies() {
    let libb = cxx_lib_package(
        "libb",
        CxxStandard::Cxx20,
        Some(interface_req(CxxStandard::Cxx20)),
        &[],
    );
    let mut hdr = target("hdr", TargetKind::HeaderOnly, &[], &[]);
    hdr.language.interface_c_standard =
        Some(StandardDeclaration::Declared(interface_req(CStandard::C99)));
    hdr.deps = vec![cabin_core::TargetDep {
        reference: "libb".to_owned(),
        public: true,
    }];
    let hdr_pkg = Package::new(pkg_name("hdr"), version(), vec![hdr], Vec::new()).unwrap();
    let app = cxx_app_package(&["hdr"], vec![dep("hdr", "../hdr")]);
    let graph = graph_with(
        vec![
            make_pkg("app", "/abs/app", app, vec![1]),
            make_pkg("hdr", "/abs/hdr", hdr_pkg, vec![2]),
            make_pkg("libb", "/abs/libb", libb, vec![]),
        ],
        vec![0],
        Some(0),
    );
    let warnings = standard_compat_warnings(&graph);
    assert_eq!(warnings.len(), 1);
    let w = &warnings[0];
    assert_eq!(w.consumer, "app:app");
    assert_eq!(w.dependency, "hdr:hdr");
    assert_eq!(w.origin_target, "libb:libb");
    assert_eq!(w.chain, vec!["hdr:hdr".to_owned(), "libb:libb".to_owned()]);
}

/// Workspace-inherited declarations cite the workspace root
/// manifest: the consumer's standard and the origin's interface
/// both carry `Workspace` / `Package` scopes with the manifest that
/// actually declares the value.
#[test]
fn standard_compat_sites_follow_declaration_tiers() {
    // app: no target-level cxx standard; the package opted into the
    // workspace default (rewritten to `Inherited` by the loader).
    let mut app_target = target("app", TargetKind::Executable, &["src/main.cc"], &["lib"]);
    app_target.language = Default::default();
    let mut app = Package::new(
        pkg_name("app"),
        version(),
        vec![app_target],
        vec![dep("lib", "../lib")],
    )
    .unwrap();
    app.language.cxx_standard = Some(StandardDeclaration::Inherited(CxxStandard::Cxx17));
    // lib: package-level interface declaration (no target override).
    let mut lib_target = target("lib", TargetKind::Library, &["src/lib.cc"], &[]);
    lib_target.language.cxx_standard = Some(StandardDeclaration::Declared(CxxStandard::Cxx20));
    let mut lib = Package::new(pkg_name("lib"), version(), vec![lib_target], Vec::new()).unwrap();
    lib.language.interface_cxx_standard = Some(StandardDeclaration::Declared(interface_req(
        CxxStandard::Cxx20,
    )));
    let graph = app_dep_graph(app, lib, "lib");
    let tc = toolchain();
    let standards = standards_for(&graph);
    let mut req = plan_request(&graph, &tc, "/abs/app/build");
    req.language_standards = &standards;
    req.standard_compat = true;
    let warnings = plan(&req).unwrap().standard_compat_warnings;
    assert_eq!(warnings.len(), 1);
    let w = &warnings[0];
    assert_eq!(w.consumer_standard, "c++17");
    assert_eq!(w.consumer_site.scope, DeclScope::Workspace);
    // `graph_with` roots the graph at the first package's manifest.
    assert_eq!(
        w.consumer_site.manifest_path,
        PathBuf::from("/abs/app/cabin.toml")
    );
    let RequirementOrigin::Declared { site } = &w.origin else {
        panic!("expected a declared origin, got {:?}", w.origin);
    };
    assert_eq!(site.scope, DeclScope::Package);
    assert_eq!(site.manifest_path, PathBuf::from("/abs/lib/cabin.toml"));
}

/// Spec D9 row 5: the permissive C-to-C++ default - a pure-C
/// library with no C++ implementation and no C++ interface
/// declaration imposes nothing on C++ consumers, so the pass stays
/// silent.
#[test]
fn standard_compat_permissive_c_to_cxx_default_imposes_nothing() {
    let mut c_lib = target("clib", TargetKind::Library, &["src/c.c"], &[]);
    c_lib.language.c_standard = Some(StandardDeclaration::Declared(CStandard::C17));
    let c_pkg = Package::new(pkg_name("clib"), version(), vec![c_lib], Vec::new()).unwrap();
    let graph = app_dep_graph(
        cxx_app_package(&["clib"], vec![dep("clib", "../clib")]),
        c_pkg,
        "clib",
    );
    assert!(standard_compat_warnings(&graph).is_empty());
}

/// Spec Example 2's diamond: two consumers at different levels
/// share one dependency; only the too-old consumer's edge is
/// violated - a compatible sibling edge neither rescues nor taints
/// it (D13 is per edge).
#[test]
fn standard_compat_diamond_reports_only_the_violating_edge() {
    let z = cxx_lib_package(
        "z",
        CxxStandard::Cxx20,
        Some(interface_req(CxxStandard::Cxx20)),
        &[],
    );
    let x = cxx_app_package(&["z"], vec![dep("z", "../z")]);
    let mut y_target = target("y", TargetKind::Executable, &["src/main.cc"], &["z"]);
    y_target.language.cxx_standard = Some(StandardDeclaration::Declared(CxxStandard::Cxx23));
    let y = Package::new(
        pkg_name("y"),
        version(),
        vec![y_target],
        vec![dep("z", "../z")],
    )
    .unwrap();
    let graph = graph_with(
        vec![
            make_pkg("app", "/abs/app", x, vec![2]),
            make_pkg("y", "/abs/y", y, vec![2]),
            make_pkg("z", "/abs/z", z, vec![]),
        ],
        vec![0, 1],
        Some(0),
    );
    let warnings = standard_compat_warnings(&graph);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].consumer, "app:app");
    assert_eq!(warnings[0].dependency, "z:z");
    assert_eq!(warnings[0].requirement, EdgeRequirement::Min("c++20"));
}

/// One C library shared by a C++ consumer and a C consumer: the
/// C++ side rides the permissive row-5 default (no warning), while
/// the C side violates the library's declared C interface minimum -
/// the two languages are judged independently per edge.
#[test]
fn standard_compat_judges_shared_c_library_per_consumer_language() {
    let mut c_lib = target("clib", TargetKind::Library, &["src/c.c"], &[]);
    c_lib.language.c_standard = Some(StandardDeclaration::Declared(CStandard::C17));
    c_lib.language.interface_c_standard =
        Some(StandardDeclaration::Declared(interface_req(CStandard::C17)));
    let c_pkg = Package::new(pkg_name("clib"), version(), vec![c_lib], Vec::new()).unwrap();
    let cxx_app = cxx_app_package(&["clib"], vec![dep("clib", "../clib")]);
    let mut c_target = target("capp", TargetKind::Executable, &["src/main.c"], &["clib"]);
    c_target.language.c_standard = Some(StandardDeclaration::Declared(CStandard::C11));
    let c_app = Package::new(
        pkg_name("capp"),
        version(),
        vec![c_target],
        vec![dep("clib", "../clib")],
    )
    .unwrap();
    let graph = graph_with(
        vec![
            make_pkg("app", "/abs/app", cxx_app, vec![2]),
            make_pkg("capp", "/abs/capp", c_app, vec![2]),
            make_pkg("clib", "/abs/clib", c_pkg, vec![]),
        ],
        vec![0, 1],
        Some(0),
    );
    let warnings = standard_compat_warnings(&graph);
    // Only the C consumer's edge is violated; the C++ consumer
    // rides the permissive default silently.
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].consumer, "capp:capp");
    assert_eq!(warnings[0].language, SourceLanguage::C);
    assert_eq!(warnings[0].requirement, EdgeRequirement::Min("c17"));
}

/// Package-level interface defaults describe a library's public
/// interface: a qualified edge to an executable-like target in a
/// package carrying an `interface-cxx-standard` default takes no
/// requirement from it, while a library target in the same package
/// still does.
#[test]
fn standard_compat_package_interface_default_skips_executable_targets() {
    let mut tool = target("tool", TargetKind::Executable, &["src/tool.cc"], &[]);
    tool.language.cxx_standard = Some(StandardDeclaration::Declared(CxxStandard::Cxx20));
    let mut toollib = target("toollib", TargetKind::Library, &["src/lib.cc"], &[]);
    toollib.language.cxx_standard = Some(StandardDeclaration::Declared(CxxStandard::Cxx20));
    let mut tools_pkg = Package::new(
        pkg_name("tools"),
        version(),
        vec![tool, toollib],
        Vec::new(),
    )
    .unwrap();
    tools_pkg.language.interface_cxx_standard = Some(StandardDeclaration::Declared(interface_req(
        CxxStandard::Cxx20,
    )));
    let app = cxx_app_package(
        &["tools:tool", "tools:toollib"],
        vec![dep("tools", "../tools")],
    );
    let graph = app_dep_graph(app, tools_pkg, "tools");
    let warnings = standard_compat_warnings(&graph);
    // Only the library edge violates; the executable edge takes no
    // package-level interface default.
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].dependency, "tools:toollib");
    let RequirementOrigin::Declared { site } = &warnings[0].origin else {
        panic!("expected a declared origin, got {:?}", warnings[0].origin);
    };
    assert_eq!(site.scope, DeclScope::Package);
}

/// An executable-like dependency target has no consumable
/// interface: the strict C++-to-C default never fires for a
/// qualified edge onto it, while the same consumer's edge onto a
/// C++ library still does.
#[test]
fn standard_compat_skips_cross_language_default_of_executable_deps() {
    let mut tool = target("tool", TargetKind::Executable, &["src/tool.cc"], &[]);
    tool.language.cxx_standard = Some(StandardDeclaration::Declared(CxxStandard::Cxx17));
    let tools_pkg = Package::new(pkg_name("tools"), version(), vec![tool], Vec::new()).unwrap();
    let cxxlib = cxx_lib_package("cxxlib", CxxStandard::Cxx17, None, &[]);
    let mut c_app = target(
        "app",
        TargetKind::Executable,
        &["src/main.c"],
        &["tools:tool", "cxxlib"],
    );
    c_app.language.c_standard = Some(StandardDeclaration::Declared(CStandard::C11));
    let app = Package::new(
        pkg_name("app"),
        version(),
        vec![c_app],
        vec![dep("tools", "../tools"), dep("cxxlib", "../cxxlib")],
    )
    .unwrap();
    let graph = graph_with(
        vec![
            make_pkg("app", "/abs/app", app, vec![1, 2]),
            make_pkg("tools", "/abs/tools", tools_pkg, vec![]),
            make_pkg("cxxlib", "/abs/cxxlib", cxxlib, vec![]),
        ],
        vec![0],
        Some(0),
    );
    let warnings = standard_compat_warnings(&graph);
    // The library edge keeps its row-6 warning; the executable
    // edge is interface-less and stays silent.
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].dependency, "cxxlib:cxxlib");
    assert_eq!(warnings[0].origin, RequirementOrigin::CrossLanguageDefault);
}

/// A requirement passing *through* an executable-like target from
/// a library behind it keeps warning: the origin is the library,
/// not the interface-less intermediary.
#[test]
fn standard_compat_keeps_requirements_through_executable_deps() {
    let libb = cxx_lib_package(
        "libb",
        CxxStandard::Cxx20,
        Some(interface_req(CxxStandard::Cxx20)),
        &[],
    );
    let mut tool = target("tool", TargetKind::Executable, &["src/tool.cc"], &[]);
    tool.language.cxx_standard = Some(StandardDeclaration::Declared(CxxStandard::Cxx20));
    tool.deps = vec![cabin_core::TargetDep {
        reference: "libb".to_owned(),
        public: true,
    }];
    let tools_pkg = Package::new(pkg_name("tools"), version(), vec![tool], Vec::new()).unwrap();
    let app = cxx_app_package(&["tools:tool"], vec![dep("tools", "../tools")]);
    let graph = graph_with(
        vec![
            make_pkg("app", "/abs/app", app, vec![1]),
            make_pkg("tools", "/abs/tools", tools_pkg, vec![2]),
            make_pkg("libb", "/abs/libb", libb, vec![]),
        ],
        vec![0],
        Some(0),
    );
    let warnings = standard_compat_warnings(&graph);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].dependency, "tools:tool");
    assert_eq!(warnings[0].origin_target, "libb:libb");
    assert_eq!(
        warnings[0].chain,
        vec!["tools:tool".to_owned(), "libb:libb".to_owned()]
    );
}

/// The pass's inputs are manifest-declared values only: the same
/// graph yields identical C and C++ violations when every
/// toolchain-shaped planner input changes at once (compiler paths,
/// dialect, MSVC include spelling).  No toolchain probing, nothing
/// read from the environment - so a lockfile-loaded graph warns
/// identically on every machine that shares the manifests.
#[test]
fn standard_compat_violations_are_toolchain_independent() {
    let mut w_lib = target("w", TargetKind::Library, &["src/w.c"], &[]);
    w_lib.language.c_standard = Some(StandardDeclaration::Declared(CStandard::C17));
    w_lib.language.interface_c_standard =
        Some(StandardDeclaration::Declared(interface_req(CStandard::C17)));
    w_lib.language.interface_cxx_standard = Some(StandardDeclaration::Declared(interface_req(
        CxxStandard::Cxx23,
    )));
    let w_pkg = Package::new(pkg_name("w"), version(), vec![w_lib], Vec::new()).unwrap();
    let app = Package::new(
        pkg_name("app"),
        version(),
        vec![target(
            "app",
            TargetKind::Executable,
            &["src/main.c", "src/main.cc"],
            &["w"],
        )],
        vec![dep("w", "../w")],
    )
    .unwrap();
    let graph = app_dep_graph(app, w_pkg, "w");

    let gnu_tc = toolchain_with_cc();
    let mut gnu_req = plan_request(&graph, &gnu_tc, "/abs/app/build");
    gnu_req.standard_compat = true;
    let gnu = plan(&gnu_req).unwrap().standard_compat_warnings;

    let mut msvc_tc = toolchain_with_cc();
    msvc_tc.cxx.path = Utf8PathBuf::from("C:/tools/msvc/cl.exe");
    msvc_tc.cc.as_mut().unwrap().path = Utf8PathBuf::from("C:/tools/msvc/cl.exe");
    let mut msvc_req = plan_request(&graph, &msvc_tc, "/abs/app/build");
    msvc_req.dialect = Dialect::Msvc;
    msvc_req.msvc_external_includes = false;
    msvc_req.standard_compat = true;
    let msvc = plan(&msvc_req).unwrap().standard_compat_warnings;

    assert_eq!(gnu.len(), 2, "the fixture violates both languages");
    assert_eq!(
        gnu, msvc,
        "violations must not depend on the resolved toolchain or dialect"
    );
}
