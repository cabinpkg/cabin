use std::path::PathBuf;

use thiserror::Error;

/// Errors produced while serializing a [`cabin_build::BuildGraph`] as Ninja
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

    /// Lowering a semantic [`cabin_build::BuildAction`] into a concrete
    /// command failed — in practice, a path that must be embedded in
    /// the command line is not valid UTF-8. The wrapped
    /// [`cabin_build::BuildError`] carries the offending path.
    #[error(transparent)]
    Lowering(#[from] cabin_build::BuildError),

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

    /// A command argument could not be safely encoded as a POSIX
    /// shell token (for example, it contained a NUL byte). The
    /// generated `build.ninja` and `compile_commands.json` would
    /// otherwise be ambiguous when re-parsed by `sh`, so we refuse
    /// the whole render. Carries the offending argument verbatim.
    #[error("command argument cannot be shell-quoted: {0:?}")]
    UnquotableArgument(String),

    #[error("path {} cannot be represented as UTF-8", .0.display())]
    NonUtf8Path(PathBuf),
}
