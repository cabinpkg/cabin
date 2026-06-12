//! Transform a normal build graph into a syntax-check graph for
//! `cabin check`. Each *workspace* compile becomes an `-fsyntax-only`
//! check that produces a stamp instead of an object; archive, link,
//! and dependency-package compiles are dropped. Pure and
//! backend-independent: the transform flips each compile's
//! [`CompileMode`] to [`CompileMode::SyntaxOnly`] semantically, and
//! the dialect lowering (`cabin-ninja` via [`cabin_driver::lower()`])
//! renders it through the `c_check` / `cxx_check` rules.

use std::path::PathBuf;

use camino::{Utf8Path, Utf8PathBuf};

use cabin_driver::{BuildAction, CompileMode};

use crate::graph::BuildGraph;

/// Stamp file Ninja `touch`es after a successful syntax check. Lives
/// beside the object the normal build would have produced, so Ninja
/// creates the same parent directory and the retained depfile write
/// lands in an existing directory.
fn check_stamp_path(object: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{object}.check"))
}

/// Rewrite a build graph into a syntax-check graph: every compile of a
/// selected workspace package becomes an `-fsyntax-only` check that
/// produces a stamp instead of an object, and all archive, link, and
/// dependency-package actions are dropped. `selected_pkg_dirs` are the
/// per-package build directories (`<build_dir>/<profile>/packages/<pkg>`)
/// whose translation units should be checked.
///
/// The transform is purely semantic: it flips each surviving compile's
/// [`CompileMode`] from [`CompileMode::Object`] to
/// [`CompileMode::SyntaxOnly`] and retypes its description. The
/// compiler driver, wrapper, depfile, flags, and implicit inputs are
/// preserved untouched; the actual `-fsyntax-only` / `-MT <stamp>`
/// argv is produced later by lowering, not here. `compile_commands` is
/// passed through unchanged so IDE tooling keeps the real object-build
/// commands. Recorded MSVC standard violations are pruned with the
/// same path filter as their compiles: a dependency compile the
/// check drops must not gate the check.
///
/// `selected_pkg_dirs` stay [`std::path::PathBuf`]: they are
/// filesystem build directories used only for an ancestor comparison
/// against each object, never embedded in a command.
pub fn into_check_graph(graph: BuildGraph, selected_pkg_dirs: &[PathBuf]) -> BuildGraph {
    let mut actions = Vec::new();
    let mut default_outputs = Vec::new();
    for action in graph.actions {
        // Only compiles survive a check; archives and links are never
        // run in check mode and are dropped.
        let BuildAction::Compile(mut compile) = action else {
            continue;
        };
        // Workspace-own scope: only check translation units whose
        // object would live under a selected package's build dir.
        if !selected_pkg_dirs
            .iter()
            .any(|dir| compile.object.as_std_path().starts_with(dir))
        {
            continue;
        }
        let stamp = check_stamp_path(&compile.object);
        compile.description = format!("CHECK {}", compile.object);
        compile.mode = CompileMode::SyntaxOnly {
            stamp: stamp.clone(),
        };
        actions.push(BuildAction::Compile(compile));
        default_outputs.push(stamp);
    }
    let msvc_standard_violations = graph
        .msvc_standard_violations
        .into_iter()
        .filter(|violation| {
            selected_pkg_dirs
                .iter()
                .any(|dir| violation.object.as_std_path().starts_with(dir))
        })
        .collect();
    BuildGraph {
        actions,
        // The check graph keeps the original build's dialect so its
        // syntax-check commands are spelled the same way.
        dialect: graph.dialect,
        default_outputs,
        compile_commands: graph.compile_commands,
        msvc_standard_violations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::CompileCommand;
    use cabin_core::{OptLevel, SourceLanguage};
    use cabin_driver::{
        ArchiveAction, CompileAction, CompileArguments, Dialect, LinkAction, LoweredActionKind,
        lower,
    };

    fn compile(language: SourceLanguage, object: &str) -> BuildAction {
        let depfile = format!("{object}.d");
        let standard = match language {
            SourceLanguage::C => cabin_core::LanguageStandard::C(cabin_core::DEFAULT_C_STANDARD),
            SourceLanguage::Cxx => {
                cabin_core::LanguageStandard::Cxx(cabin_core::DEFAULT_CXX_STANDARD)
            }
        };
        BuildAction::Compile(CompileAction {
            standard,
            source: Utf8PathBuf::from("/src/a.cc"),
            object: Utf8PathBuf::from(object),
            mode: CompileMode::Object,
            implicit_inputs: vec![],
            depfile: Some(Utf8PathBuf::from(depfile)),
            compiler: Utf8PathBuf::from("/usr/bin/c++"),
            compiler_wrapper: None,
            arguments: CompileArguments {
                opt_level: OptLevel::O0,
                debug_info: false,
                define_ndebug: false,
                include_dirs: vec![],
                system_include_dirs: vec![],
                defines: vec![],
                extra_flags: vec![],
            },
            description: format!("CXX {object}"),
        })
    }

    fn archive(object_input: &str, lib: &str) -> BuildAction {
        BuildAction::Archive(ArchiveAction {
            archiver: Utf8PathBuf::from("/usr/bin/ar"),
            output: Utf8PathBuf::from(lib),
            inputs: vec![Utf8PathBuf::from(object_input)],
            description: format!("AR {lib}"),
        })
    }

    fn link(object_input: &str, exe: &str) -> BuildAction {
        BuildAction::Link(LinkAction {
            linker: Utf8PathBuf::from("/usr/bin/c++"),
            output: Utf8PathBuf::from(exe),
            inputs: vec![Utf8PathBuf::from(object_input)],
            implicit_inputs: vec![],
            arguments: vec![],
            link_libs: vec![],
            description: format!("LINK {exe}"),
        })
    }

    #[test]
    fn rewrites_workspace_compile_to_syntax_only() {
        let object = "/b/dev/packages/app/obj/app/src/a.cc.o";
        let graph = BuildGraph {
            dialect: Dialect::GnuLike,
            actions: vec![compile(SourceLanguage::Cxx, object)],
            default_outputs: vec![Utf8PathBuf::from("/b/dev/packages/app/app")],
            compile_commands: Vec::<CompileCommand>::new(),
            msvc_standard_violations: Vec::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]);

        assert_eq!(out.actions.len(), 1);
        let stamp = Utf8PathBuf::from(format!("{object}.check"));
        // The transform is semantic: the surviving action is still a
        // compile, now in syntax-only mode with the stamp as its
        // target. Driver, depfile, and flags are untouched.
        let BuildAction::Compile(c) = &out.actions[0] else {
            panic!("expected a compile action");
        };
        assert_eq!(c.standard.language(), SourceLanguage::Cxx);
        assert_eq!(
            c.mode,
            CompileMode::SyntaxOnly {
                stamp: stamp.clone()
            }
        );
        // depfile retained for incrementality; description retyped.
        assert_eq!(c.depfile, Some(Utf8PathBuf::from(format!("{object}.d"))));
        assert_eq!(c.description, format!("CHECK {object}"));
        // The new default target is the stamp, not the old exe.
        assert_eq!(out.default_outputs, vec![stamp.clone()]);

        // Lowering yields the historic syntax-only command: drop `-c`
        // and `-o <object>`, retarget the retained depfile at the stamp
        // with `-MT`, append `-fsyntax-only`, and emit the stamp as the
        // sole output.
        let lowered = lower(Dialect::GnuLike, &out.actions[0]);
        assert_eq!(lowered.kind, LoweredActionKind::SyntaxCheckCpp);
        assert!(lowered.command.contains(&"-fsyntax-only".to_string()));
        assert!(
            !lowered.command.contains(&"-c".to_string()),
            "argv = {:?}",
            lowered.command
        );
        assert!(
            !lowered.command.contains(&"-o".to_string()),
            "argv = {:?}",
            lowered.command
        );
        assert!(lowered.command.contains(&"-MD".to_string()));
        assert!(lowered.command.contains(&"-MF".to_string()));
        let mt = lowered
            .command
            .iter()
            .position(|t| t == "-MT")
            .expect("-MT present");
        assert_eq!(lowered.command[mt + 1], format!("{object}.check"));
        assert_eq!(lowered.outputs, vec![stamp]);
        assert_eq!(
            lowered.depfile,
            Some(Utf8PathBuf::from(format!("{object}.d")))
        );
    }

    #[test]
    fn rewrites_c_compile_to_c_syntax_check() {
        let object = "/b/dev/packages/app/obj/app/src/a.c.o";
        let graph = BuildGraph {
            dialect: Dialect::GnuLike,
            actions: vec![compile(SourceLanguage::C, object)],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
            msvc_standard_violations: Vec::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]);
        assert_eq!(
            lower(Dialect::GnuLike, &out.actions[0]).kind,
            LoweredActionKind::SyntaxCheckC
        );
    }

    #[test]
    fn drops_archive_and_link_actions() {
        let object = "/b/dev/packages/app/obj/app/src/a.cc.o";
        let graph = BuildGraph {
            dialect: Dialect::GnuLike,
            actions: vec![
                compile(SourceLanguage::Cxx, object),
                archive(object, "/b/dev/packages/app/libfoo.a"),
                link(object, "/b/dev/packages/app/app"),
            ],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
            msvc_standard_violations: Vec::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]);
        assert_eq!(out.actions.len(), 1, "only the compile survives");
        assert!(matches!(
            &out.actions[0],
            BuildAction::Compile(c) if matches!(c.mode, CompileMode::SyntaxOnly { .. })
        ));
    }

    #[test]
    fn preserves_compiler_wrapper_into_check_command() {
        // A ccache-wrapped C++ compile keeps its wrapper through the
        // check transform: the field is untouched, so lowering still
        // prefixes the check command with `ccache`.
        let object = "/b/dev/packages/app/obj/app/src/a.cc.o";
        let BuildAction::Compile(mut c) = compile(SourceLanguage::Cxx, object) else {
            unreachable!("compile builds a compile action");
        };
        c.compiler_wrapper = Some(Utf8PathBuf::from("/usr/local/bin/ccache"));
        let graph = BuildGraph {
            dialect: Dialect::GnuLike,
            actions: vec![BuildAction::Compile(c)],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
            msvc_standard_violations: Vec::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]);
        let lowered = lower(Dialect::GnuLike, &out.actions[0]);
        assert_eq!(lowered.command[0], "/usr/local/bin/ccache");
        assert_eq!(lowered.command[1], "/usr/bin/c++");
        assert!(lowered.command.contains(&"-fsyntax-only".to_string()));
    }

    #[test]
    fn drops_dependency_package_compiles() {
        // `app` is selected; `dep` is a dependency package, so its
        // object lives under a different package dir and is not checked.
        let app_obj = "/b/dev/packages/app/obj/app/src/a.cc.o";
        let dep_obj = "/b/dev/packages/dep/obj/dep/src/d.cc.o";
        let graph = BuildGraph {
            dialect: Dialect::GnuLike,
            actions: vec![
                compile(SourceLanguage::Cxx, app_obj),
                compile(SourceLanguage::Cxx, dep_obj),
            ],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
            msvc_standard_violations: Vec::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]);
        assert_eq!(out.actions.len(), 1);
        let app_stamp = Utf8PathBuf::from(format!("{app_obj}.check"));
        let BuildAction::Compile(c) = &out.actions[0] else {
            panic!("expected a compile action");
        };
        assert_eq!(
            c.mode,
            CompileMode::SyntaxOnly {
                stamp: app_stamp.clone()
            }
        );
        assert_eq!(out.default_outputs, vec![app_stamp]);
    }

    #[test]
    fn passes_compile_commands_through_unchanged() {
        let cc = CompileCommand {
            directory: Utf8PathBuf::from("/b"),
            file: Utf8PathBuf::from("/src/a.cc"),
            arguments: vec!["/usr/bin/c++".into(), "-c".into(), "/src/a.cc".into()],
            output: Utf8PathBuf::from("/b/dev/packages/app/obj/app/src/a.cc.o"),
        };
        let graph = BuildGraph {
            dialect: Dialect::GnuLike,
            actions: vec![],
            default_outputs: vec![],
            compile_commands: vec![cc.clone()],
            msvc_standard_violations: Vec::new(),
        };
        let out = into_check_graph(graph, &[]);
        assert_eq!(out.compile_commands, vec![cc]);
    }

    #[test]
    fn check_rewrite_prunes_dependency_standard_violations() {
        use crate::graph::MsvcStandardViolation;
        let violation = |object: &str| MsvcStandardViolation {
            target: "dep:lib".to_owned(),
            language: "C++",
            standard: "c++23",
            object: Utf8PathBuf::from(object),
        };
        let graph = BuildGraph {
            actions: vec![compile(
                SourceLanguage::Cxx,
                "/abs/build/dev/packages/app/app/main.o",
            )],
            dialect: Dialect::Msvc,
            default_outputs: Vec::new(),
            compile_commands: Vec::new(),
            msvc_standard_violations: vec![
                violation("/abs/build/dev/packages/dep/lib/dep.o"),
                violation("/abs/build/dev/packages/app/app/exotic.o"),
            ],
        };
        let selected = vec![PathBuf::from("/abs/build/dev/packages/app")];
        let checked = into_check_graph(graph, &selected);
        // The dependency package's violation is pruned with its
        // compile; the selected package's own violation survives and
        // still gates the check.
        assert_eq!(checked.msvc_standard_violations.len(), 1);
        assert_eq!(
            checked.msvc_standard_violations[0].object,
            Utf8PathBuf::from("/abs/build/dev/packages/app/app/exotic.o")
        );
        assert!(crate::validate_planned_standards(&checked).is_err());
    }
}
