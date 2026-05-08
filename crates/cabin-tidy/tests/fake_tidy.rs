//! End-to-end tests for the `cabin-tidy` library that exercise
//! a real subprocess via the bundled fake tidy binary.
//!
//! Gated on the `test-fake-tidy` feature so the binary the tests
//! need is guaranteed to be built; a standalone
//! `cargo test -p cabin-tidy` (without the feature) compiles
//! this file to nothing and the lib unit tests run on their own.

#![cfg(feature = "test-fake-tidy")]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
    let candidate = dir.join("cabin-tidy-fake-tidy");
    assert!(
        candidate.is_file(),
        "expected fake tidy at {}; build cabin-tidy with `--features test-fake-tidy`",
        candidate.display()
    );
    candidate
}

fn write_file(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn read_record(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn clean_files_yield_tidied_report() {
    let _guard = tidy_lock();
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("main.cc");
    write_file(&file, "int main() { return 0; }\n");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file],
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
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("bad.cc");
    write_file(&file, "// CABIN-TIDY-FAIL\nint main() {}\n");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file],
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
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("main.cc");
    write_file(&file, "int main() {}\n");
    let record = dir.path().join("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", &record);
    }
    let report = run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }
    assert_eq!(report, TidyReport::Tidied { files_processed: 1 });

    let lines = read_record(&record);
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
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("main.cc");
    write_file(&file, "int main() {}\n");
    let record = dir.path().join("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Verbose,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", &record);
    }
    let report = run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }
    assert_eq!(report, TidyReport::Tidied { files_processed: 1 });

    let lines = read_record(&record);
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
    let dir = tempfile::TempDir::new().unwrap();
    let build_dir = dir.path().join("build/dev");
    std::fs::create_dir_all(&build_dir).unwrap();
    let file = dir.path().join("main.cc");
    write_file(&file, "int main() {}\n");
    let record = dir.path().join("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: build_dir.clone(),
        files: vec![file],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", &record);
    }
    run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }

    let lines = read_record(&record);
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
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("main.cc");
    write_file(&file, "int main() {}\n");
    let record = dir.path().join("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file],
        mode: TidyMode::Fix,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", &record);
    }
    run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }

    let lines = read_record(&record);
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
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("main.cc");
    write_file(&file, "int main() {}\n");
    let record = dir.path().join("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        files: vec![file],
        mode: TidyMode::Check,
        jobs: Some(BuildJobs::new(4).unwrap()),
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", &record);
    }
    run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }

    let lines = read_record(&record);
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
    let dir = tempfile::TempDir::new().unwrap();
    let a = dir.path().join("a.cc");
    let b = dir.path().join("b.cc");
    let c = dir.path().join("c.cc");
    for path in [&a, &b, &c] {
        write_file(path, "int x() { return 0; }\n");
    }
    let record = dir.path().join("argv.log");

    let req = TidyRequest {
        executable: OsString::from(fake_tidy_path()),
        compile_database_dir: dir.path().to_path_buf(),
        // Pass files out of order *and* with a duplicate; the
        // runner is contracted to dedupe and sort.
        files: vec![c.clone(), a.clone(), b.clone(), a.clone()],
        mode: TidyMode::Check,
        jobs: None,
        verbosity: TidyVerbosity::Normal,
    };
    unsafe {
        std::env::set_var("CABIN_FAKE_TIDY_RECORD", &record);
    }
    let report = run_tidy(&req).unwrap();
    unsafe {
        std::env::remove_var("CABIN_FAKE_TIDY_RECORD");
    }
    assert_eq!(report, TidyReport::Tidied { files_processed: 3 });

    let lines = read_record(&record);
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
