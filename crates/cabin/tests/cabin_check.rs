//! End-to-end tests for `cabin check`. They prove that `-fsyntax-only`
//! is actually in effect: the generated `build.ninja` carries the flag
//! and uses the check rule, and a successful run produces no object
//! files, archives, or binaries — only `.check` stamps and depfiles.

use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use assert_fs::prelude::*;

mod common;
use common::*;

/// Root of the user-facing `examples/` directory, two levels above the
/// `cabin` crate's manifest dir.
fn examples_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root should be two levels above crates/cabin")
        .join("examples")
}

/// Copy `examples/<name>/` into a fresh temp dir so checks run against
/// the copy and never accumulate `build/` directories in the source.
fn copy_example(name: &str) -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    dir.copy_from(examples_root().join(name), &["**"])
        .unwrap_or_else(|err| panic!("failed to copy example `{name}`: {err}"));
    dir
}

/// Recursively collect every file under `root` (empty if `root` is
/// absent).
fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(collect_files(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Whether any collected file has the given extension. Uses
/// `Path::extension` rather than a string suffix so a `.o` object is
/// distinguished from a `.o.check` stamp or a `.o.d` depfile.
fn any_with_ext(files: &[PathBuf], ext: &str) -> bool {
    files
        .iter()
        .any(|p| p.extension().is_some_and(|e| e == ext))
}

#[test]
fn check_produces_no_objects_archives_or_binaries() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    // `library-and-app` has both a library (would archive) and an
    // executable (would link), so the absence of a `.a` and the app
    // binary is a strong discriminator for syntax-only mode.
    let dir = copy_example("library-and-app");
    cabin()
        .args(["check", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();

    let files = collect_files(&dir.path().join("build"));
    assert!(
        !any_with_ext(&files, "o"),
        "check must not produce object files: {files:?}"
    );
    assert!(
        !any_with_ext(&files, "a"),
        "check must not produce static archives: {files:?}"
    );
    assert!(
        any_with_ext(&files, "check"),
        "check must produce syntax-check stamps: {files:?}"
    );
    assert!(
        !dir.path()
            .join("build/dev/packages/library-and-app/app")
            .exists(),
        "check must not produce the `app` executable"
    );

    // The actual command Ninja executes carries `-fsyntax-only` and
    // uses the check rule — proving the flag reaches the compiler, not
    // just that a stamp appeared.
    let ninja = std::fs::read_to_string(dir.path().join("build/dev/build.ninja"))
        .expect("build.ninja written");
    assert!(ninja.contains("-fsyntax-only"), "build.ninja:\n{ninja}");
    assert!(
        ninja.contains(": cxx_check "),
        "expected a cxx_check edge:\n{ninja}"
    );
    assert!(
        !ninja.contains(": link_executable "),
        "check must emit no link edge:\n{ninja}"
    );
    assert!(
        !ninja.contains(": cxx_archive "),
        "check must emit no archive edge:\n{ninja}"
    );
    assert!(
        !ninja.contains(": cxx_compile "),
        "check must emit no normal compile edge:\n{ninja}"
    );
}

#[test]
fn check_fails_on_semantic_error() {
    if !build_tools_available() {
        eprintln!("test skipped: requires ninja + a C++ compiler");
        return;
    }
    let dir = TempDir::new().expect("temp dir");
    dir.child("cabin.toml")
        .write_str(
            "[package]\nname = \"broken\"\nversion = \"0.1.0\"\n\n\
             [target.broken]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n",
        )
        .unwrap();
    // Body-level use of an undeclared identifier: a front-end
    // (semantic) error `-fsyntax-only` catches, NOT a link error.
    dir.child("src/main.cc")
        .write_str("int main() { return undeclared_symbol(); }\n")
        .unwrap();

    let assert = cabin()
        .args(["check", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();

    // The failure must be the compiler's front-end diagnostic — proof
    // that `-fsyntax-only` actually ran — not merely a non-zero exit
    // from some unrelated error. Ninja forwards the failed command's
    // output on its stdout, so check both streams.
    let out = assert.get_output();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("undeclared"),
        "expected the compiler's semantic diagnostic in the output, got:\n{combined}"
    );

    // Even on failure, no object file is produced.
    let files = collect_files(&dir.path().join("build"));
    assert!(
        !any_with_ext(&files, "o"),
        "failed check must not leave object files: {files:?}"
    );
}
