//! The run's checks, split by the surface each covers and sequenced by
//! the crate's `run`.  One module per contiguous span of
//! `registry/scripts/smoke.sh`, in that script's order - except
//! `signin`, post-migration coverage with no shell ancestor.

pub mod anonymous;
pub mod blobs;
pub mod claims;
pub mod finale;
pub mod publish;
pub mod revisions;
pub mod session;
pub mod signin;
