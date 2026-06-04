//! Transform a normal build graph into a syntax-check graph for
//! `cabin check`. Each *workspace* compile becomes an `-fsyntax-only`
//! check that produces a stamp instead of an object; archive, link,
//! and dependency-package compiles are dropped. Pure and
//! backend-independent: the transform flips each compile's
//! [`CompileMode`] to [`CompileMode::SyntaxOnly`] semantically, and
//! the GNU/Clang lowering (`cabin-ninja` via [`crate::lower`]) renders
//! it through the `c_check` / `cxx_check` rules.

use std::path::{Path, PathBuf};

use crate::action::{BuildAction, CompileMode};
use crate::error::BuildError;
use crate::graph::BuildGraph;

/// Stamp file Ninja `touch`es after a successful syntax check. Lives
/// beside the object the normal build would have produced, so Ninja
/// creates the same parent directory and the retained depfile write
/// lands in an existing directory.
fn check_stamp_path(object: &Path) -> PathBuf {
    let mut name = object.as_os_str().to_owned();
    name.push(".check");
    PathBuf::from(name)
}

fn path_to_str(p: &Path) -> Result<&str, BuildError> {
    p.to_str()
        .ok_or_else(|| BuildError::NonUtf8Path(p.to_path_buf()))
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
/// commands.
///
/// # Errors
/// Returns [`BuildError::NonUtf8Path`] when a checked object path is
/// not valid UTF-8 and cannot be embedded in the `CHECK` description.
pub fn into_check_graph(
    graph: BuildGraph,
    selected_pkg_dirs: &[PathBuf],
) -> Result<BuildGraph, BuildError> {
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
            .any(|dir| compile.object.starts_with(dir))
        {
            continue;
        }
        let stamp = check_stamp_path(&compile.object);
        compile.description = format!("CHECK {}", path_to_str(&compile.object)?);
        compile.mode = CompileMode::SyntaxOnly {
            stamp: stamp.clone(),
        };
        actions.push(BuildAction::Compile(compile));
        default_outputs.push(stamp);
    }
    Ok(BuildGraph {
        actions,
        default_outputs,
        compile_commands: graph.compile_commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{ArchiveAction, CompileAction, CompileArguments, LinkAction};
    use crate::graph::CompileCommand;
    use crate::lower::{LoweredActionKind, lower_gnu_like};
    use cabin_core::SourceLanguage;

    fn compile(language: SourceLanguage, object: &str) -> BuildAction {
        let depfile = format!("{object}.d");
        BuildAction::Compile(CompileAction {
            language,
            source: PathBuf::from("/src/a.cc"),
            object: PathBuf::from(object),
            mode: CompileMode::Object,
            implicit_inputs: vec![],
            depfile: Some(PathBuf::from(depfile)),
            compiler: PathBuf::from("/usr/bin/c++"),
            compiler_wrapper: None,
            arguments: CompileArguments {
                std_and_profile_flags: vec!["-std=c++17".into()],
                include_dirs: vec![],
                defines: vec![],
                extra_flags: vec![],
            },
            description: format!("CXX {object}"),
        })
    }

    fn archive(object_input: &str, lib: &str) -> BuildAction {
        BuildAction::Archive(ArchiveAction {
            archiver: PathBuf::from("/usr/bin/ar"),
            output: PathBuf::from(lib),
            inputs: vec![PathBuf::from(object_input)],
            description: format!("AR {lib}"),
        })
    }

    fn link(object_input: &str, exe: &str) -> BuildAction {
        BuildAction::Link(LinkAction {
            linker: PathBuf::from("/usr/bin/c++"),
            output: PathBuf::from(exe),
            inputs: vec![PathBuf::from(object_input)],
            implicit_inputs: vec![],
            arguments: vec![],
            description: format!("LINK {exe}"),
        })
    }

    #[test]
    fn rewrites_workspace_compile_to_syntax_only() {
        let object = "/b/dev/packages/app/obj/app/src/a.cc.o";
        let graph = BuildGraph {
            actions: vec![compile(SourceLanguage::Cxx, object)],
            default_outputs: vec![PathBuf::from("/b/dev/packages/app/app")],
            compile_commands: Vec::<CompileCommand>::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]).unwrap();

        assert_eq!(out.actions.len(), 1);
        let stamp = PathBuf::from(format!("{object}.check"));
        // The transform is semantic: the surviving action is still a
        // compile, now in syntax-only mode with the stamp as its
        // target. Driver, depfile, and flags are untouched.
        let BuildAction::Compile(c) = &out.actions[0] else {
            panic!("expected a compile action");
        };
        assert_eq!(c.language, SourceLanguage::Cxx);
        assert_eq!(
            c.mode,
            CompileMode::SyntaxOnly {
                stamp: stamp.clone()
            }
        );
        // depfile retained for incrementality; description retyped.
        assert_eq!(c.depfile, Some(PathBuf::from(format!("{object}.d"))));
        assert_eq!(c.description, format!("CHECK {object}"));
        // The new default target is the stamp, not the old exe.
        assert_eq!(out.default_outputs, vec![stamp.clone()]);

        // Lowering yields the historic syntax-only command: drop `-c`
        // and `-o <object>`, retarget the retained depfile at the stamp
        // with `-MT`, append `-fsyntax-only`, and emit the stamp as the
        // sole output.
        let lowered = lower_gnu_like(&out.actions[0]).unwrap();
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
        assert!(lowered.command.contains(&"-MMD".to_string()));
        assert!(lowered.command.contains(&"-MF".to_string()));
        let mt = lowered
            .command
            .iter()
            .position(|t| t == "-MT")
            .expect("-MT present");
        assert_eq!(lowered.command[mt + 1], format!("{object}.check"));
        assert_eq!(lowered.outputs, vec![stamp]);
        assert_eq!(lowered.depfile, Some(PathBuf::from(format!("{object}.d"))));
    }

    #[test]
    fn rewrites_c_compile_to_c_syntax_check() {
        let object = "/b/dev/packages/app/obj/app/src/a.c.o";
        let graph = BuildGraph {
            actions: vec![compile(SourceLanguage::C, object)],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]).unwrap();
        assert_eq!(
            lower_gnu_like(&out.actions[0]).unwrap().kind,
            LoweredActionKind::SyntaxCheckC
        );
    }

    #[test]
    fn drops_archive_and_link_actions() {
        let object = "/b/dev/packages/app/obj/app/src/a.cc.o";
        let graph = BuildGraph {
            actions: vec![
                compile(SourceLanguage::Cxx, object),
                archive(object, "/b/dev/packages/app/libfoo.a"),
                link(object, "/b/dev/packages/app/app"),
            ],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]).unwrap();
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
        c.compiler_wrapper = Some(PathBuf::from("/usr/local/bin/ccache"));
        let graph = BuildGraph {
            actions: vec![BuildAction::Compile(c)],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]).unwrap();
        let lowered = lower_gnu_like(&out.actions[0]).unwrap();
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
            actions: vec![
                compile(SourceLanguage::Cxx, app_obj),
                compile(SourceLanguage::Cxx, dep_obj),
            ],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]).unwrap();
        assert_eq!(out.actions.len(), 1);
        let app_stamp = PathBuf::from(format!("{app_obj}.check"));
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
            directory: PathBuf::from("/b"),
            file: PathBuf::from("/src/a.cc"),
            arguments: vec!["/usr/bin/c++".into(), "-c".into(), "/src/a.cc".into()],
            output: PathBuf::from("/b/dev/packages/app/obj/app/src/a.cc.o"),
        };
        let graph = BuildGraph {
            actions: vec![],
            default_outputs: vec![],
            compile_commands: vec![cc.clone()],
        };
        let out = into_check_graph(graph, &[]).unwrap();
        assert_eq!(out.compile_commands, vec![cc]);
    }
}
