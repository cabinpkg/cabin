use thiserror::Error;

/// Append the registry's `Retry-After` seconds to the over-budget
/// message when the response carried a usable value, mirroring the
/// publish-side rendering in `cabin-registry-api`.
fn with_retry(base: &str, retry_after_secs: Option<u64>) -> String {
    match retry_after_secs {
        Some(1) => format!("{base}; try again in 1 second"),
        Some(secs) => format!("{base}; try again in {secs} seconds"),
        None => format!("{base}; try again later"),
    }
}

/// Errors produced by the sparse HTTP index client.
#[derive(Debug, Error)]
pub enum IndexHttpError {
    #[error("invalid index URL `{url}`: {message}")]
    InvalidUrl { url: String, message: String },

    #[error("package `{name}` was not found in HTTP index")]
    PackageNotFound { name: String },

    #[error("HTTP index request failed for `{name}`: server returned {status}")]
    ServerError { name: String, status: u16 },

    #[error(
        "authentication required by registry `{origin}`; run `cabin login --index-url {origin}` \
         with `-Z remote-registry` to store a token"
    )]
    AuthRequired { origin: String },

    #[error(
        "registry `{origin}` rejected the stored token (revoked or expired); re-run `cabin login \
         --index-url {origin}`"
    )]
    TokenRejected { origin: String },

    #[error(
        "registry `{origin}` refused the request: the stored token does not have the required \
         scope"
    )]
    MissingScope { origin: String },

    #[error("{}", with_retry(
        "the registry has temporarily disabled package downloads and index reads to stay within \
         its infrastructure budget",
        *.retry_after_secs,
    ))]
    RegistryOverBudget { retry_after_secs: Option<u64> },

    #[error("HTTP transport error fetching `{name}`: {message}")]
    Transport { name: String, message: String },

    #[error("invalid package metadata from HTTP index for `{name}`: {message}")]
    InvalidMetadata { name: String, message: String },

    #[error("invalid file registry at `{base_url}`: {message}")]
    InvalidConfig { base_url: String, message: String },

    #[error(transparent)]
    Index(#[from] cabin_index::IndexError),

    #[error(
        "package name `{name}` cannot be fetched from a remote registry; a name is either bare or `<scope>/<name>` (exactly one `/`), and each part must consist only of ASCII letters, ASCII digits, `_`, `-`, and `.`, must be non-empty, must not start with `.` or `-`, and must not be `.` or `..`"
    )]
    UnsafePackageName { name: String },
}
