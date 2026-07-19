//! End-to-end coverage for compiler / tool capability
//! detection.  Each test stages a fake compiler / archiver in
//! a `TempDir`, points `--cxx` / `--ar` at it, and inspects
//! either the metadata JSON or the build error message.

// This module's tests drive Unix-only shell-script fakes.
#[cfg(unix)]
use super::*;
#[cfg(unix)]
#[cfg(unix)]
#[test]
fn metadata_reports_detected_clang_identity() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let cxx = fake_tool_with_output(
        bin.path(),
        "fake-clang++",
        "clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\nThread model: posix\n",
        "",
        0,
    );
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--cxx"])
        .arg(&cxx)
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let detected = &value["toolchain"]["detected"];
    assert_eq!(detected["cxx"]["identity"]["kind"].as_str(), Some("clang"));
    assert_eq!(
        detected["cxx"]["identity"]["version"].as_str(),
        Some("17.0.6")
    );
    assert!(
        detected["cxx"]["capabilities"]["gcc_style_flags"]["supported"]
            .as_bool()
            .unwrap()
    );
    assert_eq!(detected["ar"]["identity"]["kind"].as_str(), Some("ar"));
}

#[cfg(unix)]
#[test]
fn build_with_msvc_compiler_errors_clearly() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
    let bin = TempDir::new().unwrap();
    let cxx = fake_tool_with_output(
        bin.path(),
        "fake-cl",
        "Microsoft (R) C/C++ Optimizing Compiler Version 19.39.0 for x64\n",
        "",
        0,
    );
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
    let assertion = cabin()
        .current_dir(dir.path())
        .args(["build", "--cxx"])
        .arg(&cxx)
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("MSVC") || stderr.contains("GCC- or Clang-like"),
        "expected MSVC unsupported error, got: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn build_with_unknown_compiler_errors_clearly() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[target.demo]
type = "executable"
sources = ["src/main.cc"]
"#,
        )
        .unwrap();
    dir.child("src/main.cc").write_str(HELLO_MAIN_CC).unwrap();
    let bin = TempDir::new().unwrap();
    let cxx = fake_tool_with_output(
        bin.path(),
        "fake-funky-cxx",
        "my funky compiler 0.1\n",
        "",
        0,
    );
    let _ar = fake_tool_with_output(bin.path(), "ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
    let assertion = cabin()
        .current_dir(dir.path())
        .args(["build", "--cxx"])
        .arg(&cxx)
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(
        stderr.contains("could not be identified"),
        "expected unknown-compiler error, got: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn package_metadata_does_not_serialize_local_detection() {
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
cxx-standard = "c++17"

[target.demo]
type = "library"
sources = ["src/lib.cc"]
"#,
        )
        .unwrap();
    dir.child("src/lib.cc")
        .write_str("int demo() { return 0; }\n")
        .unwrap();
    let out = dir.path().join("dist");
    cabin()
        .args(["package", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--output-dir"])
        .arg(&out)
        .assert()
        .success();
    let body = fs::read_to_string(out.join("demo-0.1.0.json")).unwrap();
    // Detected info must never leak into published metadata.
    assert!(!body.contains("CABIN_CXX_COMPILER_KIND"), "{body}");
    assert!(!body.contains("\"detected\""), "{body}");
    assert!(!body.contains("clang version"), "{body}");
    assert!(!body.contains("Apple clang"), "{body}");
}

#[cfg(unix)]
#[test]
fn metadata_toolchain_block_is_a_stable_golden_for_a_fixed_toolchain() {
    // Pin the JSON shape of `cabin metadata`'s
    // `toolchain.detected` block end-to-end.  Uses fake
    // compiler / archiver wrappers so the golden does not
    // depend on whichever clang/gcc happens to be installed;
    // absolute paths are normalized back to placeholders
    // before the snapshot comparison so the assertion does
    // not embed machine-specific data.
    let dir = TempDir::new().unwrap();
    dir.child("cabin.toml")
        .write_str(
            r#"[package]
name = "demo"
version = "0.1.0"
"#,
        )
        .unwrap();
    let bin = TempDir::new().unwrap();
    let cxx = fake_tool_with_output(
        bin.path(),
        "fake-clang++",
        "clang version 17.0.6\nTarget: x86_64-unknown-linux-gnu\nThread model: posix\n",
        "",
        0,
    );
    let ar = fake_tool_with_output(bin.path(), "fake-ar", "GNU ar (GNU Binutils) 2.40\n", "", 0);
    let assertion = cabin()
        .args(["metadata", "--manifest-path"])
        .arg(dir.path().join("cabin.toml"))
        .args(["--cxx"])
        .arg(&cxx)
        .args(["--ar"])
        .arg(&ar)
        .env("PATH", bin.path())
        .env_remove("CXX")
        .env_remove("CC")
        .env_remove("AR")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let cxx_str = cxx.to_string_lossy().into_owned();
    let ar_str = ar.to_string_lossy().into_owned();
    let detected = serde_json::to_string_pretty(&value["toolchain"]["detected"]).unwrap();
    let detected = detected.replace(&cxx_str, "<CXX>").replace(&ar_str, "<AR>");

    let expected = r#"{
  "cxx": {
    "path": "<CXX>",
    "identity": {
      "kind": "clang",
      "version": "17.0.6",
      "target": "x86_64-unknown-linux-gnu",
      "raw_version_line": "clang version 17.0.6"
    },
    "capabilities": {
      "depfile_mmd_mf": {
        "supported": true,
        "source": "version"
      },
      "external_include_dirs": {
        "supported": true,
        "source": "version"
      },
      "gcc_style_flags": {
        "supported": true,
        "source": "version"
      },
      "msvc_style_flags": {
        "supported": false,
        "source": "assumed-default"
      }
    }
  },
  "ar": {
    "path": "<AR>",
    "identity": {
      "kind": "ar",
      "version": "2.40",
      "raw_version_line": "GNU ar (GNU Binutils) 2.40"
    },
    "capabilities": {
      "ar_crs": {
        "supported": true,
        "source": "version"
      },
      "static_library_output": {
        "supported": true,
        "source": "version"
      }
    }
  }
}"#;
    assert_eq!(detected, expected);
}
