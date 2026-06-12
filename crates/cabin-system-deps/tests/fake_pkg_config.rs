//! End-to-end tests for the probe layer using the bundled fake
//! pkg-config binary.
//!
//! These tests run the real `probe_system_dependency` function
//! against an actual subprocess, but the binary is a tiny stand-
//! in (no host-installed library required) loaded through the
//! `CABIN_FAKE_PKG_CONFIG_FIXTURES` env var. Each fixture is a
//! one-file description of a module's version, cflags, and libs.

#![cfg(feature = "test-fake-pkg-config")]
#![allow(clippy::match_wildcard_for_single_variants, clippy::manual_let_else)]

use std::ffi::OsString;
use std::path::PathBuf;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use cabin_system_deps::{
    PkgConfigError, PkgConfigTool, SystemDependencyProbeRequest, SystemDependencyResolution,
    probe_system_dependency,
};

fn fake_pkg_config_path() -> PathBuf {
    let test_exe = std::env::current_exe().expect("current_exe");
    let mut dir = test_exe
        .parent()
        .expect("test exe should live in a directory")
        .to_path_buf();
    if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
        dir.pop();
    }
    let candidate = dir.join(format!(
        "cabin-system-deps-fake-pkg-config{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        candidate.is_file(),
        "expected fake pkg-config at {}; build with `--features test-fake-pkg-config`",
        candidate.display()
    );
    candidate
}

fn write_fixture(dir: &TempDir, name: &str, body: &str) {
    dir.child(format!("{name}.json")).write_str(body).unwrap();
}

struct Harness {
    temp: TempDir,
    tool: PkgConfigTool,
}

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let tool = PkgConfigTool::new(OsString::from(fake_pkg_config_path()))
            .with_extra_env("CABIN_FAKE_PKG_CONFIG_FIXTURES", temp.path());
        Self { temp, tool }
    }

    fn probe(
        &self,
        name: &str,
        version_requirement: &str,
    ) -> Result<SystemDependencyResolution, PkgConfigError> {
        probe_system_dependency(&SystemDependencyProbeRequest {
            name,
            version_requirement,
            tool: &self.tool,
        })
    }
}

#[test]
fn probe_finds_dep_with_no_version_requirement() {
    let h = Harness::new();
    write_fixture(
        &h.temp,
        "zlib",
        r#"{
            "version": "1.2.13",
            "cflags": "-I/opt/zlib/include -DZLIB_CONST",
            "libs": "-L/opt/zlib/lib -lz"
        }"#,
    );
    let r = h.probe("zlib", "").unwrap();
    assert_eq!(r.name, "zlib");
    assert_eq!(r.version.as_deref(), Some("1.2.13"));
    // pkg-config include dirs are third-party search paths, so they
    // land in the system bucket; the plain `-I` bucket is reserved
    // for the compiler's default search dirs.
    assert_eq!(
        r.flags.system_include_dirs,
        vec![PathBuf::from("/opt/zlib/include")],
    );
    assert!(r.flags.include_dirs.is_empty());
    assert_eq!(r.flags.extra_compile_args, vec!["-DZLIB_CONST".to_owned()]);
    assert_eq!(
        r.flags.ldflags,
        vec!["-L/opt/zlib/lib".to_owned(), "-lz".to_owned()],
    );
}

#[test]
fn probe_satisfies_caret_requirement() {
    let h = Harness::new();
    write_fixture(
        &h.temp,
        "openssl",
        r#"{
            "version": "1.2.3",
            "cflags": "-I/opt/openssl/include",
            "libs": "-lssl -lcrypto"
        }"#,
    );
    let res = h.probe("openssl", "^1.0").unwrap();
    assert_eq!(res.version.as_deref(), Some("1.2.3"));
    assert_eq!(
        res.flags.ldflags,
        vec!["-lssl".to_owned(), "-lcrypto".to_owned()],
    );
}

#[test]
fn probe_reports_package_not_found() {
    let h = Harness::new();
    let err = h.probe("nope", "").unwrap_err();
    match err {
        PkgConfigError::PackageNotFound { name, .. } => assert_eq!(name, "nope"),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn probe_reports_version_mismatch_when_too_old() {
    let h = Harness::new();
    write_fixture(
        &h.temp,
        "fmt",
        r#"{
            "version": "8.1.1",
            "cflags": "-I/opt/fmt/include",
            "libs": "-lfmt"
        }"#,
    );
    let err = h.probe("fmt", ">=9").unwrap_err();
    match err {
        PkgConfigError::VersionMismatch {
            name,
            requirement,
            installed,
        } => {
            assert_eq!(name, "fmt");
            assert_eq!(requirement, ">=9");
            assert_eq!(installed.as_deref(), Some("8.1.1"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn probe_preserves_link_order_from_libs() {
    let h = Harness::new();
    write_fixture(
        &h.temp,
        "openssl",
        r#"{
            "version": "3.0.0",
            "cflags": "-I/opt/openssl/include",
            "libs": "-L/opt/openssl/lib -lssl -lcrypto -ldl -lpthread"
        }"#,
    );
    let res = h.probe("openssl", "").unwrap();
    assert_eq!(
        res.flags.ldflags,
        vec![
            "-L/opt/openssl/lib".to_owned(),
            "-lssl".to_owned(),
            "-lcrypto".to_owned(),
            "-ldl".to_owned(),
            "-lpthread".to_owned(),
        ],
    );
}

#[test]
fn probe_handles_split_dash_i_form() {
    let h = Harness::new();
    write_fixture(
        &h.temp,
        "weird",
        r#"{
            "version": "1.0",
            "cflags": "-I /opt/weird/include",
            "libs": "-lweird"
        }"#,
    );
    let res = h.probe("weird", "").unwrap();
    assert_eq!(
        res.flags.system_include_dirs,
        vec![PathBuf::from("/opt/weird/include")],
    );
    assert!(res.flags.extra_compile_args.is_empty());
}

#[test]
fn probe_dedupes_include_paths_but_keeps_link_order() {
    let h = Harness::new();
    write_fixture(
        &h.temp,
        "dup",
        r#"{
            "version": "1.0.0",
            "cflags": "-I/opt/dup/include -I/opt/dup/include -fPIC",
            "libs": "-lfoo -lbar -lfoo"
        }"#,
    );
    let res = h.probe("dup", "").unwrap();
    // Include paths get deduped at probe time so the planner
    // does not emit the same search dir twice.
    assert_eq!(
        res.flags.system_include_dirs,
        vec![PathBuf::from("/opt/dup/include")],
    );
    assert_eq!(res.flags.extra_compile_args, vec!["-fPIC".to_owned()]);
    // Link tokens are preserved exactly as pkg-config reported
    // them.
    assert_eq!(
        res.flags.ldflags,
        vec!["-lfoo".to_owned(), "-lbar".to_owned(), "-lfoo".to_owned()],
    );
}

#[test]
fn probe_returns_executable_not_found_for_missing_binary() {
    let temp = TempDir::new().unwrap();
    let missing = temp.child("definitely-not-pkg-config");
    let tool = PkgConfigTool::new(missing.to_path_buf().into_os_string());
    let err = probe_system_dependency(&SystemDependencyProbeRequest {
        name: "anything",
        version_requirement: "",
        tool: &tool,
    })
    .unwrap_err();
    match err {
        PkgConfigError::ExecutableNotFound { .. } => {}
        other => panic!("unexpected error: {other:?}"),
    }
}
