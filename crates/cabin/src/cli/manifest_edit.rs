//! Shared manifest-document I/O for the manifest-editing commands
//! (`cabin add` / `cabin remove`): read a `cabin.toml` into a
//! format-preserving document and write the edited document back
//! atomically.  Both commands report read / parse / write failures
//! through the same context strings, so the wording lives here.

use std::path::Path;

use anyhow::{Context, Result};

use cabin_manifest::edit::{self, DocumentMut};

/// Read and parse `manifest_path` into a format-preserving TOML
/// document ready for editing.
pub(crate) fn read_document(manifest_path: &Path) -> Result<DocumentMut> {
    let text = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read manifest at {}", manifest_path.display()))?;
    edit::parse_document(&text)
        .with_context(|| format!("failed to parse manifest at {}", manifest_path.display()))
}

/// Atomically write the edited document back to `manifest_path`.
pub(crate) fn write_document(manifest_path: &Path, doc: &DocumentMut) -> Result<()> {
    cabin_fs::write_atomic(manifest_path, doc.to_string())
        .with_context(|| format!("failed to write manifest at {}", manifest_path.display()))
}
