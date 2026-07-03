//! `cabin test` dev-dependency activation: `[dev-dependencies]` of
//! the selected packages become real graph edges (path, port, and
//! versioned sources), so `test` targets may link dev-only
//! packages.  Ordinary commands keep dev deps declaration-only,
//! and the activation never propagates to transitive deps.

use super::*;

/// Write the shared two-package fixture: `app` declares `harness`
/// under `[dev-dependencies]` via a path source, and only the
/// `app_test` target references it.  Returns the app manifest path.
fn write_app_with_dev_path_harness(root: &Path) -> PathBuf {
    assert_fs::fixture::ChildPath::new(root.join("harness/cabin.toml"))
        .write_str(
            r#"[package]
name = "harness"
version = "0.1.0"
cxx-standard = "c++17"

[target.harness]
type = "library"
sources = ["src/harness.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("harness/include/harness.h"))
        .write_str("#pragma once\nint harness_answer();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("harness/src/harness.cc"))
        .write_str("#include \"harness.h\"\nint harness_answer() { return 42; }\n")
        .unwrap();

    assert_fs::fixture::ChildPath::new(root.join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dev-dependencies]
harness = { path = "../harness" }

[target.applib]
type = "library"
sources = ["src/applib.cc"]
include-dirs = ["include"]

[target.app_test]
type = "test"
sources = ["tests/app_test.cc"]
deps = ["applib", "harness"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/include/applib.h"))
        .write_str("#pragma once\nint applib_answer();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/src/applib.cc"))
        .write_str("#include \"applib.h\"\nint applib_answer() { return 42; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/tests/app_test.cc"))
        .write_str(
            "#include \"applib.h\"\n#include \"harness.h\"\nint main() { return applib_answer() == harness_answer() ? 0 : 1; }\n",
        )
        .unwrap();
    root.join("app/cabin.toml")
}

#[test]
fn test_links_dev_path_dependency_into_test_target() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let manifest = write_app_with_dev_path_harness(dir.path());
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("test app:app_test ... ok"),
        "test executable linking the dev path dep should run: {stdout}"
    );
}

#[test]
fn build_keeps_dev_path_dependency_declaration_only() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let manifest = write_app_with_dev_path_harness(dir.path());
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    // The dev dep never enters the ordinary build: no compile,
    // archive, or link action mentions it.
    let ninja = fs::read_to_string(build_dir.join("dev/build.ninja")).unwrap();
    assert!(
        !ninja.contains("harness"),
        "dev path dep must stay out of the ordinary build graph: {ninja}"
    );
}

#[test]
fn build_tolerates_missing_dev_path_dependency_directory() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let manifest = write_app_with_dev_path_harness(dir.path());
    fs::remove_dir_all(dir.path().join("harness")).unwrap();
    // Ordinary commands never materialize the dev edge, so the
    // missing directory cannot break them.
    cabin()
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
}

#[test]
fn test_links_dev_path_dependency_into_c_test_target() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("charness/cabin.toml"))
        .write_str(
            r#"[package]
name = "charness"
version = "0.1.0"
c-standard = "c11"

[target.charness]
type = "library"
sources = ["src/charness.c"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("charness/include/charness.h"))
        .write_str("#pragma once\nint charness_answer(void);\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("charness/src/charness.c"))
        .write_str("#include \"charness.h\"\nint charness_answer(void) { return 7; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("capp/cabin.toml"))
        .write_str(
            r#"[package]
name = "capp"
version = "0.1.0"
c-standard = "c11"

[dev-dependencies]
charness = { path = "../charness" }

[target.capp_test]
type = "test"
sources = ["tests/capp_test.c"]
deps = ["charness"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("capp/tests/capp_test.c"))
        .write_str(
            "#include \"charness.h\"\nint main(void) { return charness_answer() == 7 ? 0 : 1; }\n",
        )
        .unwrap();
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("capp/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("test capp:capp_test ... ok"),
        "C test executable linking the dev path dep should run: {stdout}"
    );
}

#[test]
fn dev_activation_does_not_propagate_to_transitive_path_deps() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // `lib` declares a dev path dep whose directory does not
    // exist.  Because activation is scoped to the *selected*
    // packages, testing `app` must not try to materialize it.
    assert_fs::fixture::ChildPath::new(dir.path().join("lib/cabin.toml"))
        .write_str(
            r#"[package]
name = "lib"
version = "0.1.0"
cxx-standard = "c++17"

[dev-dependencies]
ghost = { path = "../ghost-does-not-exist" }

[target.lib]
type = "library"
sources = ["src/lib.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("lib/include/lib.h"))
        .write_str("#pragma once\nint lib_answer();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("lib/src/lib.cc"))
        .write_str("#include \"lib.h\"\nint lib_answer() { return 3; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
lib = { path = "../lib" }

[target.app_test]
type = "test"
sources = ["tests/app_test.cc"]
deps = ["lib"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/tests/app_test.cc"))
        .write_str("#include \"lib.h\"\nint main() { return lib_answer() == 3 ? 0 : 1; }\n")
        .unwrap();
    cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
}

#[test]
fn build_diagnoses_ordinary_target_referencing_dev_dependency() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let manifest = write_app_with_dev_path_harness(dir.path());
    // Rewire the *library* target to reference the dev dep: the
    // planner must name the dev-dependency policy instead of a
    // generic unknown-target error.
    let body = fs::read_to_string(&manifest).unwrap();
    assert_fs::fixture::ChildPath::new(&manifest)
        .write_str(&body.replace(
            "sources = [\"src/applib.cc\"]",
            "sources = [\"src/applib.cc\"]\ndeps = [\"harness\"]",
        ))
        .unwrap();
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("[dev-dependencies]") && stderr.contains("harness"),
        "expected the dev-dependency diagnostic: {stderr}"
    );
}

#[test]
fn test_resolves_versioned_dev_dependency_and_locks_it() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // Registry package `devcheck` is declared only under
    // `[dev-dependencies]`; `cabin test` must resolve, fetch,
    // build, and link it - and record it in the lockfile.
    let archive = dir.path().join("artifacts/devcheck-1.2.0.tar.gz");
    let hex = make_archive(
        &archive,
        &[
            (
                "cabin.toml",
                r#"[package]
name = "devcheck"
version = "1.2.0"
cxx-standard = "c++17"

[target.devcheck]
type = "library"
sources = ["src/devcheck.cc"]
include-dirs = ["include"]
"#,
            ),
            (
                "include/devcheck.h",
                "#pragma once\nint devcheck_answer();\n",
            ),
            (
                "src/devcheck.cc",
                "#include \"devcheck.h\"\nint devcheck_answer() { return 5; }\n",
            ),
        ],
    );
    write_index_entry(
        &dir.path().join("index"),
        "devcheck",
        "1.2.0",
        "{}",
        &hex,
        "../artifacts/devcheck-1.2.0.tar.gz",
    );
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dev-dependencies]
devcheck = ">=1 <2"

[target.applib]
type = "library"
sources = ["src/applib.cc"]

[target.app_test]
type = "test"
sources = ["tests/app_test.cc"]
deps = ["devcheck"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/applib.cc"))
        .write_str("int applib_answer() { return 5; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/tests/app_test.cc"))
        .write_str(
            "#include \"devcheck.h\"\nint main() { return devcheck_answer() == 5 ? 0 : 1; }\n",
        )
        .unwrap();

    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.path().join("index"))
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("test app:app_test ... ok"),
        "test executable linking the versioned dev dep should run: {stdout}"
    );

    // The activated dev dep is a real resolution input, so the
    // lockfile records it.
    let lock = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
    assert!(
        lock.contains(r#"name = "devcheck""#),
        "activated dev dep should be locked: {lock}"
    );

    // The same tree under `cabin build` ignores the dev dep
    // entirely - no index needed, nothing of `devcheck` in the
    // ordinary build graph.
    let build_dir = dir.path().join("build-ordinary");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();
    let ninja = fs::read_to_string(build_dir.join("dev/build.ninja")).unwrap();
    assert!(
        !ninja.contains("devcheck"),
        "versioned dev dep must stay out of the ordinary build graph: {ninja}"
    );
}

#[test]
fn test_links_dev_port_path_dependency_into_test_target() {
    require_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    let repo = FakePortRepo::new(tmp.path());
    let checkkit = repo
        .port("checkkit", "1.0.0")
        .archive_prefix("checkkit-1.0.0")
        .file(
            "include/checkkit.h",
            "#pragma once\nint checkkit_answer();\n",
        )
        .file(
            "src/checkkit.cc",
            "#include \"checkkit.h\"\nint checkkit_answer() { return 9; }\n",
        )
        .overlay_manifest(
            r#"[package]
name = "checkkit"
version = "1.0.0"
cxx-standard = "c++17"

[target.checkkit]
type = "library"
sources = ["src/checkkit.cc"]
include-dirs = ["include"]
"#,
        )
        .build();
    let server = FakeArchiveServer::new().serve(&checkkit.archive).start();

    assert_fs::fixture::ChildPath::new(tmp.path().join("consumer/cabin.toml"))
        .write_str(
            r#"[package]
name = "consumer"
version = "0.1.0"
cxx-standard = "c++17"

[dev-dependencies]
checkkit = { port-path = "../ports/checkkit/1.0.0" }

[target.consumerlib]
type = "library"
sources = ["src/lib.cc"]

[target.consumer_test]
type = "test"
sources = ["tests/consumer_test.cc"]
deps = ["checkkit"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(tmp.path().join("consumer/src/lib.cc"))
        .write_str("int consumer_lib() { return 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(tmp.path().join("consumer/tests/consumer_test.cc"))
        .write_str(
            "#include \"checkkit.h\"\nint main() { return checkkit_answer() == 9 ? 0 : 1; }\n",
        )
        .unwrap();

    // Ordinary build first: the dev port is declaration-only, so
    // nothing is downloaded.
    cabin()
        .args(["build", "--manifest-path"])
        .arg(tmp.path().join("consumer/cabin.toml"))
        .arg("--build-dir")
        .arg(tmp.path().join("build-ordinary"))
        .arg("--cache-dir")
        .arg(tmp.path().join("cache"))
        .assert()
        .success();
    assert_eq!(
        server.total_requests(),
        0,
        "cabin build must not fetch dev port archives"
    );

    // `cabin test` activates the dev port: download, build, link,
    // run.
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(tmp.path().join("consumer/cabin.toml"))
        .arg("--build-dir")
        .arg(tmp.path().join("build"))
        .arg("--cache-dir")
        .arg(tmp.path().join("cache"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("test consumer:consumer_test ... ok"),
        "test executable linking the dev port should run: {stdout}"
    );
    assert_eq!(server.requests_for(checkkit.archive.name()), 1);
}
