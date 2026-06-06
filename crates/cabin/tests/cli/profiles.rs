//! End-to-end coverage for build profiles. The tests exercise
//! the full pipeline: parser, resolver, build, metadata view,
//! and the per-profile output directory.

use super::*;

#[test]
fn metadata_reports_default_dev_profile_when_unselected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let selected = &value["profiles"]["selected"];
    assert_eq!(selected["name"].as_str(), Some("dev"));
    assert_eq!(selected["debug"].as_bool(), Some(true));
    assert_eq!(selected["opt_level"].as_str(), Some("0"));
    assert_eq!(selected["assertions"].as_bool(), Some(true));
    assert_eq!(selected["source"].as_str(), Some("builtin"));
    let available: Vec<&str> = value["profiles"]["available"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(available, vec!["dev", "release"]);
}

#[test]
fn metadata_reports_release_when_selected() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--profile", "release"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let selected = &value["profiles"]["selected"];
    assert_eq!(selected["name"].as_str(), Some("release"));
    assert_eq!(selected["opt_level"].as_str(), Some("3"));
    assert_eq!(selected["debug"].as_bool(), Some(false));
    assert_eq!(selected["assertions"].as_bool(), Some(false));
}

#[test]
fn metadata_reports_custom_profile_definitions_and_resolved_fields() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[profile.relwithdebinfo]
inherits = "release"
debug = true
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--profile", "relwithdebinfo"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let selected = &value["profiles"]["selected"];
    assert_eq!(selected["name"].as_str(), Some("relwithdebinfo"));
    assert_eq!(selected["opt_level"].as_str(), Some("3"));
    assert_eq!(selected["debug"].as_bool(), Some(true));
    assert_eq!(selected["source"].as_str(), Some("custom"));
    let chain: Vec<&str> = selected["inherits_chain"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(chain, vec!["release", "relwithdebinfo"]);
    let available: Vec<&str> = value["profiles"]["available"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(available, vec!["dev", "release", "relwithdebinfo"]);
    assert!(
        value["profiles"]["definitions"]["relwithdebinfo"].is_object(),
        "manifest definition preserved",
    );
}

#[test]
fn unknown_profile_errors_clearly_at_cli() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--profile", "fastdebug"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("unknown profile") && stderr.contains("fastdebug"),
        "expected unknown-profile error, got: {stderr}"
    );
}

#[test]
fn invalid_profile_name_errors_clearly_at_cli() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--profile", ".release"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("invalid profile name"),
        "expected invalid-profile-name error, got: {stderr}"
    );
}

#[test]
fn release_flag_and_profile_flag_conflict() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
    let assertion = cabin()
        .args(["build", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--release", "--profile", "release"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("conflicts"),
        "expected clap conflict error, got: {stderr}"
    );
}

#[test]
fn dev_and_release_use_distinct_output_directories() {
    skip_if!(
        !build_tools_available(),
        "dev_and_release_use_distinct_output_directories",
        "ninja or a C++ compiler is not available"
    );
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .args(["build", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    cabin()
        .current_dir(dir.path())
        .args(["build", "--release", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();

    assert!(build_dir.join("dev").join("build.ninja").is_file());
    assert!(build_dir.join("release").join("build.ninja").is_file());
    assert!(
        build_dir
            .join("dev")
            .join("packages")
            .join("hello")
            .join(host_exe("hello"))
            .is_file()
    );
    assert!(
        build_dir
            .join("release")
            .join("packages")
            .join("hello")
            .join(host_exe("hello"))
            .is_file()
    );

    let dev_cc =
        std::fs::read_to_string(build_dir.join("dev").join("compile_commands.json")).unwrap();
    let release_cc =
        std::fs::read_to_string(build_dir.join("release").join("compile_commands.json")).unwrap();
    assert!(dev_cc.contains(host_no_opt_flag()) && dev_cc.contains(host_debug_info_flag()));
    assert!(
        release_cc.contains(host_release_opt_flag())
            && release_cc.contains(host_define_ndebug_flag())
    );
}

#[test]
fn custom_profile_uses_its_own_output_directory() {
    skip_if!(
        !build_tools_available(),
        "custom_profile_uses_its_own_output_directory",
        "ninja or a C++ compiler is not available"
    );
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "hello"
version = "0.1.0"

[target.hello]
type = "executable"
sources = ["src/main.cc"]

[profile.relwithdebinfo]
inherits = "release"
debug = true
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();

    let build_dir = dir.path().join("build");
    cabin()
        .current_dir(dir.path())
        .args(["build", "--profile", "relwithdebinfo", "--build-dir"])
        .arg(&build_dir)
        .assert()
        .success();
    let cc = std::fs::read_to_string(
        build_dir
            .join("relwithdebinfo")
            .join("compile_commands.json"),
    )
    .unwrap();
    // Inherits release defaults (-O3 -DNDEBUG) but turns
    // debug info back on (-g).
    assert!(cc.contains(host_release_opt_flag()), "{cc}");
    assert!(cc.contains(host_define_ndebug_flag()), "{cc}");
    assert!(cc.contains(host_debug_info_flag()), "{cc}");
}

#[test]
fn metadata_build_config_appends_inherited_profile_flags() {
    // Top-level [profile] flags, the selected profile's
    // inherits chain, and the leaf [profile.<name>] block
    // must compose with **append** semantics — root → leaf —
    // so the resolved build configuration carries every
    // contributing layer in declaration order.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[profile]
cxxflags = ["-Wall"]

[profile.release]
cxxflags = ["-O3"]

[profile.bench]
inherits = "release"
cxxflags = ["-pg"]
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--profile", "bench"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let per_package = value["toolchain"]["build_flags_per_package"]
        .as_object()
        .expect("toolchain.build_flags_per_package object");
    let pkg = per_package
        .values()
        .next()
        .expect("at least one package with build flags");
    let cxx: Vec<String> = pkg["cxxflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        cxx,
        vec!["-Wall".to_owned(), "-O3".to_owned(), "-pg".to_owned(),],
        "[profile] → inherited parent → selected must append in that order",
    );
}

#[test]
fn metadata_build_config_orders_all_four_layers() {
    // Pin the full layer order documented in
    // `docs/profiles.md`:
    //   [profile] → matching [target.'cfg()'.profile]
    //             → inherited profile parent → selected profile
    // The conditional layer must land between the top-level
    // [profile] block and the profile inherits chain.
    let host_os = std::env::consts::OS;
    let dir = TempDir::new().unwrap();
    let manifest = format!(
        r#"[package]
name = "demo"
version = "0.1.0"

[profile]
cxxflags = ["-Wall"]

[target.'cfg(os = "{host_os}")'.profile]
cxxflags = ["-DCFG"]

[profile.release]
cxxflags = ["-O3"]

[profile.bench]
inherits = "release"
cxxflags = ["-pg"]
"#
    );
    dir.child("cabin.toml").write_str(&manifest).unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--profile", "bench"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let pkg = value["toolchain"]["build_flags_per_package"]
        .as_object()
        .expect("toolchain.build_flags_per_package object")
        .values()
        .next()
        .expect("at least one package with build flags");
    let cxx: Vec<String> = pkg["cxxflags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        cxx,
        vec![
            "-Wall".to_owned(),
            "-DCFG".to_owned(),
            "-O3".to_owned(),
            "-pg".to_owned(),
        ],
        "documented order: [profile] → cfg → inherited → selected",
    );
}

#[test]
fn old_manifest_without_profile_tables_still_metadata_works() {
    // Regression: older manifests have no profile
    // tables. Metadata view must still work and report the
    // built-in dev profile.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "old"
version = "0.1.0"

[dependencies]
fmt = ">=10"
"#,
        )
        .unwrap();
    cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .assert()
        .success();
}
