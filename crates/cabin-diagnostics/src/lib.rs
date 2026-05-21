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
//!   and emits a deterministic, deduplicated stderr report.
//!   Source-spanned diagnostics use `annotate-snippets` for
//!   the snippet portion; the rest is rendered by this crate
//!   directly so we never inherit a chatty backend's quirks.
//!
//! Crate boundaries:
//!
//! - this crate must stay small and dependency-light. It
//!   depends only on `miette`, `annotate-snippets`, and
//!   `thiserror`. Domain crates depend on `miette` for the
//!   `Diagnostic` derive and on this crate when they want to
//!   reference a stable code constant.
//! - the CLI orchestrator (`cabin-cli`) depends on this
//!   crate to render typed diagnostics; it must not
//!   construct domain errors itself.
//! - rendering routes through one of two entry points:
//!   `render_to_string` is byte-stable (no colour, no ANSI)
//!   and is what golden tests pin down; [`render`] takes a
//!   [`cabin_core::ColorChoice`] and emits ANSI styling on
//!   `Always` (or `Auto` when the writer is a terminal). The
//!   text content is identical between the two — color is
//!   purely additive styling on the `error[code]:` prefix and
//!   the `help:` lead-in.

#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use cabin_core::ColorChoice;
use termcolor::{Color, ColorSpec, WriteColor};

pub use miette;

/// Stable diagnostic code constants.
///
/// Codes look like `cabin::<area>::<symbol>`. They are
/// embedded in error reports for users and tooling to match
/// on. Adding a new code requires:
///
/// 1. extending this module with the new constant;
/// 2. emitting the constant from the relevant
///    `#[diagnostic(code = ...)]` attribute on the typed
///    error variant;
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
    /// `cabin::manifest::invalid_field` — the manifest is
    /// valid TOML but a field is semantically rejected
    /// (missing required key, wrong type, etc.).
    pub const MANIFEST_INVALID_FIELD: &str = "cabin::manifest::invalid_field";

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
    /// materialisation failed.
    pub const VENDOR_ERROR: &str = "cabin::vendor::error";
    /// `cabin::index::error` — local package index loading or
    /// validation failed.
    pub const INDEX_ERROR: &str = "cabin::index::error";
    /// `cabin::index_http::error` — sparse HTTP index loading
    /// or transport failed.
    pub const INDEX_HTTP_ERROR: &str = "cabin::index_http::error";
    /// `cabin::registry_file::error` — local file-registry
    /// validation or mutation failed.
    pub const REGISTRY_FILE_ERROR: &str = "cabin::registry_file::error";
    /// `cabin::publish::error` — package publication workflow
    /// failed.
    pub const PUBLISH_ERROR: &str = "cabin::publish::error";
    /// `cabin::fmt::error` — clang-format resolution or
    /// invocation failed.
    pub const FMT_ERROR: &str = "cabin::fmt::error";
    /// `cabin::tidy::error` — run-clang-tidy resolution or
    /// invocation failed.
    pub const TIDY_ERROR: &str = "cabin::tidy::error";
    /// `cabin::source_discovery::error` — shared C/C++ source
    /// discovery failed before an external tool could run.
    pub const SOURCE_DISCOVERY_ERROR: &str = "cabin::source_discovery::error";
    /// `cabin::test::error` — running a built test
    /// executable failed outside the test's own exit status.
    pub const TEST_ERROR: &str = "cabin::test::error";
    /// `cabin::explain::error` — tree/explain query
    /// construction or rendering failed.
    pub const EXPLAIN_ERROR: &str = "cabin::explain::error";
    /// `cabin::ninja::error` — Ninja file generation failed.
    pub const NINJA_ERROR: &str = "cabin::ninja::error";
    /// `cabin::feature::error` — feature resolution failed.
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

/// Render a [`miette::Diagnostic`] to a `String` using
/// Cabin's stable formatter. Use [`render`] when writing to a
/// stream; this helper is convenient for tests.
///
/// The output is byte-stable across runs on the same inputs
/// and contains no terminal colour or Unicode-only
/// decorations. Source-spanned diagnostics include an
/// `annotate-snippets` rendering of the offending region;
/// other diagnostics use the simple `error[code]: message`
/// header plus optional `note:` and `help:` lines.
pub(crate) fn render_to_string(diagnostic: &dyn miette::Diagnostic) -> String {
    let mut out = String::new();
    let _ = write_diagnostic(&mut out, diagnostic);
    out
}

/// Render a [`miette::Diagnostic`] onto `writer`, optionally
/// emitting ANSI color according to `color`.
///
/// `Auto` defers to the writer's own terminal detection; the
/// CLI feeds `render` a [`termcolor::StandardStream`] whose
/// `Auto` setting handles `NO_COLOR`, `CLICOLOR`, and TTY
/// detection. `Always` forces ANSI emission even when the
/// writer is not a terminal; `Never` strips styling.
///
/// The plain-text content of the rendering is byte-identical
/// to `render_to_string`; color only paints the
/// `error[code]:` prefix and the `help:` lead-in. Tests that
/// want byte-stable output should keep using
/// `render_to_string`.
pub fn render(
    diagnostic: &dyn miette::Diagnostic,
    writer: &mut dyn WriteColor,
    color: ColorChoice,
) -> io::Result<()> {
    let rendered = render_to_string(diagnostic);
    if matches!(color, ColorChoice::Never) || !writer.supports_color() {
        return writer.write_all(rendered.as_bytes());
    }
    paint_diagnostic_lines(writer, &rendered)
}

/// Paint the `error[code]:` / `warning[code]:` / `note[code]:`
/// prefix on each line of `rendered` and the `help:` lead-in.
/// All non-decorative bytes pass through unchanged so the text
/// content matches [`render_to_string`].
fn paint_diagnostic_lines(writer: &mut dyn WriteColor, rendered: &str) -> io::Result<()> {
    for line in rendered.split_inclusive('\n') {
        // The renderer indents related diagnostics with two
        // leading spaces; strip them for prefix detection but
        // re-emit the indent verbatim so the visual structure
        // survives painting.
        let leading_ws_len = line.len() - line.trim_start_matches(' ').len();
        let (indent, body) = line.split_at(leading_ws_len);
        if !indent.is_empty() {
            writer.write_all(indent.as_bytes())?;
        }
        if let Some(rest) = paint_severity_prefix(writer, body)? {
            writer.write_all(rest.as_bytes())?;
            continue;
        }
        if let Some(rest) = paint_help_prefix(writer, body)? {
            writer.write_all(rest.as_bytes())?;
            continue;
        }
        writer.write_all(body.as_bytes())?;
    }
    writer.flush()
}

/// If `body` starts with one of `error[`, `error:`, `warning[`,
/// `warning:`, `note[`, or `note:`, paint the prefix in the
/// matching color and return the byte-suffix (everything after
/// the painted prefix) for the caller to write verbatim.
///
/// Returns `Ok(None)` if `body` does not start with a recognised
/// severity prefix.
fn paint_severity_prefix<'a>(
    writer: &mut dyn WriteColor,
    body: &'a str,
) -> io::Result<Option<&'a str>> {
    for (label, color) in [
        ("error", Color::Red),
        ("warning", Color::Yellow),
        ("note", Color::Cyan),
    ] {
        if let Some(after_label) = body.strip_prefix(label) {
            if let Some(rest) = after_label.strip_prefix('[') {
                // `error[<code>]: ...` form. Paint the prefix
                // up to and including the `]` so the code is
                // visually attached to the severity word.
                if let Some(close) = rest.find(']') {
                    let head_end = label.len() + 1 + close + 1;
                    let head = &body[..head_end];
                    write_with_color(writer, head, color, true)?;
                    return Ok(Some(&body[head_end..]));
                }
            } else if after_label.starts_with(':') {
                // `error: ...` form (no code). Paint exactly
                // the severity word.
                let head = &body[..label.len()];
                write_with_color(writer, head, color, true)?;
                return Ok(Some(&body[label.len()..]));
            }
        }
    }
    Ok(None)
}

/// If `body` starts with `help:`, paint it cyan + bold and
/// return the remainder.
fn paint_help_prefix<'a>(
    writer: &mut dyn WriteColor,
    body: &'a str,
) -> io::Result<Option<&'a str>> {
    if let Some(rest) = body.strip_prefix("help:") {
        write_with_color(writer, "help:", Color::Cyan, true)?;
        return Ok(Some(rest));
    }
    Ok(None)
}

fn write_with_color(
    writer: &mut dyn WriteColor,
    text: &str,
    color: Color,
    bold: bool,
) -> io::Result<()> {
    let mut spec = ColorSpec::new();
    spec.set_fg(Some(color)).set_bold(bold);
    writer.set_color(&spec)?;
    writer.write_all(text.as_bytes())?;
    writer.reset()
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

/// Render an `annotate-snippets` source snippet for a
/// diagnostic that owns a `[u8]` source plus an optional
/// label region. The boundary lets domain errors stay
/// `annotate-snippets`-free; the rendering happens here.
///
/// `origin` is the path of the source file (used as the
/// snippet's filename). `label_span` is `(start_byte,
/// end_byte)` inside `source`. The renderer is forgiving:
/// invalid spans collapse to a zero-width caret at byte 0 so
/// the diagnostic still renders something useful.
pub(crate) fn render_source_snippet(
    title: &str,
    code: &str,
    origin: &Path,
    source: &str,
    label_span: Option<(usize, usize)>,
    snippet_label: &str,
) -> String {
    use annotate_snippets::{
        Annotation, AnnotationType, Renderer, Slice, Snippet, SourceAnnotation,
    };

    let origin_str = origin.display().to_string();
    let source_len = source.len();
    let mut span = label_span.unwrap_or((0, 0));
    if span.0 > source_len {
        span.0 = source_len;
    }
    if span.1 > source_len {
        span.1 = source_len;
    }
    if span.1 < span.0 {
        span.1 = span.0;
    }

    let snippet = Snippet {
        title: Some(Annotation {
            label: Some(title),
            id: Some(code),
            annotation_type: AnnotationType::Error,
        }),
        footer: vec![],
        slices: vec![Slice {
            source,
            line_start: 1,
            origin: Some(origin_str.as_str()),
            fold: false,
            annotations: vec![SourceAnnotation {
                label: snippet_label,
                annotation_type: AnnotationType::Error,
                range: span,
            }],
        }],
    };
    // `Renderer::plain()` emits no ANSI colour, so test
    // output stays byte-stable across terminals. Bind the
    // renderer + display values to locals first because
    // `render(...)` borrows `snippet` (and therefore the
    // `origin_str`); the temporaries must outlive the borrow.
    let renderer = Renderer::plain();
    let display = renderer.render(snippet);
    display.to_string()
}

fn write_diagnostic(out: &mut String, diagnostic: &dyn miette::Diagnostic) -> std::fmt::Result {
    // Source-annotated branch: when the diagnostic exposes
    // `source_code` plus at least one label, render the snippet
    // through annotate-snippets so the user sees the offending
    // line + caret in rustc / Cargo style.
    if let Some(rendered) = render_with_snippet(diagnostic) {
        out.push_str(&rendered);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        if let Some(help) = diagnostic.help() {
            writeln!(out, "help: {help}")?;
        }
        return Ok(());
    }

    // Plain branch: header + optional help + optional related.
    let code = diagnostic.code().map(|c| c.to_string());
    let severity = diagnostic.severity().unwrap_or(miette::Severity::Error);
    let severity_label = match severity {
        miette::Severity::Error => "error",
        miette::Severity::Warning => "warning",
        miette::Severity::Advice => "note",
    };
    write!(out, "{severity_label}")?;
    if let Some(code) = &code {
        write!(out, "[{code}]")?;
    }
    writeln!(out, ": {diagnostic}")?;

    // Notes from the upstream cause chain are *intentionally*
    // not appended: most domain errors already include the
    // load-bearing field values in their own message, and
    // re-displaying the source duplicates that text. Domain
    // errors that want to expose extra context use
    // `diagnostic.help()` / `diagnostic.related()` instead.

    if let Some(help) = diagnostic.help() {
        writeln!(out, "  help: {help}")?;
    }

    if let Some(related) = diagnostic.related() {
        for child in related {
            let mut nested = String::new();
            write_diagnostic(&mut nested, child)?;
            for line in nested.lines() {
                writeln!(out, "  {line}")?;
            }
        }
    }
    Ok(())
}

/// If the diagnostic owns a source span and at least one label,
/// render the snippet through annotate-snippets and return the
/// formatted block. Returns `None` when the diagnostic does not
/// have a span, in which case the plain header rendering kicks
/// in.
///
/// The renderer extracts:
///   - the source text via `source_code()`;
///   - the diagnostic's first label as the caret region;
///   - `code()` and the diagnostic's `Display` for the title.
///
/// This is the single place `annotate-snippets` is called from
/// the presentation layer; new domain errors that derive
/// `Diagnostic` and expose `#[source_code]` + `#[label]` light
/// up snippet rendering automatically.
fn render_with_snippet(diagnostic: &dyn miette::Diagnostic) -> Option<String> {
    let source_code = diagnostic.source_code()?;
    let mut labels = diagnostic.labels()?;
    let first = labels.next()?;
    let label_message = first.label().unwrap_or("here").to_owned();
    let span = *first.inner();
    let span_offset = span.offset();
    let span_len = span.len();

    // Read enough surrounding context that the renderer can
    // print at least one line above and below the offending
    // region. A ridiculously large `context_lines_before/after`
    // is fine because the source_code adapter only returns the
    // lines it actually has.
    let span_obj = miette::SourceSpan::new(span_offset.into(), span_len);
    let contents = source_code.read_span(&span_obj, 2, 2).ok()?;
    let snippet_bytes = contents.data();
    let snippet_str = std::str::from_utf8(snippet_bytes).ok()?;

    // The label region inside the *snippet* is the original
    // span shifted by `contents.span().offset()`. Both ends are
    // computed with saturating arithmetic: a malformed
    // `SourceSpan` cannot panic the renderer, it just collapses
    // the caret to the end of the snippet.
    let snippet_offset = contents.span().offset();
    let label_start = span_offset.saturating_sub(snippet_offset);
    let label_end = label_start.saturating_add(span_len.max(1));

    let title = diagnostic.to_string();
    let code = diagnostic
        .code()
        .map_or_else(|| "diagnostic".to_owned(), |c| c.to_string());
    let origin_path = contents
        .name()
        .map_or_else(|| PathBuf::from("<source>"), PathBuf::from);

    Some(render_source_snippet(
        &title,
        &code,
        &origin_path,
        snippet_str,
        Some((label_start, label_end)),
        &label_message,
    ))
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

    #[test]
    fn render_includes_code_and_help() {
        let rendered = render_to_string(&NotFound);
        assert!(
            rendered.contains(
                "error[cabin::workspace::manifest_not_found]: could not find a Cabin workspace"
            ),
            "unexpected rendering: {rendered:?}"
        );
        assert!(
            rendered.contains("  help: run `cabin init` to create a package"),
            "unexpected rendering: {rendered:?}"
        );
    }

    #[test]
    fn render_omits_code_when_diagnostic_has_none() {
        #[derive(Debug, Error, Diagnostic)]
        #[error("plain message")]
        struct Plain;

        let rendered = render_to_string(&Plain);
        assert!(
            rendered.starts_with("error: plain message\n"),
            "got: {rendered:?}"
        );
    }

    #[test]
    fn render_source_snippet_marks_label_region() {
        let source = "[package]\nversion = 1\n";
        let rendered = render_source_snippet(
            "expected string, found integer",
            "cabin::manifest::parse_error",
            Path::new("cabin.toml"),
            source,
            Some((source.find('1').unwrap(), source.find('1').unwrap() + 1)),
            "expected a string here",
        );
        // annotate-snippets always includes the origin path,
        // a line gutter, and a caret pointing at the labelled
        // span. These are the byte-stable invariants the test
        // pins down.
        assert!(rendered.contains("cabin.toml"), "got: {rendered}");
        assert!(
            rendered.contains("expected string, found integer"),
            "got: {rendered}"
        );
        assert!(
            rendered.contains("expected a string here"),
            "got: {rendered}"
        );
        assert!(rendered.contains('^'), "expected caret in: {rendered}");
    }

    #[test]
    fn render_source_snippet_clamps_label_past_eof() {
        // A diagnostic whose label runs past the end of the
        // source must not panic the renderer. The clamping in
        // `render_source_snippet` collapses the caret to the
        // end of the source instead.
        let source = "short\n";
        let rendered = render_source_snippet(
            "trailing data expected",
            "cabin::manifest::parse_error",
            Path::new("cabin.toml"),
            source,
            // Span starts well past EOF and "ends" further out
            // — the renderer should not overflow on the
            // saturating addition path.
            Some((10_000, 20_000)),
            "missing here",
        );
        // The rendering still includes the title and the
        // origin so the user sees *something* useful even on a
        // degenerate span.
        assert!(
            rendered.contains("trailing data expected"),
            "got: {rendered}"
        );
        assert!(rendered.contains("cabin.toml"), "got: {rendered}");
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
    /// skips painting when the writer cannot accept ANSI even
    /// if the user passed `--color always`.
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
        assert!(
            s.contains("error[cabin::workspace::manifest_not_found]:"),
            "expected unstyled prefix, got: {s:?}"
        );
    }

    #[test]
    fn render_with_always_emits_ansi_around_error_prefix() {
        let bytes = render_to_ansi_buffer(&NotFound, ColorChoice::Always);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(
            s.contains('\x1b'),
            "expected ANSI escape with --color always, got: {s:?}"
        );
        // Bold-red ANSI sequences come out as `\x1b[1m\x1b[38m`
        // or similar across termcolor versions; assert the
        // structural invariant rather than the exact bytes:
        // the painted prefix should immediately precede the
        // `]: ` separator so the painted region is the
        // severity label plus its code.
        let escape_idx = s.find('\x1b').unwrap();
        assert!(
            s[..escape_idx].chars().all(|c| c == ' '),
            "ANSI escape must lead the line, got: {s:?}"
        );
        assert!(
            s.contains("could not find a Cabin workspace"),
            "expected message body intact, got: {s:?}"
        );
    }

    #[test]
    fn render_with_always_paints_help_lead_in() {
        let bytes = render_to_ansi_buffer(&NotFound, ColorChoice::Always);
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains("help:"), "expected `help:` lead-in, got: {s:?}");
        // The `help:` lead-in must be wrapped in a colour
        // sequence followed by a reset; spot-check the literal
        // reset sequence appears after the `help:` text.
        let help_idx = s.find("help:").unwrap();
        let tail = &s[help_idx..];
        assert!(
            tail.contains("\x1b[0m"),
            "expected reset after `help:`, got: {tail:?}"
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
    fn render_text_content_matches_render_to_string_under_never() {
        let bytes = render_to_ansi_buffer(&NotFound, ColorChoice::Never);
        let plain = render_to_string(&NotFound);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            plain,
            "ColorChoice::Never must be byte-identical to render_to_string"
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

    #[test]
    fn render_source_snippet_handles_empty_source() {
        let rendered = render_source_snippet(
            "manifest is empty",
            "cabin::manifest::parse_error",
            Path::new("cabin.toml"),
            "",
            Some((0, 0)),
            "expected at least a [package] table",
        );
        assert!(rendered.contains("manifest is empty"), "got: {rendered}");
    }
}
