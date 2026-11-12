use super::standard_compat::flat_contains;
use super::system_deps_pkg_config::{Fixtures, cabin_with_fake_pkg_config};
use super::*;

const POLICY: &str = "\n[system-provided.\"acme/zlib\"]\npkg-config = \"zsys\"\n";

/// Write `files` under `<root>/<dir>` and publish the package there into
/// `<root>/registry`.
pub(super) fn publish(root: &Path, dir: &str, files: &[(&str, &str)]) {
    for (path, body) in files {
        assert_fs::fixture::ChildPath::new(root.join(dir).join(path))
            .write_str(body)
            .unwrap();
    }
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(root.join(dir).join("cabin.toml"))
        .arg("--registry-dir")
        .arg(root.join("registry"))
        .assert()
        .success();
}

/// Publish `acme/zlib 1.3.0` (library `z`, whose `z.h` defines
/// `Z_ORIGIN` as `"source"`) and `acme/png 1.0.0` (library `png` over
/// zlib, reporting the `Z_ORIGIN` it compiled against) into
/// `<root>/registry`.
fn publish_zlib_and_png(root: &Path) {
    publish(
        root,
        "src-zlib",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/zlib"
version = "1.3.0"
license = "MIT"
c-standard = "c11"

[target.z]
type = "library"
sources = ["src/z.c"]
include-dirs = ["include"]
"#,
            ),
            (
                "include/z.h",
                r#"#ifndef Z_H
#define Z_H
#define Z_ORIGIN "source"
#ifdef __cplusplus
extern "C" {
#endif
int z_answer(void);
#ifdef __cplusplus
}
#endif
#endif
"#,
            ),
            (
                "src/z.c",
                "#include \"z.h\"\nint z_answer(void) { return 42; }\n",
            ),
        ],
    );
    publish(
        root,
        "src-png",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/png"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[dependencies]
"acme/zlib" = ">=1.2"

[target.png]
type = "library"
sources = ["src/png.c"]
include-dirs = ["include"]
deps = ["acme/zlib"]
"#,
            ),
            (
                "include/png.h",
                r#"#ifdef __cplusplus
extern "C"
#endif
const char *png_origin(void);
"#,
            ),
            (
                "src/png.c",
                r#"#include "png.h"
#include "z.h"
const char *png_origin(void) { return Z_ORIGIN; }
"#,
            ),
        ],
    );
}

/// Write the root manifest `<root>/app/cabin.toml` with `deps` as its
/// `[dependencies]` body and `policy` appended.
fn write_app(root: &Path, deps: &str, policy: &str) -> PathBuf {
    let manifest = root.join("app/cabin.toml");
    assert_fs::fixture::ChildPath::new(&manifest)
        .write_str(&format!(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
{deps}
{policy}"#
        ))
        .unwrap();
    manifest
}

/// Move `<root>/registry/archives/<package>` aside, so a command that
/// fetches the package fails.
fn hide_archives(root: &Path, package: &str) {
    fs::rename(
        root.join("registry/archives").join(package),
        root.join(format!("hidden-{}", package.replace('/', "-"))),
    )
    .unwrap();
}

fn fetch(root: &Path, manifest: &Path) -> assert_cmd::assert::Assert {
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(manifest)
        .arg("--index-path")
        .arg(root.join("registry"))
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .assert()
}

/// One consumer builds against the source zlib or the system's, without
/// a source or lockfile change.  The graph is a diamond (`app -> png ->
/// zlib`, `app -> zlib`) whose consumers are a C library, a C++
/// executable, and the registry package `png`: the system header
/// defines `Z_ORIGIN` only under the module's `-DZ_SYSTEM`, so every
/// compile must take the module's interface.  The system build runs with
/// zlib's archive gone, proving it is never fetched.
#[test]
fn one_consumer_builds_from_source_or_from_the_system() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    let system_include = root.join("sys/include");
    assert_fs::fixture::ChildPath::new(system_include.join("z.h"))
        .write_str(
            r#"#ifndef Z_H
#define Z_H
#ifdef Z_SYSTEM
#define Z_ORIGIN "system"
#endif
static inline int z_answer(void) { return 42; }
#endif
"#,
        )
        .unwrap();
    let fixtures = Fixtures::new();
    // The fake pkg-config reads one key per line.
    fixtures.write(
        "zsys",
        &serde_json::to_string_pretty(&serde_json::json!({
            "version": "1.3.1",
            "cflags": format!("-I{} -DZ_SYSTEM", system_include.display()),
            "libs": "-lm",
        }))
        .unwrap(),
    );
    let deps = r#""acme/zlib" = "^1.3"
"acme/png" = "^1.0"

[target.glue]
type = "library"
sources = ["src/glue.c"]
c-standard = "c11"
deps = ["acme/zlib"]

[target.app]
type = "executable"
sources = ["src/main.cc"]
cxx-standard = "c++17"
deps = ["glue", "acme/png", "acme/zlib"]
"#;
    assert_fs::fixture::ChildPath::new(root.join("app/src/glue.c"))
        .write_str("#include \"z.h\"\nconst char *glue_origin(void) { return Z_ORIGIN; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.cc"))
        .write_str(
            r#"#include <cstdio>
#include "png.h"
#include "z.h"
extern "C" const char *glue_origin(void);
int main() { std::printf("%s %s %s %d\n", Z_ORIGIN, glue_origin(), png_origin(), z_answer()); return 0; }
"#,
        )
        .unwrap();
    let build = |manifest: &Path| {
        let mut cmd = cabin_with_fake_pkg_config(&fixtures);
        cmd.args(["build", "--manifest-path"])
            .arg(manifest)
            .arg("--index-path")
            .arg(root.join("registry"))
            .arg("--cache-dir")
            .arg(root.join("cache"))
            .arg("--build-dir")
            .arg(root.join("build"));
        cmd.assert()
    };
    let run = || {
        let output =
            std::process::Command::new(root.join("build/dev/packages/app").join(host_exe("app")))
                .output()
                .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    hide_archives(root, "acme/zlib");
    let manifest = write_app(root, deps, POLICY);
    if cfg!(windows) {
        // The default Windows toolchain is MSVC, which cannot take
        // pkg-config's flags.
        let stderr = build(&manifest).failure().get_output().stderr.clone();
        let stderr = String::from_utf8_lossy(&stderr);
        assert!(
            flat_contains(
                &stderr,
                "whose GNU-style flags the MSVC backend cannot consume"
            ),
            "{stderr}"
        );
        return;
    }
    build(&manifest).success();
    assert_eq!(run(), "system system system 42\n");
    let lockfile = fs::read_to_string(root.join("app/cabin.lock")).unwrap();

    fs::rename(
        root.join("hidden-acme-zlib"),
        root.join("registry/archives/acme/zlib"),
    )
    .unwrap();
    write_app(root, deps, "");
    build(&manifest).success();
    assert_eq!(run(), "source source source 42\n");
    assert_eq!(
        fs::read_to_string(root.join("app/cabin.lock")).unwrap(),
        lockfile
    );
}

/// The module is probed against the requirements of the active edges
/// onto the package, and a missing module or a version outside them
/// fails the build: it never falls back to building the source.  The
/// root's `*` and `acme/extra`'s disabled optional `^9` edge add no
/// requirement.
#[test]
fn an_absent_or_mismatched_module_fails_the_build() {
    require_c_and_cxx_build_tools();
    if cfg!(windows) {
        return;
    }
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    publish(
        root,
        "src-extra",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/extra"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[dependencies]
"acme/zlib" = { version = "^9", optional = true }

[features]
z = ["dep:acme/zlib"]

[target.extra]
type = "library"
sources = ["src/extra.c"]
"#,
            ),
            ("src/extra.c", "int extra(void) { return 0; }\n"),
        ],
    );
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = write_app(
        root,
        r#""acme/zlib" = "*"
"acme/png" = "^1.0"
"acme/extra" = "^1.0"

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
deps = ["acme/png"]
"#,
        POLICY,
    );
    let fixtures = Fixtures::new();
    let build = || {
        let mut cmd = cabin_with_fake_pkg_config(&fixtures);
        cmd.args(["build", "--manifest-path"])
            .arg(&manifest)
            .arg("--index-path")
            .arg(root.join("registry"))
            .arg("--cache-dir")
            .arg(root.join("cache"))
            .arg("--build-dir")
            .arg(root.join("build"));
        let stderr = cmd.assert().failure().get_output().stderr.clone();
        String::from_utf8_lossy(&stderr).into_owned()
    };

    let stderr = build();
    assert!(
        flat_contains(&stderr, "\"zsys\" was not found by pkg-config"),
        "{stderr}"
    );

    fixtures.write(
        "zsys",
        r#"{
"version": "1.1.0",
"cflags": "",
"libs": ""
}
"#,
    );
    let stderr = build();
    assert!(
        flat_contains(
            &stderr,
            "\"zsys\" does not satisfy version requirement \">=1.2\""
        ),
        "{stderr}"
    );
}

/// The build context keeps building its packages from source for now.
#[test]
fn a_build_dependency_cannot_reach_a_provided_package() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = write_app(
        root,
        r#"
[build-dependencies]
"acme/zlib" = "^1.3"

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        POLICY,
    );
    let stderr = cabin_with_fake_pkg_config(&Fixtures::new())
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(root.join("registry"))
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .arg("--build-dir")
        .arg(root.join("build"))
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        flat_contains(&stderr, "but a build dependency reaches it"),
        "{stderr}"
    );
}

/// `cabin fetch` follows the build: a provided package, and what only
/// it reaches, is not fetched.
#[test]
fn fetch_skips_a_provided_package_and_what_only_it_reaches() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    hide_archives(root, "acme/zlib");
    hide_archives(root, "acme/png");
    let manifest = write_app(
        root,
        "\"acme/png\" = \"^1.0\"\n",
        "\n[system-provided.\"acme/png\"]\npkg-config = \"libpng\"\n",
    );
    fetch(root, &manifest).success();
}

/// `cabin vendor` ignores the table, so the vendored tree also builds
/// where the system lacks the library.
#[test]
fn vendor_keeps_provided_packages() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    let manifest = write_app(root, "\"acme/png\" = \"^1.0\"\n", POLICY);
    cabin()
        .args(["vendor", "--manifest-path"])
        .arg(&manifest)
        .arg("--vendor-dir")
        .arg(root.join("vendor"))
        .arg("--index-path")
        .arg(root.join("registry"))
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .assert()
        .success();
    assert!(root.join("vendor/packages/acme/zlib.json").is_file());
}

/// A package a provided package reaches, here through `png`, cannot also
/// be built from source: the system library links its own copy.  An edge
/// onto it that is inactive on this platform builds nothing, so it does
/// not count, and neither does a build dependency, a tool the system
/// library does not link, beside an optional dependency left off.
#[test]
fn a_provided_package_cannot_reach_a_package_built_from_source() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    publish(
        root,
        "src-imaging",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/imaging"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[dependencies]
"acme/png" = "^1.0"

[target.imaging]
type = "library"
sources = ["src/imaging.c"]
"#,
            ),
            ("src/imaging.c", "int imaging(void) { return 0; }\n"),
        ],
    );
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let fixtures = Fixtures::new();
    fixtures.write(
        "imaging",
        r#"{
"version": "1.0.0",
"cflags": "",
"libs": ""
}
"#,
    );
    let target = r#"
[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#;
    let policy = "\n[system-provided.\"acme/imaging\"]\npkg-config = \"imaging\"\n";
    let build = |manifest: &Path| {
        let mut cmd = cabin_with_fake_pkg_config(&fixtures);
        cmd.args(["build", "--manifest-path"])
            .arg(manifest)
            .arg("--index-path")
            .arg(root.join("registry"))
            .arg("--cache-dir")
            .arg(root.join("cache"))
            .arg("--build-dir")
            .arg(root.join("build"));
        cmd.assert()
    };

    let manifest = write_app(
        root,
        &format!(
            r#""acme/imaging" = "^1.0"
"acme/zlib" = "^1.3"
{target}"#
        ),
        policy,
    );
    let stderr = build(&manifest).failure().get_output().stderr.clone();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        flat_contains(&stderr, "provide `acme/zlib` from the system too"),
        "{stderr}"
    );

    // The default Windows toolchain is MSVC, which refuses the provider.
    if cfg!(windows) {
        return;
    }
    write_app(
        root,
        &format!(
            r#""acme/imaging" = "^1.0"

[target.'cfg(os = "none-such")'.dependencies]
"acme/zlib" = "^1.3"
{target}"#
        ),
        policy,
    );
    build(&manifest).success();

    publish(
        root,
        "src-tooled",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/tooled"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[dependencies]
"acme/zlib" = { version = "^1.3", optional = true }

[build-dependencies]
"acme/zlib" = "^1.3"

[target.tooled]
type = "library"
sources = ["src/tooled.c"]
"#,
            ),
            ("src/tooled.c", "int tooled(void) { return 0; }\n"),
        ],
    );
    fixtures.write(
        "tooled",
        r#"{
"version": "1.0.0",
"cflags": "",
"libs": ""
}
"#,
    );
    write_app(
        root,
        &format!(
            r#""acme/tooled" = "^1.0"
"acme/zlib" = "^1.3"
{target}"#
        ),
        "\n[system-provided.\"acme/tooled\"]\npkg-config = \"tooled\"\n",
    );
    build(&manifest).success();
}

/// Refused even when the patch leaves nothing to resolve.
#[test]
fn a_package_cannot_be_both_patched_and_provided() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    let manifest = write_app(
        root,
        "\"acme/zlib\" = \"^1.3\"\n",
        &format!(
            r#"
[patch]
"acme/zlib" = {{ path = "../src-zlib" }}
{POLICY}"#
        ),
    );
    let stderr = fetch(root, &manifest).failure().get_output().stderr.clone();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        flat_contains(
            &stderr,
            "`acme/zlib` is both `[patch]`ed and `[system-provided]`"
        ),
        "{stderr}"
    );
}

/// A fork that only a provided package reaches is as dormant as an
/// unused patch: its own registry dependency is neither fetched nor
/// required.
#[test]
fn a_patch_only_a_provided_package_reaches_is_dormant() {
    require_c_and_cxx_build_tools();
    // The default Windows toolchain is MSVC, which refuses the provider.
    if cfg!(windows) {
        return;
    }
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    publish(
        root,
        "src-extra",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/extra"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[target.extra]
type = "library"
sources = ["src/extra.c"]
"#,
            ),
            ("src/extra.c", "int extra(void) { return 0; }\n"),
        ],
    );
    hide_archives(root, "acme/extra");
    for (path, body) in [
        (
            "zlib-fork/cabin.toml",
            r#"[package]
name = "acme/zlib"
version = "1.3.0"
license = "MIT"
c-standard = "c11"

[dependencies]
"acme/extra" = "^1.0"

[target.z]
type = "library"
sources = ["src/z.c"]
"#,
        ),
        ("zlib-fork/src/z.c", "int z_answer(void) { return 42; }\n"),
        ("app/src/main.c", "int main(void) { return 0; }\n"),
    ] {
        assert_fs::fixture::ChildPath::new(root.join(path))
            .write_str(body)
            .unwrap();
    }
    let manifest = write_app(
        root,
        r#""acme/png" = "^1.0"

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        r#"
[patch]
"acme/zlib" = { path = "../zlib-fork" }

[system-provided."acme/png"]
pkg-config = "pngsys"
"#,
    );
    let fixtures = Fixtures::new();
    fixtures.write(
        "pngsys",
        r#"{
"version": "1.0.0",
"cflags": "",
"libs": ""
}
"#,
    );
    cabin_with_fake_pkg_config(&fixtures)
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(root.join("registry"))
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .arg("--build-dir")
        .arg(root.join("build"))
        .assert()
        .success();
}

/// Only a registry package comes from the system; naming a path
/// dependency fails the build rather than building it from source.
#[test]
fn a_local_package_cannot_be_provided() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for (path, body) in [
        (
            "zlib/cabin.toml",
            r#"[package]
name = "acme/zlib"
version = "1.3.0"
license = "MIT"
c-standard = "c11"

[target.z]
type = "library"
sources = ["src/z.c"]
"#,
        ),
        ("zlib/src/z.c", "int z_answer(void) { return 42; }\n"),
        ("app/src/main.c", "int main(void) { return 0; }\n"),
    ] {
        assert_fs::fixture::ChildPath::new(root.join(path))
            .write_str(body)
            .unwrap();
    }
    let manifest = write_app(
        root,
        r#""acme/zlib" = { path = "../zlib" }

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
"#,
        POLICY,
    );
    let stderr = cabin_with_fake_pkg_config(&Fixtures::new())
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(root.join("build"))
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        flat_contains(
            &stderr,
            "`acme/zlib` is `[system-provided]`, but is a local package"
        ),
        "{stderr}"
    );
}

/// A package with several library targets takes one module per target:
/// every target must be named, every module meets the edges'
/// requirement, and each target takes its own module's interface.
#[test]
fn each_library_target_comes_from_its_own_module() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish(
        root,
        "src-tls",
        &[
            (
                "cabin.toml",
                r#"[package]
name = "acme/tls"
version = "1.0.0"
license = "MIT"
c-standard = "c11"

[target.ssl]
type = "library"
sources = ["src/ssl.c"]
include-dirs = ["include"]

[target.crypto]
type = "library"
sources = ["src/crypto.c"]
include-dirs = ["include"]
"#,
            ),
            ("include/tls.h", "#define TLS_ORIGIN \"source\"\n"),
            ("src/ssl.c", "int ssl(void) { return 0; }\n"),
            ("src/crypto.c", "int crypto(void) { return 0; }\n"),
        ],
    );
    hide_archives(root, "acme/tls");
    let system_include = root.join("sys/include");
    assert_fs::fixture::ChildPath::new(system_include.join("tls.h"))
        .write_str(
            r#"#if defined(SSL_SYSTEM) && defined(CRYPTO_SYSTEM)
#define TLS_ORIGIN "system"
#endif
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.c"))
        .write_str(
            r#"#include <stdio.h>
#include "tls.h"
int main(void) { printf("%s\n", TLS_ORIGIN); return 0; }
"#,
        )
        .unwrap();
    let fixtures = Fixtures::new();
    // The fake pkg-config reads one key per line.
    let module = |name: &str, version: &str, cflags: String| {
        fixtures.write(
            name,
            &serde_json::to_string_pretty(&serde_json::json!({
                "version": version,
                "cflags": cflags,
                "libs": "",
            }))
            .unwrap(),
        );
    };
    module(
        "libssl",
        "1.0.0",
        format!("-I{} -DSSL_SYSTEM", system_include.display()),
    );
    module("libcrypto", "0.9.0", "-DCRYPTO_SYSTEM".to_owned());
    let deps = r#""acme/tls" = "^1.0"

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
deps = ["acme/tls:ssl", "acme/tls:crypto"]
"#;
    let build = || {
        let mut cmd = cabin_with_fake_pkg_config(&fixtures);
        cmd.args(["build", "--manifest-path"])
            .arg(root.join("app/cabin.toml"))
            .arg("--index-path")
            .arg(root.join("registry"))
            .arg("--cache-dir")
            .arg(root.join("cache"))
            .arg("--build-dir")
            .arg(root.join("build"));
        cmd.assert()
    };
    let failure = || {
        let stderr = build().failure().get_output().stderr.clone();
        String::from_utf8_lossy(&stderr).into_owned()
    };

    write_app(
        root,
        deps,
        "\n[system-provided.\"acme/tls\"]\ntargets.ssl = { pkg-config = \"libssl\" }\n",
    );
    let stderr = failure();
    assert!(
        flat_contains(&stderr, "names targets {\"ssl\"} for `acme/tls 1.0.0`"),
        "{stderr}"
    );

    // The default Windows toolchain is MSVC, which refuses the provider.
    if cfg!(windows) {
        return;
    }
    write_app(
        root,
        deps,
        r#"
[system-provided."acme/tls"]
targets.ssl = { pkg-config = "libssl" }
targets.crypto = { pkg-config = "libcrypto" }
"#,
    );
    let stderr = failure();
    assert!(
        flat_contains(
            &stderr,
            "\"libcrypto\" does not satisfy version requirement \"^1.0\""
        ),
        "{stderr}"
    );

    module("libcrypto", "1.0.2", "-DCRYPTO_SYSTEM".to_owned());
    build().success();
    let output =
        std::process::Command::new(root.join("build/dev/packages/app").join(host_exe("app")))
            .output()
            .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "system\n");
}

#[test]
fn metadata_reports_the_declared_policy() {
    let dir = TempDir::new().unwrap();
    let manifest = write_app(
        dir.path(),
        "",
        &format!(
            r#"{POLICY}
[system-provided."acme/tls"]
targets.ssl = {{ pkg-config = "libssl" }}
"#
        ),
    );
    let value = run_json(
        cabin()
            .args(["metadata", "--format", "json", "--manifest-path"])
            .arg(&manifest),
    );
    assert_eq!(
        value["system_provided"],
        serde_json::json!({
            "acme/tls": { "targets": { "ssl": { "pkg-config": "libssl" } } },
            "acme/zlib": { "pkg-config": "zsys" },
        })
    );
}

/// Which packages come from the system is the root's choice: a member
/// cannot declare the table, and a published package cannot carry it.
#[test]
fn only_the_workspace_root_declares_the_policy() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str("[workspace]\nmembers = [\"member\"]\n")
        .unwrap();
    dir.child("member/cabin.toml")
        .write_str(&format!(
            r#"[package]
name = "member"
version = "0.1.0"
{POLICY}"#
        ))
        .unwrap();
    let stderr = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        flat_contains(
            &stderr,
            "declares a `[system-provided]` table, but it may only appear in the workspace root manifest"
        ),
        "{stderr}"
    );

    let package = dir.path().join("pkg");
    assert_fs::fixture::ChildPath::new(package.join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "acme/lib"
version = "0.1.0"
license = "MIT"
c-standard = "c11"

[target.lib]
type = "library"
sources = ["src/lib.c"]
{POLICY}"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(package.join("src/lib.c"))
        .write_str("int lib(void) { return 0; }\n")
        .unwrap();
    let stderr = cabin()
        .args(["package", "--manifest-path"])
        .arg(package.join("cabin.toml"))
        .arg("--output-dir")
        .arg(dir.path().join("dist"))
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        flat_contains(&stderr, "declares a `[system-provided]` table"),
        "{stderr}"
    );
}

/// A project's own config file provides a package without `[trust]`, as
/// its root manifest could: the build probes the config's module, and
/// zlib's archive is gone, so it is never fetched.
#[test]
fn an_untrusted_projects_config_file_provides_a_package() {
    require_c_and_cxx_build_tools();
    if cfg!(windows) {
        return;
    }
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    hide_archives(root, "acme/zlib");
    let system_include = root.join("sys/include");
    assert_fs::fixture::ChildPath::new(system_include.join("z.h"))
        .write_str(
            r#"#ifndef Z_H
#define Z_H
#ifdef Z_SYSTEM
#define Z_ORIGIN "system"
#endif
static inline int z_answer(void) { return 42; }
#endif
"#,
        )
        .unwrap();
    let fixtures = Fixtures::new();
    fixtures.write(
        "zsys",
        &serde_json::to_string_pretty(&serde_json::json!({
            "version": "1.3.1",
            "cflags": format!("-I{} -DZ_SYSTEM", system_include.display()),
            "libs": "",
        }))
        .unwrap(),
    );
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.c"))
        .write_str(
            r#"#include <stdio.h>
#include "z.h"
int main(void) { printf("%s %d\n", Z_ORIGIN, z_answer()); return 0; }
"#,
        )
        .unwrap();
    let manifest = write_app(
        root,
        r#""acme/zlib" = "^1.3"

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
deps = ["acme/zlib"]
"#,
        "",
    );
    assert_fs::fixture::ChildPath::new(root.join("app/.cabin/config.toml"))
        .write_str(POLICY)
        .unwrap();
    let mut cmd = cabin_with_fake_pkg_config(&fixtures);
    cmd.env_remove("CABIN_NO_CONFIG")
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(root.join("registry"))
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .arg("--build-dir")
        .arg(root.join("build"))
        .assert()
        .success();
    let output =
        std::process::Command::new(root.join("build/dev/packages/app").join(host_exe("app")))
            .output()
            .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "system 42\n");
}

/// For each package, the project's config file outranks the user's,
/// which outranks the root manifest; `cabin metadata` reports the
/// effective policy and, under `config`, where each config entry came
/// from.
#[test]
fn config_file_entries_replace_lower_layers_per_package() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let manifest = write_app(
        root,
        "",
        &format!(
            r#"{POLICY}
[system-provided."acme/tls"]
targets.ssl = {{ pkg-config = "libssl" }}

[system-provided."acme/png"]
pkg-config = "png-manifest"
"#
        ),
    );
    let user = root.join("user");
    assert_fs::fixture::ChildPath::new(user.join("config.toml"))
        .write_str(
            r#"[system-provided."acme/zlib"]
pkg-config = "z-user"

[system-provided."acme/png"]
pkg-config = "png-user"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/.cabin/config.toml"))
        .write_str(
            r#"[system-provided."acme/zlib"]
pkg-config = "z-project"

[system-provided."acme/tls"]
targets.ssl = { pkg-config = "ssl3" }
targets.crypto = { pkg-config = "crypto3" }
"#,
        )
        .unwrap();
    let value = run_json(
        cabin_with_config()
            .env("CABIN_CONFIG_HOME", &user)
            .args(["metadata", "--format", "json", "--manifest-path"])
            .arg(&manifest),
    );
    let tls = serde_json::json!({
        "targets": { "crypto": { "pkg-config": "crypto3" }, "ssl": { "pkg-config": "ssl3" } },
    });
    assert_eq!(
        value["system_provided"],
        serde_json::json!({
            "acme/png": { "pkg-config": "png-user" },
            "acme/tls": tls,
            "acme/zlib": { "pkg-config": "z-project" },
        })
    );
    assert_eq!(
        value["config"]["system_provided"],
        serde_json::json!({
            "acme/png": { "provider": { "pkg-config": "png-user" }, "value_source": "user-config" },
            "acme/tls": { "provider": tls, "value_source": "package-config" },
            "acme/zlib": { "provider": { "pkg-config": "z-project" }, "value_source": "package-config" },
        })
    );
}

/// A config file's entry yields to the project's own copy of the
/// package, a `[patch]` fork or a path dependency, since a user config's
/// entry applies to every project; the root manifest's does not.
#[test]
fn a_config_file_entry_yields_to_a_local_copy() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    assert_fs::fixture::ChildPath::new(root.join("app/.cabin/config.toml"))
        .write_str(POLICY)
        .unwrap();
    let metadata = |manifest: &Path| {
        run_json(
            cabin_with_config()
                .args(["metadata", "--format", "json", "--manifest-path"])
                .arg(manifest),
        )
    };

    let patch = "\n[patch]\n\"acme/zlib\" = { path = \"../src-zlib\" }\n";
    let manifest = write_app(root, "\"acme/zlib\" = \"^1.3\"\n", patch);
    let value = metadata(&manifest);
    assert_eq!(value["system_provided"], serde_json::json!({}));
    assert_eq!(
        value["config"]["system_provided"]["acme/zlib"]["value_source"],
        "package-config"
    );
    write_app(
        root,
        "\"acme/zlib\" = \"^1.3\"\n",
        &format!("{patch}{POLICY}"),
    );
    let stderr = cabin_with_config()
        .args(["metadata", "--manifest-path"])
        .arg(&manifest)
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&stderr);
    assert!(
        flat_contains(
            &stderr,
            "`acme/zlib` is both `[patch]`ed and `[system-provided]`"
        ),
        "{stderr}"
    );

    write_app(root, "\"acme/zlib\" = { path = \"../src-zlib\" }\n", "");
    assert_eq!(
        metadata(&manifest)["system_provided"],
        serde_json::json!({})
    );
}

/// A dev path dependency, which only a `cabin test` loads, is the
/// project's own copy too.
#[test]
fn a_config_file_entry_yields_to_a_dev_path_dependency() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    publish_zlib_and_png(root);
    assert_fs::fixture::ChildPath::new(root.join("app/.cabin/config.toml"))
        .write_str(POLICY)
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/tests/z_test.c"))
        .write_str("#include \"z.h\"\nint main(void) { return z_answer() == 42 ? 0 : 1; }\n")
        .unwrap();
    let manifest = root.join("app/cabin.toml");
    assert_fs::fixture::ChildPath::new(&manifest)
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"

[dev-dependencies]
"acme/zlib" = { path = "../src-zlib" }

[target.z_test]
type = "test"
sources = ["tests/z_test.c"]
deps = ["acme/zlib"]
"#,
        )
        .unwrap();
    cabin_with_config()
        .args(["test", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(root.join("build"))
        .assert()
        .success();
}
