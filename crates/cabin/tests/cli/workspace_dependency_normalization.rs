//! End-to-end coverage for workspace-dependency archive
//! normalization: `dep = { workspace = true }` markers are rewritten
//! to the workspace root's literal requirement strings at
//! `cabin package` time, so published packages are self-contained.
//! See docs/workspaces.md and docs/package-format.md.

use super::*;

fn write_workspace_root(dir: &Path, workspace_dep_tables: &str) {
    assert_fs::fixture::ChildPath::new(dir.join("cabin.toml"))
        .write_str(&format!(
            r#"[workspace]
members = ["packages/*"]

{workspace_dep_tables}
"#
        ))
        .unwrap();
}

/// One executable member under `packages/<name>` with the given
/// dependency tables between `[package]` and `[target.<name>]`.
fn write_member(dir: &Path, name: &str, dependency_tables: &str) {
    let base = dir.join("packages").join(name);
    assert_fs::fixture::ChildPath::new(base.join("cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "{name}"
version = "0.1.0"

{dependency_tables}

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

#[test]
fn packaged_member_with_workspace_dep_matches_literal_twin() {
    let ws = TempDir::new().unwrap();
    // The same dependency name at different requirements per kind
    // table pins the table → kind mapping: a swapped mapping would
    // archive the wrong string and break byte-equality. `cabin
    // package` never resolves dependencies, so neither requirement
    // needs to exist in any index.
    write_workspace_root(
        ws.path(),
        r#"[workspace.dependencies]
fmt = ">=10 <11"

[workspace.dev-dependencies]
fmt = "^99""#,
    );
    write_member(
        ws.path(),
        "app",
        r#"[dependencies]
fmt = { workspace = true }

[dev-dependencies]
fmt = { workspace = true }"#,
    );
    let inherited_dist = ws.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(ws.path().join("cabin.toml"))
        .args(["--package", "app", "--output-dir"])
        .arg(&inherited_dist)
        .assert()
        .success();

    // A standalone twin from the same member template, spelling the
    // requirements literally in the slots the markers occupied: the
    // archive normalization must make both packagings byte-identical.
    let twin = TempDir::new().unwrap();
    write_member(
        twin.path(),
        "app",
        r#"[dependencies]
fmt = ">=10 <11"

[dev-dependencies]
fmt = "^99""#,
    );
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
fn published_member_with_workspace_dep_resolves_at_foreign_consumer() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();

    // A real library package the inherited requirement points at.
    let core_root = dir.path().join("core");
    assert_fs::fixture::ChildPath::new(core_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "core"
version = "1.0.0"

[target.core]
type = "library"
sources = ["src/core.cc"]
include-dirs = ["include"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(core_root.join("include/core.h"))
        .write_str("#pragma once\nint core_value();\n")
        .unwrap();
    assert_fs::fixture::ChildPath::new(core_root.join("src/core.cc"))
        .write_str("#include \"core.h\"\nint core_value() { return 42; }\n")
        .unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(core_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .success();

    // Publisher workspace: the member inherits its `core`
    // requirement from the root.
    let publisher = dir.path().join("publisher");
    write_workspace_root(
        &publisher,
        r#"[workspace.dependencies]
core = "^1""#,
    );
    let app_root = publisher.join("packages/app");
    assert_fs::fixture::ChildPath::new(app_root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
core = { workspace = true }

[target.app]
type = "executable"
sources = ["src/main.cc"]
deps = ["core"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(app_root.join("src/main.cc"))
        .write_str("#include \"core.h\"\nint main() { return core_value() == 42 ? 0 : 1; }\n")
        .unwrap();
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(publisher.join("cabin.toml"))
        .args(["--package", "app", "--registry-dir"])
        .arg(&registry)
        .assert()
        .success();

    // A foreign consumer whose own workspace table contradicts the
    // publisher's: the archived manifest carries its own literal
    // `^1` requirement and never consults the consumer's table, so
    // `core` still resolves at 1.x.
    let consumer = dir.path().join("consumer");
    write_workspace_root(
        &consumer,
        r#"[workspace.dependencies]
core = "=99.0.0""#,
    );
    write_member(
        &consumer,
        "demo",
        r#"[dependencies]
app = "=0.1.0""#,
    );
    cabin()
        .args(["build", "--manifest-path"])
        .arg(consumer.join("cabin.toml"))
        .arg("--index-path")
        .arg(&registry)
        .arg("--cache-dir")
        .arg(dir.path().join("cache"))
        .arg("--build-dir")
        .arg(consumer.join("build"))
        .assert()
        .success();
    let lock = fs::read_to_string(consumer.join("cabin.lock")).unwrap();
    assert!(lock.contains(r#"name = "app""#), "lockfile: {lock}");
    // The renderer emits `name` / `version` as adjacent LF-separated
    // lines, so this pins core specifically at the publisher's 1.x.
    assert!(
        lock.contains("name = \"core\"\nversion = \"1.0.0\""),
        "lockfile: {lock}"
    );
    assert!(
        !lock.contains("99.0.0"),
        "the consumer's contradicting workspace entry must stay inert: {lock}"
    );
}
