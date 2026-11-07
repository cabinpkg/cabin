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

/// `tools` has a helper library, the generator `gen` that links it,
/// an executable `broken` that does not compile, a feature-gated
/// executable `gated`, and a test.  `app`'s build-dependency entry on
/// `tools` lists `targets`.
fn write_targeted_tools_project(root: &Path, targets: &str) {
    let root = assert_fs::fixture::ChildPath::new(root);
    root.child("tools/cabin.toml")
        .write_str(
            r#"[package]
name = "tools"
version = "0.1.0"
c-standard = "c11"

[features]
extra = []

[target.helper]
type = "library"
sources = ["src/helper.c"]

[target.gen]
type = "executable"
sources = ["src/gen.c"]
deps = ["helper"]

[target.broken]
type = "executable"
sources = ["src/broken.c"]

[target.gated]
type = "executable"
sources = ["src/gen.c"]
deps = ["helper"]
required-features = ["extra"]

[target.tools_test]
type = "test"
sources = ["src/gen.c"]
deps = ["helper"]
"#,
        )
        .unwrap();
    root.child("tools/src/helper.c")
        .write_str("int helper(void) { return 0; }\n")
        .unwrap();
    root.child("tools/src/gen.c")
        .write_str("int helper(void);\nint main(void) { return helper(); }\n")
        .unwrap();
    root.child("tools/src/broken.c")
        .write_str("this is not C\n")
        .unwrap();
    root.child("app/cabin.toml")
        .write_str(&format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[build-dependencies]\ntools = {{ path = \"../tools\", targets = {targets} }}\n\n[target.app]\ntype = \"executable\"\nsources = [\"src/main.c\"]\n"
        ))
        .unwrap();
    root.child("app/src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
}

/// `targets` narrows the build machine to the listed targets and what
/// they link: `broken` would fail the build if it were built.
#[test]
fn build_dependency_targets_select_what_the_build_machine_builds() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_targeted_tools_project(dir.path(), r#"["gen"]"#);
    let app = dir.path().join("app");
    cabin().current_dir(&app).arg("build").assert().success();
    let tools = app.join("build/dev/build-machine/packages/tools");
    assert!(tools.join(host_exe("gen")).is_file());
    assert!(tools.join(host_static_lib("helper")).is_file());
    assert!(!tools.join(host_exe("broken")).exists());
}

/// `app` reaches `mid` through a normal edge (target context) or a
/// build edge (build context), and both reach `tools` as a build
/// dependency: `app` lists `gen`, `mid` lists the C++ tool `gen2` or
/// omits `targets`.  The build machine builds the union, plus the
/// default targets when an edge omits `targets`.
#[test]
fn build_dependency_targets_union_across_edges() {
    require_c_and_cxx_build_tools();
    for (via_build_edge, mid_targets, defaults) in [
        (false, r#", targets = ["gen2"]"#, false),
        (false, "", true),
        (true, r#", targets = ["gen2"]"#, false),
        (true, "", true),
    ] {
        let dir = TempDir::new().unwrap();
        write_targeted_tools_project(dir.path(), r#"["gen"]"#);
        let root = assert_fs::fixture::ChildPath::new(dir.path());
        let tools = std::fs::read_to_string(root.child("tools/cabin.toml").path()).unwrap();
        root.child("tools/cabin.toml")
            .write_str(&format!(
                "{tools}\n[target.gen2]\ntype = \"executable\"\nsources = [\"src/gen2.cc\"]\ncxx-standard = \"c++17\"\n"
            ))
            .unwrap();
        root.child("tools/src/gen2.cc")
            .write_str("#include <string>\nint main() { return std::string(\"x\").size() == 1 ? 0 : 1; }\n")
            .unwrap();
        // The default targets include `broken`: make it build.
        root.child("tools/src/broken.c")
            .write_str("int main(void) { return 0; }\n")
            .unwrap();
        root.child("mid/cabin.toml")
            .write_str(&format!(
                "[package]\nname = \"mid\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[build-dependencies]\ntools = {{ path = \"../tools\"{mid_targets} }}\n\n[target.mid]\ntype = \"library\"\nsources = [\"src/mid.c\"]\n"
            ))
            .unwrap();
        root.child("mid/src/mid.c")
            .write_str("int mid(void) { return 0; }\n")
            .unwrap();
        let app_manifest = std::fs::read_to_string(root.child("app/cabin.toml").path()).unwrap();
        let app_manifest = if via_build_edge {
            app_manifest.replace(
                "[build-dependencies]",
                "[build-dependencies]\nmid = { path = \"../mid\" }",
            )
        } else {
            app_manifest
                .replace(
                    "[build-dependencies]",
                    "[dependencies]\nmid = { path = \"../mid\" }\n\n[build-dependencies]",
                )
                .replace(
                    "sources = [\"src/main.c\"]",
                    "sources = [\"src/main.c\"]\ndeps = [\"mid\"]",
                )
        };
        root.child("app/cabin.toml")
            .write_str(&app_manifest)
            .unwrap();
        let app = dir.path().join("app");
        cabin().current_dir(&app).arg("build").assert().success();
        let built = app.join("build/dev/build-machine/packages/tools");
        assert!(built.join(host_exe("gen")).is_file());
        assert!(built.join(host_exe("gen2")).is_file());
        assert_eq!(
            built.join(host_exe("broken")).exists(),
            defaults,
            "{via_build_edge} {mid_targets:?}"
        );
    }
}

/// `cabin metadata` reports a build-dependency entry's `targets`.
#[test]
fn metadata_reports_build_dependency_targets() {
    let dir = TempDir::new().unwrap();
    write_targeted_tools_project(dir.path(), r#"["gen", "helper"]"#);
    let output = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let app = value["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "app")
        .unwrap();
    assert_eq!(
        app["dependencies"][0]["targets"],
        serde_json::json!(["gen", "helper"])
    );
}

#[test]
fn build_dependency_targets_reject_targets_the_build_machine_cannot_build() {
    require_c_and_cxx_build_tools();
    for (targets, expected) in [
        (r#"["nope"]"#, "has no target `nope`"),
        (r#"["tools_test"]"#, "names the test target `tools_test`"),
        (r#"["gated"]"#, "requires the features [extra]"),
    ] {
        let dir = TempDir::new().unwrap();
        write_targeted_tools_project(dir.path(), targets);
        let assertion = cabin()
            .current_dir(dir.path().join("app"))
            .arg("build")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(stderr.contains(expected), "{targets}: {stderr}");
    }
}

/// Two projects require different versions of one registry build tool
/// and share an artifact cache: each builds and runs its own version.
#[test]
fn two_projects_build_two_versions_of_one_build_tool() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let index = dir.path().join("index");
    assert_fs::fixture::ChildPath::new(index.join("config.json"))
        .write_str("{\"schema\":1,\"kind\":\"file-registry\",\"packages\":\"packages\",\"artifacts\":\"artifacts\"}\n")
        .unwrap();
    let mut versions = Vec::new();
    for major in ["1", "2"] {
        let version = format!("{major}.0.0");
        let checksum = make_archive(
            &index.join(format!("artifacts/gen/gen-{version}.zip")),
            &[
                (
                    "cabin.toml",
                    &format!(
                        "[package]\nname = \"gen\"\nversion = \"{version}\"\nc-standard = \"c11\"\n\n[target.gen]\ntype = \"executable\"\nsources = [\"src/main.c\"]\n"
                    ),
                ),
                (
                    "src/main.c",
                    &format!(
                        "#include <stdio.h>\nint main(void) {{ puts(\"{major}\"); return 0; }}\n"
                    ),
                ),
            ],
        );
        let revision = &checksum[..16];
        versions.push(format!(
            "\"{version}\": {{\"dependencies\": {{}}, \"yanked\": false, \"revision\": \"{revision}\", \"revisions\": {{\"{revision}\": {{\"checksum\": \"sha256:{checksum}\", \"published-at\": \"2026-01-01T00:00:00Z\", \"source\": {{\"type\": \"archive\", \"path\": \"../artifacts/gen/gen-{version}.zip\", \"format\": \"zip\"}}}}}}}}"
        ));
    }
    assert_fs::fixture::ChildPath::new(index.join("packages/gen.json"))
        .write_str(&format!(
            "{{\"schema\": 1, \"name\": \"gen\", \"versions\": {{{}}}}}\n",
            versions.join(", ")
        ))
        .unwrap();
    for major in ["1", "2"] {
        let proj = assert_fs::fixture::ChildPath::new(dir.path().join(format!("proj{major}")));
        proj.child("cabin.toml")
            .write_str(&format!(
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[build-dependencies]\ngen = \"{major}.0.0\"\n\n[target.app]\ntype = \"executable\"\nsources = [\"src/main.c\"]\n"
            ))
            .unwrap();
        proj.child("src/main.c")
            .write_str("int main(void) { return 0; }\n")
            .unwrap();
        cabin()
            .current_dir(proj.path())
            .args(["build", "--index-path"])
            .arg(&index)
            .arg("--cache-dir")
            .arg(dir.path().join("cache"))
            .assert()
            .success();
    }
    for major in ["1", "2"] {
        let tool = dir
            .path()
            .join(format!("proj{major}/build/dev/build-machine/packages/gen"))
            .join(host_exe("gen"));
        let output = std::process::Command::new(&tool).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), major);
    }
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

/// The command of the link edge whose output path ends with `tail`.
fn link_program(ninja: &str, tail: &str) -> String {
    let needle = format!("{}: link_executable ", host_path(tail));
    let mut lines = ninja.lines();
    lines
        .find(|line| line.starts_with("build ") && line.contains(&needle))
        .unwrap_or_else(|| panic!("no link edge for {tail}: {ninja}"));
    let command = lines
        .take_while(|line| !line.is_empty())
        .find_map(|line| line.strip_prefix("  command = "))
        .unwrap();
    program_from_command(command)
}

/// Each build-machine executable links with the driver its own closure
/// needs - a C tool with the C driver, a C tool that links a C++
/// library with the C++ driver - and the C program stays on the C
/// driver.  The project and build directories contain spaces.
#[test]
fn build_machine_executables_pick_the_link_driver_their_closure_needs() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = assert_fs::fixture::ChildPath::new(dir.path().join("a project"));
    root.child("tools/cabin.toml")
        .write_str(
            r#"[package]
name = "tools"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.chelp]
type = "library"
sources = ["src/chelp.c"]

[target.cgen]
type = "executable"
sources = ["src/cgen.c"]
deps = ["chelp"]

[target.cxxhelp]
type = "library"
sources = ["src/cxxhelp.cc"]
interface-c-standard = "c11"

[target.cxxgen]
type = "executable"
sources = ["src/cxxgen.c"]
deps = ["cxxhelp"]
"#,
        )
        .unwrap();
    root.child("tools/src/chelp.c")
        .write_str("int chelp(void) { return 0; }\n")
        .unwrap();
    root.child("tools/src/cgen.c")
        .write_str("int chelp(void);\nint main(void) { return chelp(); }\n")
        .unwrap();
    root.child("tools/src/cxxhelp.cc")
        .write_str("#include <string>\nextern \"C\" int cxxhelp(void) { return static_cast<int>(std::string(\"x\").size()) - 1; }\n")
        .unwrap();
    root.child("tools/src/cxxgen.c")
        .write_str("int cxxhelp(void);\nint main(void) { return cxxhelp(); }\n")
        .unwrap();
    root.child("app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"

[build-dependencies]
tools = { path = "../tools" }

[target.app]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    root.child("app/src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let manifest = root.path().join("app/cabin.toml");
    let build_dir = root.path().join("build dir");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .arg("--build-dir")
        .arg(&build_dir)
        .assert()
        .success();

    let detected = &run_metadata(&manifest)["toolchain"]["detected"];
    let cc = detected["cc"]["path"].as_str().unwrap();
    let cxx = detected["cxx"]["path"].as_str().unwrap();
    let ninja = fs::read_to_string(build_dir.join("dev/build.ninja")).unwrap();
    let tools = "build-machine/packages/tools";
    assert_eq!(
        link_program(&ninja, &format!("{tools}/{}", host_exe("cgen"))),
        cc
    );
    assert_eq!(
        link_program(&ninja, &format!("{tools}/{}", host_exe("cxxgen"))),
        cxx
    );
    assert_eq!(
        link_program(&ninja, &format!("packages/app/{}", host_exe("app"))),
        cc
    );
    for exe in ["cgen", "cxxgen"] {
        let status =
            std::process::Command::new(build_dir.join("dev").join(tools).join(host_exe(exe)))
                .status()
                .unwrap();
        assert!(status.success(), "{exe}");
    }
}

/// A build tool that fails to compile fails the build, and the
/// failure names the build-machine unit.
#[test]
fn a_build_machine_compile_failure_fails_the_build() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let root = assert_fs::fixture::ChildPath::new(dir.path());
    root.child("gen/cabin.toml")
        .write_str(
            "[package]\nname = \"gen\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[target.gen]\ntype = \"executable\"\nsources = [\"src/main.c\"]\n",
        )
        .unwrap();
    root.child("gen/src/main.c")
        .write_str("this is not C\n")
        .unwrap();
    root.child("app/cabin.toml")
        .write_str(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nc-standard = \"c11\"\n\n[build-dependencies]\ngen = { path = \"../gen\" }\n\n[target.app]\ntype = \"executable\"\nsources = [\"src/main.c\"]\n",
        )
        .unwrap();
    root.child("app/src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let output = cabin()
        .current_dir(dir.path().join("app"))
        .arg("build")
        .assert()
        .failure()
        .get_output()
        .clone();
    let shown = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Ninja may print the `FAILED:` output with either separator.
    assert!(
        shown
            .replace('\\', "/")
            .contains("build-machine/packages/gen"),
        "{shown}"
    );
}
