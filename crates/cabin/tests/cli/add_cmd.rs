//! Integration tests for `cabin add`.
//!
//! v1 supports foundation-port dependencies (`--port`) and local path
//! dependencies (`--path`); bare registry names are rejected until a
//! registry exists.  Status output mirrors `cargo add`'s visible lines.

use super::*;

const PACKAGE_MANIFEST: &str = r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#;

/// Write a single-package manifest into a fresh temp dir and return the
/// dir plus its `cabin.toml` path.
fn package_dir() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    dir.child("cabin.toml").write_str(PACKAGE_MANIFEST).unwrap();
    dir
}

#[test]
fn add_port_writes_caret_port_dependency_and_status() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "--port", "zlib", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Adding zlib v1.3.1 to dependencies",
        ));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        body.contains("[dependencies]"),
        "expected a [dependencies] table:\n{body}"
    );
    assert!(
        body.contains("zlib = { port = true, version = \"^1.3.1\" }"),
        "expected caret-pinned port entry:\n{body}"
    );
}

#[test]
fn add_hints_to_link_the_dep_in_a_target() {
    // `[dependencies]` only declares a dep; cabin requires a target's
    // `deps` list to link it. `cabin add` should remind the
    // user of that follow-up step.
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "--port", "zlib", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains("to link it"))
        .stdout(predicate::str::contains("deps = [\"zlib\"]"));
}

#[test]
fn add_port_with_explicit_requirement_is_written_verbatim() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "--port", "zlib@^1.3", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success();

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        body.contains("zlib = { port = true, version = \"^1.3\" }"),
        "expected the user's requirement verbatim:\n{body}"
    );
}

#[test]
fn add_port_unknown_recipe_fails() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "--port", "definitely-not-a-port", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no bundled foundation port named `definitely-not-a-port`",
        ));

    // A failed add must not mutate the manifest.
    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        !body.contains("[dependencies]"),
        "manifest changed:\n{body}"
    );
}

#[test]
fn add_port_requirement_with_no_matching_recipe_fails() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "--port", "zlib@^99", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no bundled foundation port `zlib` matches `^99`",
        ));
}

#[test]
fn add_port_without_a_name_fails() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "--port", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires a port name"));
}

#[test]
fn add_port_invalid_requirement_fails() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "--port", "zlib@not-a-version", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid version requirement"));
}

#[test]
fn add_port_dev_targets_dev_dependencies() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "--port", "zlib", "--dev", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Adding zlib v1.3.1 to dev-dependencies",
        ));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(
        body.contains("[dev-dependencies]"),
        "expected a [dev-dependencies] table:\n{body}"
    );
}

#[test]
fn add_port_with_features_and_no_default_features() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args([
            "add",
            "--port",
            "sqlite3",
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
            "--port",
            "sqlite3",
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
fn add_registry_dependency_is_rejected() {
    let dir = package_dir();
    let manifest = dir.path().join("cabin.toml");
    cabin()
        .args(["add", "fmt", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "registry dependencies are not supported",
        ));

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(!body.contains("fmt"), "manifest changed:\n{body}");
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
        .args(["add", "--port", "zlib", "--manifest-path"])
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
            "--port",
            "zlib",
            "--package",
            "app",
            "--manifest-path",
        ])
        .arg(&root)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Adding zlib v1.3.1 to dependencies",
        ));

    // The member's manifest is edited; the workspace root is untouched.
    let member = fs::read_to_string(dir.path().join("packages/app/cabin.toml")).unwrap();
    assert!(
        member.contains("zlib = { port = true"),
        "member not edited:\n{member}"
    );
    let root_body = fs::read_to_string(&root).unwrap();
    assert!(
        !root_body.contains("zlib"),
        "root must be untouched:\n{root_body}"
    );
}

#[test]
fn add_preserves_existing_comments() {
    let dir = TempDir::new().expect("tempdir");
    let manifest = dir.path().join("cabin.toml");
    dir.child("cabin.toml")
        .write_str(&format!("{PACKAGE_MANIFEST}\n[dependencies]\n# keep this note\nxxhash = {{ port = true, version = \"^0.8\" }}\n"))
        .unwrap();

    cabin()
        .args(["add", "--port", "zlib", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .success();

    let body = fs::read_to_string(&manifest).unwrap();
    assert!(body.contains("# keep this note"), "comment lost:\n{body}");
    assert!(body.contains("xxhash"), "existing dep lost:\n{body}");
    assert!(body.contains("zlib"), "new dep missing:\n{body}");
}
