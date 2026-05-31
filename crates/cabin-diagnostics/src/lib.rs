//! User-facing diagnostic presentation for Cabin's typed
//! domain errors.
//!
//! ## Why this crate exists
//!
//! Domain crates (`cabin-workspace`, `cabin-manifest`,
//! `cabin-config`, …) return strongly typed `thiserror`
//! enums. Without a dedicated presentation layer, the CLI
//! orchestrator wraps each one in `anyhow::with_context`,
//! which in turn duplicates the path / operation that the
//! typed error already names. The result is the doubled-
//! chain output Cabin used to emit:
//!
//! ```text
//! error: failed to load workspace at /tmp/x/cabin.toml: failed to read /tmp/x/cabin.toml: No such file or directory (os error 2): No such file or directory (os error 2)
//! ```
//!
//! `cabin-diagnostics` is the single home for the
//! presentation contract:
//!
//! - **Stable codes.** [`code`] is the registry of diagnostic
//!   codes (`cabin::workspace::manifest_not_found`,
//!   `cabin::manifest::parse_error`, …) that user-facing
//!   errors point at. Codes are stable across releases.
//! - **Rendering.** [`render`] walks a [`miette::Diagnostic`]
//!   and emits the report through miette's `fancy`
//!   `GraphicalReportHandler`, which gives source-spanned
//!   diagnostics the box-drawing snippet view familiar from
//!   Rust's compiler errors and gives spanless diagnostics
//!   a `× <message>` header with attached code, help, and
//!   related entries.
//!
//! Crate boundaries:
//!
//! - this crate stays dependency-light: it depends only on
//!   `miette` (with `fancy` enabled), `termcolor`, and
//!   `thiserror`. Domain crates depend on `miette` for the
//!   `Diagnostic` derive and on this crate when they want to
//!   reference a stable code constant.
//! - the CLI orchestrator (`cabin`) depends on this
//!   crate to render typed diagnostics; it must not
//!   construct domain errors itself.
//! - rendering routes through one of two entry points:
//!   `render_to_string` always emits the no-color theme (used
//!   by tests so output stays byte-stable across terminals);
//!   [`render`] takes a [`cabin_core::ColorChoice`] and picks
//!   the colored or no-color theme accordingly.

#![allow(clippy::must_use_candidate)]

use std::io;

use cabin_core::ColorChoice;
use miette::{GraphicalReportHandler, GraphicalTheme};
use termcolor::WriteColor;

pub use miette;

/// Stable diagnostic code constants.
///
/// Codes look like `cabin::<area>::<symbol>`. They are
/// embedded in error reports for users and tooling to match
/// on. Adding a new code requires:
///
/// 1. extending this module with the new constant;
/// 2. spelling the same code in the owning crate's
///    `#[diagnostic(code(...))]` attribute. miette's `code(...)`
///    takes a bareword path token, not a `&str`, so it cannot
///    reference the constant here — that attribute is the
///    canonical source and these constants mirror it by hand;
/// 3. documenting the code's wording, when it fires, and the
///    user action in the diagnostics docs.
pub mod code {
    /// `cabin::workspace::manifest_not_found` — Cabin could
    /// not find a `cabin.toml` at the user's cwd or the
    /// requested `--manifest-path`.
    pub const WORKSPACE_MANIFEST_NOT_FOUND: &str = "cabin::workspace::manifest_not_found";
    /// `cabin::workspace::manifest_unreadable` — the manifest
    /// exists but Cabin could not read it (permission
    /// denied, is a directory, …).
    pub const WORKSPACE_MANIFEST_UNREADABLE: &str = "cabin::workspace::manifest_unreadable";
    /// `cabin::workspace::load_failed` — fallback for
    /// workspace-load failures that do not have a more
    /// specific code.
    pub const WORKSPACE_LOAD_FAILED: &str = "cabin::workspace::load_failed";
    /// `cabin::manifest::parse_error` — the manifest exists
    /// and is readable, but the TOML is syntactically
    /// invalid.
    pub const MANIFEST_PARSE_ERROR: &str = "cabin::manifest::parse_error";
    /// `cabin::config::load_failed` — fallback for config
    /// discovery, read, parse, and validation failures.
    pub const CONFIG_LOAD_FAILED: &str = "cabin::config::load_failed";
    /// `cabin::config::invalid_build_jobs` — `build.jobs`
    /// carried zero, a negative value, or a non-integer value.
    pub const CONFIG_INVALID_BUILD_JOBS: &str = "cabin::config::invalid_build_jobs";
    /// `cabin::lockfile::error` — fallback for `cabin.lock`
    /// read, parse, validation, or write failures.
    pub const LOCKFILE_ERROR: &str = "cabin::lockfile::error";
    /// `cabin::resolver::error` — dependency resolution could
    /// not produce a valid package set.
    pub const RESOLVER_ERROR: &str = "cabin::resolver::error";
    /// `cabin::artifact::error` — artifact fetch,
    /// verification, or extraction failed.
    pub const ARTIFACT_ERROR: &str = "cabin::artifact::error";
    /// `cabin::build::error` — build graph planning or
    /// validation failed.
    pub const BUILD_ERROR: &str = "cabin::build::error";
    /// `cabin::package::error` — package validation, archive
    /// creation, or metadata rendering failed.
    pub const PACKAGE_ERROR: &str = "cabin::package::error";
    /// `cabin::toolchain::error` — local toolchain
    /// resolution, detection, or wrapper resolution failed.
    pub const TOOLCHAIN_ERROR: &str = "cabin::toolchain::error";
    /// `cabin::vendor::error` — vendor plan construction or
    /// materialization failed.
    pub const VENDOR_ERROR: &str = "cabin::vendor::error";
    /// `cabin::index::error` — index discovery or read failed.
    pub const INDEX_ERROR: &str = "cabin::index::error";
    /// `cabin::index_http::error` — registry index HTTP
    /// transport failure.
    pub const INDEX_HTTP_ERROR: &str = "cabin::index_http::error";
    /// `cabin::publish::error` — `cabin publish` packaging
    /// or upload failure.
    pub const PUBLISH_ERROR: &str = "cabin::publish::error";
    /// `cabin::fmt::error` — `cabin fmt` (clang-format
    /// invocation) failed.
    pub const FMT_ERROR: &str = "cabin::fmt::error";
    /// `cabin::tidy::error` — `cabin tidy` (clang-tidy
    /// invocation) failed.
    pub const TIDY_ERROR: &str = "cabin::tidy::error";
    /// `cabin::source_discovery::error` — recursive source
    /// discovery (glob / walk) failed.
    pub const SOURCE_DISCOVERY_ERROR: &str = "cabin::source_discovery::error";
    /// `cabin::test::error` — `cabin test` failed before
    /// any test could run (planning, environment, or
    /// invocation failure).
    pub const TEST_ERROR: &str = "cabin::test::error";
    /// `cabin::explain::error` — `cabin explain` could not
    /// load or render the requested diagnostic.
    pub const EXPLAIN_ERROR: &str = "cabin::explain::error";
    /// `cabin::ninja::error` — failure to invoke or read
    /// from the Ninja backend.
    pub const NINJA_ERROR: &str = "cabin::ninja::error";
    /// `cabin::feature::error` — `[features]` resolution
    /// failed (unknown feature, cycle, etc.).
    pub const FEATURE_ERROR: &str = "cabin::feature::error";
}

/// Lightweight diagnostic adapter for typed errors that do not
/// need source snippets or variant-specific help text.
///
/// The adapter still routes through Cabin's single renderer and
/// emits a stable code. Domain crates with richer context should
/// implement [`miette::Diagnostic`] directly; this wrapper is for
/// area-level fallback codes while that richer coverage is not
/// needed.
pub struct CodedError<'a> {
    error: &'a (dyn std::error::Error + 'static),
    code: &'static str,
}

impl<'a> CodedError<'a> {
    /// Wrap `error` with a stable diagnostic `code`.
    pub const fn new(error: &'a (dyn std::error::Error + 'static), code: &'static str) -> Self {
        Self { error, code }
    }
}

impl std::fmt::Debug for CodedError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodedError")
            .field("error", &self.error.to_string())
            .field("code", &self.code)
            .finish()
    }
}

impl std::fmt::Display for CodedError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.error, f)
    }
}

impl std::error::Error for CodedError<'_> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

impl miette::Diagnostic for CodedError<'_> {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        Some(Box::new(self.code))
    }
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

/// Render a [`miette::Diagnostic`] onto `writer`, optionally
/// emitting ANSI color according to `color`.
///
/// Routing:
/// - `Never`, or `Auto` against a writer that does not support
///   color, uses miette's no-color Unicode theme so the
///   layout still has the box-drawing glyphs but no ANSI;
/// - `Always`, or `Auto` against a color-capable writer, uses
///   miette's full Unicode + ANSI theme.
///
/// The body bytes from `GraphicalReportHandler` already carry
/// the ANSI escapes when color is requested; the writer's
/// `WriteColor` API is unused.
///
/// # Errors
/// Returns an [`io::Error`] if `GraphicalReportHandler::render_report`
/// fails (wrapped via [`io::Error::other`]), or if writing the
/// rendered bytes to `writer` or flushing it fails.
pub fn render(
    diagnostic: &dyn miette::Diagnostic,
    writer: &mut dyn WriteColor,
    color: ColorChoice,
) -> io::Result<()> {
    // Writer capability always wins: a `NoColor` sink stays
    // plain even under `--color always`, matching how the
    // pre-miette renderer behaved.
    let colored = !matches!(color, ColorChoice::Never) && writer.supports_color();
    let theme = if colored {
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
/// shared layout choices. Cause-chain rendering is disabled
/// because most domain errors already embed the load-bearing
/// field values in their own message, and re-displaying the
/// source duplicates that text (the pre-miette cabin renderer
/// had the same policy). Source-spanned snippets continue to
/// render because they come from `source_code` + `labels`, not
/// from the cause chain.
fn build_handler(theme: GraphicalTheme) -> GraphicalReportHandler {
    GraphicalReportHandler::new_themed(theme).without_cause_chain()
}

/// Map a [`cabin_core::ColorChoice`] to a
/// [`termcolor::ColorChoice`].
///
/// `Always` maps to `AlwaysAnsi` so test output stays
/// platform-stable: on Windows, plain `Always` would attempt to
/// drive the console API instead of emitting ANSI escape
/// sequences, which would defeat the integration tests that
/// look for `\x1b[`.
pub fn termcolor_choice(choice: ColorChoice) -> termcolor::ColorChoice {
    match choice {
        ColorChoice::Auto => termcolor::ColorChoice::Auto,
        ColorChoice::Always => termcolor::ColorChoice::AlwaysAnsi,
        ColorChoice::Never => termcolor::ColorChoice::Never,
    }
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
    /// the stable code, and the help text. Exact glyphs and
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

    /// Test helper: render `diagnostic` into a `Vec<u8>` whose
    /// `WriteColor` impl is a `termcolor::Ansi` writer with the
    /// supplied color choice. Returns the captured bytes so
    /// tests can assert on ANSI escape presence.
    fn render_to_ansi_buffer(diagnostic: &dyn miette::Diagnostic, choice: ColorChoice) -> Vec<u8> {
        let mut sink: termcolor::Ansi<Vec<u8>> = termcolor::Ansi::new(Vec::new());
        render(diagnostic, &mut sink, choice).expect("render should not fail");
        sink.into_inner()
    }

    /// Same as [`render_to_ansi_buffer`] but the underlying
    /// writer is `NoColor`, which always reports
    /// `supports_color() == false`. Used to verify the renderer
    /// picks the no-color theme when the writer cannot accept
    /// ANSI.
    fn render_to_nocolor_buffer(
        diagnostic: &dyn miette::Diagnostic,
        choice: ColorChoice,
    ) -> Vec<u8> {
        let mut sink: termcolor::NoColor<Vec<u8>> = termcolor::NoColor::new(Vec::new());
        render(diagnostic, &mut sink, choice).expect("render should not fail");
        sink.into_inner()
    }

    #[test]
    fn render_with_never_emits_no_ansi_escape() {
        let bytes = render_to_ansi_buffer(&NotFound, ColorChoice::Never);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(
            !s.contains('\x1b'),
            "expected no ANSI escape with --color never, got: {s:?}"
        );
    }

    #[test]
    fn render_with_always_emits_ansi() {
        let bytes = render_to_ansi_buffer(&NotFound, ColorChoice::Always);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(
            s.contains('\x1b'),
            "expected ANSI escape with --color always, got: {s:?}"
        );
    }

    #[test]
    fn render_skips_color_when_writer_does_not_support_it() {
        // `NoColor` reports `supports_color() == false`. Even
        // with `ColorChoice::Always`, the renderer must respect
        // the writer capability and emit no escape sequences.
        let bytes = render_to_nocolor_buffer(&NotFound, ColorChoice::Always);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(
            !s.contains('\x1b'),
            "writer reports no color support, expected plain bytes: {s:?}"
        );
    }

    #[test]
    fn termcolor_choice_maps_always_to_always_ansi() {
        assert!(matches!(
            termcolor_choice(ColorChoice::Always),
            termcolor::ColorChoice::AlwaysAnsi
        ));
        assert!(matches!(
            termcolor_choice(ColorChoice::Never),
            termcolor::ColorChoice::Never
        ));
        assert!(matches!(
            termcolor_choice(ColorChoice::Auto),
            termcolor::ColorChoice::Auto
        ));
    }
}
