//! Capture stable, privacy-safe build metadata for `cabin
//! version --verbose`.
//!
//! Every value collected here is either:
//! - already public and stable (the git commit hash, ISO commit
//!   date, the rustc identity, the build profile, the target
//!   triple), or
//! - intentionally omitted when the underlying source is
//!   unavailable (a published crate tarball without `.git`, a
//!   compiler without a parseable `-vV` output, …).
//!
//! Deliberate exclusions: absolute paths, usernames, hostnames,
//! local checkout paths, Git working-tree status, build
//! timestamps.  Builds without git or without a working `rustc
//! -vV` succeed normally - the missing fields render as
//! `unknown` at runtime in the verbose version output.

use std::path::Path;
use std::process::Command;

fn main() {
    // The metadata captured here is read through `option_env!`
    // in `src/version_info.rs`.  Every emitted variable is
    // typed as a `String`, so the runtime layer never has to
    // parse build-script output.
    emit_git_commit();
    emit_git_commit_date();
    emit_target_triple();
    emit_rerun_directives();
}

/// Capture the full git commit hash if a usable `.git` is
/// present.  Missing git, a shallow tarball, or a `git` binary
/// without HEAD access all gracefully fall through; the runtime
/// layer omits the `commit-hash:` line in that case.  The
/// formatter derives the short prefix shown in the header from
/// the same value, so a single source of truth is captured.
fn emit_git_commit() {
    if !workspace_has_git_dir() {
        return;
    }
    if let Some(full) = run_git(&["rev-parse", "HEAD"]) {
        println!("cargo:rustc-env=CABIN_BUILD_COMMIT={full}");
    }
}

/// Capture the ISO-8601 date (`YYYY-MM-DD`) of the HEAD commit.
/// Authored date in UTC keeps the value reproducible across
/// machines with different local timezones.
fn emit_git_commit_date() {
    if !workspace_has_git_dir() {
        return;
    }
    if let Some(raw) = run_git(&[
        "-c",
        "log.showSignature=false",
        "log",
        "-1",
        "--date=short",
        "--pretty=%cd",
    ]) {
        let date = raw.trim();
        if !date.is_empty() {
            println!("cargo:rustc-env=CABIN_BUILD_COMMIT_DATE={date}");
        }
    }
}

/// Cargo always sets `TARGET` for build scripts - re-emit it
/// for runtime visibility without a separate probe.  The
/// formatter renders the value behind a `host:` label so the
/// verbose output matches cargo's own version block.
fn emit_target_triple() {
    if let Ok(target) = std::env::var("TARGET")
        && !target.is_empty()
    {
        println!("cargo:rustc-env=CABIN_BUILD_HOST={target}");
    }
}

/// Tell cargo to rerun the build script when the workspace
/// switches commits.  Watch three files because each one
/// flips for a different kind of git operation:
/// - `.git/HEAD` - branch switch / detached HEAD;
/// - `.git/packed-refs` - refs repack;
/// - `.git/logs/HEAD` - every commit, checkout, and reflog
///   write on the current branch (so a fresh `git commit`
///   that does not touch a tracked source file still
///   refreshes the captured short hash).
fn emit_rerun_directives() {
    // Cargo's own rebuild rules already cover source changes -
    // these directives only matter when the git metadata
    // changes without a touch to a tracked source file.
    println!("cargo:rerun-if-changed=build.rs");
    if workspace_has_git_dir() {
        for relative in [".git/HEAD", ".git/packed-refs", ".git/logs/HEAD"] {
            let path = workspace_root().join(relative);
            if path.is_file() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
        // If `.git` is itself a file (worktree pointer) rather
        // than a directory, skip; the linked git dir is private
        // to the worktree and not stable to watch from here.
    }
}

fn run_git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace_root())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim().to_owned();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Workspace root resolved from `CARGO_MANIFEST_DIR/../..`.
/// The `cabin` crate lives in `crates/cabin`; the workspace root
/// is two parents up.  Falls back to the manifest dir itself if
/// the layout ever changes, so a developer rename does not
/// turn into a hard build failure.
fn workspace_root() -> std::path::PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_owned());
    let manifest_path = Path::new(&manifest_dir);
    manifest_path
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| manifest_path.to_path_buf(), Path::to_path_buf)
}

fn workspace_has_git_dir() -> bool {
    workspace_root().join(".git").exists()
}
