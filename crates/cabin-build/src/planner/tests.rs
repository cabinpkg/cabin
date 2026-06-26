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

fn target(name: &str, kind: TargetKind, sources: &[&str], deps: &[&str]) -> CoreTarget {
    CoreTarget {
        name: target_name(name),
        kind,
        sources: sources.iter().map(Utf8PathBuf::from).collect(),
        include_dirs: Vec::new(),
        defines: Vec::new(),
        deps: deps.iter().map(|d| (*d).to_owned()).collect(),
        language: Default::default(),
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
        language: Default::default(),
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

fn empty_build_flags() -> HashMap<usize, ResolvedProfileFlags> {
    HashMap::new()
}

fn no_language_standards() -> HashMap<usize, cabin_core::ResolvedLanguageStandards> {
    HashMap::new()
}

fn no_flag_conflicts() -> HashMap<usize, Vec<cabin_core::StandardFlagConflict>> {
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
    let req = PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
    let req = PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
    };
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: Some(&wrapper),
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: Some(&wrapper),
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
    };
    let bg = plan(&req).unwrap();
    let compile = lowered(
        bg.actions
            .iter()
            .find(|a| matches!(a, BuildAction::Compile(c) if c.standard.language() == SourceLanguage::C))
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: release_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
    // A plain path dependency is the user's own code: nothing routes
    // to the system bucket.
    assert!(app_compile.arguments.system_include_dirs.is_empty());
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
    plan(&PlanRequest {
        graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect,
        msvc_external_includes,
    })
    .unwrap()
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
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &flags,
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/proj/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
    })
    .unwrap();
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

    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &build_flags,
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
    })
    .unwrap();

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
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("app:app")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("build")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("nope:thing")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language: Default::default(),
        compiler_wrapper: Default::default(),
        patches: Default::default(),
    };
    let graph = single_package_graph(package, "/abs/proj");
    let tc = toolchain();
    let err = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("hello:missing")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
    })
    .unwrap_err();
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
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/cdemo/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/mixed/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
    })
    .unwrap();
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
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/interop/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("c_runner")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/clib_only/build"),
        profile: dev_profile(),
        selected: Some(vec![ManifestTargetSelector::parse("c_runner")]),
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/cdemo/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/broken/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &map,
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/mixed/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
    let bg = plan(&PlanRequest {
        graph: &graph,
        toolchain: &tc,
        build_flags: &map,
        language_standards: &no_language_standards(),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/mixed/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect: Dialect::GnuLike,
        msvc_external_includes: true,
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
    plan(&PlanRequest {
        graph,
        toolchain: &tc,
        build_flags: &empty_build_flags(),
        language_standards: &standards_for(graph),
        standard_flag_conflicts: &no_flag_conflicts(),
        build_dir: PathBuf::from("/abs/build"),
        profile: dev_profile(),
        selected: None,
        configuration: None,
        selected_packages: None,
        compiler_wrapper: None,
        dialect,
        msvc_external_includes: true,
    })
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
                    interface_cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx14)),
                    ..Default::default()
                },
            ),
            target(
                "app",
                TargetKind::Executable,
                &["src/main.cc", "src/util.c"],
                &["core"],
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
    // source falls back to the built-in default c11.
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
                    interface_cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx17)),
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
        interface_cxx_standard: Some(StandardDeclaration::Declared(CxxStandard::Cxx20)),
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
    let plan_for = |selected: Option<Vec<ManifestTargetSelector>>| {
        plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            language_standards: &standards_for(&graph),
            standard_flag_conflicts: &no_flag_conflicts(),
            build_dir: PathBuf::from("/abs/build"),
            profile: dev_profile(),
            selected,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
            dialect: Dialect::GnuLike,
            msvc_external_includes: true,
        })
        .unwrap()
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
    let plan_for = |selected: Option<Vec<ManifestTargetSelector>>| {
        plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            language_standards: &standards_for(&graph),
            standard_flag_conflicts: &conflicts,
            build_dir: PathBuf::from("/abs/build"),
            profile: dev_profile(),
            selected,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
            dialect: Dialect::GnuLike,
            msvc_external_includes: true,
        })
        .unwrap()
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
