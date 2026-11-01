//! Test-only stand-in for `clang-format`, which is not on every
//! developer's machine.  It mimics, deterministically, the subset
//! of clang-format's command line `cabin fmt` invokes:
//!
//! - **Write mode** (`-i` is present in argv): for every file
//!   argument that *does not already end with the sentinel
//!   marker* `/* FORMATTED */`, append the marker.  The
//!   process exits with status 0.
//! - **Check mode** (`--dry-run` and `-Werror` are present): for
//!   every file argument that lacks the sentinel marker, print
//!   clang-format's `[-Wclang-format-violations]` error on stderr,
//!   and exit with status 1 if there was any; otherwise exit 0.  A
//!   file containing `SILENT_FAILURE` makes the process exit 1
//!   with nothing on stderr, like a `CABIN_FMT` wrapper that drops
//!   clang-format's output.  No files are written in this mode.
//!
//! As in real `clang-format`, a file that cannot be read or written
//! causes exit status 1 and a single-line error on stderr.
//!
//! `--style=file` (or any other `--style=` value) is required:
//! if absent, the fake exits with status 3, so every test that
//! reaches the fake asserts that `cabin fmt` passes it.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

const MARKER: &str = "/* FORMATTED */";
const SILENT_FAILURE: &str = "/* FAIL SILENTLY */";

fn main() -> ExitCode {
    // `cabin fmt` scrubs the registry credential before spawning the
    // formatter; failing loudly here turns every integration test
    // into an enforcement point for that contract.
    if std::env::var_os(cabin_env::CABIN_REGISTRY_TOKEN).is_some() {
        eprintln!("fake formatter: CABIN_REGISTRY_TOKEN leaked into the tool environment");
        return ExitCode::from(4);
    }
    if std::env::var_os(cabin_env::ACTIONS_ID_TOKEN_REQUEST_TOKEN).is_some() {
        eprintln!(
            "fake formatter: ACTIONS_ID_TOKEN_REQUEST_TOKEN leaked into the tool environment"
        );
        return ExitCode::from(4);
    }
    let mut write_mode = false;
    let mut dry_run = false;
    let mut werror = false;
    let mut style_argument_seen = false;
    let mut files: Vec<PathBuf> = Vec::new();

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-i" => write_mode = true,
            "--dry-run" => dry_run = true,
            "-Werror" => werror = true,
            other if other.starts_with("--style=") => style_argument_seen = true,
            other => files.push(PathBuf::from(other)),
        }
    }

    if !style_argument_seen {
        eprintln!(
            "cabin-fmt-fake-formatter: expected --style=<value>; cabin fmt must always pass --style=file"
        );
        return ExitCode::from(3);
    }

    if write_mode {
        for path in &files {
            let body = match fs::read_to_string(path) {
                Ok(b) => b,
                Err(err) => {
                    eprintln!("cabin-fmt-fake-formatter: {}: {}", path.display(), err);
                    return ExitCode::FAILURE;
                }
            };
            if !body.trim_end().ends_with(MARKER) {
                let mut updated = body.clone();
                if !updated.ends_with('\n') {
                    updated.push('\n');
                }
                updated.push_str(MARKER);
                updated.push('\n');
                if let Err(err) = fs::write(path, updated) {
                    eprintln!("cabin-fmt-fake-formatter: {}: {}", path.display(), err);
                    return ExitCode::FAILURE;
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    if dry_run && werror {
        let mut needs_format = false;
        for path in &files {
            let body = match fs::read_to_string(path) {
                Ok(b) => b,
                Err(err) => {
                    eprintln!("cabin-fmt-fake-formatter: {}: {}", path.display(), err);
                    return ExitCode::FAILURE;
                }
            };
            if body.contains(SILENT_FAILURE) {
                return ExitCode::FAILURE;
            }
            if !body.trim_end().ends_with(MARKER) {
                needs_format = true;
                eprintln!(
                    "{}:1:1: error: code should be clang-formatted [-Wclang-format-violations]",
                    path.display()
                );
            }
        }
        if needs_format {
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }

    // No mode flags at all: behave as a no-op success so an
    // accidental bare invocation in tests does not damage
    // fixtures.
    ExitCode::SUCCESS
}
