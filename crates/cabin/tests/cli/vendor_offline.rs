//! End-to-end coverage for `cabin vendor` and `--offline`
//! mode.  Each test stages a real `.tar.gz` archive plus the
//! file-registry index that publishes its checksum, so the
//! tests exercise the full vendor → offline build pipeline.

use super::*;
use std::path::PathBuf;

/// Build a `.tar.gz` containing the given `(relative_path,
/// body)` entries and return the archive's `sha256` hex.
/// Stage a one-package file-registry index at `<root>/index`
/// containing a single `fmt 10.2.1` entry.  Returns the
/// directory the index lives in.
fn stage_fmt_index(root: &Path) -> PathBuf {
    let index = root.join("index");
    assert_fs::fixture::ChildPath::new(index.join("config.json"))

            .write_str("{\"schema\":1,\"kind\":\"file-registry\",\"packages\":\"packages\",\"artifacts\":\"artifacts\"}\n")

            .unwrap();
    let archive = index.join("artifacts/fmt/fmt-10.2.1.tar.gz");
    let manifest = "[package]\nname = \"fmt\"\nversion = \"10.2.1\"\n\n[target.fmt]\ntype = \"library\"\nsources = [\"src/fmt.cc\"]\ninclude-dirs = [\"include\"]\n";
    let header = "#pragma once\nint fmt_value();\n";
    let body = "#include \"fmt.h\"\nint fmt_value() { return 42; }\n";
    let checksum = make_archive(
        &archive,
        &[
            ("cabin.toml", manifest),
            ("include/fmt.h", header),
            ("src/fmt.cc", body),
        ],
    );
    let entry = format!(
        "{{\n  \"schema\": 1,\n  \"name\": \"fmt\",\n  \"versions\": {{\n    \"10.2.1\": {{\n      \"dependencies\": {{}},\n      \"yanked\": false,\n      \"checksum\": \"sha256:{checksum}\",\n      \"source\": {{\"type\": \"archive\", \"path\": \"../artifacts/fmt/fmt-10.2.1.tar.gz\", \"format\": \"tar.gz\"}}\n    }}\n  }}\n}}\n",
    );
    assert_fs::fixture::ChildPath::new(index.join("packages/fmt.json"))
        .write_str(&entry)
        .unwrap();
    index
}

/// Stage a small consuming package that depends on `fmt
/// 10.2.1`.  Includes a working `main.cc` so a follow-up
/// `cabin build` can succeed.
fn stage_consumer_project(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = "10.2.1"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["fmt"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
        .write_str("extern int fmt_value();\nint main() { return fmt_value() == 42 ? 0 : 1; }\n")
        .unwrap();
}

#[test]
fn vendor_writes_deterministic_file_registry() {
    let dir = TempDir::new().unwrap();
    let index = stage_fmt_index(dir.path());
    stage_consumer_project(&dir.path().join("proj"));
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(dir.path().join("proj/vendor"))
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let vendor = dir.path().join("proj/vendor");
    // file-registry skeleton + artifact + per-package index +
    // vendor summary.
    assert!(vendor.join("config.json").is_file());
    assert!(vendor.join("packages/fmt.json").is_file());
    assert!(vendor.join("artifacts/fmt/fmt-10.2.1.tar.gz").is_file());
    assert!(vendor.join("cabin-vendor.json").is_file());

    // The vendored per-package index points at the *vendor's*
    // relative archive path, not at the source index's.  This
    // is what makes the directory portable.
    let body = fs::read_to_string(vendor.join("packages/fmt.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    let path = parsed["versions"]["10.2.1"]["source"]["path"]
        .as_str()
        .unwrap();
    assert_eq!(path, "../artifacts/fmt/fmt-10.2.1.tar.gz");

    // Re-running with the same inputs must be byte-identical.
    let summary = fs::read(vendor.join("cabin-vendor.json")).unwrap();
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(&vendor)
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let summary_again = fs::read(vendor.join("cabin-vendor.json")).unwrap();
    assert_eq!(summary, summary_again);
}

#[test]
fn vendor_then_offline_build_links_against_the_vendored_dependency() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let index = stage_fmt_index(dir.path());
    stage_consumer_project(&dir.path().join("proj"));

    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(dir.path().join("proj/vendor"))
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();

    // Offline build using only the vendored directory and a
    // fresh cache.  Must NOT touch the source index (we
    // delete it to be sure).
    fs::remove_dir_all(&index).unwrap();
    cabin()
        .args(["build", "--offline", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("proj/build"))
        .arg("--index-path")
        .arg(dir.path().join("proj/vendor"))
        .arg("--cache-dir")
        .arg(dir.path().join("vendor-cache"))
        .assert()
        .success();
    let exe = dir
        .path()
        .join("proj/build/dev/packages/app")
        .join(host_exe("app"));
    assert!(exe.is_file(), "offline build must link the executable");
}

#[test]
fn offline_rejects_index_url() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "lone"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["build", "--offline", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--index-url")
        .arg("https://example.com/index")
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("--offline forbids network access"),
        "expected offline rejection, got: {stderr}"
    );
    assert!(
        stderr.contains("https://example.com/index"),
        "diagnostic should name the rejected URL, got: {stderr}"
    );
}

#[test]
fn vendor_with_no_versioned_deps_writes_skeleton_only() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "lone"
version = "0.1.0"

[target.lone]
type = "library"
sources = ["src/lib.cc"]
"#,
        )
        .unwrap();
    dir.child("src/lib.cc")
        .write_str("int lone_value() { return 0; }\n")
        .unwrap();
    let vendor = dir.path().join("vendor");
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--vendor-dir")
        .arg(&vendor)
        .assert()
        .success();
    // Empty plan still writes the file-registry skeleton so a
    // follow-up `cabin build --offline --index-path ./vendor`
    // can be a no-op rather than an error.
    assert!(vendor.join("config.json").is_file());
    assert!(vendor.join("cabin-vendor.json").is_file());
    assert!(vendor.join("packages").is_dir());
    assert!(vendor.join("artifacts").is_dir());
}

#[test]
fn vendor_locked_succeeds_when_lockfile_is_current() {
    // First a vanilla vendor run writes both the lockfile
    // and the vendor directory, then a follow-up `--locked`
    // run must succeed without rewriting the lockfile.
    let dir = TempDir::new().unwrap();
    let index = stage_fmt_index(dir.path());
    stage_consumer_project(&dir.path().join("proj"));
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(dir.path().join("proj/vendor"))
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let lock_before = fs::read_to_string(dir.path().join("proj/cabin.lock")).unwrap();
    cabin()
        .args(["vendor", "--locked", "--manifest-path"])
        .arg(dir.path().join("proj/cabin.toml"))
        .arg("--vendor-dir")
        .arg(dir.path().join("proj/vendor"))
        .arg("--index-path")
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let lock_after = fs::read_to_string(dir.path().join("proj/cabin.lock")).unwrap();
    assert_eq!(lock_before, lock_after);
}
