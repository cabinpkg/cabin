//! Test-only stand-in for `clang-format`.
//!
//! The Cabin test suite needs a deterministic formatter
//! executable without assuming a real `clang-format` is on
//! every developer's machine.  This binary mimics the small
//! subset of clang-format's surface area `cabin fmt` invokes:
//!
//! - **Write mode** (`-i` is present in argv): for every file
//!   argument that *does not already end with the sentinel
//!   marker* `/* FORMATTED */`, append the marker.  The
//!   process exits with status 0.
//! - **Check mode** (`--dry-run` and `-Werror` are present): for
//!   every file argument, exit with status 1 if at least one
//!   file lacks the sentinel marker; otherwise exit 0.  No
//!   files are written in this mode.
//!
//! Files that do not exist cause an exit code of 2 and a
//! single-line error on stderr, matching how `clang-format`
//! itself behaves when handed a missing path.
//!
//! `--style=file` (or any other `--style=` value) is required:
//! if absent, the fake exits with status 3.  This turns the
//! library's contract - that `cabin fmt` always passes
//! `--style=file` - into a real assertion every integration
//! test reaches for free.
//!
//! The sentinel marker is an implementation detail of the
//! tests; production code never sees it.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

const MARKER: &str = "/* FORMATTED */";

fn main() -> ExitCode {
    // `cabin fmt` scrubs the registry credential before spawning the
    // formatter; failing loudly here turns every integration test
    // into an enforcement point for that contract (same pattern as
    // the `--style=file` assertion below).
    if std::env::var_os(cabin_env::CABIN_REGISTRY_TOKEN).is_some() {
        eprintln!("fake formatter: CABIN_REGISTRY_TOKEN leaked into the tool environment");
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
        // Real clang-format rewrites the file in place, which is
        // good enough to simulate.
        for path in &files {
            let body = match fs::read_to_string(path) {
                Ok(b) => b,
                Err(err) => {
                    eprintln!("cabin-fmt-fake-formatter: {}: {}", path.display(), err);
                    return ExitCode::from(2);
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
                    return ExitCode::from(2);
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
                    return ExitCode::from(2);
                }
            };
            if !body.trim_end().ends_with(MARKER) {
                needs_format = true;
                eprintln!("{}: would be reformatted", path.display());
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
