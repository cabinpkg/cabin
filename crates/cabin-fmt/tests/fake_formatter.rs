//! End-to-end tests for the `cabin-fmt` library that exercise
//! a real subprocess via the bundled fake formatter binary
//! (`cabin-fmt-fake-formatter`), verifying the library's report
//! contract.
//!
//! Gated on the `test-fake-formatter` feature so the binary
//! the tests need is guaranteed to be built; a standalone
//! `cargo test -p cabinpkg-fmt` (without the feature) compiles
//! this file to nothing and the lib unit tests run on their
//! own.

#![cfg(feature = "test-fake-formatter")]

use std::ffi::OsString;
use std::path::PathBuf;

use assert_fs::TempDir;
use assert_fs::prelude::*;
use cabin_fmt::{
    ExitStatusKind, FormatError, FormatMode, FormatReport, FormatRequest, run_formatter,
};

fn fake_formatter_path() -> PathBuf {
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
    let candidate = dir.join(format!(
        "cabin-fmt-fake-formatter{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        candidate.is_file(),
        "expected fake formatter at {}; build cabin-fmt with `--features test-fake-formatter`",
        candidate.display()
    );
    candidate
}

#[test]
fn write_mode_appends_sentinel_in_place() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int main() { return 0; }\n").unwrap();

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![file.to_path_buf()],
        mode: FormatMode::Write,
    };
    let report = run_formatter(&req).unwrap();
    assert_eq!(report, FormatReport::Wrote { files_processed: 1 });

    let body = std::fs::read_to_string(file.path()).unwrap();
    assert!(body.contains("/* FORMATTED */"), "got: {body:?}");
}

#[test]
fn check_mode_reports_clean_when_already_formatted() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int main() {}\n/* FORMATTED */\n").unwrap();

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![file.to_path_buf()],
        mode: FormatMode::Check,
    };
    let report = run_formatter(&req).unwrap();
    assert_eq!(report, FormatReport::Clean { files_inspected: 1 });
}

#[test]
fn check_mode_reports_needs_formatting_when_dirty() {
    let dir = TempDir::new().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int main() { return 0; }\n").unwrap();

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![file.to_path_buf()],
        mode: FormatMode::Check,
    };
    let report = run_formatter(&req).unwrap();
    let FormatReport::NeedsFormatting {
        files_inspected,
        stderr,
    } = report
    else {
        panic!("expected NeedsFormatting, got {report:?}");
    };
    assert_eq!(files_inspected, 1);
    assert!(
        stderr.contains("[-Wclang-format-violations]"),
        "fake formatter should forward its per-file diagnostic, got: {stderr:?}"
    );

    let body = std::fs::read_to_string(file.path()).unwrap();
    assert_eq!(body, "int main() { return 0; }\n");
}

#[test]
fn check_mode_aggregates_mixed_files() {
    let dir = TempDir::new().unwrap();
    let clean = dir.child("clean.cc");
    let dirty = dir.child("dirty.cc");
    clean.write_str("int x = 0;\n/* FORMATTED */\n").unwrap();
    dirty.write_str("int y = 1;\n").unwrap();

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![clean.to_path_buf(), dirty.to_path_buf()],
        mode: FormatMode::Check,
    };
    let report = run_formatter(&req).unwrap();
    assert!(
        matches!(
            report,
            FormatReport::NeedsFormatting {
                files_inspected: 2,
                ..
            }
        ),
        "expected NeedsFormatting over 2 files, got {report:?}"
    );
}

#[test]
fn check_mode_reports_unreadable_file_as_error_not_needs_formatting() {
    // clang-format exits 1 for an unreadable file, as it does for a
    // formatting violation.
    let dir = TempDir::new().unwrap();
    let missing = dir.child("missing.cc");

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![missing.to_path_buf()],
        mode: FormatMode::Check,
    };
    let err = run_formatter(&req).unwrap_err();
    let FormatError::InvocationFailed { status, stderr, .. } = err else {
        panic!("expected InvocationFailed, got {err:?}");
    };
    assert_eq!(status, ExitStatusKind::Code(1));
    assert!(stderr.contains("missing.cc"), "got: {stderr:?}");
}

#[test]
fn check_mode_reports_silent_failure_as_error_not_needs_formatting() {
    // A `CABIN_FMT` wrapper that exits 1 without forwarding
    // clang-format's stderr gives no evidence of a violation.
    let dir = TempDir::new().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int x = 0;\n/* FAIL SILENTLY */\n").unwrap();

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![file.to_path_buf()],
        mode: FormatMode::Check,
    };
    let err = run_formatter(&req).unwrap_err();
    let FormatError::InvocationFailed { status, stderr, .. } = err else {
        panic!("expected InvocationFailed, got {err:?}");
    };
    assert_eq!(status, ExitStatusKind::Code(1));
    assert_eq!(stderr, "");
}

#[test]
fn write_mode_processes_files_in_deterministic_order() {
    // The library dedupes/orders files internally; pass them
    // out of order and verify a duplicate is collapsed.
    let dir = TempDir::new().unwrap();
    let a = dir.child("a.cc");
    let b = dir.child("b.cc");
    a.write_str("x").unwrap();
    b.write_str("y").unwrap();
    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![b.to_path_buf(), a.to_path_buf(), b.to_path_buf()],
        mode: FormatMode::Write,
    };
    let report = run_formatter(&req).unwrap();
    assert_eq!(report, FormatReport::Wrote { files_processed: 2 });
}
