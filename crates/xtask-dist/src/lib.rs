//! The release-packaging step of `.github/workflows/dist.yml`, ported
//! one-to-one from the `run:` body it replaces.
//!
//! The crate stages and archives what the run it was invoked from has
//! already built. It reads no `GITHUB_*` context - that is
//! `xtask-workflow-guard`'s reserved surface - so every value the
//! original spliced from the run's environment arrives as an argument
//! instead, and it writes no `$GITHUB_ENV`: it prints what it made and
//! leaves the workflow to record it.

pub mod package;
