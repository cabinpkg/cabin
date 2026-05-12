use std::path::PathBuf;

use thiserror::Error;

/// Errors produced while serialising a [`cabin_build::BuildGraph`] as Ninja
/// or as `compile_commands.json`.
#[derive(Debug, Error)]
pub enum NinjaError {
    #[error("failed to write {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to serialize compile_commands.json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("path {} contains a newline; cannot be encoded in Ninja syntax", .0.display())]
    PathHasNewline(PathBuf),

    /// A Ninja variable value (e.g. `command` or `description`)
    /// carried a literal `\n` or `\r`. Such a value would terminate
    /// the variable assignment line and let the next-line content be
    /// re-parsed as a fresh Ninja statement, so we refuse it before
    /// the file is written. Carries the offending value verbatim so
    /// the diagnostic points at the manifest-derived input.
    #[error("Ninja variable value contains a newline or carriage return: {0:?}")]
    ValueHasNewline(String),

    #[error("path {} cannot be represented as UTF-8", .0.display())]
    NonUtf8Path(PathBuf),
}
