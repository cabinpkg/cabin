//! Manifest parsing for Cabin.
//!
//! `cabin.toml` is parsed via private serde structs and immediately
//! converted into [`cabin_core::Package`]. Only the conversion API is
//! public — raw TOML structures must not leak across the crate
//! boundary, so callers cannot accidentally couple to the on-disk schema.

// `ManifestError` is intentionally large: the source-annotated
// `TomlAt` variant carries the original manifest text + span so
// the diagnostic renderer can draw a snippet. Callers see the
// error only on the failure path, so the size cost is bounded
// to one allocation per failed load. Boxing the variant would
// hide the diagnostic metadata behind an extra deref without
// changing peak memory usage.
#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

mod error;
mod parse;
mod raw;

pub use error::{ManifestError, ManifestParseError};
pub use parse::{ParsedManifest, RootSettings, WorkspaceTable, load_manifest, parse_manifest_str};
