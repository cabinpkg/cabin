//! Guards that keep an out-of-order or premature workflow run from
//! mutating a shared resource, decided from git history and the GitHub
//! Actions run context.
//!
//! This is the only crate allowed to read `GITHUB_*` context, write
//! `$GITHUB_OUTPUT` / `$GITHUB_ENV`, and call the GitHub REST API. It
//! never touches the registry, and it holds no secret beyond the run's
//! own `GITHUB_TOKEN`.

pub mod superseded;
