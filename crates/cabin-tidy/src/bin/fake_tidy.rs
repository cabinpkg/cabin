//! Test-only stand-in for `run-clang-tidy`.
//!
//! The Cabin test suite needs a deterministic tidy driver
//! without assuming a real LLVM install on every developer's
//! machine.  This binary mimics the small subset of
//! `run-clang-tidy`'s behavior `cabin tidy` exercises:
//!
//! - Recognizes `-p <dir>`, `-fix`, `-quiet`, and `-j <N>` flags
//!   so tests can assert Cabin built the command line correctly.
//! - Treats every remaining argument as a file path.  For each
//!   file:
//!   - if its contents contain the sentinel marker
//!     `// CABIN-TIDY-FAIL`, emit a deterministic
//!     `<path>:1:1: warning: fake-clang-tidy diagnostic`-shaped
//!     line on stderr and remember to exit non-zero;
//!   - otherwise, in non-quiet mode, print a "checked" status
//!     line on stdout;
//!   - in `-quiet` mode, print nothing for clean files.
//! - If `CABIN_FAKE_TIDY_RECORD` points at a writable path,
//!   append a tab-separated record of the invocation so tests
//!   can inspect the exact argv Cabin produced.  This
//!   side-channel is opt-in; tests that don't set the env never
//!   write a file.
//!
//! Files that do not exist cause an exit code of 2 and a
//! single-line error on stderr, matching how `clang-tidy` itself
//! behaves when handed a missing path.
//!
//! The sentinel marker is an implementation detail of the
//! tests; production source code never contains it.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const FAIL_MARKER: &str = "// CABIN-TIDY-FAIL";

fn main() -> ExitCode {
    let mut quiet = false;
    let mut fix = false;
    let mut compile_database_dir: Option<String> = None;
    let mut jobs: Option<String> = None;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut raw_argv: Vec<String> = Vec::new();

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        raw_argv.push(arg.clone());
        match arg.as_str() {
            "-quiet" => quiet = true,
            "-fix" => fix = true,
            "-p" => {
                let Some(next) = iter.next() else {
                    eprintln!("cabin-tidy-fake-tidy: -p requires a directory argument");
                    return ExitCode::from(3);
                };
                raw_argv.push(next.clone());
                compile_database_dir = Some(next);
            }
            "-j" => {
                let Some(next) = iter.next() else {
                    eprintln!("cabin-tidy-fake-tidy: -j requires a count argument");
                    return ExitCode::from(3);
                };
                raw_argv.push(next.clone());
                jobs = Some(next);
            }
            other => files.push(PathBuf::from(other)),
        }
    }

    if let Some(record_path) = std::env::var_os("CABIN_FAKE_TIDY_RECORD") {
        let record = serde_record(
            &raw_argv,
            quiet,
            fix,
            compile_database_dir.as_deref(),
            jobs.as_deref(),
            &files,
        );
        if let Err(err) = append_line(&PathBuf::from(record_path), &record) {
            eprintln!("cabin-tidy-fake-tidy: failed to record invocation: {err}");
        }
    }

    let mut had_failure = false;
    for path in &files {
        let body = match fs::read_to_string(path) {
            Ok(body) => body,
            Err(err) => {
                eprintln!("cabin-tidy-fake-tidy: {}: {}", path.display(), err);
                return ExitCode::from(2);
            }
        };
        if body.contains(FAIL_MARKER) {
            had_failure = true;
            eprintln!(
                "{}:1:1: warning: fake-clang-tidy diagnostic [test-check]",
                path.display()
            );
        } else if !quiet {
            println!("checked {}", path.display());
        }
    }

    if had_failure {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Render a single invocation record as one tab-separated line:
/// `argv\tquiet\tfix\tcompile-db-dir\tjobs\tfiles`.  Tabs and
/// newlines in argv tokens are escaped so the line stays
/// parseable.
fn serde_record(
    argv: &[String],
    quiet: bool,
    fix: bool,
    compile_database_dir: Option<&str>,
    jobs: Option<&str>,
    files: &[PathBuf],
) -> String {
    let argv = argv.iter().map(|s| escape(s)).collect::<Vec<_>>().join(" ");
    let compile_database_dir = compile_database_dir.unwrap_or("");
    let jobs = jobs.unwrap_or("");
    let files = files
        .iter()
        .map(|p| escape(&p.display().to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{argv}\t{quiet}\t{fix}\t{compile_database_dir}\t{jobs}\t{files}")
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn append_line(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}
