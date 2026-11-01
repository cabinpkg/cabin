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
        "cannot package build dependency `{name}`; package indexes do not carry build dependencies yet"
    )]
    BuildDependencyNotPublishable { name: String },

    #[error("manifest path {} has no parent directory", path.display())]
    ManifestPathHasNoParent { path: PathBuf },

    #[error(
        "source path `{}` for target {target:?} escapes the package root", path.display()
    )]
    SourceEscapesPackageRoot { target: String, path: PathBuf },

    #[error("package archive would not contain cabin.toml at its root")]
    ArchiveMissingManifest,

    #[error(
        "target {target:?} declares source `{path}` but the package archive would not contain it; \
         check its spelling and case, and that it is not under a directory packaging skips \
         (`build`, `dist`, ...)"
    )]
    SourceMissing { target: String, path: String },

    #[error(
        "manifest declares upstream patch `{path}` but the package archive would not contain it; \
         add the file or drop the `[package.upstream]` `patches` entry"
    )]
    UpstreamPatchMissing { path: String },

    #[error("upstream patch `{path}` is {size} bytes; at most {limit} are supported")]
    UpstreamPatchTooLarge {
        path: String,
        size: u64,
        limit: usize,
    },

    #[error(
        "`[package] include` entry `{0}` matches no file in the package (entries match path spelling exactly)"
    )]
    IncludeMatchedNothing(String),

    #[error("refusing to package symlink `{0}`: symlinks in package archives are not supported")]
    SymlinkNotSupported(String),

    #[error("refusing to package `{0}` because only regular files and directories are supported")]
    UnsupportedFileType(String),

    /// A collected file or directory has a path component that would
    /// name a different file on another platform Cabin targets (a
    /// Win32-reserved device name, a trailing dot or space, a
    /// forbidden character).
    #[error(
        "refusing to package `{path}`: path component is not portable across platforms ({detail})"
    )]
    NonPortablePath { path: String, detail: &'static str },

    /// Two archive paths that collide under case folding: equal when
    /// lowercased, or a file whose name folds to another entry's
    /// directory component.  A case-insensitive filesystem (macOS,
    /// Windows) would materialize one over the other.
    #[error("refusing to package `{first}` and `{second}`: paths collide when case is ignored")]
    CaseConflictingPaths { first: String, second: String },

    #[error("path `{}` is not valid UTF-8 and cannot appear in a package archive", path.display())]
    NonUtf8Path { path: PathBuf },

    #[error(
        "output file already exists with different bytes: {}; remove it and re-run",
        path.display()
    )]
    OutputAlreadyExists { path: PathBuf },

    /// `cabin package --output-dir` was pointed at the same
    /// directory as the package source root.  The walker never
    /// excludes the root itself, so a previous run's archive there
    /// would be archived as input and fail the idempotent-rewrite
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

    /// A target's implementation standard lies outside its declared
    /// interface range (below the minimum or above the maximum).
    #[error(transparent)]
    InterfaceStandardContradiction(#[from] cabin_core::InterfaceStandardContradiction),

    #[error(
        "failed to normalize `{{ workspace = true }}` markers in the archived manifest at {}: {source}",
        path.display()
    )]
    ManifestNormalization {
        path: PathBuf,
        #[source]
        source: Box<cabin_manifest::edit::EditError>,
    },

    /// The on-disk manifest parsed with a `{ workspace = true }`
    /// marker, but the archive normalizer found nothing to rewrite.
    /// Archiving the raw bytes would publish a live marker.
    #[error(
        "internal error: the manifest at {} carries a `{{ workspace = true }}` marker the archive normalizer did not rewrite",
        path.display()
    )]
    ManifestNormalizationIncomplete { path: PathBuf },

    /// `cabin package` was asked to archive a manifest with a
    /// non-empty `[patch]` table.  The suggested `.cabin/config.toml`
    /// is excluded from package archives by the `.cabin/` exclusion
    /// rule.
    #[error(
        "package `{name}` declares a `[patch]` table; patches are local development policy and not publishable. Remove the [patch] table from this manifest before packaging, or move the patches to a .cabin/config.toml file."
    )]
    PatchTableNotPublishable { name: String },

    #[error(
        "package `{name}` declares a `[system-provided]` table; which packages come from the system is the consuming workspace's choice and not publishable. Remove the table from this manifest before packaging."
    )]
    SystemProvidedTableNotPublishable { name: String },

    #[error(
        "package version `{version}` carries build metadata; registry versions are plain upstream versions - drop the `+...` suffix (packaging corrections are published as revisions of the same version)"
    )]
    VersionBuildMetadataNotPublishable { version: String },
}
