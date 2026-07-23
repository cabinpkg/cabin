//! User-facing diagnostic presentation for Cabin's typed
//! domain errors.
//!
//! Domain crates return strongly typed `thiserror` enums;
//! without a dedicated presentation layer the CLI orchestrator
//! wraps each one in `anyhow::with_context`, duplicating the
//! path / operation the typed error already names.
//! `cabin-diagnostics` is the single home for the presentation
//! contract:
//!
//! - **Stable codes.** [`code`] is the registry of diagnostic
//!   codes (`cabin::workspace::manifest_not_found`,
//!   `cabin::manifest::parse_error`, …) that user-facing
//!   errors point at.  Codes are stable across releases.
//! - **Rendering.** [`render`] walks a [`miette::Diagnostic`]
//!   and emits the report through miette's `fancy`
//!   `GraphicalReportHandler`: source-spanned diagnostics get
//!   the box-drawing snippet view, spanless diagnostics a
//!   `× <message>` header with attached code, help, and
//!   related entries.
//!
//! Crate boundaries:
//!
//! - rendering routes through one of two entry points:
//!   `render_to_string` always emits the no-color theme (used
//!   by tests so output stays byte-stable across terminals);
//!   [`render`] picks the colored or no-color theme from the
//!   writer's own [`WriteColor::supports_color`] report.

use std::io;

use miette::{GraphicalReportHandler, GraphicalTheme};
use termcolor::WriteColor;

pub use miette;

/// Stable diagnostic code constants.
///
/// Codes look like `cabin::<area>::<symbol>`.  They are
/// embedded in error reports for users and tooling to match
/// on.  The owning crate's `#[diagnostic(code(...))]`
/// attribute is the canonical source - miette's `code(...)`
/// takes a bareword path token, not a `&str`, so it cannot
/// reference a constant - and this module hand-mirrors a
/// constant only for the codes the CLI needs as a `&str`
/// value (for [`CodedMessage`]).  Every code's wording, when
/// it fires, and the user action belong in the diagnostics
/// docs.
pub mod code {
    /// Fallback for config discovery, read, parse, and validation
    /// failures.
    pub const CONFIG_LOAD_FAILED: &str = "cabin::config::load_failed";
    /// `build.jobs` carried zero, a negative value, or a
    /// non-integer value.
    pub const CONFIG_INVALID_BUILD_JOBS: &str = "cabin::config::invalid_build_jobs";
    /// Fallback for `cabin.lock` read, parse, validation, or
    /// write failures.
    pub const LOCKFILE_ERROR: &str = "cabin::lockfile::error";
    /// Dependency resolution could not produce a valid package
    /// set.
    pub const RESOLVER_ERROR: &str = "cabin::resolver::error";
    /// Artifact fetch, verification, or extraction failed.
    pub const ARTIFACT_ERROR: &str = "cabin::artifact::error";
    /// Build graph planning or validation failed.
    pub const BUILD_ERROR: &str = "cabin::build::error";
    /// A package declares a first-class `c-standard` /
    /// `cxx-standard` while its manifest-derived `cflags` /
    /// `cxxflags` also pin one via `-std=` / `/std:`.
    pub const LANGUAGE_STANDARD_FLAG_CONFLICT: &str = "cabin::language::standard_flag_conflict";
    /// Post-resolution standard-compatibility check: a resolved
    /// dependency edge violates the standard-compatibility model
    /// of `docs/design/standard-compatibility/spec.md` for a
    /// language the consuming target compiles.  Fails the command
    /// unless the edge carries a per-edge
    /// `ignore-interface-standard = true` override, which instead
    /// emits the sibling unchecked-edge note below.
    pub const LANGUAGE_STANDARD_COMPAT_VIOLATION: &str =
        "cabin::language::standard_compat_violation";
    /// Note: a violated dependency edge was exempted from the
    /// check by `ignore-interface-standard = true` on the
    /// consuming package's `[dependencies]` entry, so it goes
    /// unchecked.
    pub const LANGUAGE_STANDARD_COMPAT_UNCHECKED_EDGE: &str =
        "cabin::language::standard_compat_unchecked_edge";
    /// Package validation, archive creation, or metadata
    /// rendering failed.
    pub const PACKAGE_ERROR: &str = "cabin::package::error";
    /// Local toolchain resolution, detection, or wrapper
    /// resolution failed.
    pub const TOOLCHAIN_ERROR: &str = "cabin::toolchain::error";
    /// Vendor plan construction or materialization failed.
    pub const VENDOR_ERROR: &str = "cabin::vendor::error";
    /// Index discovery or read failed.
    pub const INDEX_ERROR: &str = "cabin::index::error";
    /// Registry index HTTP transport failure.
    pub const INDEX_HTTP_ERROR: &str = "cabin::index_http::error";
    /// `cabin publish` packaging or upload failure.
    pub const PUBLISH_ERROR: &str = "cabin::publish::error";
    /// `cabin fmt` (clang-format invocation) failed.
    pub const FMT_ERROR: &str = "cabin::fmt::error";
    /// `cabin tidy` (clang-tidy invocation) failed.
    pub const TIDY_ERROR: &str = "cabin::tidy::error";
    /// Recursive source discovery (glob / walk) failed.
    pub const SOURCE_DISCOVERY_ERROR: &str = "cabin::source_discovery::error";
    /// `cabin test` failed before any test could run (planning,
    /// environment, or invocation failure).
    pub const TEST_ERROR: &str = "cabin::test::error";
    /// `cabin explain` could not load or render the requested
    /// diagnostic.
    pub const EXPLAIN_ERROR: &str = "cabin::explain::error";
    /// Failure to invoke or read from the Ninja backend.
    pub const NINJA_ERROR: &str = "cabin::ninja::error";
    /// `[features]` resolution failed (unknown feature, cycle,
    /// etc.).
    pub const FEATURE_ERROR: &str = "cabin::feature::error";
}

/// Diagnostic adapter for an already-rendered error-chain
/// message.
///
/// This is useful at the CLI boundary, where an area-level coded
/// domain error may still depend on outer `anyhow` context for
/// user-visible details such as the operation being attempted.
pub struct CodedMessage<'a> {
    message: &'a str,
    code: &'static str,
}

impl<'a> CodedMessage<'a> {
    /// Wrap a display-ready `message` with a stable diagnostic
    /// `code`.
    pub const fn new(message: &'a str, code: &'static str) -> Self {
        Self { message, code }
    }
}

impl std::fmt::Debug for CodedMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodedMessage")
            .field("message", &self.message)
            .field("code", &self.code)
            .finish()
    }
}

impl std::fmt::Display for CodedMessage<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for CodedMessage<'_> {}

impl miette::Diagnostic for CodedMessage<'_> {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(self.code))
    }
}

/// Render a [`miette::Diagnostic`] to a `String`, no color.
///
/// Used by tests so output stays byte-stable across runs and
/// terminals; production callers use [`render`] so the color
/// choice is honored.
#[cfg(test)]
pub(crate) fn render_to_string(diagnostic: &dyn miette::Diagnostic) -> String {
    let handler = build_handler(GraphicalTheme::unicode_nocolor());
    let mut out = String::new();
    let _ = handler.render_report(&mut out, diagnostic);
    out
}

/// Render a [`miette::Diagnostic`] onto `writer`, emitting ANSI
/// color exactly when the writer reports it supports color.
///
/// The writer capability is the single routing input: the caller
/// encodes the user's color choice in the writer it constructs,
/// so a `NoColor` sink (a `termcolor::StandardStream` built with
/// `ColorChoice::Never`) stays plain even under `--color always`.
///
/// # Errors
/// Returns an [`io::Error`] if `GraphicalReportHandler::render_report`
/// fails (wrapped via [`io::Error::other`]), or if writing the
/// rendered bytes to `writer` or flushing it fails.
pub fn render(diagnostic: &dyn miette::Diagnostic, writer: &mut dyn WriteColor) -> io::Result<()> {
    let theme = if writer.supports_color() {
        GraphicalTheme::unicode()
    } else {
        GraphicalTheme::unicode_nocolor()
    };
    let handler = build_handler(theme);
    let mut buf = String::new();
    handler
        .render_report(&mut buf, diagnostic)
        .map_err(io::Error::other)?;
    writer.write_all(buf.as_bytes())?;
    writer.flush()
}

/// Construct miette's `GraphicalReportHandler` with cabin's
/// shared layout choices.  Cause-chain rendering is disabled
/// because most domain errors already embed the load-bearing
/// field values in their own message, and re-displaying the
/// source duplicates that text.  Source-spanned snippets
/// continue to render because they come from `source_code` +
/// `labels`, not from the cause chain.
fn build_handler(theme: GraphicalTheme) -> GraphicalReportHandler {
    GraphicalReportHandler::new_themed(theme).without_cause_chain()
}

#[cfg(test)]
mod tests {
    use super::*;
    use miette::Diagnostic;
    use thiserror::Error;

    #[derive(Debug, Error, Diagnostic)]
    #[error("could not find a Cabin workspace")]
    #[diagnostic(
        code(cabin::workspace::manifest_not_found),
        help("run `cabin init` to create a package")
    )]
    struct NotFound;

    /// miette's fancy renderer carries the diagnostic message,
    /// the stable code, and the help text.  Exact glyphs and
    /// padding are miette's; this test pins the load-bearing
    /// content rather than the layout so a miette point release
    /// doesn't force a churn here.
    #[test]
    fn render_includes_message_code_and_help() {
        let rendered = render_to_string(&NotFound);
        assert!(
            rendered.contains("could not find a Cabin workspace"),
            "missing message in: {rendered:?}"
        );
        assert!(
            rendered.contains("cabin::workspace::manifest_not_found"),
            "missing code in: {rendered:?}"
        );
        assert!(
            rendered.contains("run `cabin init` to create a package"),
            "missing help in: {rendered:?}"
        );
    }

    #[test]
    fn render_works_when_diagnostic_has_no_code() {
        #[derive(Debug, Error, Diagnostic)]
        #[error("plain message")]
        struct Plain;
        let rendered = render_to_string(&Plain);
        assert!(rendered.contains("plain message"), "got: {rendered:?}");
    }

    #[test]
    fn render_emits_ansi_when_writer_supports_color() {
        // `Ansi` always reports `supports_color() == true`.
        let mut sink: termcolor::Ansi<Vec<u8>> = termcolor::Ansi::new(Vec::new());
        render(&NotFound, &mut sink).expect("render should not fail");
        let bytes = sink.into_inner();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(
            s.contains('\x1b'),
            "expected ANSI escape for a color-capable writer, got: {s:?}"
        );
    }

    #[test]
    fn render_skips_color_when_writer_does_not_support_it() {
        // `NoColor` reports `supports_color() == false`; the
        // renderer must pick the no-color theme and emit no
        // escape sequences.
        let mut sink: termcolor::NoColor<Vec<u8>> = termcolor::NoColor::new(Vec::new());
        render(&NotFound, &mut sink).expect("render should not fail");
        let bytes = sink.into_inner();
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(
            !s.contains('\x1b'),
            "writer reports no color support, expected plain bytes: {s:?}"
        );
    }
}
