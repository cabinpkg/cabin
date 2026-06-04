//! Transform a normal build graph into a syntax-check graph for
//! `cabin check`. Each *workspace* compile becomes an `-fsyntax-only`
//! check that produces a stamp instead of an object; archive, link,
//! and dependency-package compiles are dropped. Pure and
//! backend-independent: `cabin-ninja` renders the resulting actions
//! through the `c_check` / `cxx_check` rules.

use std::path::{Path, PathBuf};

use crate::error::BuildError;
use crate::graph::{Action, ActionKind, BuildGraph};

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

/// Rewrite a normal compile argv into an `-fsyntax-only` check: drop
/// `-c`, drop `-o <object>`, point the retained depfile at the stamp
/// with `-MT <stamp>`, and append `-fsyntax-only`. The
/// `-MMD -MF <depfile>` pair is preserved so Ninja keeps header-edit
/// incrementality via `deps = gcc`. A leading compiler-cache wrapper
/// (`ccache …`) is preserved because it is just a prefix of the argv.
fn syntax_only_command(command: &[String], stamp: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(command.len() + 3);
    let mut i = 0;
    while i < command.len() {
        match command[i].as_str() {
            "-c" => i += 1,
            "-o" => i += 2, // drop `-o` and the object-path argument
            "-MF" => {
                out.push(command[i].clone());
                if i + 1 < command.len() {
                    out.push(command[i + 1].clone());
                }
                out.push("-MT".to_owned());
                out.push(stamp.to_owned());
                i += 2;
            }
            _ => {
                out.push(command[i].clone());
                i += 1;
            }
        }
    }
    out.push("-fsyntax-only".to_owned());
    out
}

/// Rewrite a build graph into a syntax-check graph: every compile of a
/// selected workspace package becomes an `-fsyntax-only` check that
/// produces a stamp instead of an object, and all archive, link, and
/// dependency-package actions are dropped. `selected_pkg_dirs` are the
/// per-package build directories (`<build_dir>/<profile>/packages/<pkg>`)
/// whose translation units should be checked.
///
/// # Errors
/// Returns [`BuildError::NonUtf8Path`] when a stamp path is not valid
/// UTF-8 and cannot be embedded in the compiler command line.
pub fn into_check_graph(
    graph: BuildGraph,
    selected_pkg_dirs: &[PathBuf],
) -> Result<BuildGraph, BuildError> {
    let mut actions = Vec::new();
    let mut default_outputs = Vec::new();
    for action in graph.actions {
        let Action {
            kind,
            inputs,
            implicit_inputs,
            outputs,
            depfile,
            command,
            description: _,
        } = action;
        let check_kind = match kind {
            ActionKind::CompileC => ActionKind::SyntaxCheckC,
            ActionKind::CompileCpp => ActionKind::SyntaxCheckCpp,
            // Archives and links are never run in check mode; the
            // planner never emits SyntaxCheck* (only this function does).
            ActionKind::ArchiveStaticLibrary
            | ActionKind::LinkExecutable
            | ActionKind::SyntaxCheckC
            | ActionKind::SyntaxCheckCpp => continue,
        };
        // A compile action always carries exactly one object output
        // (planner invariant). A missing one would be a malformed
        // graph; skip defensively rather than panic.
        let Some(object) = outputs.into_iter().next() else {
            continue;
        };
        // Workspace-own scope: only check translation units whose
        // object would live under a selected package's build dir.
        if !selected_pkg_dirs.iter().any(|dir| object.starts_with(dir)) {
            continue;
        }
        let stamp = check_stamp_path(&object);
        let stamp_str = path_to_str(&stamp)?.to_owned();
        let command = syntax_only_command(&command, &stamp_str);
        let description = format!("CHECK {}", path_to_str(&object)?);
        actions.push(Action {
            kind: check_kind,
            inputs,
            implicit_inputs,
            outputs: vec![stamp.clone()],
            depfile,
            command,
            description,
        });
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
    use crate::graph::CompileCommand;

    fn compile(kind: ActionKind, object: &str) -> Action {
        let depfile = format!("{object}.d");
        Action {
            kind,
            inputs: vec![PathBuf::from("/src/a.cc")],
            implicit_inputs: vec![],
            outputs: vec![PathBuf::from(object)],
            depfile: Some(PathBuf::from(&depfile)),
            command: vec![
                "/usr/bin/c++".into(),
                "-std=c++17".into(),
                "-MMD".into(),
                "-MF".into(),
                depfile,
                "-c".into(),
                "/src/a.cc".into(),
                "-o".into(),
                object.into(),
            ],
            description: format!("CXX {object}"),
        }
    }

    fn archive(object_input: &str, lib: &str) -> Action {
        Action {
            kind: ActionKind::ArchiveStaticLibrary,
            inputs: vec![PathBuf::from(object_input)],
            implicit_inputs: vec![],
            outputs: vec![PathBuf::from(lib)],
            depfile: None,
            command: vec![
                "/usr/bin/ar".into(),
                "crs".into(),
                lib.into(),
                object_input.into(),
            ],
            description: format!("AR {lib}"),
        }
    }

    fn link(object_input: &str, exe: &str) -> Action {
        Action {
            kind: ActionKind::LinkExecutable,
            inputs: vec![PathBuf::from(object_input)],
            implicit_inputs: vec![],
            outputs: vec![PathBuf::from(exe)],
            depfile: None,
            command: vec![
                "/usr/bin/c++".into(),
                object_input.into(),
                "-o".into(),
                exe.into(),
            ],
            description: format!("LINK {exe}"),
        }
    }

    #[test]
    fn rewrites_workspace_compile_to_syntax_only() {
        let object = "/b/dev/packages/app/obj/app/src/a.cc.o";
        let graph = BuildGraph {
            actions: vec![compile(ActionKind::CompileCpp, object)],
            default_outputs: vec![PathBuf::from("/b/dev/packages/app/app")],
            compile_commands: Vec::<CompileCommand>::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]).unwrap();

        assert_eq!(out.actions.len(), 1);
        let a = &out.actions[0];
        assert_eq!(a.kind, ActionKind::SyntaxCheckCpp);
        assert!(a.command.contains(&"-fsyntax-only".to_string()));
        assert!(
            !a.command.contains(&"-c".to_string()),
            "argv = {:?}",
            a.command
        );
        assert!(
            !a.command.contains(&"-o".to_string()),
            "argv = {:?}",
            a.command
        );
        // depfile retained for incrementality, retargeted at the stamp.
        assert!(a.command.contains(&"-MMD".to_string()));
        assert!(a.command.contains(&"-MF".to_string()));
        let stamp = format!("{object}.check");
        let mt = a
            .command
            .iter()
            .position(|t| t == "-MT")
            .expect("-MT present");
        assert_eq!(a.command[mt + 1], stamp);
        // output is the stamp, not the object; depfile preserved.
        assert_eq!(a.outputs, vec![PathBuf::from(&stamp)]);
        assert_eq!(a.depfile, Some(PathBuf::from(format!("{object}.d"))));
        assert_eq!(out.default_outputs, vec![PathBuf::from(&stamp)]);
    }

    #[test]
    fn rewrites_c_compile_to_c_syntax_check() {
        let object = "/b/dev/packages/app/obj/app/src/a.c.o";
        let graph = BuildGraph {
            actions: vec![compile(ActionKind::CompileC, object)],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]).unwrap();
        assert_eq!(out.actions[0].kind, ActionKind::SyntaxCheckC);
    }

    #[test]
    fn drops_archive_and_link_actions() {
        let object = "/b/dev/packages/app/obj/app/src/a.cc.o";
        let graph = BuildGraph {
            actions: vec![
                compile(ActionKind::CompileCpp, object),
                archive(object, "/b/dev/packages/app/libfoo.a"),
                link(object, "/b/dev/packages/app/app"),
            ],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]).unwrap();
        assert_eq!(out.actions.len(), 1, "only the compile survives");
        assert!(out.actions.iter().all(|a| matches!(
            a.kind,
            ActionKind::SyntaxCheckC | ActionKind::SyntaxCheckCpp
        )));
    }

    #[test]
    fn drops_dependency_package_compiles() {
        // `app` is selected; `dep` is a dependency package, so its
        // object lives under a different package dir and is not checked.
        let app_obj = "/b/dev/packages/app/obj/app/src/a.cc.o";
        let dep_obj = "/b/dev/packages/dep/obj/dep/src/d.cc.o";
        let graph = BuildGraph {
            actions: vec![
                compile(ActionKind::CompileCpp, app_obj),
                compile(ActionKind::CompileCpp, dep_obj),
            ],
            default_outputs: vec![],
            compile_commands: Vec::<CompileCommand>::new(),
        };
        let out = into_check_graph(graph, &[PathBuf::from("/b/dev/packages/app")]).unwrap();
        assert_eq!(out.actions.len(), 1);
        assert_eq!(
            out.actions[0].outputs,
            vec![PathBuf::from(format!("{app_obj}.check"))]
        );
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
