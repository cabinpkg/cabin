//! End-to-end coverage for workspace-inherited language standards:
//! `[workspace]`-level standard defaults that member packages opt
//! into per field with `<field> = { workspace = true }`.
//! See docs/language-standards.md and docs/workspaces.md.

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

fn write_workspace_root(dir: &Path, workspace_fields: &str) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str(&format!(
            r#"[workspace]
members = ["packages/*"]
{workspace_fields}
"#
        ))
        .unwrap();
}

/// One executable member under `packages/<name>` with the given
/// extra `[package]` fields.
fn write_member(dir: &Path, name: &str, package_fields: &str) {
    let base = dir.join("packages").join(name);
    assert_fs::fixture::ChildPath::new(base.join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
{package_fields}

[target.{name}]
type = "executable"
sources = ["src/main.cc"]
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(base.join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
}

fn build_at_root(dir: &Path) -> assert_cmd::assert::Assert {
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.join("build"))
        .assert()
}

fn root_ninja(dir: &Path) -> String {
    fs::read_to_string(dir.join("build/dev/build.ninja")).unwrap()
}

#[test]
fn opted_in_cxx_standard_reaches_member_ninja_and_compile_commands() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_workspace_root(dir.path(), "cxx-standard = \"c++14\"");
    write_member(dir.path(), "app", "cxx-standard = { workspace = true }");
    build_at_root(dir.path()).success();
    let ninja = root_ninja(dir.path());
    let inherited = host_std_flag("c++14");
    let other = host_std_flag("c++17");
    assert!(
        ninja.contains(&inherited),
        "expected the inherited `{inherited}` in build.ninja: {ninja}"
    );
    assert!(
        !ninja.contains(&other),
        "only the inherited standard may appear: {ninja}"
    );
    let ccdb = fs::read_to_string(dir.path().join("build/dev/compile_commands.json")).unwrap();
    assert!(
        ccdb.contains(&inherited),
        "expected the inherited `{inherited}` in compile_commands.json: {ccdb}"
    );
}

#[test]
fn opted_in_c_standard_reaches_member_ninja() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // c17 has a stable MSVC flag, so a single value exercises every
    // CI leg.
    write_workspace_root(dir.path(), "c-standard = \"c17\"");
    let base = dir.path().join("packages/capp");
    assert_fs::fixture::ChildPath::new(base.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "capp"
version = "0.1.0"
c-standard = { workspace = true }

[target.capp]
type = "executable"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(base.join("src/main.c"))
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    build_at_root(dir.path()).success();
    let ninja = root_ninja(dir.path());
    let inherited = host_std_flag("c17");
    let other = host_std_flag("c11");
    assert!(
        ninja.contains(&inherited),
        "expected the inherited `{inherited}` in build.ninja: {ninja}"
    );
    assert!(
        !ninja.contains(&other),
        "only the inherited standard may appear: {ninja}"
    );
}

#[test]
fn member_without_opt_in_gets_no_workspace_default() {
    let dir = TempDir::new().unwrap();
    // Workspace standard defaults are opt-in per field: a member
    // that never opts in inherits nothing, and with no built-in
    // default its compiled C++ has no standard - a load error, not
    // a silent fallback.
    write_workspace_root(dir.path(), "cxx-standard = \"c++14\"");
    write_member(dir.path(), "app", "");
    let assertion = build_at_root(dir.path()).failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("cxx-standard") && stderr.contains("workspace = true"),
        "expected the missing-standard diagnostic suggesting the opt-in, got: {stderr}"
    );
}

#[test]
fn target_literal_overrides_inherited_standard() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_workspace_root(dir.path(), "cxx-standard = \"c++14\"");
    let base = dir.path().join("packages/app");
    assert_fs::fixture::ChildPath::new(base.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = { workspace = true }

[target.app]
type = "executable"
sources = ["src/main.cc"]
cxx-standard = "c++20"
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(base.join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
        .unwrap();
    build_at_root(dir.path()).success();
    let ninja = root_ninja(dir.path());
    let target_literal = host_std_flag("c++20");
    let inherited = host_std_flag("c++14");
    assert!(
        ninja.contains(&target_literal),
        "expected the target-level `{target_literal}` in build.ninja: {ninja}"
    );
    assert!(
        !ninja.contains(&inherited),
        "the target literal must beat the inherited `{inherited}`: {ninja}"
    );
}

#[test]
fn metadata_reports_workspace_source() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_workspace_root(dir.path(), "cxx-standard = \"c++20\"");
    write_member(dir.path(), "app", "cxx-standard = { workspace = true }");
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let language = &package_in(&value, "app")["configuration"]["language"];
    assert_eq!(language["cxx"]["standard"], "c++20");
    assert_eq!(language["cxx"]["source"], "workspace");
    assert!(
        language.get("c").is_none(),
        "an undeclared language reports no entry: {language}"
    );
}

#[test]
fn opt_in_without_workspace_declaration_fails() {
    let dir = TempDir::new().unwrap();
    write_workspace_root(dir.path(), "cxx-standard = \"c++20\"");
    write_member(
        dir.path(),
        "app",
        "cxx-standard = \"c++17\"\ninterface-cxx-standard = { workspace = true }",
    );
    let assertion = build_at_root(dir.path()).failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("interface-cxx-standard") && stderr.contains("[workspace]"),
        "expected the unresolved-marker diagnostic naming the field and `[workspace]`, got: {stderr}"
    );
}

#[test]
fn marker_outside_workspace_fails() {
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(
            r#"[package]
name = "solo"
version = "0.1.0"
cxx-standard = { workspace = true }

[target.solo]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("src/main.cc"))
        .write_str("int main() { return 0; }\n")
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
        stderr.contains("cxx-standard") && stderr.contains("workspace"),
        "expected the standalone-marker diagnostic, got: {stderr}"
    );
}

#[test]
fn inherited_standard_conflicts_with_root_profile_std_flag() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // The root's `[profile.dev]` flag overlay applies workspace-wide
    // (the bare `[profile]` table is package-scoped, and a pure
    // workspace root is not a package), so the member's inherited
    // standard conflicts with the escape hatch exactly as a literal
    // declaration would.
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["packages/*"]
cxx-standard = "c++17"

[profile.dev]
cxxflags = ["-std=gnu++17"]
"#,
        )
        .unwrap();
    write_member(dir.path(), "app", "cxx-standard = { workspace = true }");
    let assertion = build_at_root(dir.path()).failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("cxx-standard") && stderr.contains("-std=gnu++17"),
        "expected the conflict diagnostic naming both sides, got: {stderr}"
    );
    assert!(
        stderr.contains("cabin::language::standard_flag_conflict"),
        "expected the stable diagnostic code, got: {stderr}"
    );
}

#[test]
fn consumer_below_inherited_interface_requirement_fails() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_workspace_root(dir.path(), "interface-cxx-standard = \"c++20\"");
    // The member's implementation is declared c++17, so `app`
    // (c++17) sits below `core`'s inherited c++20 interface.
    let base = dir.path().join("packages/demo");
    assert_fs::fixture::ChildPath::new(base.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"
interface-cxx-standard = { workspace = true }

[target.core]
type = "library"
sources = ["src/core.cc"]
include-dirs = ["include"]

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["core"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(base.join("include/core.h"))
        .write_str("#pragma once\nint core_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(base.join("src/core.cc"))
        .write_str("#include \"core.h\"\nint core_value() { return 42; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(base.join("src/main.cc"))
        .write_str("#include \"core.h\"\nint main() { return core_value() == 42 ? 0 : 1; }\n")
        .unwrap();
    let assertion = build_at_root(dir.path()).failure();
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
fn packaged_member_archive_matches_literal_twin() {
    let ws = TempDir::new().unwrap();
    write_workspace_root(ws.path(), "cxx-standard = \"c++20\"");
    write_member(ws.path(), "app", "cxx-standard = { workspace = true }");
    let inherited_dist = ws.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(ws.path().join("cabin.toml"))
        .args(["--package", "app", "--output-dir"])
        .arg(&inherited_dist)
        .assert()
        .success();

    // A standalone twin from the same member template, declaring the
    // standard in the same slot the marker occupied: the archive
    // normalization must make both packagings byte-identical.
    let twin = TempDir::new().unwrap();
    write_member(twin.path(), "app", "cxx-standard = \"c++20\"");
    let literal_dist = twin.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(twin.path().join("packages/app/cabin.toml"))
        .arg("--output-dir")
        .arg(&literal_dist)
        .assert()
        .success();

    for artifact in ["app-0.1.0.tar.gz", "app-0.1.0.json"] {
        let inherited = fs::read(inherited_dist.join(artifact)).unwrap();
        let literal = fs::read(literal_dist.join(artifact)).unwrap();
        assert_eq!(
            inherited, literal,
            "`{artifact}` must be byte-identical to the literal-declaring twin's"
        );
    }
}

#[test]
fn standalone_package_command_with_marker_fails() {
    let dir = TempDir::new().unwrap();
    write_member(dir.path(), "solo", "cxx-standard = { workspace = true }");
    let assertion = cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("packages/solo/cabin.toml"))
        .arg("--output-dir")
        .arg(dir.path().join("dist"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("cxx-standard") && stderr.contains("workspace"),
        "expected the standalone-marker diagnostic, got: {stderr}"
    );
}
