//! End-to-end coverage for `--sysroot`, the config `[sysroot]` table,
//! and Apple SDK discovery.

use super::*;

/// A C and a C++ executable that need the sysroot's headers and
/// libraries.
fn write_project(root: &Path) {
    let root = assert_fs::fixture::ChildPath::new(root);
    root.child("cabin.toml")
        .write_str(
            r#"[package]
name = "app"
version = "0.1.0"
c-standard = "c11"
cxx-standard = "c++17"

[target.capp]
type = "executable"
sources = ["src/main.c"]

[target.cxxapp]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    root.child("src/main.c")
        .write_str("#include <stdio.h>\nint main(void) { puts(\"c ok\"); return 0; }\n")
        .unwrap();
    root.child("src/main.cc")
        .write_str("#include <cstdio>\nint main() { std::puts(\"cxx ok\"); }\n")
        .unwrap();
}

fn run_both(dir: &Path, build: &Path) {
    for (exe, line) in [("capp", "c ok"), ("cxxapp", "cxx ok")] {
        let output = std::process::Command::new(build.join("packages/app").join(exe))
            .current_dir(dir)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), line);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn apple_sdk_choices_reach_the_commands_and_rebuild() {
    use std::os::unix::fs::symlink;
    require_c_and_cxx_build_tools();
    let sdk = std::process::Command::new("xcrun")
        .args(["--sdk", "macosx", "--show-sdk-path"])
        .output()
        .unwrap();
    let sdk = String::from_utf8(sdk.stdout).unwrap().trim().to_owned();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    // The same SDK under a path with spaces.
    fs::create_dir(dir.path().join("my sdks")).unwrap();
    symlink(&sdk, dir.path().join("my sdks/My SDK.sdk")).unwrap();
    // Another SDK path Xcode knows: the `xcrun` shim behind `cc` fails
    // under an `SDKROOT` it does not.
    let other = fs::read_dir(Path::new(&sdk).parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_str() != Some(&*sdk) && path.extension().is_some_and(|e| e == "sdk"))
        .unwrap();
    let build_dir = dir.path().join("build/dev");
    let build = |sysroot: Option<&str>, sdkroot: Option<PathBuf>| {
        let mut command = cabin();
        command
            .current_dir(dir.path())
            .arg("build")
            .env_remove("SDKROOT");
        if let Some(sysroot) = sysroot {
            command.args(["--sysroot", sysroot]);
        }
        if let Some(sdkroot) = sdkroot {
            command.env("SDKROOT", sdkroot);
        }
        command.assert().success();
        fs::read_to_string(build_dir.join("compile_commands.json")).unwrap()
    };
    let modified = || {
        ["capp", "cxxapp"].map(|exe| {
            let path = build_dir.join("packages/app").join(exe);
            fs::metadata(path).unwrap().modified().unwrap()
        })
    };

    // Apple Clang's own SDK, made explicit.
    let commands = build(None, None);
    assert!(commands.contains(&sdk), "{commands}");
    let discovered = modified();
    build(None, None);
    assert_eq!(modified(), discovered, "an unchanged SDK must not relink");

    let commands = build(Some("my sdks/My SDK.sdk"), None);
    assert!(commands.contains("my sdks/My SDK.sdk"), "{commands}");
    let first = modified();
    assert!(discovered.iter().zip(&first).all(|(a, b)| a != b));
    run_both(dir.path(), &build_dir);

    // `SDKROOT` moves the discovered SDK, as it moves Apple Clang's.
    let commands = build(None, Some(other.clone()));
    assert!(commands.contains(other.to_str().unwrap()), "{commands}");
    assert!(first.iter().zip(&modified()).all(|(a, b)| a != b));
    run_both(dir.path(), &build_dir);

    // `cabin tidy` rewrites the same compile database, with the same SDK.
    cabin()
        .current_dir(dir.path())
        .arg("tidy")
        .env("CABIN_TIDY", workspace_test_bin("cabin-tidy-fake-tidy"))
        .env_remove("SDKROOT")
        .assert()
        .success();
    let commands = fs::read_to_string(build_dir.join("compile_commands.json")).unwrap();
    assert!(commands.contains(&sdk), "{commands}");
}

#[cfg(target_os = "macos")]
#[test]
fn metadata_tells_a_configured_sysroot_from_a_detected_sdk() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    let triple = if std::env::consts::ARCH == "aarch64" {
        "arm64-apple-macos13"
    } else {
        "x86_64-apple-macos13"
    };
    fs::create_dir(dir.path().join(".cabin")).unwrap();
    fs::write(
        dir.path().join(".cabin/config.toml"),
        format!("[sysroot]\n\"{triple}\" = \"sdks/Pinned.sdk\"\n"),
    )
    .unwrap();
    let metadata = |args: &[&str], config: Option<&str>| {
        let mut command = cabin_with_config();
        command
            .current_dir(dir.path())
            .arg("metadata")
            .args(args)
            .env_remove("SDKROOT");
        if let Some(config) = config {
            command.env("CABIN_CONFIG", config);
        }
        let output = command.output().unwrap();
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let native = metadata(&[], None);
    assert_eq!(native["toolchain"]["sysroot"]["source"], "detected");
    assert_eq!(
        native["config"]["sysroots"][triple]["value"],
        "sdks/Pinned.sdk"
    );
    let configured = &metadata(&["--target", triple], None)["toolchain"]["sysroot"];
    assert_eq!(configured["source"], "package-config");
    assert!(
        configured["path"]
            .as_str()
            .unwrap()
            .ends_with(".cabin/sdks/Pinned.sdk")
    );
    let cli_sdk = dir.path().join("cli.sdk");
    let cli_sdk = cli_sdk.to_str().unwrap();
    assert_eq!(
        metadata(&["--target", triple, "--sysroot", cli_sdk], None)["toolchain"]["sysroot"],
        serde_json::json!({"path": cli_sdk, "source": "cli"})
    );
    // The configuration records a choice as written, and leaves out
    // the detected SDK, a local fact.
    let configured = |args: &[&str]| {
        let output = cabin_with_config()
            .current_dir(dir.path())
            .args(["explain", "--format", "json"])
            .args(args)
            .args(["build-config", "app"])
            .env_remove("SDKROOT")
            .output()
            .unwrap();
        let explain: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        explain["configuration"]["toolchain"]["sysroot"].clone()
    };
    assert_eq!(configured(&["--sysroot", cli_sdk]), cli_sdk);
    assert_eq!(configured(&["--target", triple]), "sdks/Pinned.sdk");
    assert_eq!(configured(&[]), serde_json::Value::Null);
    // A relative `CABIN_CONFIG` still yields an absolute sysroot, since
    // Ninja runs the commands from the build directory.
    let explicit =
        &metadata(&["--target", triple], Some(".cabin/config.toml"))["toolchain"]["sysroot"];
    assert_eq!(explicit["source"], "explicit-config");
    let path = explicit["path"].as_str().unwrap();
    assert!(Path::new(path).is_absolute(), "{path}");
}

#[cfg(target_os = "linux")]
#[test]
fn a_sysroot_path_with_spaces_builds_c_and_cxx() {
    require_c_and_cxx_build_tools();
    let dir = TempDir::new().unwrap();
    write_project(dir.path());
    // The build machine's own root, under a path with a space.
    std::os::unix::fs::symlink("/", dir.path().join("my root")).unwrap();
    cabin()
        .current_dir(dir.path())
        .args(["build", "--sysroot", "my root"])
        .assert()
        .success();
    let build_dir = dir.path().join("build/dev");
    let commands = fs::read_to_string(build_dir.join("compile_commands.json")).unwrap();
    assert!(commands.contains("--sysroot="), "{commands}");
    assert!(commands.contains("/my root"), "{commands}");
    run_both(dir.path(), &build_dir);
}
