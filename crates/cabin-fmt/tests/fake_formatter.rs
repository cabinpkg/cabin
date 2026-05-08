//! End-to-end tests for the `cabin-fmt` library that exercise
//! a real subprocess via the bundled fake formatter binary.
//!
//! The fake formatter (`cabin-fmt-fake-formatter`) implements
//! the minimum slice of `clang-format`'s command-line surface
//! the library invokes: `-i` rewrites files in place by
//! appending a sentinel marker, and `--dry-run -Werror` exits
//! non-zero if any input file lacks the marker.  These tests
//! treat the fake formatter as a black box and verify the
//! library's report contract.
//!
//! Gated on the `test-fake-formatter` feature so the binary
//! the tests need is guaranteed to be built; a standalone
//! `cargo test -p cabin-fmt` (without the feature) compiles
//! this file to nothing and the lib unit tests run on their
//! own.

#![cfg(feature = "test-fake-formatter")]

use std::ffi::OsString;
use std::path::PathBuf;

use cabin_fmt::{FormatMode, FormatReport, FormatRequest, run_formatter};

fn fake_formatter_path() -> PathBuf {
    // `cargo test` builds bins in the same target directory as
    // the test binary.  Walk up to the deps directory's parent
    // (e.g. `target/debug`) and look for the bin there.
    let test_exe = std::env::current_exe().expect("current_exe");
    let mut dir = test_exe
        .parent()
        .expect("test exe should live in a directory")
        .to_path_buf();
    // `target/debug/deps/<test>` → `target/debug/deps`.
    if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
        dir.pop();
    }
    let candidate = dir.join("cabin-fmt-fake-formatter");
    assert!(
        candidate.is_file(),
        "expected fake formatter at {}; build cabin-fmt with `--features test-fake-formatter`",
        candidate.display()
    );
    candidate
}

fn write_file(path: &std::path::Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

#[test]
fn write_mode_appends_sentinel_in_place() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("main.cc");
    write_file(&file, "int main() { return 0; }\n");

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![file.clone()],
        mode: FormatMode::Write,
    };
    let report = run_formatter(&req).unwrap();
    assert_eq!(report, FormatReport::Wrote { files_processed: 1 });

    let body = std::fs::read_to_string(&file).unwrap();
    assert!(body.contains("/* FORMATTED */"), "got: {body:?}");
}

#[test]
fn check_mode_reports_clean_when_already_formatted() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("main.cc");
    write_file(&file, "int main() {}\n/* FORMATTED */\n");

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![file],
        mode: FormatMode::Check,
    };
    let report = run_formatter(&req).unwrap();
    assert_eq!(report, FormatReport::Clean { files_inspected: 1 });
}

#[test]
fn check_mode_reports_needs_formatting_when_dirty() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("main.cc");
    write_file(&file, "int main() { return 0; }\n");

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![file.clone()],
        mode: FormatMode::Check,
    };
    let report = run_formatter(&req).unwrap();
    assert_eq!(report, FormatReport::NeedsFormatting { files_inspected: 1 });

    // Check mode must not modify files.
    let body = std::fs::read_to_string(&file).unwrap();
    assert_eq!(body, "int main() { return 0; }\n");
}

#[test]
fn check_mode_aggregates_mixed_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let clean = dir.path().join("clean.cc");
    let dirty = dir.path().join("dirty.cc");
    write_file(&clean, "int x = 0;\n/* FORMATTED */\n");
    write_file(&dirty, "int y = 1;\n");

    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![clean, dirty],
        mode: FormatMode::Check,
    };
    let report = run_formatter(&req).unwrap();
    assert_eq!(report, FormatReport::NeedsFormatting { files_inspected: 2 });
}

#[test]
fn write_mode_processes_files_in_deterministic_order() {
    // The library dedupes/orders files internally; pass them
    // out of order and verify a duplicate is collapsed.
    let dir = tempfile::TempDir::new().unwrap();
    let a = dir.path().join("a.cc");
    let b = dir.path().join("b.cc");
    write_file(&a, "x");
    write_file(&b, "y");
    let req = FormatRequest {
        executable: OsString::from(fake_formatter_path()),
        files: vec![b.clone(), a, b],
        mode: FormatMode::Write,
    };
    let report = run_formatter(&req).unwrap();
    assert_eq!(report, FormatReport::Wrote { files_processed: 2 });
}
