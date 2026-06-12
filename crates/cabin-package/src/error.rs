use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Errors produced while validating, archiving, or describing a
/// package for publication.
#[derive(Debug, Error)]
pub enum PackageError {
    #[error("failed to load manifest at {}: {source}", path.display())]
    Manifest {
        path: PathBuf,
        #[source]
        source: Box<cabin_manifest::ManifestError>,
    },

    #[error("failed to read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error(
        "cannot package workspace root without a [package] section; pass --manifest-path for a package"
    )]
    WorkspaceRootHasNoProject,

    #[error("cannot package path dependency `{name}`; path dependencies are not publishable")]
    PathDependencyNotPublishable { name: String },

    #[error(
        "cannot package port dependency `{name}`; foundation-port dependencies describe local development policy and are not publishable"
    )]
    PortDependencyNotPublishable { name: String },

    #[error("manifest path {} has no parent directory", path.display())]
    ManifestPathHasNoParent { path: PathBuf },

    #[error(
        "source path `{}` for target {target:?} escapes the package root", path.display()
    )]
    SourceEscapesPackageRoot { target: String, path: PathBuf },

    #[error(
        "include directory `{}` for target {target:?} escapes the package root", path.display()
    )]
    IncludeEscapesPackageRoot { target: String, path: PathBuf },

    #[error("package archive would not contain cabin.toml at its root")]
    ArchiveMissingManifest,

    #[error("refusing to package symlink `{0}`: symlinks in package archives are not supported")]
    SymlinkNotSupported(String),

    #[error("refusing to package `{0}` because only regular files and directories are supported")]
    UnsupportedFileType(String),

    #[error("path `{}` is not valid UTF-8 and cannot appear in a package archive", path.display())]
    NonUtf8Path { path: PathBuf },

    #[error(
        "output file already exists with different bytes: {}; remove it and re-run",
        path.display()
    )]
    OutputAlreadyExists { path: PathBuf },

    /// `cabin package --output-dir` was pointed at the same
    /// directory as the package source root. Without rejecting
    /// this, the staging walker would treat the next run's
    /// archive as input on subsequent invocations and either
    /// embed it in the archive or fail the idempotent-rewrite
    /// check.
    #[error(
        "--output-dir {} equals the package source root; choose a directory outside the package or remove --output-dir to use the default `dist/`",
        path.display()
    )]
    OutputDirIsPackageRoot { path: PathBuf },

    #[error("failed to write package archive: {0}")]
    ArchiveWrite(#[source] io::Error),

    #[error("failed to render package metadata as JSON: {0}")]
    Metadata(#[from] serde_json::Error),

    #[error(
        "dependency `{name}` uses workspace = true, but package metadata was generated without workspace resolution; package this manifest from inside its workspace so [workspace.dependencies] can be applied"
    )]
    UnresolvedWorkspaceDependency { name: String },

    #[error(
        "`{field}` uses workspace = true, but package metadata was generated without workspace resolution; package this manifest from inside its workspace so the `[workspace]` standard defaults can be applied"
    )]
    UnresolvedWorkspaceStandard { field: &'static str },

    #[error(
        "failed to normalize `{{ workspace = true }}` standard fields in the archived manifest at {}: {source}",
        path.display()
    )]
    ManifestNormalization {
        path: PathBuf,
        #[source]
        source: Box<cabin_manifest::edit::EditError>,
    },

    /// The on-disk manifest parsed with a `{ workspace = true }`
    /// standard marker, but the archive normalizer found nothing to
    /// rewrite. Archiving the raw bytes would publish a live marker,
    /// so the mismatch fails loudly instead of shipping a
    /// non-self-contained manifest.
    #[error(
        "internal error: the manifest at {} carries a `{{ workspace = true }}` standard marker the archive normalizer did not rewrite",
        path.display()
    )]
    ManifestNormalizationIncomplete { path: PathBuf },

    #[error(
        "package name `{name}` is not path-safe for registry publishing; package names cannot contain `/`, `\\`, `..`, leading dots, or platform path prefixes"
    )]
    UnsafeRegistryPackageName { name: String },

    /// `cabin package` was asked to archive a manifest with a
    /// non-empty `[patch]` table. Patches are local development
    /// policy and must not enter published archives; remove the
    /// table or move the patches to a `.cabin/config.toml` file
    /// (the latter is excluded from package archives by the
    /// `.cabin/` exclusion rule).
    #[error(
        "package `{name}` declares a `[patch]` table; patches are local development policy and not publishable. Remove the [patch] table from this manifest before packaging, or move the patches to a .cabin/config.toml file."
    )]
    PatchTableNotPublishable { name: String },
}
