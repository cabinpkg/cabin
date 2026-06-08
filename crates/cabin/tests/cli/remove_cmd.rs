//! Integration tests for `cabin remove`.
//!
//! `cabin remove <name>` deletes a `[dependencies]` (or, with `--dev`,
//! `[dev-dependencies]`) entry, preserving the rest of the manifest.
//! Status output mirrors `cargo remove`'s `Removing <name> from
//! <table>` line.

use super::*;

const MANIFEST_WITH_DEPS: &str = r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
zlib = { port = true, version = "^1.3.1" }
# xxhash stays pinned
xxhash = { port = true, version = "^0.8" }

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#;

fn manifest_with(body: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    dir.child("cabin.toml").write_str(body).unwrap();
    let path = dir.path().join("cabin.toml");
    (dir, path)
}

#[test]
fn remove_deletes_entry_and_reports_status() {
    let (_dir, manifest) = manifest_with(MANIFEST_WITH_DEPS);
    cabin()
        .args(["remove", "zlib", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains("Removing zlib from dependencies"));

    let body = std::fs::read_to_string(&manifest).unwrap();
    assert!(!body.contains("zlib"), "zlib should be gone:\n{body}");
    assert!(
        body.contains("xxhash"),
        "sibling dep should remain:\n{body}"
    );
    // Untouched sections and the surviving dep's comment survive.
    assert!(body.contains("[target.demo]"), "target lost:\n{body}");
    assert!(
        body.contains("# xxhash stays pinned"),
        "comment on surviving dep lost:\n{body}"
    );
}

#[test]
fn remove_last_dependency_drops_empty_table() {
    let body = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\nzlib = { port = true, version = \"^1.3.1\" }\n";
    let (_dir, manifest) = manifest_with(body);
    cabin()
        .args(["remove", "zlib", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success();

    let after = std::fs::read_to_string(&manifest).unwrap();
    assert!(
        !after.contains("[dependencies]"),
        "empty table should be removed:\n{after}"
    );
}

#[test]
fn remove_from_dev_dependencies() {
    let body = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dev-dependencies]\ntinyxml2 = { port = true, version = \"^11\" }\n";
    let (_dir, manifest) = manifest_with(body);
    cabin()
        .args(["remove", "tinyxml2", "--dev", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Removing tinyxml2 from dev-dependencies",
        ));

    let after = std::fs::read_to_string(&manifest).unwrap();
    assert!(
        !after.contains("tinyxml2"),
        "dev dep should be gone:\n{after}"
    );
}

#[test]
fn remove_missing_dependency_fails_with_cargo_wording() {
    let (_dir, manifest) = manifest_with(MANIFEST_WITH_DEPS);
    cabin()
        .args(["remove", "nonexistent", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "the dependency `nonexistent` could not be found in `dependencies`",
        ));

    // The manifest is left untouched on failure.
    let after = std::fs::read_to_string(&manifest).unwrap();
    assert!(
        after.contains("zlib"),
        "manifest should be unchanged:\n{after}"
    );
}

#[test]
fn remove_normal_dependency_does_not_match_dev_table() {
    // `zlib` lives in [dependencies]; `cabin remove zlib --dev` must
    // report it missing from [dev-dependencies] rather than deleting it.
    let (_dir, manifest) = manifest_with(MANIFEST_WITH_DEPS);
    cabin()
        .args(["remove", "zlib", "--dev", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "could not be found in `dev-dependencies`",
        ));

    let after = std::fs::read_to_string(&manifest).unwrap();
    assert!(
        after.contains("zlib"),
        "normal dep must be untouched:\n{after}"
    );
}

#[test]
fn remove_targets_selected_workspace_member() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cabin.toml");
    dir.child("cabin.toml")
        .write_str("[workspace]\nmembers = [\"packages/*\"]\n")
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nzlib = { port = true, version = \"^1.3.1\" }\n",
        )
        .unwrap();

    cabin()
        .args(["remove", "zlib", "--package", "app", "--manifest-path"])
        .arg(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains("Removing zlib from dependencies"));

    let member = std::fs::read_to_string(dir.path().join("packages/app/cabin.toml")).unwrap();
    assert!(
        !member.contains("zlib"),
        "member dep should be gone:\n{member}"
    );
}
