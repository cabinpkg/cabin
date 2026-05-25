//! End-to-end tests for the bundled fake ninja binary.
//!
//! The fake ninja (`cabin-ninja-fake-ninja`) records the
//! invocation argv to a file specified by
//! `CABIN_FAKE_NINJA_RECORD` and exits with status 0.  Cabin's
//! CLI integration tests use it to assert exactly which argv
//! `cabin build` / `cabin run` / `cabin test` pass to the
//! backend without depending on a real Ninja install.
//!
//! Gated on the `test-fake-ninja` feature so the binary the
//! tests need is guaranteed to be built; a standalone
//! `cargo test -p cabin-ninja` (without the feature) compiles
//! this file to nothing and the lib unit tests run on their
//! own.

#![cfg(feature = "test-fake-ninja")]

use std::path::PathBuf;
use std::process::Command;

use assert_fs::TempDir;
use assert_fs::prelude::*;

fn fake_ninja_path() -> PathBuf {
    // `cargo test` builds bins in the same target directory as
    // the test binary.  Walk up to the deps directory's parent
    // (e.g. `target/debug`) and look for the bin there.
    let test_exe = std::env::current_exe().expect("current_exe");
    let mut dir = test_exe
        .parent()
        .expect("test exe should live in a directory")
        .to_path_buf();
    if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
        dir.pop();
    }
    let candidate = dir.join("cabin-ninja-fake-ninja");
    assert!(
        candidate.is_file(),
        "expected fake ninja at {}; build cabin-ninja with `--features test-fake-ninja`",
        candidate.display()
    );
    candidate
}

#[test]
fn records_argv_when_record_env_is_set() {
    let tmp = TempDir::new().unwrap();
    let record = tmp.child("ninja-argv.log");

    let status = Command::new(fake_ninja_path())
        .args(["-j", "4", "-C", "build"])
        .env("CABIN_FAKE_NINJA_RECORD", record.path())
        .status()
        .unwrap();
    assert!(status.success());

    let body = std::fs::read_to_string(record.path()).unwrap();
    let line = body.lines().next().unwrap();
    let argv: Vec<&str> = line.split('\u{001f}').collect();
    assert_eq!(argv, vec!["-j", "4", "-C", "build"]);
}

#[test]
fn appends_one_line_per_invocation() {
    let tmp = TempDir::new().unwrap();
    let record = tmp.child("ninja-argv.log");

    for args in [vec!["-j", "1"], vec!["-j", "2"], vec!["all"]] {
        let status = Command::new(fake_ninja_path())
            .args(&args)
            .env("CABIN_FAKE_NINJA_RECORD", record.path())
            .status()
            .unwrap();
        assert!(status.success());
    }

    let body = std::fs::read_to_string(record.path()).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0], "-j\u{001f}1");
    assert_eq!(lines[1], "-j\u{001f}2");
    assert_eq!(lines[2], "all");
}

#[test]
fn exits_zero_without_record_env() {
    let status = Command::new(fake_ninja_path())
        .args(["-j", "4"])
        .env_remove("CABIN_FAKE_NINJA_RECORD")
        .status()
        .unwrap();
    assert!(status.success());
}
