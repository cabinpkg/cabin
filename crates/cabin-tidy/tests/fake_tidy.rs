//! End-to-end tests for the `cabin-tidy` library that exercise
//! a real subprocess via the bundled fake tidy binary.
//!
//! Gated on the `test-fake-tidy` feature so the binary the tests
//! need is guaranteed to be built; a standalone
//! `cargo test -p cabin-tidy` (without the feature) compiles
//! this file to nothing and the lib unit tests run on their own.

#![cfg(feature = "test-fake-tidy")]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use assert_fs::TempDir;
use assert_fs::prelude::*;

use cabin_core::BuildJobs;
use cabin_tidy::{ExitStatusKind, TidyMode, TidyReport, TidyRequest, TidyVerbosity, run_tidy};

/// Process-wide mutex for every test in this file.  Each test
/// spawns the fake tidy via `Command::status()`, which inherits
/// the parent process's environment.  When one test sets
/// `CABIN_FAKE_TIDY_RECORD` to its own log path, any concurrent
/// test's spawn would inherit that same value and silently write
/// to the wrong file.  Holding this lock for the full
/// spawn-and-assert window prevents the leak without requiring
/// `--test-threads=1`.
fn tidy_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fake_tidy_path() -> PathBuf {
    let test_exe = std::env::current_exe().expect("current_exe");
    let mut dir = test_exe
        .parent()
        .expect("test exe should live in a directory")
        .to_path_buf();
    if dir.file_name().and_then(|n| n.to_str()) == Some("deps") {
        dir.pop();
    }
    let candidate = dir.join(format!(
        "cabin-tidy-fake-tidy{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        candidate.is_file(),
        "expected fake tidy at {}; build cabin-tidy with `--features test-fake-tidy`",
        candidate.display()
    );
    candidate
}

fn read_record(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn clean_files_yield_tidied_report() {
    let _guard = tidy_lock();
    let dir = TempDir::new().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int main() { return 0; }\n").unwrap();

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file.to_path_buf()],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    let report = run_tidy(&req).unwrap();
    assert_eq!(report, TidyReport::Tidied { files_processed: 1 });
}

#[test]
fn fail_marker_yields_tidy_failed_with_exit_code_one() {
    let _guard = tidy_lock();
    let dir = TempDir::new().unwrap();
    let file = dir.child("bad.cc");
    file.write_str("// CABIN-TIDY-FAIL\nint main() {}\n")
        .unwrap();

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file.to_path_buf()],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    let report = run_tidy(&req).unwrap();
    match report {
        TidyReport::TidyFailed {
            status,
            files_processed,
        } => {
            assert_eq!(files_processed, 1);
            assert_eq!(status, ExitStatusKind::Code(1));
        }
        other => panic!("expected TidyFailed, got {other:?}"),
    }
}

#[test]
fn quiet_flag_passed_for_normal_verbosity() {
    let _guard = tidy_lock();
    let dir = TempDir::new().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int main() {}\n").unwrap();
    let record = dir.child("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file.to_path_buf()],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", record.path());
    }
    let report = run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }
    assert_eq!(report, TidyReport::Tidied { files_processed: 1 });

    let lines = read_record(record.path());
    assert_eq!(lines.len(), 1);
    let mut parts = lines[0].split('\t');
    let argv = parts.next().unwrap();
    let quiet = parts.next().unwrap();
    assert!(argv.contains("-quiet"));
    assert_eq!(quiet, "true");
}

#[test]
fn quiet_flag_omitted_for_verbose_verbosity() {
    let _guard = tidy_lock();
    let dir = TempDir::new().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int main() {}\n").unwrap();
    let record = dir.child("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file.to_path_buf()],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Verbose,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", record.path());
    }
    let report = run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }
    assert_eq!(report, TidyReport::Tidied { files_processed: 1 });

    let lines = read_record(record.path());
    assert_eq!(lines.len(), 1);
    let mut parts = lines[0].split('\t');
    let argv = parts.next().unwrap();
    let quiet = parts.next().unwrap();
    assert!(!argv.contains("-quiet"));
    assert_eq!(quiet, "false");
}

#[test]
fn compile_database_dir_passed_via_dash_p() {
    let _guard = tidy_lock();
    let dir = TempDir::new().unwrap();
    let build_dir = dir.child("build/dev");
    build_dir.create_dir_all().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int main() {}\n").unwrap();
    let record = dir.child("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: build_dir.to_path_buf(),
        files: vec![file.to_path_buf()],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", record.path());
    }
    run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }

    let lines = read_record(record.path());
    let mut parts = lines[0].split('\t');
    let _argv = parts.next().unwrap();
    let _quiet = parts.next().unwrap();
    let _fix = parts.next().unwrap();
    let compile_db = parts.next().unwrap();
    assert_eq!(compile_db, build_dir.display().to_string());
}

#[test]
fn fix_flag_forwarded_in_fix_mode() {
    let _guard = tidy_lock();
    let dir = TempDir::new().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int main() {}\n").unwrap();
    let record = dir.child("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file.to_path_buf()],
        mode: TidyMode::Fix,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", record.path());
    }
    run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }

    let lines = read_record(record.path());
    let mut parts = lines[0].split('\t');
    let argv = parts.next().unwrap();
    let _quiet = parts.next().unwrap();
    let fix = parts.next().unwrap();
    assert!(argv.contains("-fix"));
    assert_eq!(fix, "true");
}

#[test]
fn jobs_flag_forwarded_when_set() {
    let _guard = tidy_lock();
    let dir = TempDir::new().unwrap();
    let file = dir.child("main.cc");
    file.write_str("int main() {}\n").unwrap();
    let record = dir.child("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file.to_path_buf()],
        mode: TidyMode::Check,
        jobs: Some(BuildJobs::new(4).unwrap()),
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", record.path());
    }
    run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }

    let lines = read_record(record.path());
    let mut parts = lines[0].split('\t');
    let argv = parts.next().unwrap();
    let _quiet = parts.next().unwrap();
    let _fix = parts.next().unwrap();
    let _compile_db = parts.next().unwrap();
    let jobs = parts.next().unwrap();
    assert!(argv.contains("-j 4"));
    assert_eq!(jobs, "4");
}

#[test]
fn files_passed_in_deterministic_order() {
    let _guard = tidy_lock();
    let dir = TempDir::new().unwrap();
    let a = dir.child("a.cc");
    let b = dir.child("b.cc");
    let c = dir.child("c.cc");
    for path in [&a, &b, &c] {
        path.write_str("int x() { return 0; }\n").unwrap();
    }
    let record = dir.child("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        // Pass files out of order *and* with a duplicate; the
        // runner is contracted to dedupe and sort.
        files: vec![
            c.to_path_buf(),
            a.to_path_buf(),
            b.to_path_buf(),
            a.to_path_buf(),
        ],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", record.path());
    }
    let report = run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }
    assert_eq!(report, TidyReport::Tidied { files_processed: 3 });

    let lines = read_record(record.path());
    let mut parts = lines[0].split('\t');
    for _ in 0..5 {
        parts.next().unwrap();
    }
    let files = parts.next().unwrap();
    let positions: Vec<usize> = [&a, &b, &c]
        .iter()
        .map(|p| {
            files
                .find(&p.display().to_string())
                .unwrap_or_else(|| panic!("{} not in argv: {files}", p.display()))
        })
        .collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "expected ascending file order, got positions {positions:?} in {files}"
    );
}
