//! Manifest parsing for Cabin.
//!
//! `cabin.toml` is parsed via private serde structs and immediately
//! converted into [`cabin_core::Package`].  Only the conversion API is
//! public - raw TOML structures must not leak across the crate
//! boundary, so callers cannot accidentally couple to the on-disk schema.

pub mod edit;
pub mod span;

mod error;
mod parse;
mod raw;

pub use error::{ManifestError, ManifestParseError};
pub use parse::{ParsedManifest, RootSettings, WorkspaceTable, load_manifest, parse_manifest_str};
pub use span::{StandardFieldScope, standard_field_span};
