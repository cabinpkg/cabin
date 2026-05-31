//! `cabin.lock` reader, writer, and validator.
//!
//! The lockfile records the registry packages and versions chosen by
//! the resolver. Local path packages are intentionally omitted; patch
//! and source-replacement policy is recorded only for stale-lockfile
//! detection under `--locked`.

#![allow(clippy::must_use_candidate)]

pub mod error;
pub mod io;
pub mod model;
pub mod validate;

pub use error::LockfileError;
pub use io::{read_lockfile, write_lockfile};
pub use model::{
    LOCKFILE_VERSION, LockedPackage, LockedPatch, LockedPatchKind, LockedSource,
    LockedSourceLocatorKind, LockedSourceReplacement, Lockfile,
};
pub use validate::validate;
