//! End-to-end coverage for target-dep reference resolution: the
//! bare-name shorthand (`deps = ["foo"]` means `foo:foo`) and the
//! hard error when a dependency package declares no same-named
//! target.

use super::*;

/// Workspace whose `toolkit` package exports a target named `kit`
/// (not `toolkit`), plus an `app` whose `deps` entry is set by the
/// caller.  `app_dep` is spliced into the array verbatim, so it can
/// be a quoted string (`"\"toolkit:kit\""`) or an inline table.
fn write_shorthand_workspace(root: &Path, app_dep: &str) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["packages/*"]
default-members = ["packages/app"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/toolkit/cabin.toml"))
        .write_str(
            r#"[package]
name = "toolkit"
version = "0.1.0"
cxx-standard = "c++17"

[target.kit]
type = "library"
sources = ["src/kit.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/toolkit/include/kit.h"))
        .write_str("#pragma once\nint kit_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/toolkit/src/kit.cc"))
        .write_str("#include \"kit.h\"\nint kit_value() { return 7; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
toolkit = {{ path = "../toolkit" }}

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = [{app_dep}]
"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/src/main.cc"))
        .write_str("#include \"kit.h\"\nint main() { return kit_value() == 7 ? 0 : 1; }\n")
        .unwrap();
}

#[test]
fn bare_name_without_same_name_target_fails_with_suggestion() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // `deps = ["toolkit"]` is shorthand for `toolkit:toolkit`,
    // which does not exist; the error must not silently pick `kit`
    // as a "default library" and must suggest the qualified form.
    write_shorthand_workspace(dir.path(), "\"toolkit\"");
    cabin()
        .args(["build", "-p", "app", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no library or header-only target named \"toolkit\"",
        ))
        .stderr(predicate::str::contains("toolkit:kit"));
}

#[test]
fn qualified_reference_links_differently_named_target() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_shorthand_workspace(dir.path(), "\"toolkit:kit\"");
    cabin()
        .args(["build", "-p", "app", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
}

#[test]
fn public_table_form_dep_builds_like_a_string_entry() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // `{ name = ..., public = true }` is declarative today: the
    // build must behave exactly like the string spelling.
    write_shorthand_workspace(dir.path(), r#"{ name = "toolkit:kit", public = true }"#);
    cabin()
        .args(["build", "-p", "app", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
}

#[test]
fn bare_name_shorthand_links_same_name_target() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // Same fixture, but the dependency exports a target named like
    // the package - the shorthand resolves to `util:util`.
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["packages/*"]
default-members = ["packages/app"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("packages/util/cabin.toml"))
        .write_str(
            r#"[package]
name = "util"
version = "0.1.0"
cxx-standard = "c++17"

[target.util]
type = "library"
sources = ["src/util.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("packages/util/include/util.h"))
        .write_str("#pragma once\nint util_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("packages/util/src/util.cc"))
        .write_str("#include \"util.h\"\nint util_value() { return 9; }\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("packages/app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[dependencies]
util = { path = "../util" }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["util"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("packages/app/src/main.cc"))
        .write_str("#include \"util.h\"\nint main() { return util_value() == 9 ? 0 : 1; }\n")
        .unwrap();
    cabin()
        .args(["build", "-p", "app", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--build-dir")
        .arg(dir.path().join("build"))
        .assert()
        .success();
}
