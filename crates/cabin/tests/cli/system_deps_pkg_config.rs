use super::*;
use std::path::PathBuf;

/// Build with the `test-fake-pkg-config` feature on
/// `cabin-system-deps`.
fn fake_pkg_config_path() -> PathBuf {
    workspace_test_bin("cabin-system-deps-fake-pkg-config")
}

/// Pre-built TempDir holding fixture JSON files for the fake
/// pkg-config.  Tests call `.write` to publish a module's
/// metadata, then point `CABIN_FAKE_PKG_CONFIG_FIXTURES` at
/// the directory path through the command env.
pub(super) struct Fixtures {
    dir: TempDir,
}

impl Fixtures {
    pub(super) fn new() -> Self {
        Self {
            dir: TempDir::new().expect("tempdir"),
        }
    }

    pub(super) fn write(&self, name: &str, body: &str) {
        assert_fs::fixture::ChildPath::new(self.dir.path().join(format!("{name}.json")))
            .write_str(body)
            .unwrap();
    }

    pub(super) fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// Build a `cabin` command pre-loaded with the fake
/// pkg-config and a freshly-created fixture directory.  The
/// caller publishes fixtures via the returned `Fixtures`
/// handle.
pub(super) fn cabin_with_fake_pkg_config(fixtures: &Fixtures) -> Command {
    let mut cmd = cabin();
    cmd.env("CABIN_PKG_CONFIG", fake_pkg_config_path());
    cmd.env("CABIN_FAKE_PKG_CONFIG_FIXTURES", fixtures.path());
    // Poison the environment with a registry credential: the fake
    // pkg-config hard-fails when it sees the variable, so every test
    // in this module enforces the child-env scrub.
    cmd.env("CABIN_REGISTRY_TOKEN", "cabin_secretToken1234");
    cmd
}

/// Manifest declaring exactly one system dependency.  Tests
/// override the requirement / required field by formatting
/// it as needed.
fn manifest_with_system_dep(version: &str, required_clause: &str) -> String {
    format!(
        "[package]\nname = \"hello\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.hello]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n\n[dependencies]\nzlib = {{ version = \"{version}\", system = true{required_clause} }}\n",
    )
}

fn write_hello_main(root: &Path) {
    assert_fs::fixture::ChildPath::new(root.join("src/main.cc"))
        .write_str(HELLO_MAIN_CC)
        .unwrap();
}

#[test]
fn build_succeeds_with_no_system_deps_even_when_pkg_config_missing() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml").write_str(VALID_MANIFEST).unwrap();
    write_hello_main(dir.path());

    let mut cmd = cabin();
    // Point CABIN_PKG_CONFIG at a path that does not exist;
    // because the manifest has no `system = true` deps,
    // Cabin must not try to spawn pkg-config.
    cmd.env("CABIN_PKG_CONFIG", dir.path().join("missing-pkg-config"));
    // metadata exercises the same code path without
    // requiring a real toolchain.
    cmd.current_dir(dir.path())
        .arg("metadata")
        .assert()
        .success();
}

#[test]
fn metadata_reflects_pkg_config_cflags_in_build_flags_per_package() {
    let fixtures = Fixtures::new();
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include -DZLIB_CONST",
                "libs": "-L/opt/zlib/lib -lz"
            }"#,
    );
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_with_system_dep("", ""))
        .unwrap();
    write_hello_main(dir.path());

    let assertion = cabin_with_fake_pkg_config(&fixtures)
        .current_dir(dir.path())
        .arg("metadata")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let view: serde_json::Value =
        serde_json::from_str(&stdout).expect("metadata output should be JSON");
    let pkg = package_build_flags(&view);
    // pkg-config include dirs are third-party search paths, so they
    // land in the *system* include bucket (`-isystem`), not the
    // plain `-I` list.
    let system_includes: Vec<String> = pkg["system_include_dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        system_includes.iter().any(|p| p == "/opt/zlib/include"),
        "system include dirs must reflect pkg-config -I path: {system_includes:?}",
    );
    assert!(
        !pkg["include_dirs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some("/opt/zlib/include")),
        "pkg-config -I path must not stay in the plain include bucket",
    );
    let extra_compile: Vec<String> = pkg["extra_compile_args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        extra_compile.contains(&"-DZLIB_CONST".to_owned()),
        "extra compile args must carry non-include cflags: {extra_compile:?}",
    );
    let extra_link: Vec<String> = pkg["ldflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        extra_link,
        vec!["-L/opt/zlib/lib".to_owned(), "-lz".to_owned()],
        "pkg-config --libs must reach the planner verbatim and in order",
    );
}

/// Lookup helper: `cabin metadata`'s build flags live under
/// `toolchain.build_flags_per_package.<name>`.  Returns the
/// first package's block; only one package is declared in
/// these fixtures.
fn package_build_flags(view: &serde_json::Value) -> &serde_json::Value {
    let per_package = view["toolchain"]["build_flags_per_package"]
        .as_object()
        .expect("toolchain.build_flags_per_package object");
    per_package
        .values()
        .next()
        .expect("at least one package with build flags")
}

#[test]
fn metadata_fails_when_system_dep_is_missing() {
    let fixtures = Fixtures::new();
    // No fixture published; fake pkg-config will report
    // "not found".
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_with_system_dep("", ""))
        .unwrap();
    write_hello_main(dir.path());

    let assertion = cabin_with_fake_pkg_config(&fixtures)
        .current_dir(dir.path())
        .arg("metadata")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("zlib"),
        "diagnostic should name the missing dep: {stderr}",
    );
    assert!(
        stderr.contains("not found"),
        "diagnostic should describe the failure mode: {stderr}",
    );
}

#[test]
fn metadata_fails_when_system_dep_version_unsatisfied() {
    let fixtures = Fixtures::new();
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.1.0",
                "cflags": "",
                "libs": "-lz"
            }"#,
    );
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_with_system_dep(">=2", ""))
        .unwrap();
    write_hello_main(dir.path());

    let assertion = cabin_with_fake_pkg_config(&fixtures)
        .current_dir(dir.path())
        .arg("metadata")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("zlib"),
        "diagnostic should name the dep: {stderr}",
    );
    assert!(
        stderr.contains(">=2"),
        "diagnostic should quote the requirement: {stderr}",
    );
    assert!(
        stderr.contains("1.1.0"),
        "diagnostic should report the installed version: {stderr}",
    );
}

#[test]
fn metadata_fails_when_pkg_config_missing_and_system_dep_declared() {
    let fixtures = Fixtures::new();
    let dir = TempDir::new().unwrap();
    let missing_pkg_config = dir.path().join("nope-pkg-config");
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_with_system_dep("", ""))
        .unwrap();
    write_hello_main(dir.path());

    let mut cmd = cabin();
    cmd.env("CABIN_PKG_CONFIG", &missing_pkg_config);
    cmd.env("CABIN_FAKE_PKG_CONFIG_FIXTURES", fixtures.path());
    let assertion = cmd
        .current_dir(dir.path())
        .arg("metadata")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("not found"),
        "diagnostic should mention `not found`: {stderr}",
    );
    assert!(
        stderr.contains("CABIN_PKG_CONFIG"),
        "diagnostic should mention the override env var: {stderr}",
    );
}

#[test]
fn cabin_pkg_config_env_var_overrides_executable() {
    // A fixture-publishing test that depends on the env var
    // being honored.  If the env var were ignored, the test
    // would fail to spawn pkg-config and metadata would error.
    let fixtures = Fixtures::new();
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include",
                "libs": "-lz"
            }"#,
    );
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_with_system_dep("", ""))
        .unwrap();
    write_hello_main(dir.path());

    cabin_with_fake_pkg_config(&fixtures)
        .current_dir(dir.path())
        .arg("metadata")
        .assert()
        .success();
}

#[test]
fn manifest_rejects_required_field_on_system_dep() {
    // System dependencies are unconditionally required.  The
    // CLI must reject any attempt to declare `required = …`
    // with a diagnostic that explicitly names the offending
    // field - the snippet alone is too weak because the source
    // line happens to contain the field name regardless.
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_with_system_dep(">=1", ", required = false"))
        .unwrap();
    write_hello_main(dir.path());

    let assertion = cabin()
        .current_dir(dir.path())
        .arg("metadata")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("unknown field `required`"),
        "diagnostic should call out the unknown field by name: {stderr}",
    );
}

#[test]
fn build_compile_commands_carry_include_paths_from_pkg_config() {
    require_cxx_build_tools();
    let fixtures = Fixtures::new();
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include -DZLIB_CONST",
                "libs": "-L/opt/zlib/lib -lz"
            }"#,
    );
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_with_system_dep("", ""))
        .unwrap();
    write_hello_main(dir.path());

    // The default Windows toolchain is MSVC, and Cabin rejects
    // `system = true` dependencies under MSVC: pkg-config emits
    // GNU-style flags (`-I`, `-L`, `-lz`) that `cl` / `link` cannot
    // consume, and on Windows the `.pc` files reference the MinGW
    // ABI.  There is nothing to propagate, so the build is refused
    // before any probe runs - assert the actionable diagnostic
    // instead.  A GNU/Clang toolchain (the non-Windows default) flows
    // the discovered flags through to the compile and link commands.
    if cfg!(windows) {
        let assertion = cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("build")
            .assert()
            .failure();
        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
        assert!(
            stderr.contains("not supported with an MSVC toolchain"),
            "MSVC build must reject system dependencies with a clear diagnostic: {stderr}",
        );
        return;
    }

    cabin_with_fake_pkg_config(&fixtures)
        .current_dir(dir.path())
        .arg("build")
        .assert()
        .success();

    let ccdb_path = dir.path().join("build/dev/compile_commands.json");
    let ccdb = std::fs::read_to_string(&ccdb_path).expect("compile_commands.json");
    // pkg-config include dirs are system search paths, emitted as two
    // argv tokens (`-isystem` followed by the path), so assert the
    // flag and the path separately rather than requiring a fixed
    // adjacency.
    assert!(
        ccdb.contains("-isystem") && ccdb.contains("opt/zlib/include"),
        "compile_commands.json must carry the pkg-config include as a system dir: {ccdb}",
    );
    assert!(
        ccdb.contains("-DZLIB_CONST"),
        "compile_commands.json must carry pkg-config -D: {ccdb}",
    );

    let ninja_path = dir.path().join("build/dev/build.ninja");
    let ninja = std::fs::read_to_string(&ninja_path).expect("build.ninja");
    assert!(
        ninja.contains("-lz"),
        "build.ninja link command must carry pkg-config -l: {ninja}",
    );
    assert!(
        ninja.contains("-L/opt/zlib/lib"),
        "build.ninja link command must carry pkg-config -L: {ninja}",
    );
}

#[test]
fn fingerprint_moves_when_pkg_config_flags_change() {
    let fixtures = Fixtures::new();
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include",
                "libs": "-lz"
            }"#,
    );
    let dir = TempDir::new().unwrap();
    // The metadata view only emits `configuration` (and
    // hence the fingerprint) when the package declares at
    // least one feature.  Declare a trivial feature so the
    // fingerprint surface is populated.
    dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.hello]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n\n[features]\ndefault = []\nflag-a = []\n\n[dependencies]\nzlib = { version = \"\", system = true }\n")

            .unwrap();
    write_hello_main(dir.path());

    let stdout1 = String::from_utf8_lossy(
        &cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .to_string();
    let view1: serde_json::Value = serde_json::from_str(&stdout1).unwrap();
    let fp1 = find_fingerprint(&view1);

    // Republish with different libs - the discovered link
    // args change, so the fingerprint must move.
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include",
                "libs": "-lz -lother"
            }"#,
    );
    let stdout2 = String::from_utf8_lossy(
        &cabin_with_fake_pkg_config(&fixtures)
            .current_dir(dir.path())
            .arg("metadata")
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .to_string();
    let view2: serde_json::Value = serde_json::from_str(&stdout2).unwrap();
    let fp2 = find_fingerprint(&view2);

    assert_ne!(
        fp1, fp2,
        "fingerprint must move when discovered pkg-config flags change",
    );
}

/// Walk the metadata view looking for the first build-config
/// fingerprint.  Build-configurations live under
/// `configurations.<package>.fingerprint`; the value is a
/// hex string.  Robust against schema reshuffles.
fn find_fingerprint(value: &serde_json::Value) -> String {
    fn walk(v: &serde_json::Value) -> Option<String> {
        if let Some(map) = v.as_object() {
            if let Some(fp) = map.get("fingerprint").and_then(|f| f.as_str()) {
                return Some(fp.to_owned());
            }
            for child in map.values() {
                if let Some(found) = walk(child) {
                    return Some(found);
                }
            }
        }
        if let Some(arr) = v.as_array() {
            for item in arr {
                if let Some(found) = walk(item) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(value).expect("metadata view should expose a fingerprint")
}

#[test]
fn non_matching_target_conditional_system_dep_does_not_require_pkg_config() {
    // Declare a system dep gated on a condition that the
    // host platform cannot match.  Cabin must not spawn
    // pkg-config - and the integration test exercises that
    // by pointing `CABIN_PKG_CONFIG` at a non-existent path.
    let dir = TempDir::new().unwrap();
    let unreachable = dir.path().join("never-reached-pkg-config");
    dir.child("cabin.toml")

            .write_str("[package]\nname = \"hello\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.hello]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n\n[target.'cfg(os = \"none-such\")'.dependencies]\nzlib = { version = \"\", system = true }\n")

            .unwrap();
    write_hello_main(dir.path());

    let mut cmd = cabin();
    cmd.env("CABIN_PKG_CONFIG", &unreachable);
    cmd.current_dir(dir.path())
        .arg("metadata")
        .assert()
        .success();
}

#[test]
fn matching_target_conditional_system_dep_is_probed() {
    let fixtures = Fixtures::new();
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.2.13",
                "cflags": "-I/opt/zlib/include",
                "libs": "-lz"
            }"#,
    );
    let dir = TempDir::new().unwrap();
    let host_os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))

            .write_str(&format!(
                "[package]\nname = \"hello\"\nversion = \"0.1.0\"\ncxx-standard = \"c++17\"\n\n[target.hello]\ntype = \"executable\"\nsources = [\"src/main.cc\"]\n\n[target.'cfg(os = \"{host_os}\")'.dependencies]\nzlib = {{ version = \"\", system = true }}\n",
            ))

            .unwrap();
    write_hello_main(dir.path());

    let assertion = cabin_with_fake_pkg_config(&fixtures)
        .current_dir(dir.path())
        .arg("metadata")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let view: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let pkg = package_build_flags(&view);
    let includes: Vec<String> = pkg["system_include_dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert!(
        includes.iter().any(|p| p == "/opt/zlib/include"),
        "matching conditional system dep must contribute flags: {includes:?}",
    );
}

#[test]
fn verbose_mode_prints_probe_progress() {
    let fixtures = Fixtures::new();
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.2.13",
                "cflags": "",
                "libs": "-lz"
            }"#,
    );
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_with_system_dep("", ""))
        .unwrap();
    write_hello_main(dir.path());

    let assertion = cabin_with_fake_pkg_config(&fixtures)
        .current_dir(dir.path())
        .arg("-v")
        .arg("metadata")
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("probing"),
        "verbose stderr should mention probing: {stderr}",
    );
    assert!(
        stderr.contains("zlib"),
        "verbose stderr should mention the dep name: {stderr}",
    );
    assert!(
        stderr.contains("1.2.13"),
        "verbose stderr should mention the resolved version: {stderr}",
    );
}

#[test]
fn metadata_stdout_stays_clean_under_verbose_with_system_deps() {
    let fixtures = Fixtures::new();
    fixtures.write(
        "zlib",
        r#"{
                "version": "1.2.13",
                "cflags": "",
                "libs": "-lz"
            }"#,
    );
    let dir = TempDir::new().unwrap();
    assert_fs::fixture::ChildPath::new(dir.path().join("cabin.toml"))
        .write_str(&manifest_with_system_dep("", ""))
        .unwrap();
    write_hello_main(dir.path());

    let assertion = cabin_with_fake_pkg_config(&fixtures)
        .current_dir(dir.path())
        .arg("-v")
        .arg("metadata")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    // Stdout must still be parseable JSON - probe chatter
    // belongs on stderr only.
    let _view: serde_json::Value =
        serde_json::from_str(&stdout).expect("metadata stdout must remain valid JSON under -v");
}
