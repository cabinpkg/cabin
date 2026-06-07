//! End-to-end coverage for first-class C support.
//!
//! These tests exercise the C/C++ source-language model
//! across the manifest parser, build planner, Ninja
//! generator, and `cabin test`. Each test stages a small
//! temp package rather than depending on a fixed fixture
//! tree so failures point at the actual source / manifest
//! that broke.

use super::*;

fn write_c_only_library(dir: &Path) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "cdemo"
version = "0.1.0"

[target.cdemo]
type = "library"
sources = ["src/lib.c"]
include_dirs = ["include"]

[target.runner]
type = "executable"
sources = ["src/main.c"]
deps = ["cdemo"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("include/cdemo.h"))
        .write_str("#pragma once\nint cdemo(void);\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/lib.c"))
        .write_str("#include \"cdemo.h\"\nint cdemo(void) { return 7; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/main.c"))
        .write_str("#include \"cdemo.h\"\nint main(void) { return cdemo() == 7 ? 0 : 1; }\n")
        .unwrap();
}

fn write_mixed_library(dir: &Path) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "mixed"
version = "0.1.0"

[target.mixedlib]
type = "library"
sources = ["src/c_part.c", "src/cpp_part.cc"]
include_dirs = ["include"]

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["mixedlib"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("include/mixed.h"))
            .write_str("#pragma once\n#ifdef __cplusplus\nextern \"C\" {\n#endif\nint c_value(void);\n#ifdef __cplusplus\n}\n#endif\nint cpp_value();\n")
            .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/c_part.c"))
        .write_str("#include \"mixed.h\"\nint c_value(void) { return 21; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/cpp_part.cc"))
        .write_str("#include \"mixed.h\"\nint cpp_value() { return 21; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.join("src/main.cc"))
            .write_str("#include \"mixed.h\"\nint main() { return (c_value() + cpp_value()) == 42 ? 0 : 1; }\n")
            .unwrap();
}

#[test]
fn metadata_reports_target_kinds_for_c_only_project() {
    let dir = TempDir::new().unwrap();
    write_c_only_library(dir.path());
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let pkg = package_in(&value, "cdemo");
    let target_kinds: std::collections::BTreeMap<String, String> = pkg["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap().to_owned(),
                t["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        target_kinds.get("cdemo").map(String::as_str),
        Some("library")
    );
    assert_eq!(
        target_kinds.get("runner").map(String::as_str),
        Some("executable")
    );
}

#[test]
fn build_c_only_project_emits_c_compile_rule_and_c_link_driver() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_c_only_library(dir.path());
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
    // Only the C compile rule is exercised on a pure-C package.
    // The link line must use the C compiler driver — never `c++`
    // — so the binary stays off the C++ runtime.
    assert!(
        ninja.contains("c_compile"),
        "expected c_compile rule to be referenced: {ninja}"
    );
    assert!(
        !ninja
            .lines()
            .any(|l| l.contains("cxx_compile") && l.starts_with("build ")),
        "no cxx_compile build edges expected for pure-C package: {ninja}"
    );
    // Link command line: must include `cc` (or `clang` / `gcc`)
    // not `c++` / `clang++` / `g++`.
    let runner_target = host_path("/runner");
    let link_line = ninja
        .lines()
        .find(|l| l.contains("link_executable") && l.contains(&runner_target))
        .expect("link edge for runner");
    let next = ninja
        .lines()
        .skip_while(|l| *l != link_line)
        .nth(1)
        .expect("link edge has a command line");
    assert!(
        !next.contains("c++") && !next.contains("g++") && !next.contains("clang++"),
        "C-only link must not use a C++ driver, got: {next}"
    );
}

#[test]
fn build_mixed_project_uses_cxx_link_driver_when_any_object_is_cxx() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_mixed_library(dir.path());
    // Resolved C++ compiler path from metadata; the link edge's
    // program must equal it, which decouples the assertion from
    // how the host names its C++ driver (`c++` / `clang++` on
    // GNU hosts, `cl.exe` on MSVC where C and C++ share a
    // driver).
    let metadata = run_metadata(&dir.path().join("cabin.toml"));
    let cxx_path = metadata["toolchain"]["detected"]["cxx"]["path"]
        .as_str()
        .expect("metadata must report a resolved cxx path on this host");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
    // Both compile rules are exercised — one per language.
    assert!(
        ninja.contains("c_compile"),
        "expected a C compile edge for mixed package: {ninja}"
    );
    assert!(
        ninja.contains("cxx_compile"),
        "expected a C++ compile edge for mixed package: {ninja}"
    );
    // Link line must use the C++ driver because the closure
    // contains a C++ object.
    let link_cmds = compile_command_lines_for_rule(&ninja, "link_executable");
    assert_eq!(link_cmds.len(), 1, "expected one link edge");
    assert_eq!(
        program_from_command(&link_cmds[0]),
        cxx_path,
        "mixed link must use the resolved C++ driver, got: {}",
        link_cmds[0]
    );
}

#[test]
fn link_driver_path_matches_resolved_cc_path_for_pure_c_target() {
    // Structural variant of
    // `build_c_only_project_emits_c_compile_rule_and_c_link_driver`:
    // instead of pattern-matching driver-name substrings
    // (`cc` / `clang` / `gcc`) on the link command, this
    // test reads the resolved CC path from `cabin metadata`
    // and asserts the link command's first argument equals
    // it. Decouples the assertion from how the host names
    // its C compiler.
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_c_only_library(dir.path());
    // First, ask metadata for the resolved toolchain so the
    // assertion below knows the host's *actual* CC path.
    let metadata = run_metadata(&dir.path().join("cabin.toml"));
    // The resolved CC path lives under
    // `toolchain.detected.cc.path` — `toolchain.tools.cc`
    // carries the user-visible spec / source / kind, while
    // `toolchain.detected.cc.path` is the absolute path the
    // planner threads into the build graph.
    let cc_path = metadata["toolchain"]["detected"]["cc"]["path"]
        .as_str()
        .expect("metadata must report a resolved cc path on this host");
    // Then build, and inspect the link edge's command.
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
    let link_cmds = compile_command_lines_for_rule(&ninja, "link_executable");
    assert_eq!(link_cmds.len(), 1, "expected one link edge");
    assert_eq!(
        program_from_command(&link_cmds[0]),
        cc_path,
        "pure-C target must link with the resolved C compiler, got: {}",
        link_cmds[0]
    );
}

#[test]
fn cabin_test_runs_pure_c_test_executable() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "cdemo"
version = "0.1.0"

[target.cdemo]
type = "library"
sources = ["src/lib.c"]
include_dirs = ["include"]

[target.cdemo_test]
type = "test"
sources = ["tests/lib_test.c"]
deps = ["cdemo"]
"#,
        )
        .unwrap();
    dir.child("include/cdemo.h")
        .write_str("#pragma once\nint cdemo(void);\n")
        .unwrap();
    dir.child("src/lib.c")
        .write_str("#include \"cdemo.h\"\nint cdemo(void) { return 9; }\n")
        .unwrap();
    dir.child("tests/lib_test.c")
        .write_str("#include \"cdemo.h\"\nint main(void) { return cdemo() == 9 ? 0 : 1; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("test cdemo:cdemo_test ... ok"),
        "expected passing C test, got: {stdout}"
    );
}

#[test]
fn unrecognized_source_extension_is_rejected() {
    // Cabin rejects an unrecognized source extension during
    // build planning, before any compile is invoked.
    // Toolchain validation does run before the planner,
    // though, so a C++ compiler must be present on PATH.
    skip_if!(
        !build_tools_available(),
        "unrecognized_source_extension_is_rejected",
        "ninja or a C++ compiler is unavailable on PATH"
    );
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "broken"
version = "0.1.0"

[target.broken]
type = "library"
sources = ["src/file.txt"]
"#,
        )
        .unwrap();
    dir.child("src/file.txt")
        .write_str("not a source\n")
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
        stderr.contains("unrecognized extension"),
        "expected explicit extension diagnostic, got: {stderr}"
    );
    assert!(
        stderr.contains(".c") && stderr.contains(".cc"),
        "diagnostic should list supported extensions, got: {stderr}"
    );
}

#[test]
fn cflags_and_cxxflags_do_not_leak_across_languages() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "splitflags"
version = "0.1.0"

[profile]
cflags = ["-DCABIN_TEST_C_FLAG=1"]
cxxflags = ["-DCABIN_TEST_CXX_FLAG=1"]

[target.splitflags]
type = "library"
sources = ["src/c_part.c", "src/cpp_part.cc"]
"#,
        )
        .unwrap();
    dir.child("src/c_part.c")
        .write_str("int c_part_value(void) { return 0; }\n")
        .unwrap();
    dir.child("src/cpp_part.cc")
        .write_str("int cpp_part_value() { return 0; }\n")
        .unwrap();
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
    // Locate compile command lines by walking the build edges
    // and inspecting the rule name on each `build` line.
    // Anchoring on the rule (rather than on a hardcoded
    // standard flag like `-std=c11`) keeps the test stable if
    // the planner's default standard ever changes.
    let c_compile_lines = compile_command_lines_for_rule(&ninja, "c_compile");
    let cxx_compile_lines = compile_command_lines_for_rule(&ninja, "cxx_compile");
    assert!(
        !c_compile_lines.is_empty(),
        "expected at least one c_compile edge: {ninja}"
    );
    assert!(
        !cxx_compile_lines.is_empty(),
        "expected at least one cxx_compile edge: {ninja}"
    );
    for line in &c_compile_lines {
        assert!(
            line.contains("-DCABIN_TEST_C_FLAG=1"),
            "C compile must include the C-only define, got: {line}"
        );
        assert!(
            !line.contains("-DCABIN_TEST_CXX_FLAG=1"),
            "C-only define must NOT leak into the C++ compile, got: {line}"
        );
    }
    for line in &cxx_compile_lines {
        assert!(
            line.contains("-DCABIN_TEST_CXX_FLAG=1"),
            "C++ compile must include the C++-only define, got: {line}"
        );
        assert!(
            !line.contains("-DCABIN_TEST_C_FLAG=1"),
            "C++-only define must NOT leak into the C compile, got: {line}"
        );
    }
}

#[test]
fn cabin_test_runs_cpp_test_depending_on_c_library() {
    // A C++ test target consumes a pure-C library through the
    // ordinary `[target.X].deps` mechanism. The build planner
    // must compile each source through its language-appropriate
    // driver and link the test executable through the C++
    // driver.
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "interop"
version = "0.1.0"

[target.clib]
type = "library"
sources = ["src/clib.c"]
include_dirs = ["include"]

[target.cpp_test]
type = "test"
sources = ["tests/clib_test.cc"]
deps = ["clib"]
"#,
        )
        .unwrap();
    dir.child("include/clib.h")

            .write_str("#pragma once\n#ifdef __cplusplus\nextern \"C\" {\n#endif\nint c_value(void);\n#ifdef __cplusplus\n}\n#endif\n")

            .unwrap();
    dir.child("src/clib.c")
        .write_str("#include \"clib.h\"\nint c_value(void) { return 99; }\n")
        .unwrap();
    dir.child("tests/clib_test.cc")
        .write_str("#include \"clib.h\"\nint main() { return c_value() == 99 ? 0 : 1; }\n")
        .unwrap();
    // Resolved C++ compiler path; the link edge's program must
    // equal it. Decouples the assertion from the host's C++
    // driver naming (`c++` / `clang++` on GNU, `cl.exe` on
    // MSVC).
    let metadata = run_metadata(&dir.path().join("cabin.toml"));
    let cxx_path = metadata["toolchain"]["detected"]["cxx"]["path"]
        .as_str()
        .expect("metadata must report a resolved cxx path on this host")
        .to_owned();
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("test interop:cpp_test ... ok"),
        "expected passing C++ test that consumes a C library, got: {stdout}"
    );
    // Both compile rules must have been used and the link
    // must use a C++ driver because the test sources are C++.
    let ninja = fs::read_to_string(dir.path().join("build/dev/build.ninja")).unwrap();
    let c_compile_lines = compile_command_lines_for_rule(&ninja, "c_compile");
    let cxx_compile_lines = compile_command_lines_for_rule(&ninja, "cxx_compile");
    assert!(!c_compile_lines.is_empty(), "expected C compile edge");
    assert!(!cxx_compile_lines.is_empty(), "expected C++ compile edge");
    let link_cmds = compile_command_lines_for_rule(&ninja, "link_executable");
    assert_eq!(link_cmds.len(), 1, "expected one link edge");
    assert_eq!(
        program_from_command(&link_cmds[0]),
        cxx_path,
        "C++ test target must link with the resolved C++ driver, got: {}",
        link_cmds[0]
    );
}

#[test]
fn cabin_test_runs_mixed_c_and_cpp_tests_in_deterministic_order() {
    // A workspace with two test targets — one C, one C++ —
    // must run in `(package, target)` ascending order
    // regardless of TOML declaration order.
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "mixedtests"
version = "0.1.0"

[target.zz_cpp_test]
type = "test"
sources = ["tests/zz_cpp.cc"]

[target.aa_c_test]
type = "test"
sources = ["tests/aa_c.c"]
"#,
        )
        .unwrap();
    dir.child("tests/zz_cpp.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    dir.child("tests/aa_c.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    let assertion = cabin()
        .args(["test", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let aa_pos = stdout
        .find("test mixedtests:aa_c_test ... ok")
        .expect("aa_c_test result must be present");
    let zz_pos = stdout
        .find("test mixedtests:zz_cpp_test ... ok")
        .expect("zz_cpp_test result must be present");
    assert!(
        aa_pos < zz_pos,
        "tests must run in (package, target) ascending order regardless of language; got: {stdout}"
    );
}

#[test]
fn missing_c_compiler_yields_actionable_diagnostic() {
    // Cabin's toolchain resolver requires a C++ compiler
    // unconditionally; this test points `--cc` at a path
    // that does not exist so we can observe the
    // user-visible diagnostic without depending on the
    // host's `cc` / `clang` / `gcc` PATH state.
    skip_if!(
        !build_tools_available(),
        "missing_c_compiler_yields_actionable_diagnostic",
        "ninja or a C++ compiler is unavailable on PATH"
    );
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "needscc"
version = "0.1.0"

[target.needscc]
type = "library"
sources = ["src/lib.c"]
"#,
        )
        .unwrap();
    dir.child("src/lib.c")
        .write_str("int needscc_value(void) { return 0; }\n")
        .unwrap();
    // Build a non-existent path inside the temp dir so the
    // test does not depend on a hardcoded host-specific
    // path like `/this/path/does/not/exist/cc`. The path
    // simply must not resolve to an executable; nothing
    // here is invoked.
    let missing_cc = dir.path().join("missing-cc");
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .arg("--cc")
        .arg(&missing_cc)
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("C compiler") || stderr.contains("`cc`"),
        "expected error to mention the C compiler, got: {stderr}"
    );
}

/// Return the `command = ...` lines for every Ninja edge
/// whose rule equals `rule_name`. The returned slices are
/// owned `String`s for ergonomics. Anchoring on the rule
/// name decouples assertions from incidental command-line
/// content (standard flag, optimization level, etc.).
fn compile_command_lines_for_rule(ninja: &str, rule_name: &str) -> Vec<String> {
    let needle = format!(": {rule_name} ");
    let mut out: Vec<String> = Vec::new();
    let mut lines = ninja.lines();
    while let Some(line) = lines.next() {
        if !line.starts_with("build ") || !line.contains(&needle) {
            continue;
        }
        // The next non-blank line of an edge starts with
        // `  command = ...`. Walk forward until we find it,
        // stopping at the next blank line that terminates
        // the edge so a malformed `build.ninja` doesn't
        // silently hide regressions.
        for inner in lines.by_ref() {
            if inner.is_empty() {
                break;
            }
            if let Some(rest) = inner.strip_prefix("  command = ") {
                out.push(rest.to_owned());
                break;
            }
        }
    }
    out
}
