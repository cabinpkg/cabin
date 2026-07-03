use super::*;

/// Workspace with `app` (C++ executable) plus an unrelated
/// member `b` that declares a versioned dep. `cabin build -p
/// app` must not require an index in this case.
fn write_workspace_with_app_and_versioned_unrelated(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
cxx-standard = "c++17"

[target.app]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/app/src/main.cc"))
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("packages/b/cabin.toml"))
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
}

#[test]
fn build_p_app_does_not_require_index_when_unrelated_member_has_versioned_dep() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_workspace_with_app_and_versioned_unrelated(dir.path());
    let build_dir = dir.path().join("build");
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "app", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    assert!(
        build_dir
            .join("dev/packages/app")
            .join(host_exe("app"))
            .is_file()
    );
}

#[test]
fn fetch_p_app_does_not_require_index_when_unrelated_member_has_versioned_dep() {
    let dir = TempDir::new().unwrap();
    write_workspace_with_app_and_versioned_unrelated(dir.path());
    cabin()
        .args(["fetch", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "app"])
        .assert()
        .success();
}

/// Path-dep transitive registry deps reach the resolver when
/// the user selects only `app`.
#[test]
fn resolve_p_app_includes_registry_deps_from_path_dep_lib() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
lib = { path = "../lib" }
"#,
        )
        .unwrap();
    dir.child("packages/lib/cabin.toml")
        .write_str(
            r#"[package]
name = "lib"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
                }
            }"#)

            .unwrap();
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "app", "--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let lock = fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
    assert!(lock.contains(r#"name = "fmt""#), "lockfile: {lock}");
}

/// Feature CLI requests apply only to selected packages.
/// Unrelated packages that do not declare the requested
/// feature must not fail the build.
#[test]
fn features_apply_only_to_selected_packages() {
    require_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    // a declares ssl; b does not.  Selecting -p a --features ssl
    // must succeed.
    dir.child("packages/a/cabin.toml")
        .write_str(
            r#"[package]
name = "a"
version = "0.1.0"
cxx-standard = "c++17"

[features]
ssl = []

[target.a]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("packages/a/src/main.cc")
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    dir.child("packages/b/cabin.toml")
        .write_str(
            r#"[package]
name = "b"
version = "0.1.0"
cxx-standard = "c++17"

[target.b]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("packages/b/src/main.cc")
        .write_str(HELLO_MAIN_CC)
        .unwrap();
    cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "a", "--features", "ssl", "--build-dir"])
        .arg(dir.path().join("build"))
        .assert()
        .success();
}

/// `package` / `publish` in workspace context must see
/// `dep = { workspace = true }` resolved against
/// `[workspace.dependencies]`.  Otherwise the package metadata
/// would silently omit the dep.
#[test]
fn package_resolves_workspace_dep_inheritance() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/app"]

[workspace.dependencies]
fmt = ">=10 <11"
"#,
        )
        .unwrap();
    dir.child("packages/app/cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
        )
        .unwrap();
    let dist = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["-p", "app", "--output-dir"])
        .arg(&dist)
        .assert()
        .success();
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dist.join("app-0.1.0.json")).unwrap()).unwrap();
    assert!(
        json["dependencies"]["fmt"].is_string(),
        "fmt missing from package metadata: {json}"
    );
}

/// Standalone `cabin package` against a manifest with
/// `dep = { workspace = true }` must error rather than
/// silently drop the dep.
#[test]
fn package_standalone_workspace_dep_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = { workspace = true }
"#,
        )
        .unwrap();
    let dist = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--output-dir")
        .arg(&dist)
        .assert()
        .failure()
        .stderr(predicate::str::contains("workspace = true"));
}

/// Registry path safety.  A package called `../evil` must not
/// be allowed to publish.
#[test]
fn publish_unsafe_package_name_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "../evil"
version = "0.1.0"
"#,
        )
        .unwrap();
    let registry = dir.path().join("registry");
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .assert()
        .failure();
    // The cabin-package layer rejects the name before any
    // registry write happens.
}

/// `--exclude` requires `--workspace` or
/// `--default-members`.  Using it with the no-flag default
/// errors clearly.
#[test]
fn exclude_without_workspace_errors() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["packages/*"]
"#,
        )
        .unwrap();
    dir.child("packages/a/cabin.toml")
        .write_str("[package]\nname = \"a\"\nversion = \"0.1.0\"\n")
        .unwrap();
    dir.child("packages/b/cabin.toml")
        .write_str("[package]\nname = \"b\"\nversion = \"0.1.0\"\n")
        .unwrap();
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--exclude", "b"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("--workspace")
                .or(predicate::str::contains("--default-members")),
        );
}

/// A nested workspace invoked from inside is rejected by the
/// strict nested-workspace discovery rule: discovery itself
/// errors when it finds two `[workspace]` manifests above the
/// starting path, naming both roots.
#[test]
fn nested_workspace_from_inside_rejected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[workspace]
members = ["nested"]
"#,
        )
        .unwrap();
    dir.child("nested/cabin.toml")
        .write_str(
            r#"[workspace]
members = []
"#,
        )
        .unwrap();
    cabin()
        .current_dir(dir.path().join("nested"))
        .args(["metadata"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nested workspace"));
}

/// `cabin update --package <name>` keeps its
/// dep-targeted-update meaning.  Unknown name reports the
/// "not a versioned dependency" error consistently.
#[test]
fn update_package_back_compat() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = ">=10.0.0 <11.0.0"
"#,
        )
        .unwrap();
    dir.child("index/fmt.json")

            .write_str(r#"{
                "schema": 1,
                "name": "fmt",
                "versions": {
                    "10.2.1": { "dependencies": {}, "yanked": false, "checksum": "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
                }
            }"#)

            .unwrap();
    cabin()
        .args(["update", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--package", "missing", "--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "not a direct versioned dependency",
        ));
}
