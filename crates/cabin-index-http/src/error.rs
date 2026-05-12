use thiserror::Error;

/// Errors produced by the sparse HTTP index client.
#[derive(Debug, Error)]
pub enum IndexHttpError {
    #[error("invalid index URL `{url}`: {message}")]
    InvalidUrl { url: String, message: String },

    #[error("package `{name}` was not found in HTTP index")]
    PackageNotFound { name: String },

    #[error("HTTP index request failed for `{name}`: server returned {status}")]
    ServerError { name: String, status: u16 },

    #[error("HTTP transport error fetching `{name}`: {message}")]
    Transport { name: String, message: String },

    #[error("invalid package metadata from HTTP index for `{name}`: {message}")]
    InvalidMetadata { name: String, message: String },

    #[error("invalid file registry at `{base_url}`: {message}")]
    InvalidConfig { base_url: String, message: String },

    #[error("HTTP transport error: {0}")]
    PlainTransport(String),

    #[error(transparent)]
    Index(#[from] cabin_index::IndexError),

    #[error(
        "package name `{name}` is not valid; package names must consist only of ASCII letters, ASCII digits, `_`, `-`, and `.`, must be non-empty, must not start with `.` or `-`, and must not be `.` or `..`"
    )]
    UnsafePackageName { name: String },
}
