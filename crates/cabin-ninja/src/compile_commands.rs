use std::path::Path;

use cabin_build::BuildGraph;
use serde::Serialize;

use crate::error::NinjaError;
use crate::writer::{atomically_write, shell_join};

/// Write a Clang JSON Compilation Database describing every C/C++ compile in
/// `graph` to `path`.
///
/// Replacement is atomic: the rendered JSON lands in a sibling
/// temporary file and only renames onto `path` after a successful
/// write, so an interrupted run leaves the previous
/// `compile_commands.json` in place.  The parent directory must
/// already exist.
///
/// # Errors
/// Propagates rendering failures from [`render_compile_commands`]
/// ([`NinjaError::UnquotableArgument`] or [`NinjaError::Json`]), and
/// returns [`NinjaError::Io`] when the atomic write to `path` fails (for
/// example, when the parent directory is missing).
pub fn write_compile_commands(path: &Path, graph: &BuildGraph) -> Result<(), NinjaError> {
    let body = render_compile_commands(graph)?;
    atomically_write(path, body.as_bytes())
}

/// Render the compilation database as a UTF-8 JSON string.  Pulled out so
/// unit tests can assert on the body without touching the filesystem.
///
/// The directory, file, and output paths come from the semantic build
/// graph as `Utf8Path`s, so emitting them as JSON strings is
/// infallible.
///
/// # Errors
/// Returns [`NinjaError::UnquotableArgument`] when a compile command
/// argument cannot be shell-quoted, and [`NinjaError::Json`] if
/// `serde_json` fails to serialize the entries.
pub fn render_compile_commands(graph: &BuildGraph) -> Result<String, NinjaError> {
    let mut entries: Vec<Entry<'_>> = Vec::with_capacity(graph.compile_commands.len());
    for cc in &graph.compile_commands {
        entries.push(Entry {
            directory: cc.directory.as_str(),
            file: cc.file.as_str(),
            command: shell_join(&cc.arguments)?,
            output: cc.output.as_str(),
        });
    }
    let json = serde_json::to_string_pretty(&entries)?;
    Ok(json)
}

#[derive(Serialize)]
struct Entry<'a> {
    directory: &'a str,
    file: &'a str,
    command: String,
    output: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_build::{BuildGraph, CompileCommand, Dialect};
    use camino::Utf8PathBuf;
    use std::collections::BTreeSet;

    fn graph_with_single_compile() -> BuildGraph {
        BuildGraph {
            actions: Vec::new(),
            dialect: Dialect::GnuLike,
            default_outputs: Vec::new(),
            standard_violations: Vec::new(),
            standard_compat_violations: Vec::new(),
            planned_packages: BTreeSet::default(),
            compile_commands: vec![CompileCommand {
                directory: Utf8PathBuf::from("/abs/build"),
                file: Utf8PathBuf::from("/abs/src/main.cc"),
                arguments: vec![
                    "/usr/bin/g++".into(),
                    "-std=c++17".into(),
                    "-c".into(),
                    "/abs/src/main.cc".into(),
                    "-o".into(),
                    "/abs/build/main.o".into(),
                ],
                output: Utf8PathBuf::from("/abs/build/main.o"),
            }],
        }
    }

    #[test]
    fn produces_valid_json_array() {
        let body = render_compile_commands(&graph_with_single_compile()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).expect("must be valid JSON");
        let arr = value.as_array().expect("top level must be an array");
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn entry_has_required_fields() {
        let body = render_compile_commands(&graph_with_single_compile()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let entry = &value[0];
        assert_eq!(entry["directory"], "/abs/build");
        assert_eq!(entry["file"], "/abs/src/main.cc");
        assert_eq!(entry["output"], "/abs/build/main.o");
        let command = entry["command"].as_str().unwrap();
        assert!(command.contains("/usr/bin/g++"));
        assert!(command.contains("-std=c++17"));
        assert!(command.contains("/abs/src/main.cc"));
        assert!(command.contains("/abs/build/main.o"));
    }

    #[test]
    fn empty_graph_renders_empty_array() {
        let graph = BuildGraph {
            actions: Vec::new(),
            dialect: Dialect::GnuLike,
            default_outputs: Vec::new(),
            compile_commands: Vec::new(),
            standard_violations: Vec::new(),
            standard_compat_violations: Vec::new(),
            planned_packages: BTreeSet::default(),
        };
        let body = render_compile_commands(&graph).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(value.as_array().unwrap().is_empty());
    }

    #[test]
    fn write_compile_commands_creates_file_with_rendered_body() {
        let dir = assert_fs::TempDir::new().unwrap();
        let path = dir.path().join("compile_commands.json");
        let graph = graph_with_single_compile();
        write_compile_commands(&path, &graph).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, render_compile_commands(&graph).unwrap());
    }

    #[test]
    fn write_compile_commands_replaces_existing_contents() {
        let dir = assert_fs::TempDir::new().unwrap();
        let path = dir.path().join("compile_commands.json");
        std::fs::write(&path, "stale\n").unwrap();
        let graph = graph_with_single_compile();
        write_compile_commands(&path, &graph).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, render_compile_commands(&graph).unwrap());
    }

    #[test]
    fn write_compile_commands_reports_destination_when_parent_directory_missing() {
        let dir = assert_fs::TempDir::new().unwrap();
        let missing_parent = dir.path().join("nonexistent").join("compile_commands.json");
        let graph = graph_with_single_compile();
        let err = write_compile_commands(&missing_parent, &graph).unwrap_err();
        match err {
            NinjaError::Io { path, .. } => assert_eq!(path, missing_parent),
            other => panic!("expected NinjaError::Io, got {other:?}"),
        }
    }
}
