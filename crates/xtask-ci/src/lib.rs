//! The checks that keep this repository's own automation honest.
//!
//! Crate boundaries: this crate polices repository policy - what the
//! automation is written in, and (as the migration proceeds) what the
//! workflows are allowed to inline. It reads the committed tree and the
//! git index, takes no credentials, makes no network calls, and mutates
//! nothing.

pub mod scripts;

use std::path::{Path, PathBuf};

/// The root of the checkout this tool was built from.
///
/// Resolved from the crate's own manifest directory rather than the
/// working directory: the Cargo aliases are run from the repository
/// root, but nothing about the checks should depend on that.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
