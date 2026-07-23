//! Scratch-tree harness shared by the guard regression tests: each
//! test copies its real guard script(s) into a fresh scratch tree,
//! seeds the tree with synthetic inputs, and runs the guard exactly
//! as CI does.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A fresh scratch directory keyed by `name` under cargo's per-crate
/// `target/tmp`; any previous run's tree is removed so stale inputs
/// cannot leak between runs.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// Copies the named guard scripts from the real `scripts/` into
/// `<dir>/scripts/`.
pub fn copy_scripts(dir: &Path, scripts: &[&str]) {
    fs::create_dir_all(dir.join("scripts")).expect("create scratch scripts/");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
    for script in scripts {
        fs::copy(source.join(script), dir.join("scripts").join(script)).expect("copy the guard");
    }
}

/// Runs `bash scripts/<script> <args>` in `dir`; `true` means the
/// guard accepted.
pub fn bash_accepts(dir: &Path, script: &str, args: &[&str]) -> bool {
    Command::new("bash")
        .arg(format!("scripts/{script}"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run the guard")
        .status
        .success()
}
