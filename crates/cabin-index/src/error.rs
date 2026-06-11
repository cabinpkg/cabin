use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Errors produced while loading a local JSON package index.
#[derive(Debug, Error)]
pub enum IndexError {
    #[error("failed to read index entry {path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to parse index entry {path}: {source}", path = path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error(
        "index file {path} declares package {declared:?} but the filename stem is {expected:?}",
        path = path.display()
    )]
    NameMismatch {
        path: PathBuf,
        declared: String,
        /// The name the entry was expected to declare. For the file
        /// loader this is the filename stem; for the HTTP fetcher it
        /// is the requested package name. Kept transport-neutral so
        /// the HTTP caller does not carry file-loader vocabulary.
        expected: String,
    },

    #[error(
        "index entry {path} uses unsupported schema version {schema}; only schema 1 is supported",
        path = path.display()
    )]
    UnsupportedSchema { path: PathBuf, schema: u32 },

    #[error("index path {path} is not a directory", path = path.display())]
    NotADirectory { path: PathBuf },

    #[error(
        "invalid index entry for package {package:?}: version {value:?} is not valid SemVer ({source})"
    )]
    InvalidVersion {
        package: String,
        value: String,
        #[source]
        source: semver::Error,
    },

    #[error(
        "invalid index entry for package {package:?} version {version}: dependency {dep:?} declares a compiler-conditioned `target` ({condition:?}); compiler identity is detected from the local toolchain, so index dependency gates must stay platform-only"
    )]
    CompilerConditionedDependency {
        package: String,
        version: String,
        dep: String,
        condition: String,
    },

    #[error(
        "invalid index entry for package {package:?} version {version}: dependency {dep:?} has invalid requirement {requirement:?} ({source})"
    )]
    InvalidRequirement {
        package: String,
        version: String,
        dep: String,
        requirement: String,
        #[source]
        source: semver::Error,
    },

    #[error("invalid index entry for package {package:?}: {message}")]
    InvalidPackageName { package: String, message: String },

    #[error(
        "unsupported source type {value:?} for package {package:?} version {version}; only local archive sources are currently supported"
    )]
    UnsupportedSourceType {
        package: String,
        version: String,
        value: String,
    },

    #[error(
        "unsupported source format {value:?} for package {package:?} version {version}; only `tar.gz` is currently supported"
    )]
    UnsupportedSourceFormat {
        package: String,
        version: String,
        value: String,
    },

    #[error("missing `source.path` for package {package:?} version {version}")]
    MissingSourcePath { package: String, version: String },

    #[error(
        "invalid file registry at {}: {message}", path.display()
    )]
    InvalidRegistryConfig { path: PathBuf, message: String },
}
