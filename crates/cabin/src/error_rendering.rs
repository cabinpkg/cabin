//! Top-level error display for `cabin`.
//!
//! The dispatcher returns `anyhow::Error`; some leaves wrap a
//! typed [`miette::Diagnostic`] (today: `ManifestError`,
//! `WorkspaceError`, `ResolveError`, `PkgConfigError`, and any
//! future domain error that derives `Diagnostic`).
//! [`render_error`] peels through the anyhow chain, asks
//! [`crate::diagnostic_registry`] for the deepest typed
//! candidate it can find, and routes the report through
//! [`cabin_diagnostics::render`] so the user sees a stable
//! `error[<code>]: <message>` block plus the `help:` text the
//! domain author attached.  When no typed diagnostic is found,
//! the formatter falls back to anyhow's default `{:#}`
//! rendering via [`write_plain_error`], which is what the rest
//! of the CLI has emitted historically.

use cabin_core::ColorChoice;
use termcolor::{StandardStream, WriteColor};

use crate::diagnostic_registry::{DiagnosticCandidate, downcast_diagnostic};

/// Render a top-level error to stderr, honoring the
/// caller-resolved [`ColorChoice`].
pub(crate) fn render_error(error: &anyhow::Error, color: ColorChoice) {
    // Walk the entire `Error::source` chain and remember the
    // deepest typed `Diagnostic` we can recover.  The deepest
    // one is the most specific (e.g.  `ManifestError::TomlAt`
    // with source-annotated labels rather than the wrapping
    // `WorkspaceError::Manifest`), so the user sees the
    // diagnostic that carries help text and span info.
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error.as_ref());
    let mut deepest: Option<DiagnosticCandidate<'_>> = None;
    while let Some(err) = current {
        if let Some(diag) = downcast_diagnostic(err) {
            deepest = Some(diag);
        }
        current = err.source();
    }
    let mut stderr = StandardStream::stderr(cabin_diagnostics::termcolor_choice(color));
    if let Some(candidate) = deepest {
        let _ = candidate.render(error, &mut stderr, color);
        return;
    }
    let _ = write_plain_error(&mut stderr, &format!("{error:#}"));
}

/// Emit a plain `error: <message>` line, painting only the
/// `error:` prefix.  Used by both the env-validation failure
/// path and the anyhow fallback.
pub(crate) fn write_plain_error(stderr: &mut StandardStream, message: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    if stderr.supports_color() {
        let mut spec = termcolor::ColorSpec::new();
        spec.set_fg(Some(termcolor::Color::Red)).set_bold(true);
        stderr.set_color(&spec)?;
        stderr.write_all(b"error")?;
        stderr.reset()?;
        writeln!(stderr, ": {message}")
    } else {
        writeln!(stderr, "error: {message}")
    }
}
