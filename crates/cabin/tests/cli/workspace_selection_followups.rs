use super::*;

const FMT_PKG_MANIFEST: &str = r#"[package]
name = "fmt"
version = "10.2.1"
cxx-standard = "c++17"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#;
const FMT_HEADER: &str = "#pragma once\nvoid say_hello();\n";
const FMT_SRC: &str =
    "#include <iostream>\n#include \"fmt.h\"\nvoid say_hello() { std::cout << \"hello\\n\"; }\n";
const APP_MAIN_USING_FMT: &str = "#include \"fmt.h\"\nint main() { say_hello(); return 0; }\n";

/// Selection-aware fixture: `app` (which declares a versioned
/// dep on `fmt`) plus an unrelated workspace member `b` which
/// declares a versioned dep on `spdlog` that the index does
/// *not* cover.  The fixture builds a real `fmt-10.2.1.tar.gz`
/// archive and writes a matching index entry pointing at it,
/// so `cabin fetch -p app` and `cabin build -p app` can
/// succeed end-to-end without ever consulting `spdlog`.
///
/// Returns the sha256 hex of the produced archive so callers
/// can assert against the cache layout.
fn write_workspace_with_real_fmt_archive(root: &Path) -> String {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["fmt"]

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/src/main.cc"))
        .write_str(APP_MAIN_USING_FMT)
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/b/cabin.toml"))
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
spdlog = "^1"
"#,
        )
        .unwrap();
    let archive_path = root.join("artifacts/fmt-10.2.1.tar.gz");
    let hex = make_archive(
        &archive_path,
        &[
            ("cabin.toml", FMT_PKG_MANIFEST),
            ("include/fmt.h", FMT_HEADER),
            ("src/fmt.cc", FMT_SRC),
        ],
    );
    let index_body = format!(
        r#"{{
  "schema": 1,
  "name": "fmt",
  "versions": {{
    "10.2.1": {{
      "dependencies": {{}},
      "yanked": false,
      "checksum": "sha256:{hex}",
      "source": {{ "type": "archive", "path": "../artifacts/fmt-10.2.1.tar.gz", "format": "tar.gz" }}
    }}
  }}
}}"#
    );
    assert_fs::fixture::ChildPath::new(root.join("index/fmt.json"))
        .write_str(&index_body)
        .unwrap();
    hex
}

/// `cabin resolve -p app` must succeed when only `app`'s
/// versioned deps are covered by the index, even if an
/// unrelated workspace member declares a versioned dep that
/// the index does not know about.
#[test]
fn resolve_p_app_succeeds_when_unrelated_dep_missing_from_index() {
    let dir = TempDir::new().unwrap();
    write_workspace_with_real_fmt_archive(dir.path());
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "app", "--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("fmt"),
        "resolve -p app should report `fmt` in its output: {stdout}"
    );
}

/// `cabin fetch -p app` against the same fixture must fully
/// succeed: the `fmt` archive is in the index, has a real
/// checksum, and selection-aware loading must skip the
/// unrelated `spdlog` dep declared by `b`.  We verify both
/// cache state (the archive lands in `archives/sha256/<hex>`)
/// and lockfile state (the lockfile pins `fmt` at the
/// archive's checksum).
#[test]
fn fetch_p_app_extracts_fmt_and_skips_unrelated_dep() {
    let dir = TempDir::new().unwrap();
    let hex = write_workspace_with_real_fmt_archive(dir.path());
    let cache = dir.path().join("cache");
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "app", "--index-path"])
        .arg(dir.path().join("index"))
        .args(["--cache-dir"])
        .arg(&cache)
        .assert()
        .success();
    let archive_in_cache = cache.join("archives/sha256").join(format!("{hex}.tar.gz"));
    assert!(
        archive_in_cache.is_file(),
        "fmt archive must be cached at {archive_in_cache:?}"
    );
    let source_in_cache = cache.join("sources/sha256").join(&hex);
    assert!(
        source_in_cache.join("cabin.toml").is_file(),
        "fmt source must be extracted with cabin.toml at root"
    );
    let lock_path = dir.path().join("cabin.lock");
    assert!(lock_path.is_file(), "workspace lockfile should be written");
    let lock_body = fs::read_to_string(&lock_path).unwrap();
    assert!(
        lock_body.contains(r#"name = "fmt""#),
        "lockfile must pin fmt: {lock_body}"
    );
    assert!(
        lock_body.contains(&format!("checksum = \"sha256:{hex}\"")),
        "lockfile must record fmt's archive checksum: {lock_body}"
    );
    assert!(
        !lock_body.contains("spdlog"),
        "selection-aware fetch must not pin spdlog: {lock_body}"
    );
}

/// `cabin build -p app` against the same fixture must succeed
/// end-to-end when the host toolchain is available: the
/// `fmt` archive is fetched and extracted, the C++ link picks
/// up its `library` target, and the resulting `app`
/// executable lands under the build directory. `b` and its
/// unindexed `spdlog` dep never enter the build graph.
#[test]
fn build_p_app_links_against_real_fmt_archive() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_workspace_with_real_fmt_archive(dir.path());
    let build_dir = dir.path().join("build");
    let cache = dir.path().join("cache");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "app", "--build-dir"])
        .arg(&build_dir)
        .args(["--cache-dir"])
        .arg(&cache)
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let app_exe = build_dir.join("dev/packages/app").join(host_exe("app"));
    assert!(
        app_exe.is_file(),
        "app executable must be produced at {app_exe:?}"
    );
}

/// An unsafe package name in a workspace member manifest must
/// fail at manifest parsing time, *before* any sparse-HTTP
/// URL is constructed.  This pins the rule that
/// `PackageName::new` is the structural gate.
#[test]
fn unsafe_package_name_in_manifest_rejected_before_http_url() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "foo?bar"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("\"foo?bar\""),
        "error must echo the offending name: {stderr}"
    );
    assert!(
        stderr.contains("ASCII letters") && stderr.contains("ASCII digits"),
        "error must describe the allowed alphabet: {stderr}"
    );
}

/// The manifest dependency *name* is also validated up-front.
/// A direct dep named `foo#bar` (a URL-reserved character) is
/// rejected at parse time so a later `--index-url` flow
/// cannot expand it into a hostile URL.
#[test]
fn unsafe_dep_name_in_manifest_rejected_before_http_url() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")

            .write_str("[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\n[dependencies]\n\"foo#bar\" = \"1.0.0\"\n")

            .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("foo#bar"),
        "expected error mentioning unsafe dep name foo#bar; stderr was: {stderr}"
    );
}
