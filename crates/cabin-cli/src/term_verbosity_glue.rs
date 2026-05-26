//! Glue between Cabin's CLI surface and the typed
//! [`cabin_core::Verbosity`].
//!
//! Two pieces live here:
//! - [`resolve_verbosity`] applies Cabin's documented precedence
//!   rule: CLI > `CABIN_TERM_VERBOSE` / `CABIN_TERM_QUIET` env
//!   vars > config `term.verbose` / `term.quiet` > default.  The
//!   function is pure: tests pass a closure for env lookup so
//!   they never depend on the host environment;
//! - [`Reporter`] is the small, typed display context every
//!   subcommand uses to emit Cabin-owned status / verbose /
//!   very-verbose lines.  It honors `--quiet` and `--verbose`
//!   so the verbosity check does not have to be re-implemented
//!   per call site.
//!
//! Stream policy: human-facing commands (`cabin build`, `cabin
//! run`, `cabin test`, `cabin clean`, `cabin vendor`, `cabin
//! init`, `cabin new`, `cabin package`, `cabin publish`) emit
//! status to **stdout**; JSON-emitting commands (`cabin resolve
//! --format json`, `cabin update --format json`, …) emit status
//! to **stderr** so the JSON document on stdout stays
//! machine-parseable.  The reporter offers both spellings
//! (`status` / `aux_status`) so callers pick the right stream
//! once and never re-derive the choice.

use std::fmt;
use std::io::Write;

use cabin_config::{
    ConfigDiscoveryInputs, EffectiveConfig, discover_config_files, merge_loaded_files,
};
use cabin_core::{Verbosity, VerbosityEnvError};

/// Discover the user-level Cabin config (no workspace context)
/// and return an [`EffectiveConfig`] suitable for passing to
/// [`resolve_verbosity`].  Errors are swallowed and an empty
/// effective config is returned: a missing or unparsable
/// config must not block the early reporter setup.  The
/// subcommand-level dispatcher will surface any parse errors
/// later through its normal error chain.
pub(crate) fn discover_early_config_verbosity() -> EffectiveConfig {
    let inputs = ConfigDiscoveryInputs::from_process(None);
    match discover_config_files(&inputs) {
        Ok(discovery) => merge_loaded_files(discovery.loaded_files),
        Err(_) => EffectiveConfig::default(),
    }
}

/// Validated verbosity inputs at the CLI boundary.  Mirrors the
/// raw `Cli` flags one-for-one so the dispatcher can produce a
/// single typed value without scattering `if quiet ... else if
/// verbose` branches over the call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CliVerbosity {
    /// Number of `-v` / `--verbose` occurrences.  Clamped at the
    /// caller — clap's `ArgAction::Count` already saturates at
    /// `u8::MAX`.
    pub(crate) verbose_count: u8,
    /// Whether `-q` / `--quiet` was passed.
    pub(crate) quiet: bool,
}

impl CliVerbosity {
    /// Translate the raw flag pair into a typed [`Verbosity`] if
    /// the user explicitly opted in / out, or `None` when neither
    /// flag was supplied so the next layer in the precedence
    /// chain can take over.
    fn into_verbosity(self) -> Option<Verbosity> {
        if self.quiet {
            return Some(Verbosity::Quiet);
        }
        if self.verbose_count > 0 {
            return Some(Verbosity::from_verbose_count(self.verbose_count));
        }
        None
    }
}

/// Apply Cabin's verbosity precedence:
/// 1. `--quiet` / `-v` / `--verbose` flags;
/// 2. `CABIN_TERM_QUIET` / `CABIN_TERM_VERBOSE` env vars;
/// 3. config `term.quiet` / `term.verbose`;
/// 4. default [`Verbosity::Normal`].
///
/// The function is pure: callers pass an env lookup closure so
/// tests can drive every branch without touching the process
/// environment.  An invalid env value bubbles up as a
/// [`VerbosityEnvError`].  Clap already rejects the `--quiet
/// --verbose` combination at parse time, so a CLI-level conflict
/// is never observed here.
pub(crate) fn resolve_verbosity<F>(
    cli: CliVerbosity,
    env: F,
    config: &EffectiveConfig,
) -> Result<Verbosity, VerbosityEnvError>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(level) = cli.into_verbosity() {
        return Ok(level);
    }

    let env_quiet = read_bool_env(&env, cabin_env::CABIN_TERM_QUIET)?;
    let env_verbose = read_bool_env(&env, cabin_env::CABIN_TERM_VERBOSE)?;
    if env_quiet || env_verbose {
        // Env vars are independent variables; an explicit truthy
        // value on either one wins over the config layer.  When
        // both are set the typed combiner rejects the pair with
        // the same wording the config layer uses.
        let combined =
            Verbosity::from_config_pair(env_verbose.then_some(true), env_quiet.then_some(true))
                .map_err(|_| VerbosityEnvError {
                    variable: cabin_env::CABIN_TERM_QUIET,
                    value: "1".to_owned(),
                })?;
        if let Some(level) = combined {
            return Ok(level);
        }
    }

    if let Some(setting) = &config.term.verbosity {
        return Ok(setting.level);
    }
    Ok(Verbosity::default())
}

fn read_bool_env<F>(env: &F, key: &'static str) -> Result<bool, VerbosityEnvError>
where
    F: Fn(&str) -> Option<String>,
{
    match env(key) {
        None => Ok(false),
        Some(raw) => Verbosity::parse_bool_env(key, &raw),
    }
}

/// Stream the [`Reporter`] should send a single line to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

/// Display context every subcommand uses to print Cabin-owned
/// status and verbose / very-verbose context lines.
///
/// The reporter is intentionally small: it owns a verbosity, not
/// a writer, so call sites stay easy to grep (`reporter.status(
/// ...)`, `reporter.verbose(...)`).  The actual writes go through
/// the process's stdout / stderr handles so existing tests that
/// match on either stream keep working.  Both `Clone` and `Copy`
/// are derived so the value can flow by-value through the few
/// helper chains that would otherwise need a borrow.
/// Width the cargo-style verb (`Compiling`, `Finished`,
/// `Created`, …) is right-aligned to inside `cargo_status`.
/// Matches cargo's own banner layout.
const COLUMN_WIDTH: usize = 12;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Reporter {
    verbosity: Verbosity,
    /// Resolved at construction time.  `true` means every
    /// styled write may emit ANSI escape sequences; `false`
    /// guarantees plain-text output.  The flag captures the
    /// `--color` / `CABIN_TERM_COLOR` / config / tty resolution
    /// so individual emit sites no longer probe the environment.
    styled: bool,
}

impl Reporter {
    /// Build a reporter for the given verbosity, with styled
    /// output disabled.  Callers that resolve a [`ColorChoice`]
    /// should prefer [`Reporter::with_color`] so cargo-style
    /// banners (`Compiling foo`, `Finished `dev` profile …`)
    /// render in color when the user asked for it.
    pub(crate) fn new(verbosity: Verbosity) -> Self {
        Self {
            verbosity,
            styled: false,
        }
    }

    /// Build a reporter that emits styled status lines when the
    /// resolved [`ColorChoice`] says it should.  `Auto` is
    /// honored by probing whether the current stdout handle is
    /// a terminal — matching what the rest of Cabin's
    /// diagnostic renderer does for stderr.
    pub(crate) fn with_color(verbosity: Verbosity, color: cabin_core::ColorChoice) -> Self {
        let styled = match color {
            cabin_core::ColorChoice::Always => true,
            cabin_core::ColorChoice::Never => false,
            cabin_core::ColorChoice::Auto => std::io::IsTerminal::is_terminal(&std::io::stdout()),
        };
        Self { verbosity, styled }
    }

    /// Read the resolved verbosity back.  Callers that need
    /// the typed value read it through this accessor instead
    /// of stashing the value alongside the reporter.
    pub(crate) fn verbosity(self) -> Verbosity {
        self.verbosity
    }

    /// Emit a Cabin-owned status line on stdout, suppressed in
    /// `Quiet` mode.  Status lines describe progress
    /// (`cabin: wrote build.ninja`, `cabin: removed N paths`),
    /// not user-facing results.  Use `aux_status` instead when
    /// stdout is reserved for a JSON document or other
    /// machine-readable output.
    pub(crate) fn status(self, args: fmt::Arguments<'_>) {
        if self.verbosity.shows_status() {
            self.write(Stream::Stdout, args);
        }
    }

    /// Emit a cargo-style banner: `<right-padded verb> <rest>`,
    /// where the verb (`Compiling`, `Finished`, `Created`, …)
    /// renders in bright green + bold when the reporter is
    /// styled and as plain text otherwise.  The verb is
    /// right-aligned to column 12 to match cargo's banner
    /// layout, so `Compiling` and `Finished` align cleanly even
    /// though they differ in length.
    pub(crate) fn cargo_status(self, verb: &str, args: fmt::Arguments<'_>) {
        if !self.verbosity.shows_status() {
            return;
        }
        // Pad spaces before the styled span so the leading
        // alignment is plain text; only the verb itself carries
        // the ANSI escape.  Both cargo and the C++ Cabin emit
        // their banner with this shape.
        let padding = COLUMN_WIDTH.saturating_sub(verb.len());
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = if self.styled {
            // SGR 1 (bold) + 32 (green foreground), reset
            // afterwards so the rest of the line stays plain.
            writeln!(
                handle,
                "{:padding$}\x1b[1;32m{verb}\x1b[0m {args}",
                "",
                padding = padding,
            )
        } else {
            writeln!(handle, "{:padding$}{verb} {args}", "", padding = padding)
        };
    }

    /// Same as [`Reporter::status`] but routes the line to
    /// stderr so JSON-emitting commands (`cabin resolve --format
    /// json`, `cabin update --format json`, …) keep their stdout
    /// document clean.
    pub(crate) fn aux_status(self, args: fmt::Arguments<'_>) {
        if self.verbosity.shows_status() {
            self.write(Stream::Stderr, args);
        }
    }

    /// Emit a user-facing warning on stderr. Warnings are not
    /// verbosity-gated: they report a degraded or partial result
    /// rather than ordinary progress.
    pub(crate) fn warning(self, args: fmt::Arguments<'_>) {
        let stderr = std::io::stderr();
        let mut handle = stderr.lock();
        let _ = writeln!(handle, "cabin: warning: {args}");
    }

    /// Emit a Rust-compiler-style `help:` block on stderr,
    /// styled to match what `cabin-diagnostics` paints for typed
    /// errors:
    ///
    /// - one blank line of separation from the preceding error,
    /// - the literal `help:` in cyan + bold when styling is on,
    /// - the first line of `body` after a single space,
    /// - every continuation line indented six columns so its
    ///   first non-space byte lines up under the first
    ///   non-`help:` byte of line one (`help: ` is six bytes),
    /// - blank lines inside `body` are emitted as truly empty
    ///   lines (no trailing whitespace) so paragraph breaks
    ///   stay clean,
    /// - a trailing newline after the final line so the next
    ///   error sits visually apart.
    ///
    /// Used by failure sites that have extra context which
    /// helps the user fix the underlying problem (e.g. the
    /// linker-error diagnostic that points at a declared-but-
    /// unlinked `[dependencies]` entry).
    pub(crate) fn help(self, body: &str) {
        let stderr = std::io::stderr();
        let mut handle = stderr.lock();
        let _ = writeln!(handle);
        let mut lines = body.lines();
        let Some(first) = lines.next() else {
            return;
        };
        if self.styled {
            // SGR 1 (bold) + 36 (cyan foreground), reset
            // afterwards so the body stays plain.
            let _ = writeln!(handle, "\x1b[1;36mhelp:\x1b[0m {first}");
        } else {
            let _ = writeln!(handle, "help: {first}");
        }
        for line in lines {
            if line.is_empty() {
                let _ = writeln!(handle);
            } else {
                let _ = writeln!(handle, "      {line}");
            }
        }
    }

    /// Verbose-only status line on stdout.  Suppressed below
    /// `Verbose`.  Used to surface the resolved profile, build
    /// directory, and similar Cabin-owned context.
    pub(crate) fn verbose(self, args: fmt::Arguments<'_>) {
        if self.verbosity.shows_verbose() {
            self.write(Stream::Stdout, args);
        }
    }

    /// Same as [`Reporter::verbose`] but routes to stderr for
    /// shared orchestration paths that may run under
    /// machine-readable stdout commands.
    pub(crate) fn aux_verbose(self, args: fmt::Arguments<'_>) {
        if self.verbosity.shows_verbose() {
            self.write(Stream::Stderr, args);
        }
    }

    /// Very-verbose status line on stdout.  Suppressed below
    /// `VeryVerbose`.  Used for executed command lines and
    /// similar local-build diagnostics.
    pub(crate) fn very_verbose(self, args: fmt::Arguments<'_>) {
        if self.verbosity.shows_very_verbose() {
            self.write(Stream::Stdout, args);
        }
    }

    /// Same as [`Reporter::very_verbose`] but routes to stderr for
    /// shared orchestration paths that may run under
    /// machine-readable stdout commands.
    pub(crate) fn aux_very_verbose(self, args: fmt::Arguments<'_>) {
        if self.verbosity.shows_very_verbose() {
            self.write(Stream::Stderr, args);
        }
    }

    fn write(self, stream: Stream, args: fmt::Arguments<'_>) {
        match stream {
            Stream::Stdout => {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                let _ = writeln!(handle, "{args}");
            }
            Stream::Stderr => {
                let stderr = std::io::stderr();
                let mut handle = stderr.lock();
                let _ = writeln!(handle, "{args}");
            }
        }
    }
}

impl Default for Reporter {
    fn default() -> Self {
        Self::new(Verbosity::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn env_with<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn cfg() -> EffectiveConfig {
        EffectiveConfig::default()
    }

    fn cfg_with_verbosity(level: Verbosity) -> EffectiveConfig {
        let mut effective = EffectiveConfig::default();
        effective.term.verbosity = Some(cabin_config::EffectiveVerbosity {
            level,
            source: cabin_config::ConfigSource::User,
        });
        effective
    }

    fn cli(verbose_count: u8, quiet: bool) -> CliVerbosity {
        CliVerbosity {
            verbose_count,
            quiet,
        }
    }

    #[test]
    fn defaults_to_normal_with_no_inputs() {
        let resolved = resolve_verbosity(cli(0, false), no_env, &cfg()).unwrap();
        assert_eq!(resolved, Verbosity::Normal);
    }

    #[test]
    fn cli_verbose_count_one_yields_verbose() {
        let resolved = resolve_verbosity(cli(1, false), no_env, &cfg()).unwrap();
        assert_eq!(resolved, Verbosity::Verbose);
    }

    #[test]
    fn cli_verbose_count_two_or_more_yields_very_verbose() {
        let resolved = resolve_verbosity(cli(2, false), no_env, &cfg()).unwrap();
        assert_eq!(resolved, Verbosity::VeryVerbose);
        let resolved = resolve_verbosity(cli(7, false), no_env, &cfg()).unwrap();
        assert_eq!(resolved, Verbosity::VeryVerbose);
    }

    #[test]
    fn cli_quiet_overrides_config_verbose() {
        let resolved = resolve_verbosity(
            cli(0, true),
            no_env,
            &cfg_with_verbosity(Verbosity::Verbose),
        )
        .unwrap();
        assert_eq!(resolved, Verbosity::Quiet);
    }

    #[test]
    fn cli_verbose_overrides_config_quiet() {
        let resolved =
            resolve_verbosity(cli(1, false), no_env, &cfg_with_verbosity(Verbosity::Quiet))
                .unwrap();
        assert_eq!(resolved, Verbosity::Verbose);
    }

    #[test]
    fn env_verbose_applies_when_cli_silent() {
        let resolved = resolve_verbosity(
            cli(0, false),
            env_with(&[(cabin_env::CABIN_TERM_VERBOSE, "1")]),
            &cfg(),
        )
        .unwrap();
        assert_eq!(resolved, Verbosity::Verbose);
    }

    #[test]
    fn env_quiet_applies_when_cli_silent() {
        let resolved = resolve_verbosity(
            cli(0, false),
            env_with(&[(cabin_env::CABIN_TERM_QUIET, "true")]),
            &cfg(),
        )
        .unwrap();
        assert_eq!(resolved, Verbosity::Quiet);
    }

    #[test]
    fn env_overrides_config() {
        let resolved = resolve_verbosity(
            cli(0, false),
            env_with(&[(cabin_env::CABIN_TERM_VERBOSE, "1")]),
            &cfg_with_verbosity(Verbosity::Quiet),
        )
        .unwrap();
        assert_eq!(resolved, Verbosity::Verbose);
    }

    #[test]
    fn env_both_truthy_is_rejected() {
        let err = resolve_verbosity(
            cli(0, false),
            env_with(&[
                (cabin_env::CABIN_TERM_VERBOSE, "1"),
                (cabin_env::CABIN_TERM_QUIET, "1"),
            ]),
            &cfg(),
        )
        .unwrap_err();
        // The error names one of the two variables; either is
        // an actionable hint for the user to remove the conflict.
        assert!(
            err.variable == cabin_env::CABIN_TERM_QUIET
                || err.variable == cabin_env::CABIN_TERM_VERBOSE
        );
    }

    #[test]
    fn invalid_env_value_bubbles_up_as_typed_error() {
        let err = resolve_verbosity(
            cli(0, false),
            env_with(&[(cabin_env::CABIN_TERM_VERBOSE, "loud")]),
            &cfg(),
        )
        .unwrap_err();
        assert_eq!(err.variable, cabin_env::CABIN_TERM_VERBOSE);
        assert_eq!(err.value, "loud");
    }

    #[test]
    fn config_applies_when_cli_and_env_silent() {
        let resolved = resolve_verbosity(
            cli(0, false),
            no_env,
            &cfg_with_verbosity(Verbosity::Verbose),
        )
        .unwrap();
        assert_eq!(resolved, Verbosity::Verbose);
    }

    #[test]
    fn empty_env_falls_through_to_config() {
        let resolved = resolve_verbosity(
            cli(0, false),
            env_with(&[(cabin_env::CABIN_TERM_VERBOSE, "")]),
            &cfg_with_verbosity(Verbosity::Quiet),
        )
        .unwrap();
        assert_eq!(resolved, Verbosity::Quiet);
    }
}
