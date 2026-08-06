//! Shared test-support harness for the guard's differential test
//! binaries (`superseded_differential.rs`,
//! `migrations_pending_differential.rs`, `await_deploy_differential.rs`).
//!
//! This module exists so the tool probe and the git environment - the
//! two things every scenario's reproducibility rests on - live in
//! exactly one place.  Each test binary declares `mod common;` and the
//! file is compiled as a private submodule of that binary (Cargo does
//! not treat `tests/common/mod.rs` as its own test target).
//!
//! Each test binary uses a different subset of these helpers, so the
//! ones a given binary does not reach are not dead code in any
//! meaningful sense - silence the per-binary `dead_code` lint here.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

/// Handed to every git call the harness makes and to both sides of
/// every run, so the corpus is reproducible and the machine's own git
/// configuration is neither read nor writable.
pub const GIT_ENVIRONMENT: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "cabin differential"),
    ("GIT_AUTHOR_EMAIL", "differential@example.invalid"),
    ("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+00:00"),
    ("GIT_COMMITTER_NAME", "cabin differential"),
    ("GIT_COMMITTER_EMAIL", "differential@example.invalid"),
    ("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+00:00"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_SYSTEM", "/dev/null"),
    // An unreachable remote must fail rather than block on a credential
    // prompt: an interactive git would hang the suite, not fail it.
    ("GIT_TERMINAL_PROMPT", "0"),
];

pub fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub fn git(dir: &Path, args: &[&str]) {
    let _ = git_output(dir, args);
}

pub fn git_output(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.current_dir(dir).args(args);
    for &(name, value) in GIT_ENVIRONMENT {
        command.env(name, value);
    }
    let done = command
        .output()
        .expect("running one of the harness's own git commands");
    assert!(
        done.status.success(),
        "the harness's `git {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&done.stderr)
    );
    String::from_utf8_lossy(&done.stdout).into_owned()
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

/// Whether every tool `test` drives is on `PATH`.  A differential
/// compares the port against the shell original, so without the
/// original's own tools there is nothing to compare and the scenario
/// skips rather than fails.
pub fn ready(test: &str, tools: &[&str]) -> bool {
    for tool in tools {
        if !have(tool) {
            eprintln!("skipping {test}: {tool} is not on PATH");
            return false;
        }
    }
    true
}
