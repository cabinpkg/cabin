//! Integration tests for `cabin add`.
//!
//! Covers registry dependencies (`<scope>/<name>@<REQ>`) and local
//! path dependencies (`--path`); bare registry names are rejected
//! with the scoped-name explanation.
//! Status output mirrors `cargo add`'s visible lines.

use super::*;

const PACKAGE_MANIFEST: &str = r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#;

/// Write a single-package manifest into a fresh temp dir and return the
/// dir.
fn package_dir() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    dir.child("cabin.toml").write_str(PACKAGE_MANIFEST).unwrap();
    dir
}

#[test]
fn add_hints_to_link_the_dep_in_a_target() {
    // `[dependencies]` only declares a dep; cabin requires a target's
    // `deps` list to link it. `cabin add` should remind the
    // user of that follow-up step.
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "fmtlib/fmt@^10", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains("to link it"))
        .stdout(predicate::str::contains("deps = [\"fmtlib/fmt\"]"));
}

#[test]
fn add_dev_targets_dev_dependencies() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "fmtlib/fmt@^10", "--dev", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Adding fmtlib/fmt@^10 to dev-dependencies",
        ));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        body.contains("[dev-dependencies]"),
        "expected a [dev-dependencies] table:\n{body}"
    );
}

#[test]
fn add_with_features_and_no_default_features() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args([
            "add",
            "fmtlib/fmt@^10",
            "--features",
            "single-threaded",
            "--no-default-features",
            "--manifest-path",
        ])
        .arg(&manifest)
        .assert()
        .success();

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        body.contains("features = [\"single-threaded\"]"),
        "expected features list:\n{body}"
    );
    assert!(
        body.contains("default-features = false"),
        "expected default-features = false:\n{body}"
    );
}

#[test]
fn add_features_splits_commas_and_repeats() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args([
            "add",
            "fmtlib/fmt@^10",
            "--features",
            "a,b",
            "--features",
            "c",
            "--manifest-path",
        ])
        .arg(&manifest)
        .assert()
        .success();

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        body.contains("features = [\"a\", \"b\", \"c\"]"),
        "expected comma-split and repeated --features merged in order:\n{body}"
    );
}

#[test]
fn add_path_dependency_writes_path_entry_and_local_status() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    dir.child("mylib/cabin.toml")
        .write_str("[package]\nname = \"mylib\"\nversion = \"0.2.0\"\n")
        .unwrap();

    cabin()
        .args(["add", "--path", "mylib", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Adding mylib (local) to dependencies",
        ));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        body.contains("mylib = { path = \"mylib\" }"),
        "expected path entry written verbatim:\n{body}"
    );
}

#[test]
fn add_path_rejects_an_explicit_name() {
    // Cabin keys path deps by the target's own package name, so passing
    // a name with --path is rejected rather than silently aliased.
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    dir.child("mylib/cabin.toml")
        .write_str("[package]\nname = \"mylib\"\nversion = \"0.2.0\"\n")
        .unwrap();

    cabin()
        .args(["add", "renamed", "--path", "mylib", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "do not pass a dependency name with `--path`",
        ));
}

#[test]
fn add_path_to_missing_target_fails() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "--path", "nope", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        // The missing target manifest is surfaced by the manifest loader,
        // naming the path it tried to read.
        .stderr(predicate::str::contains("nope"));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        !body.contains("[dependencies]"),
        "manifest changed:\n{body}"
    );
}

#[test]
fn add_scoped_registry_dependency_writes_a_quoted_key() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "fmtlib/fmt@^10", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Adding fmtlib/fmt@^10 to dependencies",
        ));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        body.contains("\"fmtlib/fmt\" = \"^10\""),
        "expected a quoted scoped dependency key:\n{body}"
    );
}

/// Registry packages are always `<scope>/<name>`; a bare name is
/// explained, never guessed or searched for.
#[test]
fn add_bare_registry_name_is_rejected_with_the_scoped_explanation() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "fmt", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "registry packages are named `<scope>/<name>`",
        ));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        !body.contains("[dependencies]"),
        "manifest changed:\n{body}"
    );
}

/// `cabin add` never queries the registry, so a scoped dependency
/// without `@<REQ>` has no version to write and fails with that
/// explanation.
#[test]
fn add_scoped_registry_dependency_requires_a_requirement() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "fmtlib/fmt", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains("specify a version requirement"));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(!body.contains("fmtlib"), "manifest changed:\n{body}");
}

/// The requirement is validated the same lenient way the manifest
/// parser reads it, so a spec `cabin add` accepts also resolves.
#[test]
fn add_scoped_registry_dependency_rejects_invalid_requirements() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "fmtlib/fmt@banana", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "invalid version requirement `banana`",
        ));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(!body.contains("fmtlib"), "manifest changed:\n{body}");
}

#[test]
fn add_into_workspace_without_package_selection_fails() {
    let dir = TempDir::new().expect("tempdir");
    let manifest = dir.path().join("cabin.toml");
    dir.child("cabin.toml")
        .write_str("[workspace]\nmembers = [\"packages/*\"]\n")
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str("[package]\nname = \"app\"\nversion = \"0.1.0\"\n")
        .unwrap();

    cabin()
        .args(["add", "fmtlib/fmt@^10", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--package"));
}

#[test]
fn add_targets_selected_workspace_member() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cabin.toml");
    dir.child("cabin.toml")
        .write_str("[workspace]\nmembers = [\"packages/*\"]\n")
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str("[package]\nname = \"app\"\nversion = \"0.1.0\"\n")
        .unwrap();

    cabin()
        .args([
            "add",
            "fmtlib/fmt@^10",
            "--package",
            "app",
            "--manifest-path",
        ])
        .arg(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Adding fmtlib/fmt@^10 to dependencies",
        ));

    // The member's manifest is edited; the workspace root is untouched.
    let member = fs::read_to_string(dir.path().join("packages/app/cabin.toml")).unwrap();
    assert!(
        member.contains("\"fmtlib/fmt\" = \"^10\""),
        "member not edited:\n{member}"
    );
    let root_body = fs::read_to_string(&root).unwrap();
    assert!(
        !root_body.contains("fmtlib/fmt"),
        "root must be untouched:\n{root_body}"
    );
}

#[test]
fn add_preserves_existing_comments() {
    let dir = TempDir::new().expect("tempdir");
    let manifest = dir.path().join("cabin.toml");
    dir.child("cabin.toml")
        .write_str(&format!(
            "{PACKAGE_MANIFEST}\n[dependencies]\n# keep this note\n\"cabin-ports/xxhash\" = \"=0.8.3\"\n"
        ))
        .unwrap();

    cabin()
        .args(["add", "cabin-ports/zlib@=1.3.1", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success();

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(body.contains("# keep this note"), "comment lost:\n{body}");
    assert!(body.contains("xxhash"), "existing dep lost:\n{body}");
    assert!(body.contains("zlib"), "new dep missing:\n{body}");
}
