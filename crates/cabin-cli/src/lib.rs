//! Library half of the `cabin` CLI binary.
//!
//! The bin (`src/main.rs`) is intentionally a thin shim that
//! calls [`run`]; the typed parser ([`Cli`]), the
//! command dispatcher, and every glue module live here so
//! integration tests can re-use the same surface the binary
//! does — `Cli::command()` is the single source of truth for
//! which subcommands exist and which are hidden.

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::redundant_closure_for_method_calls,
    clippy::struct_excessive_bools,
    clippy::stable_sort_primitive,
    clippy::uninlined_format_args,
    clippy::format_push_string,
    clippy::map_unwrap_or,
    clippy::manual_let_else,
    clippy::too_many_lines,
    clippy::doc_markdown,
    clippy::single_match_else,
    clippy::match_wildcard_for_single_variants,
    clippy::if_not_else,
    clippy::unused_self,
    clippy::semicolon_if_nothing_returned,
    clippy::unnecessary_trailing_comma,
    clippy::default_trait_access
)]

use std::process::ExitCode;

use cabin_core::ColorChoice;
use clap::{CommandFactory, FromArgMatches};
use termcolor::{StandardStream, WriteColor};

pub use crate::cli::Cli;
use crate::term_verbosity_glue::{
    CliVerbosity, Reporter, discover_early_config_verbosity, resolve_verbosity,
};

/// Marker name for the cargo-style `...` row that appears at
/// the end of the `cabin --help` Commands block.  It points
/// users at `cabin --list` without polluting the Subcommand
/// enum: the row is injected into the clap command tree only
/// for help / parsing, and the dispatcher treats it as an
/// alias for `--list`.  `command_list`, `completions`, and
/// `manpages` build their output from the unmodified
/// [`Cli::command()`] tree so the row never leaks into the
/// `--list` view, generated completions, or man pages.
const DOTS_HINT: &str = "...";

/// About text rendered next to the [`DOTS_HINT`] row.  Matches
/// cargo's wording for the equivalent hint in `cargo --help`.
const DOTS_ABOUT: &str = "See all commands with --list";

/// Render the styled `Commands:` block for `cabin --help`,
/// using cargo's `name, alias` rendering instead of clap's
/// default `[aliases: alias]` form.
///
/// Embedded ANSI escapes paint:
/// - the `Commands:` heading bright green + bold (matching
///   clap's auto styling of `Usage:`);
/// - each `<name>[, <alias>]` cell bright cyan + bold;
/// - the about text stays plain.
///
/// anstream strips the escapes when the writer disables
/// colour, so `cabin --color never --help` and pipe-redirected
/// output stay clean.  Hidden subcommands are skipped because
/// `cabin --help` is the curated view; the full directory lives
/// in `cabin --list`.
fn format_commands_block(cmd: &clap::Command) -> String {
    use std::fmt::Write as _;

    /// One subcommand row: the canonical name plus any
    /// visible aliases, paired with the short about text.  The
    /// `tokens` list keeps each name / alias separate so the
    /// renderer can style them individually while leaving the
    /// `, ` separators unstyled — same as cargo.
    struct Row {
        tokens: Vec<String>,
        about: String,
    }

    let rows: Vec<Row> = cmd
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .map(|sub| {
            let mut tokens = vec![sub.get_name().to_owned()];
            for alias in sub.get_visible_aliases() {
                tokens.push(alias.to_string());
            }
            let about = sub
                .get_about()
                .map(|s| s.to_string().lines().next().unwrap_or("").trim().to_owned())
                .unwrap_or_default();
            Row { tokens, about }
        })
        .collect();

    // The display width of a row is the length of all tokens
    // joined by `, ` (the printed separator).  ANSI escapes
    // around each token do not add display width because they
    // do not advance the cursor, but they do add bytes — we
    // compute the visible width from the plain-text join.
    let width = rows
        .iter()
        .map(|row| row.tokens.join(", ").len())
        .max()
        .unwrap_or(0);

    // clap prepends a blank line before `{after-help}`, so
    // our block starts directly with the styled heading.
    let mut out = String::new();
    let _ = writeln!(out, "\x1b[1m\x1b[92mCommands:\x1b[0m");
    for row in &rows {
        out.push_str("  ");
        let plain_width: usize = row.tokens.join(", ").len();
        for (i, token) in row.tokens.iter().enumerate() {
            if i > 0 {
                // Cargo emits the `, ` between aliases as plain
                // text; only the name / alias tokens get the
                // bright-cyan + bold styling.
                out.push_str(", ");
            }
            let _ = write!(out, "\x1b[1m\x1b[96m{token}\x1b[0m");
        }
        if row.about.is_empty() {
            out.push('\n');
        } else {
            // Pad to the column where the about text begins.
            let padding = width.saturating_sub(plain_width);
            for _ in 0..padding {
                out.push(' ');
            }
            let _ = writeln!(out, "  {about}", about = row.about);
        }
    }
    out
}

// The clap parser stays reachable for this crate's glue modules,
// but `Cli` is re-exported at the crate root for integration
// tests and downstream command-tree generation.
mod cli;
mod command_list;
mod completions;
mod config_glue;
mod env_flags_glue;
mod explain_glue;
mod fetch_output_glue;
mod fmt_glue;
mod manpages;
mod metadata_glue;
mod patch_glue;
mod port_glue;
mod port_subcommand;
mod run_glue;
mod source_tooling_glue;
mod system_deps_glue;
mod term_color_glue;
mod term_verbosity_glue;
mod test_glue;
mod tidy_glue;
mod tree_glue;
mod vendor_glue;
mod version_glue;
mod version_info;

/// Return the English plural suffix for a count: empty for one,
/// `s` for everything else. Used across the reporter glue files
/// to keep `"<n> file"` / `"<n> files"` consistent.
pub(crate) fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Run the `cabin` CLI to completion using the given argv
/// iterator.  Owns parsing, color/verbosity resolution,
/// dispatch, and top-level error rendering.  The binary
/// `main` calls this with the process's own arguments.
pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    // Build the parser from the typed `Cli` definition and
    // append a cargo-style `...   See all commands with
    // --list` row as the last visible entry in the Commands
    // block.  Two steps make that ordering work:
    //
    // 1. `Command::build` forces clap to materialise its
    //    auto-injected `help` pseudo-subcommand so we can
    //    address it by name.
    // 2. `mut_subcommand("help", …)` hides the help row from
    //    the Commands block — `cabin help <cmd>` still
    //    works, the row just is not advertised, matching
    //    cargo's `cargo --help` curation.
    //
    // Then we append the `...` row.  Because the auto-help is
    // hidden, our row is the visible final entry.
    //
    // The row is purely visual: the dispatcher treats `cabin
    // ...` as a shortcut for `cabin --list` (so the row is
    // also a working command), and the canonical
    // `Cli::command()` consumed by `command_list`,
    // `completions`, and `manpages` never sees the marker.
    let mut cmd = Cli::command();
    cmd.build();
    let cmd = cmd.mut_subcommand("help", |sub| sub.hide(true)).subcommand(
        clap::Command::new(DOTS_HINT)
            .about(DOTS_ABOUT)
            .disable_help_subcommand(true),
    );
    // Render the Commands block manually so visible aliases
    // appear in cargo's `name, alias` style (`build, b`).
    // Clap's `{subcommands}` placeholder uses the default
    // `[aliases: b]` rendering, which is not what cargo
    // emits.  See `format_commands_block` for the format.
    //
    // Append the cargo-style trailer that points users at
    // `cabin help <command>` for per-subcommand detail.
    let mut after_help = format_commands_block(&cmd);
    after_help.push('\n');
    after_help.push_str("See 'cabin help <command>' for more information on a specific command.\n");
    let cmd = cmd.after_help(after_help);
    let matches = match cmd.try_get_matches_from(args) {
        Ok(m) => m,
        Err(err) => {
            // `clap::Error` already routes `--help` /
            // `--version` to stdout and real errors to stderr
            // with the correct ANSI handling.  Just hand it
            // through.
            err.exit();
        }
    };
    // `cabin ...` is a help-row affordance that doubles as a
    // shortcut for `cabin --list`.  The unmapped subcommand
    // produces `cmd: None` after `from_arg_matches`; promote
    // it to `list = true` so the downstream dispatcher renders
    // the listing with the same colour-aware code path as the
    // real flag.
    let dots_shortcut = matches.subcommand_name() == Some(DOTS_HINT);
    let mut parsed = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(err) => err.exit(),
    };
    if dots_shortcut {
        parsed.list = true;
    }
    // Resolve the terminal-color choice as early as possible
    // so even errors emitted while loading the workspace honor
    // `--color`. The chain is `--color` ▶ `CABIN_TERM_COLOR` ▶
    // user-level `[term] color` config ▶ `auto`. The user-level
    // config is the only layer reachable without a workspace,
    // which is the right shape: workspace-level overrides
    // affect *that* workspace's commands, and we have no
    // workspace context yet.
    let config_color = term_color_glue::discover_early_config_color();
    let early_color = match term_color_glue::resolve_color_choice(
        parsed.color.map(|c| c.into()),
        |key| std::env::var(key).ok(),
        config_color,
    ) {
        Ok(choice) => choice,
        Err(env_err) => {
            // Use `Auto` for the styling of the error itself —
            // we cannot trust the value the user gave us.
            let mut stderr =
                StandardStream::stderr(cabin_diagnostics::termcolor_choice(ColorChoice::Auto));
            let _ = write_plain_error(&mut stderr, &env_err.to_string());
            return ExitCode::FAILURE;
        }
    };

    // Resolve verbosity once, against the same user-level
    // config the early color resolution observed.  Subcommands
    // that load their own workspace-level config will see any
    // workspace-level `term.verbose` / `term.quiet` overrides
    // through their own dispatcher loop; the early resolve is
    // sufficient for status output gated through the reporter.
    let cli_verbosity = CliVerbosity {
        verbose_count: parsed.verbose,
        quiet: parsed.quiet,
    };
    let early_config_verbosity = discover_early_config_verbosity();
    let verbosity = match resolve_verbosity(
        cli_verbosity,
        |key| std::env::var(key).ok(),
        &early_config_verbosity,
    ) {
        Ok(level) => level,
        Err(env_err) => {
            let mut stderr =
                StandardStream::stderr(cabin_diagnostics::termcolor_choice(early_color));
            let _ = write_plain_error(&mut stderr, &env_err.to_string());
            return ExitCode::FAILURE;
        }
    };
    let reporter = Reporter::with_color(verbosity, early_color);

    match cli::run(parsed, reporter, early_color) {
        Ok(code) => code,
        Err(error) => {
            render_error(&error, early_color);
            ExitCode::FAILURE
        }
    }
}

/// Render a top-level error to stderr, honouring the
/// caller-resolved [`ColorChoice`].
///
/// The dispatcher returns `anyhow::Error`; some leaves wrap a
/// typed `miette::Diagnostic` (today: `WorkspaceError` and
/// any future domain error that derives `Diagnostic`).
/// `render_error` peels through the anyhow chain and routes
/// the first `Diagnostic` it finds to
/// [`cabin_diagnostics::render`], so the user sees a stable
/// `error[<code>]: <message>` block plus the `help:` text the
/// domain author attached.
///
/// When the chain carries no typed diagnostic, the formatter
/// falls back to anyhow's default `{:#}` rendering, which is
/// what the rest of the CLI has emitted historically.
fn render_error(error: &anyhow::Error, color: ColorChoice) {
    // Walk the entire `Error::source` chain and remember the
    // deepest typed `Diagnostic` we can recover. The deepest
    // one is the most specific (e.g. `ManifestError::TomlAt`
    // with source-annotated labels rather than the wrapping
    // `WorkspaceError::Manifest`), so the user sees the
    // diagnostic that actually carries help text and span info.
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
/// `error:` prefix. Used by both the env-validation failure
/// path and the anyhow fallback.
fn write_plain_error(stderr: &mut StandardStream, message: &str) -> std::io::Result<()> {
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

/// A diagnostic recovered from one item in the anyhow source
/// chain.
enum DiagnosticCandidate<'a> {
    /// The domain error itself implements `miette::Diagnostic`
    /// and may carry source snippets or variant-specific help.
    Rich(&'a dyn cabin_diagnostics::miette::Diagnostic),
    /// The domain error is typed and user-facing, but only needs
    /// an area-level stable code. Wrap it so rendering still goes
    /// through `cabin-diagnostics`.
    Coded { code: &'static str },
}

impl DiagnosticCandidate<'_> {
    fn render(
        &self,
        root: &anyhow::Error,
        stderr: &mut StandardStream,
        color: ColorChoice,
    ) -> std::io::Result<()> {
        match self {
            Self::Rich(diag) => cabin_diagnostics::render(*diag, stderr, color),
            Self::Coded { code } => {
                let message = format!("{root:#}");
                let diagnostic = cabin_diagnostics::CodedMessage::new(&message, code);
                cabin_diagnostics::render(&diagnostic, stderr, color)
            }
        }
    }
}

/// Helper that walks the known typed-error roots and yields a
/// diagnostic candidate when one matches.
///
/// Adding a new diagnostic-bearing error type is a one-line
/// change here. The cost of explicit listing is small relative
/// to the boundary clarity it gives — we never accidentally
/// route a typed error away from the diagnostic renderer
/// because of an unsafe blanket impl.
fn downcast_diagnostic<'a>(
    err: &'a (dyn std::error::Error + 'static),
) -> Option<DiagnosticCandidate<'a>> {
    use cabin_diagnostics::code;

    // The order matters: try the most specific typed error
    // first, then fall through to looser wrappers that share
    // the same source chain. ManifestError can hide either
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
    if let Some(e) = err.downcast_ref::<cabin_config::ConfigError>() {
        return Some(DiagnosticCandidate::Coded {
            code: config_error_code(e),
        });
    }
    if err
        .downcast_ref::<cabin_lockfile::LockfileError>()
        .is_some()
    {
        return Some(DiagnosticCandidate::Coded {
            code: code::LOCKFILE_ERROR,
        });
    }
    if let Some(e) = err.downcast_ref::<cabin_resolver::ResolveError>() {
        return Some(DiagnosticCandidate::Rich(e));
    }
    if err
        .downcast_ref::<cabin_artifact::ArtifactError>()
        .is_some()
    {
        return Some(DiagnosticCandidate::Coded {
            code: code::ARTIFACT_ERROR,
        });
    }
    if err.downcast_ref::<cabin_build::BuildError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::BUILD_ERROR,
        });
    }
    if err.downcast_ref::<cabin_package::PackageError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::PACKAGE_ERROR,
        });
    }
    if err
        .downcast_ref::<cabin_toolchain::ToolchainError>()
        .is_some()
        || err
            .downcast_ref::<cabin_toolchain::ToolchainDetectionFailure>()
            .is_some()
        || err.downcast_ref::<cabin_toolchain::RunError>().is_some()
        || err
            .downcast_ref::<cabin_toolchain::CompilerWrapperResolutionError>()
            .is_some()
    {
        return Some(DiagnosticCandidate::Coded {
            code: code::TOOLCHAIN_ERROR,
        });
    }
    if err.downcast_ref::<cabin_vendor::VendorError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::VENDOR_ERROR,
        });
    }
    if err.downcast_ref::<cabin_index::IndexError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::INDEX_ERROR,
        });
    }
    if err
        .downcast_ref::<cabin_index_http::IndexHttpError>()
        .is_some()
    {
        return Some(DiagnosticCandidate::Coded {
            code: code::INDEX_HTTP_ERROR,
        });
    }
    if err.downcast_ref::<cabin_publish::PublishError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::PUBLISH_ERROR,
        });
    }
    if err.downcast_ref::<cabin_fmt::FormatError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::FMT_ERROR,
        });
    }
    if err.downcast_ref::<cabin_tidy::TidyError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::TIDY_ERROR,
        });
    }
    if err
        .downcast_ref::<cabin_source_discovery::SourceDiscoveryError>()
        .is_some()
    {
        return Some(DiagnosticCandidate::Coded {
            code: code::SOURCE_DISCOVERY_ERROR,
        });
    }
    if err.downcast_ref::<cabin_test::TestRunError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::TEST_ERROR,
        });
    }
    if err.downcast_ref::<cabin_explain::ExplainError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::EXPLAIN_ERROR,
        });
    }
    if err.downcast_ref::<cabin_ninja::NinjaError>().is_some() {
        return Some(DiagnosticCandidate::Coded {
            code: code::NINJA_ERROR,
        });
    }
    if err
        .downcast_ref::<cabin_feature::FeatureResolverError>()
        .is_some()
    {
        return Some(DiagnosticCandidate::Coded {
            code: code::FEATURE_ERROR,
        });
    }
    None
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
