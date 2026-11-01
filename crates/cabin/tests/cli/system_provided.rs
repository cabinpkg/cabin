use super::standard_compat::flat_contains;
use super::system_deps_pkg_config::{Fixtures, cabin_with_fake_pkg_config};
use super::*;

const POLICY: &str = "\n[system-provided.\"acme/zlib\"]\npkg-config = \"zsys\"\n";

/// Write `files` under `<root>/<dir>` and publish the package there into
/// `<root>/registry`.
fn publish(root: &Path, dir: &str, files: &[(&str, &str)]) {
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
                "[package]\nname = \"acme/zlib\"\nversion = \"1.3.0\"\nc-standard = \"c11\"\n\n\
                 [target.z]\ntype = \"library\"\nsources = [\"src/z.c\"]\ninclude-dirs = [\"include\"]\n",
            ),
            (
                "include/z.h",
                "#ifndef Z_H\n#define Z_H\n#define Z_ORIGIN \"source\"\n#ifdef __cplusplus\nextern \"C\" {\n#endif\n\
                 int z_answer(void);\n#ifdef __cplusplus\n}\n#endif\n#endif\n",
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
                "[package]\nname = \"acme/png\"\nversion = \"1.0.0\"\nc-standard = \"c11\"\n\n\
                 [dependencies]\n\"acme/zlib\" = \">=1.2\"\n\n\
                 [target.png]\ntype = \"library\"\nsources = [\"src/png.c\"]\ninclude-dirs = [\"include\"]\n\
                 deps = [\"acme/zlib\"]\n",
            ),
            (
                "include/png.h",
                "#ifdef __cplusplus\nextern \"C\"\n#endif\nconst char *png_origin(void);\n",
            ),
            (
                "src/png.c",
                "#include \"png.h\"\n#include \"z.h\"\nconst char *png_origin(void) { return Z_ORIGIN; }\n",
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
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n{deps}\n{policy}"
        ))
        .unwrap();
    manifest
}

/// Move `<root>/registry/artifacts/<package>` aside, so a command that
/// fetches the package fails.
fn hide_archives(root: &Path, package: &str) {
    fs::rename(
        root.join("registry/artifacts").join(package),
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
            "#ifndef Z_H\n#define Z_H\n#ifdef Z_SYSTEM\n#define Z_ORIGIN \"system\"\n#endif\n\
             static inline int z_answer(void) { return 42; }\n#endif\n",
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
    let deps = "\"acme/zlib\" = \"^1.3\"\n\"acme/png\" = \"^1.0\"\n\n\
                [target.glue]\ntype = \"library\"\nsources = [\"src/glue.c\"]\nc-standard = \"c11\"\n\
                deps = [\"acme/zlib\"]\n\n\
                [target.app]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\ncxx-standard = \"c++17\"\n\
                deps = [\"glue\", \"acme/png\", \"acme/zlib\"]\n";
    assert_fs::fixture::ChildPath::new(root.join("app/src/glue.c"))
        .write_str("#include \"z.h\"\nconst char *glue_origin(void) { return Z_ORIGIN; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.cc"))
        .write_str(
            "#include <cstdio>\n#include \"png.h\"\n#include \"z.h\"\n\
             extern \"C\" const char *glue_origin(void);\n\
             int main() { std::printf(\"%s %s %s %d\\n\", Z_ORIGIN, glue_origin(), png_origin(), \
             z_answer()); return 0; }\n",
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
        root.join("registry/artifacts/acme/zlib"),
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
                "[package]\nname = \"acme/extra\"\nversion = \"1.0.0\"\nc-standard = \"c11\"\n\n\
                 [dependencies]\n\"acme/zlib\" = { version = \"^9\", optional = true }\n\n\
                 [features]\nz = [\"dep:acme/zlib\"]\n\n\
                 [target.extra]\ntype = \"library\"\nsources = [\"src/extra.c\"]\n",
            ),
            ("src/extra.c", "int extra(void) { return 0; }\n"),
        ],
    );
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = write_app(
        root,
        "\"acme/zlib\" = \"*\"\n\"acme/png\" = \"^1.0\"\n\"acme/extra\" = \"^1.0\"\n\n\
         [target.app]\ntype = \"executable\"\nsources = [\"src/main.c\"]\nc-standard = \"c11\"\n\
         deps = [\"acme/png\"]\n",
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
        "{\n\"version\": \"1.1.0\",\n\"cflags\": \"\",\n\"libs\": \"\"\n}\n",
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
        "\n[build-dependencies]\n\"acme/zlib\" = \"^1.3\"\n\n\
         [target.app]\ntype = \"executable\"\nsources = [\"src/main.c\"]\nc-standard = \"c11\"\n",
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
/// not count.
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
                "[package]\nname = \"acme/imaging\"\nversion = \"1.0.0\"\nc-standard = \"c11\"\n\n\
                 [dependencies]\n\"acme/png\" = \"^1.0\"\n\n\
                 [target.imaging]\ntype = \"library\"\nsources = [\"src/imaging.c\"]\n",
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
        "{\n\"version\": \"1.0.0\",\n\"cflags\": \"\",\n\"libs\": \"\"\n}\n",
    );
    let target = "\n[target.app]\ntype = \"executable\"\nsources = [\"src/main.c\"]\n\
                  c-standard = \"c11\"\n";
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
        &format!("\"acme/imaging\" = \"^1.0\"\n\"acme/zlib\" = \"^1.3\"\n{target}"),
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
            "\"acme/imaging\" = \"^1.0\"\n\n\
             [target.'cfg(os = \"none-such\")'.dependencies]\n\"acme/zlib\" = \"^1.3\"\n{target}"
        ),
        policy,
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
        &format!("\n[patch]\n\"acme/zlib\" = {{ path = \"../src-zlib\" }}\n{POLICY}"),
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
                "[package]\nname = \"acme/extra\"\nversion = \"1.0.0\"\nc-standard = \"c11\"\n\n\
                 [target.extra]\ntype = \"library\"\nsources = [\"src/extra.c\"]\n",
            ),
            ("src/extra.c", "int extra(void) { return 0; }\n"),
        ],
    );
    hide_archives(root, "acme/extra");
    for (path, body) in [
        (
            "zlib-fork/cabin.toml",
            "[package]\nname = \"acme/zlib\"\nversion = \"1.3.0\"\nc-standard = \"c11\"\n\n\
             [dependencies]\n\"acme/extra\" = \"^1.0\"\n\n\
             [target.z]\ntype = \"library\"\nsources = [\"src/z.c\"]\n",
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
        "\"acme/png\" = \"^1.0\"\n\n\
         [target.app]\ntype = \"executable\"\nsources = [\"src/main.c\"]\nc-standard = \"c11\"\n",
        "\n[patch]\n\"acme/zlib\" = { path = \"../zlib-fork\" }\n\n\
         [system-provided.\"acme/png\"]\npkg-config = \"pngsys\"\n",
    );
    let fixtures = Fixtures::new();
    fixtures.write(
        "pngsys",
        "{\n\"version\": \"1.0.0\",\n\"cflags\": \"\",\n\"libs\": \"\"\n}\n",
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
            "[package]\nname = \"acme/zlib\"\nversion = \"1.3.0\"\nc-standard = \"c11\"\n\n\
             [target.z]\ntype = \"library\"\nsources = [\"src/z.c\"]\n",
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
        "\"acme/zlib\" = { path = \"../zlib\" }\n\n\
         [target.app]\ntype = \"executable\"\nsources = [\"src/main.c\"]\nc-standard = \"c11\"\n",
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

#[test]
fn metadata_reports_the_declared_policy() {
    let dir = TempDir::new().unwrap();
    let manifest = write_app(dir.path(), "", POLICY);
    let value = run_json(
        cabin()
            .args(["metadata", "--format", "json", "--manifest-path"])
            .arg(&manifest),
    );
    assert_eq!(
        value["system_provided"],
        serde_json::json!({ "acme/zlib": { "pkg-config": "zsys" } })
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
            "[package]\nname = \"member\"\nversion = \"0.1.0\"\n{POLICY}"
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
            "[package]\nname = \"acme/lib\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n\
             [target.lib]\ntype = \"library\"\nsources = [\"src/lib.c\"]\n{POLICY}"
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
