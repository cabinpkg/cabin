use thiserror::Error;

use crate::lints::LintFinding;

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

    /// One or more rejecting standard-compatibility lints (PL1) failed
    /// the publish before any registry artifact or index write.
    #[error("{}", format_lint_errors(.0))]
    StandardCompatibility(Vec<LintFinding>),

    #[error(transparent)]
    Package(#[from] cabin_package::PackageError),

    #[error(transparent)]
    Registry(#[from] cabin_registry_file::RegistryError),
}

/// Render the rejecting lint findings as a single stderr message.
fn format_lint_errors(findings: &[LintFinding]) -> String {
    let mut message =
        String::from("standard-compatibility checks rejected this publish before any write:");
    for finding in findings {
        message.push_str("\n  - ");
        message.push_str(&finding.message);
    }
    message
}
