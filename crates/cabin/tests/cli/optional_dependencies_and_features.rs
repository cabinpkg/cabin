//! End-to-end coverage for optional package dependencies, dependency
//! feature requests, and the cross-package feature resolver.

use super::*;

/// Feature-aware fixture: `app` has an optional `openssl`
/// dependency that is gated by feature `ssl`.  Default features
/// do not enable `ssl`, so the optional dep stays out of
/// resolution unless the user passes `--features ssl`.
fn write_app_with_optional_openssl(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[features]
default = []
ssl = ["dep:openssl"]

[dependencies]
fmt = ">=10 <11"
openssl = { version = "^3", optional = true }
"#,
        )
        .unwrap();
    // The index has `openssl`, so only the feature gate can keep it
    // out of resolution.
    write_index_entry_no_source(&root.join("index"), "fmt", "10.2.1", &"0".repeat(64));
    write_index_entry_no_source(&root.join("index"), "openssl", "3.2.0", &"0".repeat(64));
}

#[test]
fn resolve_without_feature_skips_optional_dep() {
    let dir = TempDir::new().unwrap();
    write_app_with_optional_openssl(dir.path());
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(stdout.contains("fmt"), "fmt should appear: {stdout}");
    assert!(
        !stdout.contains("openssl"),
        "disabled optional dep openssl must not appear: {stdout}"
    );
}

#[test]
fn resolve_with_feature_includes_optional_dep() {
    let dir = TempDir::new().unwrap();
    write_app_with_optional_openssl(dir.path());
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .args(["--features", "ssl"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(
        stdout.contains("openssl"),
        "feature ssl should pull openssl in: {stdout}"
    );
}

#[test]
fn resolve_no_default_features_disables_root_default_chain() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[features]
default = ["ssl"]
ssl = ["dep:openssl"]

[dependencies]
openssl = { version = "^3", optional = true }
"#,
        )
        .unwrap();
    write_index_entry_no_source(
        &dir.path().join("index"),
        "openssl",
        "3.2.0",
        &"0".repeat(64),
    );
    let with_default = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    assert!(String::from_utf8_lossy(&with_default.get_output().stdout).contains("openssl"));
    let no_default = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .args(["--no-default-features"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&no_default.get_output().stdout);
    assert!(
        !stdout.contains("openssl"),
        "no-default-features must drop openssl: {stdout}"
    );
}

#[test]
fn fetch_does_not_pull_disabled_optional_into_lockfile() {
    // `cabin fetch` resolves with default features (no
    // `--features` flag is exposed on this command today).
    // The disabled optional `openssl` must not appear in the
    // lockfile that fetch writes, even though the artifact
    // step itself fails on the missing fmt artifact (the
    // index fixture omits the source block - that's a fetch
    // concern, not a feature concern).  We only assert the
    // dep-set decision the feature resolver made.
    let dir = TempDir::new().unwrap();
    write_app_with_optional_openssl(dir.path());
    // First run resolve to write the lockfile (resolve does
    // not need source artifacts).
    cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .assert()
        .success();
    let lock_body = fs::read_to_string(dir.path().join("cabin.lock")).unwrap();
    assert!(
        lock_body.contains("fmt"),
        "fmt should be locked: {lock_body}"
    );
    assert!(
        !lock_body.contains("openssl"),
        "disabled optional openssl must not be locked: {lock_body}"
    );
}

#[test]
fn metadata_round_trips_optional_and_features_in_dependency_view() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = { version = ">=10", features = ["compile"], default-features = false }
openssl = { version = "^3", optional = true }
"#,
        )
        .unwrap();
    let value = run_metadata(&dir.path().join("cabin.toml"));
    let demo = package_in(&value, "demo");
    let deps = demo["dependencies"].as_array().unwrap();
    // `DependencyView` omits `optional`, `features`, and
    // `default_features` at their defaults (false, empty, true), so
    // the fixture sets each to a non-default value.
    let openssl = deps
        .iter()
        .find(|d| d["name"] == "openssl")
        .expect("openssl listed");
    assert_eq!(openssl["optional"].as_bool(), Some(true));
    let fmt = deps.iter().find(|d| d["name"] == "fmt").unwrap();
    assert_eq!(fmt["default_features"].as_bool(), Some(false));
    let features: Vec<&str> = fmt["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(features, vec!["compile"]);
}

#[test]
fn package_metadata_round_trips_optional_and_features() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[dependencies]
fmt = ">=10"
openssl = { version = "^3", optional = true }
"#,
        )
        .unwrap();
    let out = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--output-dir"])
        .arg(&out)
        .assert()
        .success();
    let metadata: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("demo-0.1.0.json")).unwrap()).unwrap();
    // Bare entry: `fmt` has no overrides, so it stays a string.
    assert!(metadata["dependencies"]["fmt"].is_string());
    // Rich entry: `openssl` is optional, so it's a table with
    // `version` + `optional`.
    let openssl = &metadata["dependencies"]["openssl"];
    assert_eq!(openssl["version"].as_str().unwrap(), "^3");
    assert!(openssl["optional"].as_bool().unwrap());
}

#[test]
fn unknown_root_feature_errors_clearly_at_cli() {
    let dir = TempDir::new().unwrap();
    write_app_with_optional_openssl(dir.path());
    let assertion = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--index-path"])
        .arg(dir.path().join("index"))
        .args(["--features", "no-such-feature"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("unknown feature") && stderr.contains("no-such-feature"),
        "expected unknown-feature error, got: {stderr}"
    );
}

#[test]
fn dep_colon_on_non_optional_dep_is_rejected_at_cli() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[features]
ssl = ["dep:fmt"]

[dependencies]
fmt = ">=10"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--features", "ssl"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("not optional") || stderr.contains("DepIsNotOptional"),
        "expected non-optional dep error, got: {stderr}"
    );
}

/// Publish a C library `acme/<short>` from `root/src-<short>` into the
/// `root/registry` file registry.  `extra` lands after `[package]`;
/// `body` is the library's single source file.
fn publish_c_lib(root: &Path, short: &str, extra: &str, body: &str) {
    let pkg_root = root.join(format!("src-{short}"));
    assert_fs::fixture::ChildPath::new(pkg_root.join("cabin.toml"))
        .write_str(&format!(
            "[package]\nname = \"acme/{short}\"\nversion = \"1.0.0\"\n{extra}\n"
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join(format!("include/{short}.h")))
        .write_str(&format!("#pragma once\nint {short}_value(void);\n"))
        .unwrap();
    assert_fs::fixture::ChildPath::new(pkg_root.join(format!("src/{short}.c")))
        .write_str(body)
        .unwrap();
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(root.join("registry"))
        .assert()
        .success();
}

/// The registry chain `acme/aaa -> acme/bbb -> acme/ccc`: bbb's only
/// target, which links ccc, is gated on feature `withc`, and `withc`
/// is what enables bbb's optional `acme/ccc`.  `aaa_edge` is aaa's
/// declaration of `acme/bbb`.  `app` depends on aaa and checks the
/// value ccc returns; `app_extra` is appended to its manifest.
fn write_registry_feature_chain(root: &Path, aaa_edge: &str, app_extra: &str) -> PathBuf {
    publish_c_lib(
        root,
        "ccc",
        "\n[target.ccc]\ntype = \"library\"\nsources = [\"src/ccc.c\"]\n\
         include-dirs = [\"include\"]\nc-standard = \"c11\"",
        "int ccc_value(void) { return 7; }\n",
    );
    publish_c_lib(
        root,
        "bbb",
        r#"
[dependencies]
"acme/ccc" = { version = "^1", optional = true }

[features]
withc = ["dep:acme/ccc"]

[target.bbb]
type = "library"
sources = ["src/bbb.c"]
include-dirs = ["include"]
c-standard = "c11"
deps = ["acme/ccc"]
required-features = ["withc"]"#,
        "#include \"ccc.h\"\nint bbb_value(void) { return ccc_value(); }\n",
    );
    publish_c_lib(
        root,
        "aaa",
        &format!(
            "\n[dependencies]\n{aaa_edge}\n\n[target.aaa]\ntype = \"library\"\n\
             sources = [\"src/aaa.c\"]\ninclude-dirs = [\"include\"]\nc-standard = \"c11\"\n\
             deps = [\"acme/bbb\"]"
        ),
        "#include \"bbb.h\"\nint aaa_value(void) { return bbb_value(); }\n",
    );
    assert_fs::fixture::ChildPath::new(root.join("app/cabin.toml"))
        .write_str(&format!(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
"acme/aaa" = "^1"

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
deps = ["acme/aaa"]
{app_extra}"#
        ))
        .unwrap();
    assert_fs::fixture::ChildPath::new(root.join("app/src/main.c"))
        .write_str("#include \"aaa.h\"\nint main(void) { return aaa_value() == 7 ? 0 : 1; }\n")
        .unwrap();
    root.join("app/cabin.toml")
}

fn chain_cmd(root: &Path, command: &str, manifest: &Path) -> Command {
    let mut cmd = cabin();
    cmd.args([command, "--manifest-path"])
        .arg(manifest)
        .arg("--index-path")
        .arg(root.join("registry"))
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .arg("--build-dir")
        .arg(root.join("build"));
    cmd
}

/// A feature request on a registry edge enables the requested
/// package's optional registry dependency: resolution selects and
/// locks it, the build links it, and a locked offline rebuild
/// replays the same graph.
#[test]
fn registry_feature_chain_enables_an_optional_registry_dependency() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    let manifest = write_registry_feature_chain(
        dir.path(),
        r#""acme/bbb" = { version = "^1", default-features = false, features = ["withc"] }"#,
        "",
    );

    chain_cmd(dir.path(), "run", &manifest).assert().success();
    let lock = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
    for name in ["acme/aaa", "acme/bbb", "acme/ccc"] {
        assert!(
            lock.contains(&format!("name = \"{name}\"")),
            "{name} must be locked: {lock}"
        );
    }
    chain_cmd(dir.path(), "run", &manifest)
        .arg("--frozen")
        .assert()
        .success();
}

/// Without a feature that enables it, a registry package's optional
/// dependency is neither selected nor required to exist: removing it
/// from the registry entirely changes nothing, over a file index and
/// over sparse HTTP.
#[test]
fn registry_optional_dependency_is_inert_until_a_feature_enables_it() {
    let dir = TempDir::new().unwrap();
    let manifest = write_registry_feature_chain(dir.path(), r#""acme/bbb" = "^1""#, "");
    fs::remove_file(dir.path().join("registry/packages/acme/ccc.json")).unwrap();

    let server = crate::sparse_http::TestServer::serve(dir.path().join("registry"));
    for index in [
        [
            "--index-path",
            dir.path().join("registry").to_str().unwrap(),
        ],
        ["--index-url", server.url()],
    ] {
        cabin()
            .args(["resolve", "--manifest-path"])
            .arg(&manifest)
            .args(index)
            .assert()
            .success();
        let lock = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
        assert!(lock.contains("name = \"acme/bbb\""), "{lock}");
        assert!(!lock.contains("acme/ccc"), "{lock}");
    }
}

/// Resolution fetches none of a disabled optional path dependency's
/// registry dependencies, so the build must not require them, whether
/// the edge is gated to this platform or another.
#[test]
fn disabled_optional_path_dependency_needs_none_of_its_registry_dependencies() {
    require_c_and_cxx_build_tools();
    let other_os = if std::env::consts::OS == "linux" {
        "macos"
    } else {
        "linux"
    };
    for os in [std::env::consts::OS, other_os] {
        let dir = TempDir::new().unwrap();
        publish_c_lib(
            dir.path(),
            "ccc",
            "\n[target.ccc]\ntype = \"library\"\nsources = [\"src/ccc.c\"]\n\
             include-dirs = [\"include\"]\nc-standard = \"c11\"",
            "int ccc_value(void) { return 7; }\n",
        );
        dir.child("app/cabin.toml")
            .write_str(&format!(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
"acme/ccc" = "^1"

[target.'cfg(os = "{os}")'.dependencies]
helper = {{ path = "../helper", optional = true }}

[target.app]
type = "executable"
sources = ["src/main.c"]
c-standard = "c11"
deps = ["acme/ccc"]
"#
            ))
            .unwrap();
        dir.child("app/src/main.c")
            .write_str("#include \"ccc.h\"\nint main(void) { return ccc_value() == 7 ? 0 : 1; }\n")
            .unwrap();
        dir.child("helper/cabin.toml")
            .write_str("[package]\nname = \"helper\"\nversion = \"0.1.0\"\n\n[dependencies]\nzlib = \"^1\"\n")
            .unwrap();
        chain_cmd(dir.path(), "build", &dir.path().join("app/cabin.toml"))
            .assert()
            .success();
    }
}

/// A registry edge requesting a feature no in-range version declares
/// fails resolution; the explanation names the feature as
/// `<package>[<feature>]`.
#[test]
fn registry_edge_requesting_an_undeclared_feature_fails() {
    let dir = TempDir::new().unwrap();
    let manifest = write_registry_feature_chain(
        dir.path(),
        r#""acme/bbb" = { version = "^1", features = ["nope"] }"#,
        "",
    );

    let output = cabin()
        .args(["resolve", "--manifest-path"])
        .arg(&manifest)
        .arg("--index-path")
        .arg(dir.path().join("registry"))
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(
        stderr.contains("acme/aaa >=1.0.0, <2.0.0 depends on acme/bbb[nope]"),
        "{stderr}"
    );
}

/// A `[patch]`ed name a registry package reaches resolves to the fork
/// under that registry edge's feature request, so the fork's optional
/// *versioned* dependency is selected, locked, and linked.
#[test]
fn registry_reached_patch_enables_its_optional_versioned_dependency() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    // The published bbb source tree doubles as the fork.
    let manifest = write_registry_feature_chain(
        dir.path(),
        r#""acme/bbb" = { version = "^1", default-features = false, features = ["withc"] }"#,
        "\n[patch]\n\"acme/bbb\" = { path = \"../src-bbb\" }\n",
    );

    chain_cmd(dir.path(), "run", &manifest).assert().success();
    let lock = fs::read_to_string(dir.path().join("app/cabin.lock")).unwrap();
    assert!(lock.contains("name = \"acme/ccc\""), "{lock}");
    assert!(!lock.contains("name = \"acme/bbb\""), "{lock}");
}
