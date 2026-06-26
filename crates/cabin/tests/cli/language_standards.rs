//! End-to-end coverage for first-class C/C++ language standards.
//!
//! These tests exercise the manifest fields (`c-standard` /
//! `cxx-standard` / `interface-c-standard` /
//! `interface-cxx-standard`) across the parser, planner, dialect
//! lowering, Ninja generation, the escape-hatch conflict rule,
//! interface enforcement, the file registry, and the metadata view.
//! Real compiles stick to standards every CI toolchain accepts;
//! exotic values are covered by planner / driver unit tests.

use super::*;

/// Dialect-appropriate standard flag for the host: `-std=<v>` on
/// GCC/Clang, `/std:<v>` on MSVC (Windows CI is the MSVC leg).
fn host_std_flag(value: &str) -> String {
    if cfg!(windows) {
        format!("/std:{value}")
    } else {
        format!("-std={value}")
    }
}

fn write_lib_and_app(dir: &Path, package_fields: &str, lib_fields: &str, app_fields: &str) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "demo"
version = "0.1.0"
{package_fields}

[target.core]
type = "library"
sources = ["src/core.cc"]
include-dirs = ["include"]
{lib_fields}

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["core"]
{app_fields}
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("include/core.h"))
        .write_str("#pragma once\nint core_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/core.cc"))
        .write_str("#include \"core.h\"\nint core_value() { return 42; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/main.cc"))
        .write_str("#include \"core.h\"\nint main() { return core_value() == 42 ? 0 : 1; }\n")
        .unwrap();
}

fn build_ninja(dir: &Path) -> String {
    fs::read_to_string(dir.join("build/dev/build.ninja")).unwrap()
}

#[test]
fn declared_cxx_standard_reaches_ninja_and_compile_commands() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_lib_and_app(dir.path(), "cxx-standard = \"c++14\"", "", "");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let ninja = build_ninja(dir.path());
    let declared = host_std_flag("c++14");
    let default = host_std_flag("c++17");
    assert!(
        ninja.contains(&declared),
        "expected `{declared}` in build.ninja: {ninja}"
    );
    assert!(
        !ninja.contains(&default),
        "the built-in default `{default}` must be replaced: {ninja}"
    );
    let ccdb = fs::read_to_string(dir.path().join("build/dev/compile_commands.json")).unwrap();
    assert!(
        ccdb.contains(&declared),
        "expected `{declared}` in compile_commands.json: {ccdb}"
    );
}

#[test]
fn declared_c_standard_applies_to_c_sources_only() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // MSVC has no `/std:c99`; use `c17` on the Windows leg so the
    // build runs there (the c99 gap is covered by the
    // capability unit tests).
    let c_standard = if cfg!(windows) { "c17" } else { "c99" };
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "mixedstd"
version = "0.1.0"
c-standard = "{c_standard}"

[target.app]
type = "executable"
sources = ["src/main.cc", "src/util.c"]
include-dirs = ["include"]
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("include/util.h"))
        .write_str(
            "#pragma once\n#ifdef __cplusplus\nextern \"C\" {\n#endif\nint util(void);\n#ifdef __cplusplus\n}\n#endif\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/util.c"))
        .write_str("#include \"util.h\"\nint util(void) { return 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/main.cc"))
        .write_str("#include \"util.h\"\nint main() { return util() == 1 ? 0 : 1; }\n")
        .unwrap();
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let ninja = build_ninja(dir.path());
    // The C compile carries the declared C standard; the C++ side
    // keeps the built-in default.
    assert!(
        ninja.contains(&host_std_flag(c_standard)),
        "expected `{}` in build.ninja: {ninja}",
        host_std_flag(c_standard)
    );
    assert!(
        ninja.contains(&host_std_flag("c++17")),
        "the C++ compile must keep the default standard: {ninja}"
    );
    assert!(
        !ninja.contains(&host_std_flag("c11")),
        "the built-in C default must be replaced: {ninja}"
    );
}

#[test]
fn target_override_beats_package_default() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_lib_and_app(
        dir.path(),
        "cxx-standard = \"c++14\"",
        "cxx-standard = \"c++17\"\ninterface-cxx-standard = \"c++14\"",
        "",
    );
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let ninja = build_ninja(dir.path());
    // Both standards appear: the library override and the package
    // default for the executable.
    assert!(ninja.contains(&host_std_flag("c++17")), "{ninja}");
    assert!(ninja.contains(&host_std_flag("c++14")), "{ninja}");
}

#[test]
fn conflict_between_declared_standard_and_cxxflags_errors() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_lib_and_app(dir.path(), "cxx-standard = \"c++17\"", "", "");
    let manifest_path = dir.path().join("cabin.toml");
    let mut manifest = fs::read_to_string(&manifest_path).unwrap();
    manifest.push_str("\n[profile]\ncxxflags = [\"-std=c++14\"]\n");
    assert_fs::fixture::ChildPath::new(&manifest_path)
        .write_str(&manifest)
        .unwrap();
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(&manifest_path)
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("cxx-standard") && stderr.contains("-std=c++14"),
        "expected the conflict diagnostic naming both sides, got: {stderr}"
    );
    assert!(
        stderr.contains("cabin::language::standard_flag_conflict"),
        "expected the stable diagnostic code, got: {stderr}"
    );
}

#[test]
fn undeclared_project_with_std_in_cxxflags_still_builds() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let escape_hatch = host_std_flag("c++14");
    write_lib_and_app(dir.path(), "", "", "");
    let manifest_path = dir.path().join("cabin.toml");
    let mut manifest = fs::read_to_string(&manifest_path).unwrap();
    manifest.push_str("\n[profile]\ncxxflags = [\"");
    manifest.push_str(&escape_hatch);
    manifest.push_str("\"]\n");
    assert_fs::fixture::ChildPath::new(&manifest_path)
        .write_str(&manifest)
        .unwrap();
    cabin()
        .args(["build", "--manifest-path"])
        .arg(&manifest_path)
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    // The escape hatch comes later in argv, so it keeps winning over
    // the built-in default when no first-class standard is declared.
    let ninja = build_ninja(dir.path());
    assert!(ninja.contains(&escape_hatch), "{ninja}");
}

#[test]
fn lower_consumer_of_declared_cxx20_library_fails_before_ninja() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_lib_and_app(
        dir.path(),
        "",
        "cxx-standard = \"c++20\"",
        "cxx-standard = \"c++17\"",
    );
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("c++20") && stderr.contains("demo:core"),
        "expected the interface-compatibility diagnostic, got: {stderr}"
    );
    assert!(
        !dir.path().join("build/dev/build.ninja").exists(),
        "the plan must fail before any Ninja file is written"
    );
}

#[test]
fn interface_standard_relaxes_the_requirement() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_lib_and_app(
        dir.path(),
        "",
        "cxx-standard = \"c++20\"\ninterface-cxx-standard = \"c++17\"",
        "cxx-standard = \"c++17\"",
    );
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    // The library still compiles with its declared implementation
    // standard.
    let ninja = build_ninja(dir.path());
    assert!(ninja.contains(&host_std_flag("c++20")), "{ninja}");
}

#[test]
fn published_index_preserves_language_standards() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"
cxx-standard = "c++20"
interface-cxx-standard = "c++17"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("include/fmt.h"))
        .write_str("#pragma once\nint fmt_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("src/fmt.cc"))
        .write_str("#include \"fmt.h\"\nint fmt_value() { return 1; }\n")
        .unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();
    let body = fs::read_to_string(registry.join("packages/fmt.json")).unwrap();
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    let entry = &value["versions"]["10.2.1"];
    assert_eq!(
        entry["language"],
        serde_json::json!({
            "cxx_standard": "c++20",
            "interface_cxx_standard": "c++17",
        })
    );
}

#[test]
fn registry_package_standards_apply_at_the_consumer() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();

    // Publish a library that declares its implementation standard.
    let pkg_root = dir.path().join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"
cxx-standard = "c++14"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("include/fmt.h"))
        .write_str("#pragma once\nint fmt_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("src/fmt.cc"))
        .write_str("#include \"fmt.h\"\nint fmt_value() { return 41; }\n")
        .unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    // Consume it: the extracted dependency's own manifest drives its
    // compile standard.
    let app_root = dir.path().join("app");
    assert_fs::fixture::ChildPath::new(app_root.join("cabin.toml"))
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
    assert_fs::fixture::ChildPath::new(app_root.join("src/main.cc"))
        .write_str("#include \"fmt.h\"\nint main() { return fmt_value() == 41 ? 0 : 1; }\n")
        .unwrap();
    cabin()
        .args(["build", "--manifest-path"])
        .arg(app_root.join("cabin.toml"))
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--build-dir")
        .arg(app_root.join("build"))
        .assert()
        .success();
    let ninja = fs::read_to_string(app_root.join("build/dev/build.ninja")).unwrap();
    // The dependency compiles with its declared c++14; the consumer
    // keeps the built-in c++17.
    assert!(ninja.contains(&host_std_flag("c++14")), "{ninja}");
    assert!(ninja.contains(&host_std_flag("c++17")), "{ninja}");
}

#[test]
fn metadata_language_block_is_deterministic_and_reports_sources() {
    let dir = TempDir::new().unwrap();
    write_lib_and_app(
        dir.path(),
        "cxx-standard = \"c++20\"",
        "interface-cxx-standard = \"c++17\"",
        "",
    );
    let manifest = dir.path().join("cabin.toml");
    let value = run_metadata(&manifest);
    let config = &package_in(&value, "demo")["configuration"];
    let language = &config["language"];
    assert_eq!(language["cxx"]["standard"], "c++20");
    assert_eq!(language["cxx"]["source"], "package");
    assert_eq!(language["c"]["standard"], "c11");
    assert_eq!(language["c"]["source"], "builtin-default");
    let core = &language["targets"]["core"];
    assert_eq!(core["cxx"]["standard"], "c++20");
    assert_eq!(core["cxx"]["source"], "package");
    assert_eq!(core["interface_cxx"]["standard"], "c++17");
    assert_eq!(core["interface_cxx"]["source"], "target");
    assert_eq!(core["interface_c"]["source"], "compile-standard");
    let app = &language["targets"]["app"];
    assert!(
        app.get("interface_cxx").is_none(),
        "executables carry no interface standards: {app}"
    );
    // Deterministic across runs.
    let again = run_metadata(&manifest);
    assert_eq!(
        config,
        &package_in(&again, "demo")["configuration"],
        "configuration block must be byte-stable across runs"
    );
}

#[test]
fn explain_build_config_reports_effective_standards() {
    let dir = TempDir::new().unwrap();
    write_lib_and_app(dir.path(), "cxx-standard = \"c++20\"", "", "");
    cabin()
        .args(["explain", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["build-config", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cxx standard: c++20 (package)"))
        .stdout(predicate::str::contains(
            "c standard: c11 (builtin-default)",
        ));
}

#[test]
fn sibling_target_standard_does_not_gate_selected_target() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // `exotic` declares c++23 - which MSVC has no stable flag for -
    // but `cabin run --bin app` never plans it, so toolchain
    // validation must not reject the run (the regression Codex
    // reported on the package-level requested-standards collection).
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[target.app]
type = "executable"
sources = ["src/main.cc"]

[target.exotic]
type = "executable"
sources = ["src/exotic.cc"]
cxx-standard = "c++23"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/exotic.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    cabin()
        .args(["run", "--bin", "app", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
}

#[test]
fn check_ignores_dependency_implementation_standards() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // The path dependency is implemented as c++23 (which MSVC has no
    // stable flag for) but exposes a c++17 interface. `cabin check`
    // drops dependency compiles, so checking the consumer must
    // succeed everywhere - including the MSVC leg, where the dep's
    // implementation standard could neither be validated nor lowered.
    assert_fs::fixture::ChildPath::new(dir.path().join("dep/cabin.toml"))
        .write_str(
            r#"[package]
name = "dep"
version = "0.1.0"

[target.dep]
type = "library"
sources = ["src/dep.cc"]
include-dirs = ["include"]
cxx-standard = "c++23"
interface-cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("dep/include/dep.h"))
        .write_str("#pragma once\nint dep_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("dep/src/dep.cc"))
        .write_str("#include \"dep.h\"\nint dep_value() { return 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
dep = { path = "../dep" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["dep"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("#include \"dep.h\"\nint main() { return dep_value() == 1 ? 0 : 1; }\n")
        .unwrap();
    cabin()
        .args(["check", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .success();
}

#[test]
fn check_prunes_dependency_internal_interface_violation() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // app (c++20) -> liba (impl c++17) -> libb (interface c++20):
    // the incompatible pair lives entirely inside the dependency
    // closure. `cabin check` of app prunes liba's compiles, so the
    // syntax-only pass succeeds; `cabin build` plans them and must
    // surface the incompatibility before writing any Ninja file.
    assert_fs::fixture::ChildPath::new(dir.path().join("libb/cabin.toml"))
        .write_str(
            r#"[package]
name = "libb"
version = "0.1.0"

[target.libb]
type = "library"
sources = ["src/b.cc"]
include-dirs = ["include"]
cxx-standard = "c++20"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("libb/include/b.h"))
        .write_str("#pragma once\nint b_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("libb/src/b.cc"))
        .write_str("#include \"b.h\"\nint b_value() { return 2; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("liba/cabin.toml"))
        .write_str(
            r#"[package]
name = "liba"
version = "0.1.0"

[dependencies]
libb = { path = "../libb" }

[target.liba]
type = "library"
sources = ["src/a.cc"]
include-dirs = ["include"]
deps = ["libb"]
cxx-standard = "c++17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("liba/include/a.h"))
        .write_str("#pragma once\nint a_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("liba/src/a.cc"))
        .write_str("#include \"a.h\"\n#include \"b.h\"\nint a_value() { return b_value() + 1; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
liba = { path = "../liba" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["liba"]
cxx-standard = "c++20"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.cc"))
        .write_str("#include \"a.h\"\nint main() { return a_value() == 3 ? 0 : 1; }\n")
        .unwrap();
    cabin()
        .args(["check", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .success();
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build2"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("liba:liba") && stderr.contains("libb:libb") && stderr.contains("c++20"),
        "expected the dependency-internal interface diagnostic, got: {stderr}"
    );
    assert!(
        !dir.path().join("app/build2/dev/build.ninja").exists(),
        "the build must fail before any Ninja file is written"
    );
}

#[test]
fn sibling_target_conflict_does_not_gate_selected_target() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // The conflict candidate (target-level `cxx-standard` on `exotic`
    // plus a package-level `-std=` escape hatch) covers only
    // `exotic`'s compiles. `cabin run --bin app` never plans them, so
    // it must succeed; the default `cabin build` plans both and must
    // surface the conflict.
    let escape_hatch = host_std_flag("c++14");
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "demo"
version = "0.1.0"

[profile]
cxxflags = ["{escape_hatch}"]

[target.app]
type = "executable"
sources = ["src/main.cc"]

[target.exotic]
type = "executable"
sources = ["src/exotic.cc"]
cxx-standard = "c++17"
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/exotic.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    cabin()
        .args(["run", "--bin", "app", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build2"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("cabin::language::standard_flag_conflict"),
        "the default build plans the conflicting target, got: {stderr}"
    );
}

/// Write a single-package fixture whose library imposes a C17
/// interface (via its target-level implementation standard) on a
/// C-compiling consumer.  Both standards have stable MSVC flags, so
/// the Windows leg exercises the same paths.
fn write_c_lib_and_app(dir: &Path, lib_fields: &str, app_fields: &str) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "demo"
version = "0.1.0"

[target.clib]
type = "library"
sources = ["src/clib.c"]
include-dirs = ["include"]
{lib_fields}

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["clib"]
{app_fields}
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("include/clib.h"))
        .write_str("#pragma once\nint clib_value(void);\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/clib.c"))
        .write_str("#include \"clib.h\"\nint clib_value(void) { return 42; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/main.c"))
        .write_str("#include \"clib.h\"\nint main(void) { return clib_value() == 42 ? 0 : 1; }\n")
        .unwrap();
}

#[test]
fn lower_consumer_of_declared_c17_library_fails_before_ninja() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // clib's declared c17 binds C-compiling consumers (interface
    // defaults to the effective implementation standard); the app's
    // declared c11 is below it.
    write_c_lib_and_app(dir.path(), "c-standard = \"c17\"", "c-standard = \"c11\"");
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("c17") && stderr.contains("demo:clib"),
        "expected the C interface-compatibility diagnostic, got: {stderr}"
    );
    assert!(
        !dir.path().join("build/dev/build.ninja").exists(),
        "the plan must fail before any Ninja file is written"
    );
}

#[test]
fn conflict_between_declared_c_standard_and_cflags_errors() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
c-standard = "c17"

[profile]
cflags = ["-std=c11"]

[target.app]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("c-standard") && stderr.contains("-std=c11"),
        "expected the C conflict diagnostic naming both sides, got: {stderr}"
    );
    assert!(
        stderr.contains("cabin::language::standard_flag_conflict"),
        "expected the stable diagnostic code, got: {stderr}"
    );
}

#[test]
fn check_prunes_dependency_internal_c_interface_violation() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // app (c17) -> liba (impl c11) -> libb (interface c17): the C
    // mirror of the C++ deferral test. `cabin check` of app prunes
    // liba's compiles and with them the violation; `cabin build`
    // surfaces it before writing any Ninja file.
    assert_fs::fixture::ChildPath::new(dir.path().join("libb/cabin.toml"))
        .write_str(
            r#"[package]
name = "libb"
version = "0.1.0"

[target.libb]
type = "library"
sources = ["src/b.c"]
include-dirs = ["include"]
c-standard = "c17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("libb/include/b.h"))
        .write_str("#pragma once\nint b_value(void);\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("libb/src/b.c"))
        .write_str("#include \"b.h\"\nint b_value(void) { return 2; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("liba/cabin.toml"))
        .write_str(
            r#"[package]
name = "liba"
version = "0.1.0"

[dependencies]
libb = { path = "../libb" }

[target.liba]
type = "library"
sources = ["src/a.c"]
include-dirs = ["include"]
deps = ["libb"]
c-standard = "c11"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("liba/include/a.h"))
        .write_str("#pragma once\nint a_value(void);\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("liba/src/a.c"))
        .write_str(
            "#include \"a.h\"\n#include \"b.h\"\nint a_value(void) { return b_value() + 1; }\n",
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
liba = { path = "../liba" }

[target.app]
type = "executable"
sources = ["src/main.c"]
deps = ["liba"]
c-standard = "c17"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("app/src/main.c"))
        .write_str("#include \"a.h\"\nint main(void) { return a_value() == 3 ? 0 : 1; }\n")
        .unwrap();
    cabin()
        .args(["check", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build"))
        .assert()
        .success();
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("app/cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("app/build2"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("liba:liba") && stderr.contains("libb:libb") && stderr.contains("c17"),
        "expected the dependency-internal C interface diagnostic, got: {stderr}"
    );
    assert!(
        !dir.path().join("app/build2/dev/build.ninja").exists(),
        "the build must fail before any Ninja file is written"
    );
}

/// Write a conflicting manifest (declared `cxx-standard` plus a
/// `-std=` escape hatch) with a library and a test target, so every
/// planning command can exercise the violation-surfacing path.
fn write_conflicting_package_with_test_target(dir: &Path) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[profile]
cxxflags = ["-std=c++14"]

[target.demo]
type = "library"
sources = ["src/demo.cc"]
include-dirs = ["include"]

[target.demo_test]
type = "test"
sources = ["tests/demo_test.cc"]
deps = ["demo"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("include/demo.h"))
        .write_str("#pragma once\nint demo_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/demo.cc"))
        .write_str("#include \"demo.h\"\nint demo_value() { return 7; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("tests/demo_test.cc"))
        .write_str("#include \"demo.h\"\nint main() { return demo_value() == 7 ? 0 : 1; }\n")
        .unwrap();
}

#[test]
fn cabin_test_surfaces_standard_violations() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_conflicting_package_with_test_target(dir.path());
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("cabin::language::standard_flag_conflict"),
        "`cabin test` must surface the surviving conflict, got: {stderr}"
    );
}

#[test]
fn cabin_tidy_surfaces_standard_violations() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_conflicting_package_with_test_target(dir.path());
    // The violation fires after planning and before the compile
    // database is written, so the test never spawns run-clang-tidy
    // and stays runnable on hosts without it.
    let assertion = cabin()
        .args(["tidy", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("cabin::language::standard_flag_conflict"),
        "`cabin tidy` must surface the surviving conflict, got: {stderr}"
    );
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "exercises the MSVC no-stable-flag guard; Windows CI is the MSVC-dialect e2e leg"
)]
fn msvc_dialect_rejects_unstable_standard_end_to_end() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_lib_and_app(dir.path(), "cxx-standard = \"c++23\"", "", "");
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("c++23") && stderr.contains("no stable MSVC"),
        "expected the MSVC no-stable-flag diagnostic, got: {stderr}"
    );
    assert!(
        !dir.path().join("build/dev/build.ninja").exists(),
        "the build must fail before any Ninja file is written"
    );
}

#[test]
fn metadata_stays_fail_soft_on_conflicting_manifest() {
    let dir = TempDir::new().unwrap();
    // A manifest whose build fails on the escape-hatch conflict:
    // `cabin metadata` is an observer and must still succeed and
    // report the declared configuration.
    write_lib_and_app(dir.path(), "cxx-standard = \"c++17\"", "", "");
    let manifest_path = dir.path().join("cabin.toml");
    let mut manifest = fs::read_to_string(&manifest_path).unwrap();
    manifest.push_str("\n[profile]\ncxxflags = [\"-std=c++14\"]\n");
    assert_fs::fixture::ChildPath::new(&manifest_path)
        .write_str(&manifest)
        .unwrap();
    let value = run_metadata(&manifest_path);
    let language = &package_in(&value, "demo")["configuration"]["language"];
    assert_eq!(language["cxx"]["standard"], "c++17");
    assert_eq!(language["cxx"]["source"], "package");
}

#[test]
fn registry_target_level_standard_applies_at_the_consumer() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();

    // Publish a library whose *target* declares the implementation
    // standard; the extracted manifest must carry it to the consumer.
    let pkg_root = dir.path().join("pkg");
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "fmt"
version = "10.2.1"

[target.fmt]
type = "library"
sources = ["src/fmt.cc"]
include-dirs = ["include"]
cxx-standard = "c++14"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("include/fmt.h"))
        .write_str("#pragma once\nint fmt_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join("src/fmt.cc"))
        .write_str("#include \"fmt.h\"\nint fmt_value() { return 41; }\n")
        .unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    let app_root = dir.path().join("app");
    assert_fs::fixture::ChildPath::new(app_root.join("cabin.toml"))
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
    assert_fs::fixture::ChildPath::new(app_root.join("src/main.cc"))
        .write_str("#include \"fmt.h\"\nint main() { return fmt_value() == 41 ? 0 : 1; }\n")
        .unwrap();
    cabin()
        .args(["build", "--manifest-path"])
        .arg(app_root.join("cabin.toml"))
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--build-dir")
        .arg(app_root.join("build"))
        .assert()
        .success();
    let ninja = fs::read_to_string(app_root.join("build/dev/build.ninja")).unwrap();
    // The dependency compiles with its target-declared c++14; the
    // consumer keeps the built-in c++17.
    assert!(ninja.contains(&host_std_flag("c++14")), "{ninja}");
    assert!(ninja.contains(&host_std_flag("c++17")), "{ninja}");
}
