//! End-to-end coverage for `[build-dependencies]`: build-context
//! units for the build machine, beside the target context's.

use super::*;

/// `util` built with feature `loud` returns "HELLO", without it
/// "hello".  `gen` links `util` with `loud` and writes a C source
/// returning `util`'s greeting; `app` links `util` without `loud` and
/// uses `gen` as a build dependency.  `app-generated` compiles that
/// source once `generated` is enabled.
pub(crate) fn write_generator_project(root: &Path) {
    let root = assert_fs::fixture::ChildPath::new(root);
    root.child("util/cabin.toml")
        .write_str(
            r#"[package]
name = "util"
version = "0.1.0"
c-standard = "c11"

[features]
loud = []

[target.util]
type = "library"
sources = ["src/util.c"]
include-dirs = ["include"]

[target.'cfg(feature = "loud")'.profile]
defines = ["UTIL_LOUD=1"]
"#,
        )
        .unwrap();
    root.child("util/include/util.h")
        .write_str("const char *util_greeting(void);\n")
        .unwrap();
    root.child("util/src/util.c")
        .write_str(
            "#include <util.h>\n#ifdef UTIL_LOUD\nconst char *util_greeting(void) { return \"HELLO\"; }\n#else\nconst char *util_greeting(void) { return \"hello\"; }\n#endif\n",
        )
        .unwrap();
    root.child("gen/cabin.toml")
        .write_str(
            r#"[package]
name = "gen"
version = "0.1.0"
c-standard = "c11"

[dependencies]
util = { path = "../util", features = ["loud"] }

[target.gen]
type = "executable"
sources = ["src/main.c"]
deps = ["util:util"]
"#,
        )
        .unwrap();
    root.child("gen/src/main.c")
        .write_str(
            "#include <stdio.h>\n#include <util.h>\nint main(int argc, char **argv) {\n    FILE *out;\n    if (argc != 2 || !(out = fopen(argv[1], \"w\"))) return 1;\n    fprintf(out, \"const char *generated(void) { return \\\"%s\\\"; }\\n\", util_greeting());\n    return fclose(out) != 0;\n}\n",
        )
        .unwrap();
    root.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[features]
generated = []

[dependencies]
util = { path = "../util" }

[build-dependencies]
gen = { path = "../gen" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["util:util"]

[target.app-generated]
type = "executable"
sources = ["src/generated_main.cc", "generated/generated.c"]
deps = ["util:util"]
required-features = ["generated"]
"#,
        )
        .unwrap();
    root.child("app/src/main.cc")
        .write_str(
            "#include <cstdio>\nextern \"C\" {\n#include <util.h>\n}\nint main() { std::puts(util_greeting()); }\n",
        )
        .unwrap();
    root.child("app/src/generated_main.cc")
        .write_str(
            "#include <cstdio>\nextern \"C\" const char *generated(void);\nextern \"C\" {\n#include <util.h>\n}\nint main() { std::printf(\"%s %s\\n\", generated(), util_greeting()); }\n",
        )
        .unwrap();
}

/// Run the build-context generator `gen` from `build_root` (the
/// directory holding `build-machine/`) to write `app`'s generated
/// source.
pub(crate) fn run_generator(build_root: &Path, project: &Path) {
    let generated = project.join("app/generated/generated.c");
    fs::create_dir_all(generated.parent().unwrap()).unwrap();
    let status = std::process::Command::new(
        build_root
            .join("build-machine/packages/gen")
            .join(host_exe("gen")),
    )
    .arg(&generated)
    .status()
    .unwrap();
    assert!(status.success());
    let source = fs::read_to_string(&generated).unwrap();
    assert!(
        source.contains("\"HELLO\""),
        "the build context's util must have `loud`: {source}"
    );
}

/// One source, `util.c`, builds in both contexts with different
/// features, and the build context's generator runs on the build
/// machine to feed the target context.
#[test]
fn build_dependencies_build_for_the_build_machine_with_their_own_features() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_generator_project(dir.path());
    let app = dir.path().join("app");
    cabin().current_dir(&app).arg("build").assert().success();
    let profile_dir = app.join("build/dev");
    assert!(
        profile_dir
            .join("build-machine/packages/util")
            .join(host_static_lib("util"))
            .is_file()
    );
    assert!(
        !profile_dir.join("build-machine/packages/app").exists(),
        "only build dependencies have build-context units"
    );
    let output = std::process::Command::new(profile_dir.join("packages/app").join(host_exe("app")))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");

    run_generator(&profile_dir, dir.path());
    cabin()
        .current_dir(&app)
        .args(["build", "--features", "generated"])
        .assert()
        .success();
    let output = std::process::Command::new(
        profile_dir
            .join("packages/app")
            .join(host_exe("app-generated")),
    )
    .output()
    .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "HELLO hello"
    );
}

/// A build tool and the target program are separate link/runtime
/// domains: each may contain a native identity once.
#[test]
fn links_uniqueness_applies_per_context() {
    let dir = TempDir::new().unwrap();
    let root = assert_fs::fixture::ChildPath::new(dir.path());
    for name in ["za", "zb"] {
        root.child(format!("{name}/cabin.toml"))
            .write_str(&format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[target.{name}]\ntype = \"library\"\nsources = [\"src/z.c\"]\nlinks = \"z\"\n"
            ))
            .unwrap();
    }
    root.child("gen/cabin.toml")
        .write_str("[package]\nname = \"gen\"\nversion = \"0.1.0\"\n\n[dependencies]\nzb = { path = \"../zb\" }\n")
        .unwrap();
    let app_manifest = |extra: &str| {
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nza = {{ path = \"../za\" }}\n{extra}\n[build-dependencies]\ngen = {{ path = \"../gen\" }}\n"
        )
    };
    root.child("app/cabin.toml")
        .write_str(&app_manifest(""))
        .unwrap();
    cabin()
        .current_dir(dir.path().join("app"))
        .arg("fetch")
        .assert()
        .success();

    root.child("app/cabin.toml")
        .write_str(&app_manifest("zb = { path = \"../zb\" }\n"))
        .unwrap();
    cabin()
        .current_dir(dir.path().join("app"))
        .arg("fetch")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "native library \"z\" is claimed by multiple packages",
        ));
}

/// A registry build dependency links in the build context even when
/// no local package is built there: its `links` claims never meet the
/// target context's.
#[test]
fn registry_build_dependencies_claim_links_in_their_own_context() {
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    for (name, deps, links) in [
        ("gen", r#""miniz": ">=1""#, ""),
        (
            "miniz",
            "",
            r#", "standards": { "targets": { "miniz": {} } }, "links": { "miniz": "z" }"#,
        ),
        (
            "zlib",
            "",
            r#", "standards": { "targets": { "zlib": {} } }, "links": { "zlib": "z" }"#,
        ),
    ] {
        assert_fs::fixture::ChildPath::new(index.join(format!("{name}.json")))
            .write_str(&format!(
                r#"{{
  "schema": 1,
  "name": "{name}",
  "versions": {{
    "1.0.0": {{ "dependencies": {{ {deps} }}, "yanked": false{links} }}
  }}
}}"#
            ))
            .unwrap();
    }
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nzlib = \">=1\"\n\n[build-dependencies]\ngen = \">=1\"\n",
        )
        .unwrap();
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--index-path")
        .arg(&index)
        .assert()
        .success();
}

/// A registry package used only as a build dependency joins the
/// lockfile and the vendor directory, and an offline build from that
/// directory builds it for the build machine.
#[test]
fn registry_build_dependencies_are_locked_vendored_and_built_offline() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index.join("config.json"))
        .write_str("{\"schema\":1,\"kind\":\"file-registry\",\"packages\":\"packages\",\"artifacts\":\"artifacts\"}\n")
        .unwrap();
    let checksum = make_archive(
        &index.join("artifacts/gen/gen-1.0.0.zip"),
        &[
            (
                "cabin.toml",
                "[package]\nname = \"gen\"\nversion = \"1.0.0\"\nc-standard = \"c11\"\n\n[target.gen]\ntype = \"executable\"\nsources = [\"src/main.c\"]\n",
            ),
            ("src/main.c", "int main(void) { return 0; }\n"),
        ],
    );
    let revision = &checksum[..16];
    assert_fs::fixture::ChildPath::new(index.join("packages/gen.json"))
        .write_str(&format!(
            "{{\"schema\": 1, \"name\": \"gen\", \"versions\": {{\"1.0.0\": {{\"dependencies\": {{}}, \"yanked\": false, \"revision\": \"{revision}\", \"revisions\": {{\"{revision}\": {{\"checksum\": \"sha256:{checksum}\", \"published-at\": \"2026-01-01T00:00:00Z\", \"source\": {{\"type\": \"archive\", \"path\": \"../artifacts/gen/gen-1.0.0.zip\", \"format\": \"zip\"}}}}}}}}}}}}\n"
        ))
        .unwrap();
    let proj = assert_fs::fixture::ChildPath::new(dir.path().join("proj"));
    proj.child("cabin.toml")
        .write_str(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[build-dependencies]\ngen = \"1.0.0\"\n\n[target.app]\ntype = \"executable\"\nsources = [\"src/main.c\"]\n",
        )
        .unwrap();
    proj.child("src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();

    cabin()
        .current_dir(proj.path())
        .args(["vendor", "--vendor-dir", "vendor", "--index-path"])
        .arg(&index)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .assert()
        .success();
    let lockfile = fs::read_to_string(proj.path().join("cabin.lock")).unwrap();
    assert!(lockfile.contains("name = \"gen\""), "{lockfile}");
    assert!(proj.path().join("vendor/packages/gen.json").is_file());

    fs::remove_dir_all(&index).unwrap();
    cabin()
        .current_dir(proj.path())
        .args(["build", "--offline", "--locked", "--index-path", "vendor"])
        .arg("--cache-dir")
        .arg(dir.path().join("vendor-cache"))
        .assert()
        .success();
    assert!(
        proj.path()
            .join("build/dev/build-machine/packages/gen")
            .join(host_exe("gen"))
            .is_file()
    );
}

#[test]
fn packaging_refuses_build_dependencies() {
    let dir = TempDir::new().unwrap();
    let root = assert_fs::fixture::ChildPath::new(dir.path());
    root.child("cabin.toml")
        .write_str(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[build-dependencies]\ngen = \"1\"\n\n[target.app]\ntype = \"library\"\nsources = [\"src/app.c\"]\n",
        )
        .unwrap();
    root.child("src/app.c").write_str("int app;\n").unwrap();
    cabin()
        .current_dir(dir.path())
        .arg("package")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "cannot package build dependency `gen`",
        ));
}
