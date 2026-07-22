//! Registry mapping known typed domain errors to the
//! diagnostic-rendering shape they should use.
//!
//! The `cabin` dispatcher returns `anyhow::Error`; the leaves
//! of that chain are typed domain errors (`ManifestError`,
//! `WorkspaceError`, `ConfigError`, …).  Some implement
//! [`miette::Diagnostic`] and carry rich source-spanned reports;
//! others only need a stable area code so the renderer can wrap
//! them in `error[<code>]: <message>`.
//!
//! [`downcast_diagnostic`] is the lookup: it walks the known
//! typed-error roots and yields a [`DiagnosticCandidate`] when
//! one matches.  Adding a new diagnostic-bearing error type is
//! a one-line edit here, isolated from the top-level
//! orchestration in `lib.rs::run`.
//!
//! The candidate type lives in this module too - it is the
//! vocabulary the registry uses to describe a renderable
//! error.  The top-level error renderer
//! ([`crate::error_rendering`]) then asks each candidate to
//! render itself through [`cabin_diagnostics`].

use termcolor::StandardStream;

/// A diagnostic recovered from one item in the anyhow source
/// chain.
pub(crate) enum DiagnosticCandidate<'a> {
    /// The domain error itself implements `miette::Diagnostic`
    /// and may carry source snippets or variant-specific help.
    Rich(&'a dyn cabin_diagnostics::miette::Diagnostic),
    /// The domain error is typed and user-facing, but only needs
    /// an area-level stable code.  Wrap it so rendering still goes
    /// through `cabin-diagnostics`.
    Coded { code: &'static str },
}

impl DiagnosticCandidate<'_> {
    pub(crate) fn render(
        &self,
        root: &anyhow::Error,
        stderr: &mut StandardStream,
    ) -> std::io::Result<()> {
        match self {
            Self::Rich(diag) => cabin_diagnostics::render(*diag, stderr),
            Self::Coded { code } => {
                let message = format!("{root:#}");
                let diagnostic = cabin_diagnostics::CodedMessage::new(&message, code);
                cabin_diagnostics::render(&diagnostic, stderr)
            }
        }
    }
}

/// Concrete-type probe for one entry of the area-coded table in
/// [`downcast_diagnostic`].
type Probe = fn(&(dyn std::error::Error + 'static)) -> bool;

/// Walk the known typed-error roots and yield a diagnostic
/// candidate when one matches.
///
/// Adding a new diagnostic-bearing error type is a one-line
/// change here.  The cost of explicit listing is small relative
/// to the boundary clarity it gives - we never accidentally
/// route a typed error away from the diagnostic renderer
/// because of an unsafe blanket impl.
pub(crate) fn downcast_diagnostic<'a>(
    err: &'a (dyn std::error::Error + 'static),
) -> Option<DiagnosticCandidate<'a>> {
    use cabin_diagnostics::code;

    // The order matters: try the most specific typed error
    // first, then fall through to looser wrappers that share
    // the same source chain.  ManifestError can hide either
    // standalone (e.g. `cabin package`) or behind
    // WorkspaceError::Manifest, so we look at the leaf first.
    if let Some(e) = err.downcast_ref::<cabin_manifest::ManifestError>() {
        return Some(DiagnosticCandidate::Rich(e));
    }
    // Workspace / artifact / package errors box their inner
    // `ManifestError` (the boxed variant keeps the outer error
    // small enough to pass `clippy::result_large_err`); the
    // chain walker would otherwise skip the manifest layer.
    if let Some(e) = err.downcast_ref::<Box<cabin_manifest::ManifestError>>() {
        return Some(DiagnosticCandidate::Rich(e.as_ref()));
    }
    if let Some(e) = err.downcast_ref::<cabin_workspace::WorkspaceError>() {
        return Some(DiagnosticCandidate::Rich(e));
    }
    if let Some(e) = err.downcast_ref::<cabin_system_deps::PkgConfigError>() {
        return Some(DiagnosticCandidate::Rich(e));
    }
    if let Some(e) = err.downcast_ref::<cabin_resolver::ResolveError>() {
        return Some(DiagnosticCandidate::Rich(e));
    }
    if let Some(e) = err.downcast_ref::<cabin_config::ConfigError>() {
        return Some(DiagnosticCandidate::Coded {
            code: config_error_code(e),
        });
    }

    // Everything below only needs an area-level code.
    // `BuildError::StandardFlagConflict` boxes the typed conflict
    // (keeping the enum small), so its chain element can be the
    // `Box` - both shapes are listed, like the boxed
    // `ManifestError` above.
    let coded: &[(Probe, &'static str)] = &[
        (
            |e| e.is::<cabin_lockfile::LockfileError>(),
            code::LOCKFILE_ERROR,
        ),
        (
            |e| e.is::<cabin_artifact::ArtifactError>(),
            code::ARTIFACT_ERROR,
        ),
        (|e| e.is::<cabin_build::BuildError>(), code::BUILD_ERROR),
        (
            |e| e.is::<cabin_core::StandardFlagConflict>(),
            code::LANGUAGE_STANDARD_FLAG_CONFLICT,
        ),
        (
            |e| e.is::<Box<cabin_core::StandardFlagConflict>>(),
            code::LANGUAGE_STANDARD_FLAG_CONFLICT,
        ),
        (
            |e| e.is::<cabin_package::PackageError>(),
            code::PACKAGE_ERROR,
        ),
        (
            |e| e.is::<cabin_toolchain::ToolchainError>(),
            code::TOOLCHAIN_ERROR,
        ),
        (
            |e| e.is::<cabin_toolchain::ToolchainDetectionFailure>(),
            code::TOOLCHAIN_ERROR,
        ),
        (
            |e| e.is::<cabin_toolchain::RunError>(),
            code::TOOLCHAIN_ERROR,
        ),
        (
            |e| e.is::<cabin_toolchain::CompilerWrapperResolutionError>(),
            code::TOOLCHAIN_ERROR,
        ),
        (|e| e.is::<cabin_vendor::VendorError>(), code::VENDOR_ERROR),
        (|e| e.is::<cabin_index::IndexError>(), code::INDEX_ERROR),
        (
            |e| e.is::<cabin_index_http::IndexHttpError>(),
            code::INDEX_HTTP_ERROR,
        ),
        (
            |e| e.is::<cabin_publish::PublishError>(),
            code::PUBLISH_ERROR,
        ),
        (|e| e.is::<cabin_fmt::FormatError>(), code::FMT_ERROR),
        (|e| e.is::<cabin_tidy::TidyError>(), code::TIDY_ERROR),
        (
            |e| e.is::<cabin_source_discovery::SourceDiscoveryError>(),
            code::SOURCE_DISCOVERY_ERROR,
        ),
        (|e| e.is::<cabin_test::TestRunError>(), code::TEST_ERROR),
        (
            |e| e.is::<cabin_explain::ExplainError>(),
            code::EXPLAIN_ERROR,
        ),
        (|e| e.is::<cabin_ninja::NinjaError>(), code::NINJA_ERROR),
        (
            |e| e.is::<cabin_feature::FeatureResolverError>(),
            code::FEATURE_ERROR,
        ),
    ];
    coded
        .iter()
        .find(|(is_match, _)| is_match(err))
        .map(|&(_, code)| DiagnosticCandidate::Coded { code })
}

fn config_error_code(error: &cabin_config::ConfigError) -> &'static str {
    use cabin_config::{ConfigError, ConfigParseError};
    use cabin_diagnostics::code;

    match error {
        ConfigError::Parse {
            source: ConfigParseError::InvalidBuildJobs { .. },
            ..
        } => code::CONFIG_INVALID_BUILD_JOBS,
        _ => code::CONFIG_LOAD_FAILED,
    }
}
