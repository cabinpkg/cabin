use std::path::Path;

use cabin_build::BuildGraph;
use serde::Serialize;

use crate::error::NinjaError;
use crate::writer::shell_join;

/// Write a Clang JSON Compilation Database describing every C/C++ compile in
/// `graph` to `path`.
pub fn write_compile_commands(path: &Path, graph: &BuildGraph) -> Result<(), NinjaError> {
    let body = render_compile_commands(graph)?;
    std::fs::write(path, body).map_err(|source| NinjaError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Render the compilation database as a UTF-8 JSON string. Pulled out so
/// unit tests can assert on the body without touching the filesystem.
pub fn render_compile_commands(graph: &BuildGraph) -> Result<String, NinjaError> {
    let mut entries: Vec<Entry<'_>> = Vec::with_capacity(graph.compile_commands.len());
    for cc in &graph.compile_commands {
        entries.push(Entry {
            directory: path_to_str(&cc.directory)?,
            file: path_to_str(&cc.file)?,
            command: shell_join(&cc.arguments)?,
            output: path_to_str(&cc.output)?,
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

fn path_to_str(p: &Path) -> Result<&str, NinjaError> {
    p.to_str()
        .ok_or_else(|| NinjaError::NonUtf8Path(p.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_build::{BuildGraph, CompileCommand};
    use std::path::PathBuf;

    fn graph_with_single_compile() -> BuildGraph {
        BuildGraph {
            actions: Vec::new(),
            default_outputs: Vec::new(),
            compile_commands: vec![CompileCommand {
                directory: PathBuf::from("/abs/build"),
                file: PathBuf::from("/abs/src/main.cc"),
                arguments: vec![
                    "/usr/bin/g++".into(),
                    "-std=c++17".into(),
                    "-c".into(),
                    "/abs/src/main.cc".into(),
                    "-o".into(),
                    "/abs/build/main.o".into(),
                ],
                output: PathBuf::from("/abs/build/main.o"),
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
            default_outputs: Vec::new(),
            compile_commands: Vec::new(),
        };
        let body = render_compile_commands(&graph).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(value.as_array().unwrap().is_empty());
    }
}
