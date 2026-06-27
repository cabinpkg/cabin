//! End-to-end coverage for build profiles.  The tests exercise
//! the full pipeline: parser, resolver, build, metadata view,
//! and the per-profile output directory.

use super::*;

fn metadata_for_profile(dir: &Path, profile: &str) -> serde_json::Value {
    let assertion = cabin()
        .current_dir(dir)
        .args(["metadata", "--profile", profile])
        .assert()
        .success();
    serde_json::from_slice(&assertion.get_output().stdout).expect("metadata JSON")
}

fn package_ldflags<'a>(metadata: &'a serde_json::Value, package: &str) -> Vec<&'a str> {
    let per_package = metadata["toolchain"]["build_flags_per_package"]
        .as_object()
        .expect("build_flags_per_package object");
    let flags = per_package.get(package).unwrap_or_else(|| {
        assert_eq!(
            per_package.len(),
            1,
            "package {package:?} not found in {per_package:?}",
        );
        per_package.values().next().unwrap()
    });
    flags["ldflags"]
        .as_array()
        .expect("package ldflags array")
        .iter()
        .map(|flag| flag.as_str().expect("ldflag string"))
        .collect()
}

fn fingerprint_for_profile(dir: &Path, profile: &str) -> String {
    let assertion = cabin()
        .current_dir(dir)
        .args([
            "explain",
            "build-config",
            "demo",
            "--format",
            "json",
            "--profile",
            profile,
        ])
        .assert()
        .success();
    let value: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("explain JSON");
    value["configuration"]["fingerprint"]
        .as_str()
        .expect("package configuration fingerprint")
        .to_owned()
}

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
fn named_overlay_does_not_define_a_selectable_profile() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"

[target.'cfg(os = "linux")'.profile.release-lto]
cxxflags = ["-fno-semantic-interposition"]
"#,
        )
        .unwrap();
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--profile", "release-lto"])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("unknown profile `release-lto`")
            && stderr.contains("define it with `[profile.release-lto]` and an `inherits` field"),
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
    require_cxx_build_tools();
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
    require_cxx_build_tools();
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
    // must compose with **append** semantics - root → leaf -
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
    // [profile] → matching [target.'cfg()'.profile]
    // → inherited profile parent → selected profile
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
fn metadata_and_fingerprint_include_only_applicable_named_overlays() {
    let host_os = std::env::consts::OS;
    let other_os = if host_os == "linux" { "macos" } else { "linux" };
    let dir = TempDir::new().unwrap();
    let write_manifest = |overlay: Option<(&str, &str, &str)>| {
        let overlay = overlay.map_or_else(String::new, |(os, profile, flag)| {
            format!(
                r#"
[target.'cfg(os = "{os}")'.profile.{profile}]
ldflags = ["{flag}"]
"#
            )
        });
        dir.child("cabin.toml")
            .write_str(&format!(
                r#"[package]
name = "demo"
version = "0.1.0"

[profile]
ldflags = ["base"]

[profile.static]
inherits = "release"
{overlay}"#
            ))
            .unwrap();
    };

    write_manifest(Some((host_os, "release", "applicable-a")));
    let release = metadata_for_profile(dir.path(), "release");
    let static_profile = metadata_for_profile(dir.path(), "static");
    let dev = metadata_for_profile(dir.path(), "dev");
    assert_eq!(
        package_ldflags(&release, "demo"),
        vec!["base", "applicable-a"],
    );
    assert_eq!(
        package_ldflags(&static_profile, "demo"),
        vec!["base", "applicable-a"],
    );
    assert_eq!(package_ldflags(&dev, "demo"), vec!["base"]);
    let applicable_a_fingerprint = fingerprint_for_profile(dir.path(), "static");

    write_manifest(Some((host_os, "release", "applicable-b")));
    let applicable_b = metadata_for_profile(dir.path(), "static");
    assert_ne!(
        applicable_a_fingerprint,
        fingerprint_for_profile(dir.path(), "static"),
        "changing an inherited applicable overlay must change the fingerprint",
    );
    assert_eq!(
        package_ldflags(&applicable_b, "demo"),
        vec!["base", "applicable-b"],
    );

    write_manifest(None);
    let baseline = metadata_for_profile(dir.path(), "static");
    assert_eq!(package_ldflags(&baseline, "demo"), vec!["base"]);
    let baseline_fingerprint = fingerprint_for_profile(dir.path(), "static");
    assert_ne!(
        applicable_a_fingerprint, baseline_fingerprint,
        "adding an applicable overlay must change the fingerprint",
    );

    write_manifest(Some((other_os, "release", "target-mismatch")));
    let target_mismatch = metadata_for_profile(dir.path(), "static");
    assert_eq!(package_ldflags(&target_mismatch, "demo"), vec!["base"]);
    assert_eq!(
        baseline_fingerprint,
        fingerprint_for_profile(dir.path(), "static"),
        "a target-mismatched overlay must not affect the fingerprint",
    );

    write_manifest(Some((host_os, "unused", "profile-mismatch")));
    let profile_mismatch = metadata_for_profile(dir.path(), "static");
    assert_eq!(package_ldflags(&profile_mismatch, "demo"), vec!["base"]);
    assert_eq!(
        baseline_fingerprint,
        fingerprint_for_profile(dir.path(), "static"),
        "an unrelated overlay profile must not affect the fingerprint",
    );
}

#[test]
fn dependency_package_named_overlay_uses_workspace_profile_chain() {
    let host_os = std::env::consts::OS;
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
dep = { path = "dep" }

[profile.static]
inherits = "release"
"#,
        )
        .unwrap();
    dir.child("dep/cabin.toml")
        .write_str(&format!(
            r#"[package]
name = "dep"
version = "0.1.0"

[target.'cfg(os = "{host_os}")'.profile.release]
defines = ["DEPENDENCY_RELEASE"]
"#
        ))
        .unwrap();

    let metadata = metadata_for_profile(dir.path(), "static");
    let defines = metadata["toolchain"]["build_flags_per_package"]["dep"]["defines"]
        .as_array()
        .expect("dependency defines array");
    assert!(
        defines.iter().any(|value| value == "DEPENDENCY_RELEASE"),
        "dependency overlay should observe the workspace profile chain: {defines:?}",
    );
}

#[test]
fn linux_release_named_overlay_reaches_c_and_cxx_ninja_links() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "links"
version = "0.1.0"

[target.c_app]
type = "executable"
sources = ["src/main.c"]

[target.cxx_app]
type = "executable"
sources = ["src/main.cc"]

[profile.static]
inherits = "release"

[target.'cfg(os = "linux")'.profile.release]
ldflags = ["-static"]
"#,
        )
        .unwrap();
    dir.child("src/main.c")
        .write_str("int main(void) { return 0; }\n")
        .unwrap();
    dir.child("src/main.cc")
        .write_str("int main() { return 0; }\n")
        .unwrap();
    let build_dir = dir.path().join("build");
    let fake_ninja = workspace_test_bin("cabin-ninja-fake-ninja");

    for profile in ["dev", "release", "static"] {
        let mut command = cabin();
        command
            .current_dir(dir.path())
            .env("NINJA", &fake_ninja)
            .args(["build", "--build-dir"])
            .arg(&build_dir);
        if profile != "dev" {
            command.args(["--profile", profile]);
        }
        command.assert().success();
    }

    let dev_ninja = fs::read_to_string(build_dir.join("dev/build.ninja")).unwrap();
    let release_ninja = fs::read_to_string(build_dir.join("release/build.ninja")).unwrap();
    let static_ninja = fs::read_to_string(build_dir.join("static/build.ninja")).unwrap();
    assert!(!dev_ninja.contains("-static"), "{dev_ninja}");
    if cfg!(target_os = "linux") {
        assert!(
            release_ninja.matches("-static").count() >= 2,
            "release C and C++ links must both contain -static: {release_ninja}",
        );
        assert!(
            static_ninja.matches("-static").count() >= 2,
            "inherited static C and C++ links must both contain -static: {static_ninja}",
        );
    } else {
        assert!(!release_ninja.contains("-static"), "{release_ninja}");
        assert!(!static_ninja.contains("-static"), "{static_ninja}");
    }
}

#[test]
fn old_manifest_without_profile_tables_still_metadata_works() {
    // Regression: older manifests have no profile
    // tables.  Metadata view must still work and report the
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
