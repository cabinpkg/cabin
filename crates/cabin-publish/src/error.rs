use thiserror::Error;

/// Errors produced by the publish workflow.
///
/// `cabin publish` requires either `--dry-run` (stage the package
/// to a directory without touching any registry) or
/// `--registry-dir` (publish into a local file registry);
/// otherwise [`PublishError::DryRunRequired`] is raised.
#[derive(Debug, Error)]
pub enum PublishError {
    #[error(
        "`cabin publish` requires either `--registry-dir <DIR>` to publish to a local file registry, or `--dry-run` to stage without modifying any registry"
    )]
    DryRunRequired,

    #[error(transparent)]
    Package(#[from] cabin_package::PackageError),

    #[error(transparent)]
    Registry(#[from] cabin_registry_file::RegistryError),
}
