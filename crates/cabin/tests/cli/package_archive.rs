use super::*;
use flate2::read::GzDecoder;
use std::collections::BTreeSet;

fn write_simple_package(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"
cxx-standard = "c++17"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("include/example.h"))
        .write_str("#pragma once\nvoid say_hello();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/fmt.cc"))
        .write_str("#include \"example.h\"\nvoid say_hello() {}\n")
        .unwrap();
}

/// Same sources as [`write_simple_package`], but with a scoped package
/// name so `cabin publish` accepts it: the registry rejects bare names.
/// The staged artifacts flatten the scope into `acme-fmt-10.2.1.*`.
fn write_scoped_package(root: &Path) {
    write_simple_package(root);
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "acme/fmt"
version = "10.2.1"
cxx-standard = "c++17"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
}

fn read_archive_entries(archive: &Path) -> BTreeSet<String> {
    let f = fs::File::open(archive).unwrap();
    let dec = GzDecoder::new(f);
    let mut tar = tar::Archive::new(dec);
    let mut out = BTreeSet::new();
    for entry in tar.entries().unwrap() {
        let entry = entry.unwrap();
        out.insert(entry.path().unwrap().to_string_lossy().into_owned());
    }
    out
}

#[test]
fn package_creates_archive_and_metadata() {
    let dir = TempDir::new().unwrap();
    write_simple_package(dir.path());
    let dist = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success();

    let archive = dist.join("fmt-10.2.1.tar.gz");
    let metadata = dist.join("fmt-10.2.1.json");
    assert!(archive.is_file(), "archive missing: {archive:?}");
    assert!(metadata.is_file(), "metadata missing: {metadata:?}");

    let body = fs::read_to_string(&metadata).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["name"], "fmt");
    assert_eq!(value["version"], "10.2.1");
    assert_eq!(value["yanked"], false);
    assert!(value["checksum"].as_str().unwrap().starts_with("sha256:"));
    assert_eq!(value["source"]["type"], "archive");
    assert_eq!(value["source"]["format"], "tar.gz");
    assert!(
        value["source"]["path"]
            .as_str()
            .unwrap()
            .ends_with("fmt-10.2.1.tar.gz")
    );
}

#[test]
fn package_metadata_preserves_manifest_compiler_wrapper_setting() {
    let dir = TempDir::new().unwrap();
    write_simple_package(dir.path());
    let manifest_path = dir.path().join("cabin.toml");
    let mut manifest = fs::read_to_string(&manifest_path).unwrap();
    manifest.push_str(
        r#"
[build]
compiler-wrapper = "ccache"
"#,
    );
    assert_fs::fixture::ChildPath::new(&manifest_path)
        .write_str(&manifest)
        .unwrap();
    let dist = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(&manifest_path)
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success();

    let body = fs::read_to_string(dist.join("fmt-10.2.1.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        value["compiler_wrapper"],
        serde_json::json!({"kind": "use", "wrapper": "ccache"})
    );
}

#[test]
fn package_json_format_emits_machine_readable_summary() {
    let dir = TempDir::new().unwrap();
    write_simple_package(dir.path());
    let dist = dir.path().join("dist");
    let value = run_json(
        cabin()
            .args(["package", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .args(["--format", "json"]),
    );
    assert_eq!(value["name"], "fmt");
    assert_eq!(value["version"], "10.2.1");
    assert!(
        value["archive_path"]
            .as_str()
            .unwrap()
            .ends_with("fmt-10.2.1.tar.gz")
    );
    assert!(
        value["metadata_path"]
            .as_str()
            .unwrap()
            .ends_with("fmt-10.2.1.json")
    );
    assert!(value["checksum"].as_str().unwrap().starts_with("sha256:"));
}

#[test]
fn package_excludes_generated_and_vcs_files() {
    let dir = TempDir::new().unwrap();
    write_simple_package(dir.path());
    // Files that must NOT appear in the archive.
    dir.child(".git/config").write_str("leak-this").unwrap();
    dir.child("build/build.ninja")
        .write_str("leak-this")
        .unwrap();
    dir.child("dist/old.tar.gz").write_str("leak-this").unwrap();
    dir.child(".cabin/cache/x").write_str("leak-this").unwrap();
    dir.child("node_modules/foo/x")
        .write_str("leak-this")
        .unwrap();
    dir.child("compile_commands.json")
        .write_str("leak-this")
        .unwrap();
    dir.child("cabin.lock").write_str("leak-this").unwrap();
    dir.child("build.ninja").write_str("leak-this").unwrap();

    let dist = dir.path().join("artifact-out");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success();

    let entries = read_archive_entries(&dist.join("fmt-10.2.1.tar.gz"));
    assert!(entries.contains("cabin.toml"));
    assert!(entries.contains("src/fmt.cc"));
    assert!(entries.contains("include/example.h"));
    for forbidden in &[
        ".git/config",
        "build/build.ninja",
        "dist/old.tar.gz",
        ".cabin/cache/x",
        "node_modules/foo/x",
        "compile_commands.json",
        "cabin.lock",
        "build.ninja",
    ] {
        assert!(
            !entries.iter().any(|e| e == forbidden),
            "archive leaked {forbidden}: {entries:?}"
        );
    }
}

#[test]
fn package_excludes_in_tree_custom_output_dir() {
    // A custom --output-dir living inside the package source
    // tree (and not on the hard-coded EXCLUDED_DIR_NAMES list)
    // must be skipped during staging so the next archive does
    // not embed last run's `.tar.gz` / `.json` and the
    // idempotent-rewrite check stays meaningful.
    let dir = TempDir::new().unwrap();
    write_simple_package(dir.path());
    let out = dir.path().join("myoutput");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&out)
        .assert()
        .success();
    // Second run uses the same in-tree output dir.  With the
    // bug present, the staging walker pulls last run's
    // archive into the new archive, the bytes drift, and the
    // idempotent rewrite refuses the differing existing
    // archive.  With the fix, the second run is a no-op.
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&out)
        .assert()
        .success();
    let entries = read_archive_entries(&out.join("fmt-10.2.1.tar.gz"));
    assert!(entries.contains("cabin.toml"));
    assert!(entries.contains("src/fmt.cc"));
    assert!(
        !entries.iter().any(|e| e.starts_with("myoutput/")),
        "custom output dir leaked into archive: {entries:?}"
    );
}

#[test]
fn package_rejects_output_dir_equal_to_package_root() {
    let dir = TempDir::new().unwrap();
    write_simple_package(dir.path());
    let assertion = cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(dir.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("equals the package source root"),
        "expected output_dir == package_root rejection, got: {stderr}"
    );
}

#[test]
fn package_is_byte_deterministic_across_runs() {
    // Write the package and the two output directories in
    // *separate* trees: `pkg-a/` and `pkg-b/`.  Both packages have
    // identical source content.  Each run targets an output dir
    // outside the package root so neither archive picks up the
    // other run's `dist-*/` contents.
    let dir = TempDir::new().unwrap();
    let pkg_a = dir.path().join("pkg-a");
    let pkg_b = dir.path().join("pkg-b");
    write_simple_package(&pkg_a);
    write_simple_package(&pkg_b);

    let dist_a = dir.path().join("dist-a");
    let dist_b = dir.path().join("dist-b");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(pkg_a.join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist_a)
        .assert()
        .success();
    cabin()
        .args(["package", "--manifest-path"])
        .arg(pkg_b.join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist_b)
        .assert()
        .success();

    let bytes_a = fs::read(dist_a.join("fmt-10.2.1.tar.gz")).unwrap();
    let bytes_b = fs::read(dist_b.join("fmt-10.2.1.tar.gz")).unwrap();
    assert_eq!(bytes_a, bytes_b, "archives must be byte-identical");
}

#[test]
fn publish_dry_run_creates_archive_and_reports_no_registry_modified() {
    let dir = TempDir::new().unwrap();
    write_scoped_package(dir.path());
    let dist = dir.path().join("dist");
    let output = cabin()
        .args(["publish", "--dry-run", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Publish dry-run"));
    assert!(stdout.contains("No registry was modified"));
    assert!(dist.join("acme-fmt-10.2.1.tar.gz").is_file());
    assert!(dist.join("acme-fmt-10.2.1.json").is_file());
}

#[test]
fn publish_dry_run_json_format_is_valid_json() {
    let dir = TempDir::new().unwrap();
    write_scoped_package(dir.path());
    let dist = dir.path().join("dist");
    let value = run_json(
        cabin()
            .args(["publish", "--dry-run", "--manifest-path"])
            .arg(dir.path().join("cabin.toml"))
            .arg("--output-dir")
            .arg(&dist)
            .args(["--format", "json"]),
    );
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["registry_modified"], false);
    assert_eq!(value["name"], "acme/fmt");
    assert_eq!(value["version"], "10.2.1");
}

#[test]
fn publish_without_dry_run_fails_clearly() {
    let dir = TempDir::new().unwrap();
    write_simple_package(dir.path());
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("--dry-run"));
}

#[test]
fn package_with_path_dependency_fails_clearly() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
local = { path = "../local" }
"#,
        )
        .unwrap();
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("path dependencies"));
}

#[test]
fn package_workspace_root_without_project_fails_clearly() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    dir.child("packages/a/cabin.toml")
        .write_str(
            r#"[package]
name = "a"
version = "0.1.0"
"#,
        )
        .unwrap();
    // `cabin package` against a workspace root must refuse
    // without a single `--package <name>` selection.
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("--package <name>"));
}

#[test]
fn package_overwrite_with_identical_bytes_succeeds() {
    let dir = TempDir::new().unwrap();
    write_simple_package(dir.path());
    let dist = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success();
    // Second run with the same input must succeed silently.
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success();
}

#[test]
fn package_overwrite_with_different_bytes_fails() {
    let dir = TempDir::new().unwrap();
    write_simple_package(dir.path());
    let dist = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .success();
    // Stomp on the existing archive with junk; a re-run must fail.
    assert_fs::fixture::ChildPath::new(dist.join("fmt-10.2.1.tar.gz"))
        .write_binary(b"not the same bytes")
        .unwrap();
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}
