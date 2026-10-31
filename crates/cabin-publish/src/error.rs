use thiserror::Error;

use crate::lints::LintFinding;

/// Errors produced by the publish workflow.
///
/// `cabin publish` raises [`PublishError::DryRunRequired`] when it
/// has no destination: neither `--dry-run` (stage the package to a
/// directory without touching any registry) nor `--registry-dir`
/// (publish into a local file registry) is given and no HTTP index
/// is configured, or `--output-dir` is given without `--dry-run` or
/// `--registry-dir`.
#[derive(Debug, Error)]
pub enum PublishError {
    #[error(
        "`cabin publish` requires either `--registry-dir <DIR>` to publish to a local file registry, or `--dry-run` to stage without modifying any registry"
    )]
    DryRunRequired,

    /// The staged package carries a bare (unscoped) name.  Registry
    /// packages are always `<scope>/<name>`; bare names exist only in
    /// local manifests.  Raised before any lint, registry, or network
    /// work so the fix is the first thing the user sees.
    #[error(
        "registry packages must be named `<scope>/<name>`, but `{name}` is a bare name; in {manifest_path}, change\n    name = \"{name}\"\nto your claimed scope, e.g.\n    name = \"<scope>/{name}\"\n(local-only builds and path dependencies may keep bare names)"
    )]
    BarePackageName { name: String, manifest_path: String },

    /// A `dependencies` or `dev-dependencies` key is not a scoped
    /// `<scope>/<name>` registry name (see
    /// [`crate::require_scoped_dependency_names`]).  Raised beside
    /// [`PublishError::BarePackageName`], before any registry or
    /// network work.
    #[error(
        "registry dependencies must use the canonical `<scope>/<name>` grammar (lowercase throughout), but `{name}` in [{table}] does not; in {manifest_path}, change the dependency key to the package's registry name (path dependencies may keep bare names)"
    )]
    InvalidDependencyName {
        table: &'static str,
        name: String,
        manifest_path: String,
    },

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
