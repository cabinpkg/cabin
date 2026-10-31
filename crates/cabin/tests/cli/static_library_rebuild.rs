//! Incremental static-library rebuilds against real tools.
//!
//! `ar crs` adds and replaces members but never drops one, so an archive
//! updated in place keeps the object of a source removed or renamed since
//! the last build.  These tests change a library's sources between builds
//! without cleaning and check the link and program behavior a clean build
//! of the final state would have.

use super::*;

fn write(dir: &Path, path: &str, body: &str) {
    assert_fs::fixture::ChildPath::new(dir.join(path))
        .write_str(body)
        .unwrap();
}

fn cabin_in(dir: &Path, build_dir: &str, args: &[&str]) -> assert_cmd::assert::Assert {
    cabin()
        .args(args)
        .arg("--manifest-path")
        .arg(dir.join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.join(build_dir))
        .assert()
}

fn stale_manifest(sources: &str) -> String {
    format!(
        r#"[package]
name = "stale"
version = "0.1.0"
c-standard = "c11"

[target.stale]
type = "library"
sources = [{sources}]

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["stale"]
"#
    )
}

#[test]
fn removing_a_library_source_drops_its_member() {
    require_c_and_cxx_build_tools();
    let tmp = TempDir::new().unwrap();
    // Ninja shell-quotes `$out` into the archive runner's argv, so a path
    // with shell metacharacters must still name the archive to remove.
    let dir = &tmp.path().join("a b&c$d(e)");
    write(
        dir,
        "cabin.toml",
        &stale_manifest(r#""src/keep.c", "src/gone.c""#),
    );
    write(dir, "src/keep.c", "int keep_fn(void) { return 1; }\n");
    write(dir, "src/gone.c", "int gone_fn(void) { return 2; }\n");
    write(
        dir,
        "src/main.c",
        "int keep_fn(void);\nint gone_fn(void);\nint main(void) { return keep_fn() + gone_fn() == 3 ? 0 : 1; }\n",
    );
    cabin_in(dir, "build", &["build"]).success();

    // `main.c` still calls `gone_fn`, which only the dropped source
    // defined: the rebuilt library must not supply it, matching a clean
    // build of the same state.
    write(dir, "cabin.toml", &stale_manifest(r#""src/keep.c""#));
    for build_dir in ["build", "build-clean"] {
        cabin_in(dir, build_dir, &["build"])
            .failure()
            .stdout(predicate::str::contains("gone_fn"));
    }
}

#[test]
fn renaming_a_library_source_replaces_its_member() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let dir = dir.path();
    let manifest = |source: &str| {
        format!(
            r#"[package]
name = "renamed"
version = "0.1.0"
cxx-standard = "c++17"

[target.ver]
type = "library"
sources = ["src/keep.cc", "src/{source}"]

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["ver"]
"#
        )
    };
    write(dir, "cabin.toml", &manifest("v1.cc"));
    write(dir, "src/keep.cc", "int keep_fn() { return 40; }\n");
    write(dir, "src/v1.cc", "int version() { return 1; }\n");
    write(
        dir,
        "src/main.cc",
        "#include <cstdio>\nint keep_fn();\nint version();\nint main() { std::printf(\"value=%d\\n\", keep_fn() + version()); }\n",
    );
    cabin_in(dir, "build", &["run"])
        .success()
        .stdout(predicate::str::contains("value=41"));

    // The renamed source defines the same symbol: a library still
    // holding `v1.cc`'s object would let the linker pick the old one.
    fs::remove_file(dir.join("src/v1.cc")).unwrap();
    write(dir, "src/v2.cc", "int version() { return 2; }\n");
    write(dir, "cabin.toml", &manifest("v2.cc"));
    cabin_in(dir, "build", &["run"])
        .success()
        .stdout(predicate::str::contains("value=42"));

    // With the same `cabin` binary, an unchanged build reruns neither the
    // archive nor the link that consumes it.
    cabin_in(dir, "build", &["-v", "build"])
        .success()
        .stdout(predicate::str::contains("ninja: no work to do."));
}
