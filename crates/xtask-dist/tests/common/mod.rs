//! Shared test-support harness for the release tooling's differential
//! test binaries (`package_differential.rs`, `checksums_differential.rs`).
//!
//! This module exists so the tool probe every scenario gates on lives
//! in exactly one place.  Each test binary declares `mod common;` and
//! the file is compiled as a private submodule of that binary (Cargo
//! does not treat `tests/common/mod.rs` as its own test target).
//!
//! Each test binary uses a different subset of these helpers, so the
//! ones a given binary does not reach are not dead code in any
//! meaningful sense - silence the per-binary `dead_code` lint here.
#![allow(dead_code)]

use std::process::Command;

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
