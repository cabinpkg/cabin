//! The long-lived verifier secret's variable name (spelled from parts
//! below, so this file does not flag itself) is retired: the verify
//! workflow mints per run through the trusted-publishing exchange, and
//! the admin tooling reads `CABIN_REGISTRY_TOKEN`.  The dead name must
//! not come back as a reader, a scrub entry, or fresh documentation -
//! only history may mention it, and history says so.
//!
//! A lexical sweep on purpose, not behavioral coverage: the retirement
//! is a naming contract across docs, scrub lists and operator prose,
//! most of which no behavior exercises - a returning reader would
//! behave exactly like the live one, just under the wrong name.

use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn the_retired_verify_token_name_is_only_mentioned_as_history() {
    let needle = ["REGISTRY", "VERIFY", "TOKEN"].join("_");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    sources(&root, &mut files);
    assert!(!files.is_empty(), "the walk found the repository");

    let mut offenses = Vec::new();
    for path in files {
        let Ok(text) = fs::read_to_string(&path) else {
            continue; // non-UTF-8 bytes carry no variable names
        };
        if !text.contains(&needle) {
            continue;
        }
        if path.extension().is_some_and(|extension| extension == "md") {
            // Prose has no comment marker; a document that mentions the
            // name must say somewhere that the credential is retired.
            if !text.contains("retired") {
                offenses.push(format!(
                    "{}: mentions {needle} without calling it retired",
                    path.display()
                ));
            }
            continue;
        }
        for (index, line) in text.lines().enumerate() {
            if line.contains(&needle)
                && !(line.trim_start().starts_with("//") && line.contains("retired"))
            {
                offenses.push(format!(
                    "{}:{}: {needle} outside a comment marked retired",
                    path.display(),
                    index + 1
                ));
            }
        }
    }
    assert!(offenses.is_empty(), "\n{}", offenses.join("\n"));
}

/// Every `.rs` and `.md` file under `dir`, skipping dot-directories
/// (`.git`, `.wrangler`, ...) and build/dependency trees.
fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read a repository directory") {
        let entry = entry.expect("read a directory entry");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        if path.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                sources(&path, found);
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension == "rs" || extension == "md")
        {
            found.push(path);
        }
    }
}
