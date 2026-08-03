//! Static guards over the hosted registry Worker
//! (`registry/`), run by `registry.yml` on every pull
//! request.
//!
//! Crate boundaries: the guards inspect committed sources and
//! configuration only.  They take no credentials, make no network
//! calls, and mutate nothing - which is why they are separate from the
//! operator tooling that does all three.  Each guard reports its
//! violations as diagnostic lines and leaves exit codes and printing to
//! the binary.
//!
//! The guards are lexical, not syntactic: they are regression tripwires
//! that force review at a seam, not proofs.  Each module states its own
//! ceiling.

pub mod deploy;
pub mod lexical;
pub mod r2;
pub mod source;
pub mod sql;

use std::path::{Path, PathBuf};

/// The `registry/` directory of the checkout this tool was built from.
///
/// Resolved from the crate's own manifest directory rather than the
/// working directory: the Cargo aliases are run from the repository
/// root, but nothing about the guards should depend on that.
#[must_use]
pub fn registry_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry")
}
