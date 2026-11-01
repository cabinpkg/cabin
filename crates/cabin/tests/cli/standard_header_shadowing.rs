use super::*;

/// A registry package whose include dir holds `stdint.h` and
/// `version`: each defines a marker instead of the real header, so a
/// consumer that sees the marker compiled against the package's file
/// instead of the standard one.
const SHIM_MANIFEST: &str = r#"[package]
name = "shim"
version = "1.0.0"
interface-c-standard = "c99"
interface-cxx-standard = "c++11"

[target.shim]
type = "header-only"
include-dirs = ["include"]
"#;

const APP_MANIFEST: &str = r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[dependencies]
shim = "^1"

[target.app]
type = "executable"
sources = ["src/main.c", "src/other.cpp"]
deps = ["shim"]
"#;

const ALLOW_SHIM: &str = "\n[standard-header-shadowing]\nallow = [\"shim\"]\n";

/// Publish `shim` (its manifest plus `shim_extra`) to a local index
/// and write an `app` consuming it (its manifest plus `app_extra`).
fn shim_and_app(dir: &Path, shim_extra: &str, app_extra: &str) {
    let shim_manifest = format!("{SHIM_MANIFEST}{shim_extra}");
    let hex = make_archive(
        &dir.join("index/artifacts/shim-1.0.0.zip"),
        &[
            ("cabin.toml", &shim_manifest),
            ("include/stdint.h", "#define SHIM_STDINT 1\n"),
            ("include/version", "#define SHIM_VERSION 1\n"),
        ],
    );
    write_index_entry(
        &dir.join("index"),
        "shim",
        "1.0.0",
        "{}",
        &hex,
        "artifacts/shim-1.0.0.zip",
    );
    let app = assert_fs::fixture::ChildPath::new(dir.join("app"));
    app.child("cabin.toml")
        .write_str(&format!("{APP_MANIFEST}{app_extra}"))
        .unwrap();
    app.child("src/main.c")
        .write_str(
            "#include <stdint.h>\n#ifndef SHIM_STDINT\n#error \"<stdint.h> is the standard one\"\n#endif\nint main(void) { return 0; }\n",
        )
        .unwrap();
    app.child("src/other.cpp")
        .write_str(
            "#include <version>\n#ifndef SHIM_VERSION\n#error \"<version> is the standard one\"\n#endif\n",
        )
        .unwrap();
}

fn build(dir: &Path) -> assert_cmd::assert::Assert {
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.join("app/cabin.toml"))
        .arg("--index-path")
        .arg(dir.join("index"))
        .arg("--cache-dir")
        .arg(dir.join("cache"))
        .arg("--build-dir")
        .arg(dir.join("build"))
        .assert()
}

/// The failing build's stderr with miette's line wrapping undone, so
/// assertions do not depend on where a long temp path breaks a line.
fn refusal(dir: &Path) -> String {
    let output = build(dir).failure().get_output().stderr.clone();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| {
            line.trim_start_matches([' ', '\u{2502}', '|', '\u{d7}'])
                .trim_end()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn registry_package_shadowing_standard_headers_is_refused() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    shim_and_app(dir.path(), "", "");
    let stderr = refusal(dir.path());
    for expected in [
        "registry package `shim` places `",
        "stdint.h` directly in include dir `include` of target `shim`, so `#include <stdint.h>`",
        "[standard-header-shadowing] allow = [\"shim\"]",
    ] {
        assert!(stderr.contains(expected), "{expected:?} not in {stderr:?}");
    }
}

#[test]
fn root_manifest_allow_lets_a_registry_package_replace_standard_headers() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // The sources `#error` unless both includes open the package's
    // files, so a successful build proves the shadowing took effect.
    shim_and_app(dir.path(), "", ALLOW_SHIM);
    build(dir.path()).success();
}

#[test]
fn registry_package_cannot_allow_its_own_shadowing() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // The table is read only from the root manifest, so the shim's
    // own allow is ignored and the consumer is still refused.
    shim_and_app(dir.path(), ALLOW_SHIM, "");
    let stderr = refusal(dir.path());
    assert!(
        stderr.contains("registry package `shim` places `"),
        "{stderr:?}"
    );
}
