//! End-to-end coverage for the standard-compatibility publish-time
//! lints of `docs/design/standard-compatibility/publish-lints.md`.
//!
//! One fixture per lint, fire and non-fire, with a C variant beside
//! the C++ one:
//! - **PL1** (error) rejects a publish whose declared interface
//!   minimum is newer than the target's implementation standard,
//!   before any registry write.  The header-only direct pair is used
//!   because the load-time contradiction lint (compiled sources only)
//!   would otherwise reject a compiled fixture first.
//! - **PL2** (warning) fires on a header-only target that leaves an
//!   implemented language's interface to inference, and the publish
//!   still succeeds.
//! - **PL3** (warning) fires when a patch release raises a declared
//!   requirement versus the immediately previous version; a first
//!   publish and a minor release do not fire.
//! - A staging-only `--dry-run` skips PL3 and says so, while still
//!   running the manifest lints.
//!
//! Publishing never compiles, so these tests need no toolchain.

use super::*;

/// Write `manifest` as `<root>/cabin.toml` plus the referenced source
/// tree entries.
fn write_pkg(root: &Path, manifest: &str, files: &[(&str, &str)]) {
    assert_fs::fixture::ChildPath::new(root.join("cabin.toml"))
        .write_str(manifest)
        .unwrap();
    for (rel, content) in files {
        assert_fs::fixture::ChildPath::new(root.join(rel))
            .write_str(content)
            .unwrap();
    }
}

/// A header-only C++ library declaring `cxx-standard` (its
/// implementation) and `interface-cxx-standard` (its promise).
fn header_only_cxx(version: &str, cxx: &str, interface_cxx: &str) -> String {
    format!(
        r#"[package]
name = "acme/hdr"
version = "{version}"

[target.hdr]
type = "header-only"
include-dirs = ["include"]
cxx-standard = "{cxx}"
interface-cxx-standard = "{interface_cxx}"
"#
    )
}

/// A compiled C++ library (`cxx-standard` implementation,
/// `interface-cxx-standard` promise) whose published `standards`
/// table PL3 compares across versions.
fn compiled_cxx(version: &str, interface_cxx: &str) -> String {
    format!(
        r#"[package]
name = "acme/demo"
version = "{version}"

[target.demo]
type = "library"
sources = ["src/demo.cc"]
include-dirs = ["include"]
cxx-standard = "c++20"
interface-cxx-standard = "{interface_cxx}"
"#
    )
}

/// A compiled C library, the C sibling of [`compiled_cxx`].
fn compiled_c(version: &str, interface_c: &str) -> String {
    format!(
        r#"[package]
name = "acme/demo"
version = "{version}"

[target.demo]
type = "library"
sources = ["src/demo.c"]
include-dirs = ["include"]
c-standard = "c17"
interface-c-standard = "{interface_c}"
"#
    )
}

const CXX_SOURCES: &[(&str, &str)] = &[
    ("include/demo.h", "#pragma once\nint demo_value();\n"),
    (
        "src/demo.cc",
        "#include \"demo.h\"\nint demo_value() { return 1; }\n",
    ),
];

const C_SOURCES: &[(&str, &str)] = &[
    ("include/demo.h", "#pragma once\nint demo_value(void);\n"),
    (
        "src/demo.c",
        "#include \"demo.h\"\nint demo_value(void) { return 1; }\n",
    ),
];

const HDR_SOURCES: &[(&str, &str)] = &[("include/hdr.h", "#pragma once\n#define HDR 1\n")];

fn publish(pkg_root: &Path, registry: &Path) -> assert_cmd::assert::Assert {
    cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(registry)
        .assert()
}

// --- PL1 -----------------------------------------------------------

/// PL1 rejects the publish with exit code 1 and leaves the registry
/// entirely unwritten - the header-only direct pair `c++17`
/// implementation, `c++20` interface.
#[test]
fn pl1_rejects_and_leaves_registry_unwritten() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_pkg(
        &pkg_root,
        &header_only_cxx("1.0.0", "c++17", "c++20"),
        HDR_SOURCES,
    );
    let registry = dir.path().join("registry");

    let assertion = publish(&pkg_root, &registry).failure().code(1);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("interface-cxx-standard") && stderr.contains("c++20"),
        "expected the PL1 message in: {stderr}"
    );
    assert!(
        stderr.contains("rejected this publish"),
        "expected the rejection preamble in: {stderr}"
    );
    // Nothing was written: the registry was never even initialized.
    assert!(!registry.join("config.json").exists());
    assert!(!registry.join("packages").exists());
}

/// PL1 fires for C too: a `c17` interface over a `c11` implementation.
#[test]
fn pl1_rejects_for_c() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_pkg(
        &pkg_root,
        r#"[package]
name = "acme/hdr"
version = "1.0.0"

[target.hdr]
type = "header-only"
include-dirs = ["include"]
c-standard = "c11"
interface-c-standard = "c17"
"#,
        HDR_SOURCES,
    );
    let registry = dir.path().join("registry");

    let assertion = publish(&pkg_root, &registry).failure().code(1);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("interface-c-standard") && stderr.contains("c17") && stderr.contains("c11"),
        "expected the PL1 C message in: {stderr}"
    );
    assert!(!registry.join("config.json").exists());
}

// --- PL2 -----------------------------------------------------------

/// PL2 warns when a header-only target implements C++ but leaves its
/// C++ interface to inference (declaring only the C interface), and
/// the publish still succeeds.
#[test]
fn pl2_warns_on_inferred_cxx_interface() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_pkg(
        &pkg_root,
        r#"[package]
name = "acme/hdr"
version = "1.0.0"

[target.hdr]
type = "header-only"
include-dirs = ["include"]
c-standard = "c11"
interface-c-standard = "c11"
cxx-standard = "c++20"
"#,
        HDR_SOURCES,
    );
    let registry = dir.path().join("registry");

    let assertion = publish(&pkg_root, &registry).success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("warning:")
            && stderr.contains("interface-cxx-standard")
            && stderr.contains("c++20")
            && stderr.contains("inferred"),
        "expected the PL2 warning in: {stderr}"
    );
    assert!(registry.join("packages/acme/hdr.json").is_file());
}

/// PL2's C sibling: a header-only target declaring only the C++
/// interface warns that its C interface is inferred.
#[test]
fn pl2_warns_on_inferred_c_interface() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_pkg(
        &pkg_root,
        r#"[package]
name = "acme/hdr"
version = "1.0.0"

[target.hdr]
type = "header-only"
include-dirs = ["include"]
cxx-standard = "c++20"
interface-cxx-standard = "c++20"
c-standard = "c11"
"#,
        HDR_SOURCES,
    );
    let registry = dir.path().join("registry");

    let assertion = publish(&pkg_root, &registry).success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("warning:")
            && stderr.contains("interface-c-standard")
            && stderr.contains("inferred"),
        "expected the PL2 C warning in: {stderr}"
    );
}

/// PL2 stays quiet when every implemented language's interface is
/// declared explicitly.
#[test]
fn pl2_quiet_when_all_interfaces_declared() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    write_pkg(
        &pkg_root,
        &header_only_cxx("1.0.0", "c++20", "c++17"),
        HDR_SOURCES,
    );
    let registry = dir.path().join("registry");

    let assertion = publish(&pkg_root, &registry).success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains("warning:"),
        "no lint warning expected in: {stderr}"
    );
}

// --- PL3 -----------------------------------------------------------

/// PL3 warns when a patch release raises a declared C++ interface
/// minimum versus the previous version, and cites the policy.
#[test]
fn pl3_warns_on_patch_raise_cxx() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    let registry = dir.path().join("registry");

    write_pkg(&pkg_root, &compiled_cxx("1.0.0", "c++17"), CXX_SOURCES);
    publish(&pkg_root, &registry).success();

    // Patch release raising the interface minimum to c++20.
    write_pkg(&pkg_root, &compiled_cxx("1.0.1", "c++20"), CXX_SOURCES);
    let assertion = publish(&pkg_root, &registry).success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("warning:")
            && stderr.contains("narrowed from")
            && stderr.contains("c++17")
            && stderr.contains("c++20")
            && stderr.contains("discouraged in patches"),
        "expected the PL3 warning in: {stderr}"
    );
}

/// PL3's C sibling: raising `interface-c-standard` in a patch release
/// warns.
#[test]
fn pl3_warns_on_patch_raise_c() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    let registry = dir.path().join("registry");

    write_pkg(&pkg_root, &compiled_c("1.0.0", "c11"), C_SOURCES);
    publish(&pkg_root, &registry).success();

    write_pkg(&pkg_root, &compiled_c("1.0.1", "c17"), C_SOURCES);
    let assertion = publish(&pkg_root, &registry).success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        stderr.contains("warning:")
            && stderr.contains("narrowed from")
            && stderr.contains("c11")
            && stderr.contains("c17"),
        "expected the PL3 C warning in: {stderr}"
    );
}

/// A first publish has no baseline, so PL3 does not fire.
#[test]
fn pl3_quiet_on_first_publish() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    let registry = dir.path().join("registry");

    write_pkg(&pkg_root, &compiled_cxx("1.0.0", "c++20"), CXX_SOURCES);
    let assertion = publish(&pkg_root, &registry).success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains("raised from"),
        "a first publish must not fire PL3: {stderr}"
    );
}

/// A minor release may raise a requirement without a PL3 warning:
/// raises are minor incompatibilities, allowed in minor releases.
#[test]
fn pl3_quiet_on_minor_release() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    let registry = dir.path().join("registry");

    write_pkg(&pkg_root, &compiled_cxx("1.0.0", "c++17"), CXX_SOURCES);
    publish(&pkg_root, &registry).success();

    write_pkg(&pkg_root, &compiled_cxx("1.1.0", "c++20"), CXX_SOURCES);
    let assertion = publish(&pkg_root, &registry).success();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).to_string();
    assert!(
        !stderr.contains("raised from"),
        "a minor release must not fire PL3: {stderr}"
    );
}

// --- Dry-run and JSON ----------------------------------------------

/// A staging-only `--dry-run` (no registry) runs the manifest lints
/// but skips the registry-backed PL3, and says so in its output.
#[test]
fn dry_run_staging_only_skips_pl3_and_still_warns() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    let out = dir.path().join("dist");
    write_pkg(
        &pkg_root,
        r#"[package]
name = "acme/hdr"
version = "1.0.0"

[target.hdr]
type = "header-only"
include-dirs = ["include"]
c-standard = "c11"
interface-c-standard = "c11"
cxx-standard = "c++20"
"#,
        HDR_SOURCES,
    );

    let assertion = cabin()
        .args(["publish", "--dry-run", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--output-dir")
        .arg(&out)
        .assert()
        .success();
    let output = assertion.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        stdout.contains("Patch-release requirement check (PL3) skipped"),
        "expected the PL3-skip note in stdout: {stdout}"
    );
    // PL2 still runs on the staging-only path.
    assert!(
        stderr.contains("warning:") && stderr.contains("interface-cxx-standard"),
        "expected the PL2 warning on the dry-run path: {stderr}"
    );
}

/// The JSON publish output carries the lint warnings as an array, so
/// machine consumers see them without scraping stderr.
#[test]
fn publish_json_reports_warnings() {
    let dir = TempDir::new().unwrap();
    let pkg_root = dir.path().join("pkg");
    let registry = dir.path().join("registry");
    write_pkg(
        &pkg_root,
        r#"[package]
name = "acme/hdr"
version = "1.0.0"

[target.hdr]
type = "header-only"
include-dirs = ["include"]
c-standard = "c11"
interface-c-standard = "c11"
cxx-standard = "c++20"
"#,
        HDR_SOURCES,
    );

    let assertion = cabin()
        .args(["publish", "--manifest-path"])
        .arg(pkg_root.join("cabin.toml"))
        .arg("--registry-dir")
        .arg(&registry)
        .args(["--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).to_string();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let warnings = value["warnings"].as_array().expect("warnings array");
    assert_eq!(warnings.len(), 1, "expected one warning in: {stdout}");
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .contains("interface-cxx-standard"),
        "expected the PL2 warning text in JSON: {stdout}"
    );
}
