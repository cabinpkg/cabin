//! `pkg-config` runner used to probe Cabin's
//! `system = true` dependencies.
//!
//! `cabin-system-deps` is the only crate that knows how to spawn
//! `pkg-config`, parse its output, and translate the result into
//! typed flag contributions for Cabin's build planner. It mirrors
//! the boundary the other developer-tools crates
//! (`cabin-fmt`, `cabin-tidy`) keep:
//!
//! - the crate owns executable resolution and `pkg-config`
//!   command-line construction;
//! - it accepts typed inputs ([`SystemDependencyProbeRequest`])
//!   and emits typed outcomes ([`SystemDependencyResolution`]);
//! - it never reads Cabin's configuration files, walks the
//!   filesystem, or merges resolved flags into the build planner
//!   — the orchestration layer threads the report into
//!   `ResolvedProfileFlags`.
//!
//! The crate is invoked once per `cabin build` / `cabin run` /
//! `cabin test` / `cabin tidy` / `cabin metadata` invocation when
//! the selected workspace declares at least one system dependency;
//! a workspace with no system dependencies never spawns
//! `pkg-config`.

#![deny(missing_docs)]
#![allow(clippy::return_self_not_must_use)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::process::Command;

use camino::Utf8PathBuf;

use miette::Diagnostic;
use thiserror::Error;

use cabin_env::CABIN_PKG_CONFIG as CABIN_PKG_CONFIG_ENV;

/// Default executable name Cabin spawns when [`CABIN_PKG_CONFIG_ENV`]
/// is not set. Resolved against `PATH` by the child process spawn.
pub const DEFAULT_PKG_CONFIG_EXECUTABLE: &str = "pkg-config";

/// Resolve the `pkg-config` executable Cabin should spawn.
///
/// Reads [`CABIN_PKG_CONFIG_ENV`] via the supplied env-lookup
/// closure: a non-empty value is used verbatim, otherwise the
/// default executable name is returned and the spawn relies on
/// `PATH`. The closure interface keeps the function pure so
/// tests can pass a fake env without touching the process
/// environment.
pub fn resolve_pkg_config_executable<F>(env: F) -> OsString
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(value) = env(CABIN_PKG_CONFIG_ENV)
        && !value.is_empty()
    {
        return value;
    }
    OsString::from(DEFAULT_PKG_CONFIG_EXECUTABLE)
}

/// Typed `pkg-config` tool handle.
///
/// Holds the resolved executable name (or absolute path) and the
/// optional extra env vars Cabin applies to every child
/// invocation. Constructed once per orchestrating call and
/// reused across every system dependency to keep the diagnostic
/// shape stable. Tests can attach a fixture-pointing env var
/// here without touching the process-wide environment.
#[derive(Debug, Clone)]
pub struct PkgConfigTool {
    executable: OsString,
    extra_env: BTreeMap<OsString, OsString>,
}

impl PkgConfigTool {
    /// Build a [`PkgConfigTool`] from a resolved executable.
    pub fn new(executable: OsString) -> Self {
        Self {
            executable,
            extra_env: BTreeMap::new(),
        }
    }

    /// Build a [`PkgConfigTool`] by reading
    /// [`CABIN_PKG_CONFIG_ENV`] from the supplied env closure.
    /// Convenience for callers that do not already have a
    /// resolved [`OsString`] handy.
    pub fn from_env<F>(env: F) -> Self
    where
        F: Fn(&str) -> Option<OsString>,
    {
        Self::new(resolve_pkg_config_executable(env))
    }

    /// Set an additional environment variable that is applied to
    /// every spawned `pkg-config` invocation. Used by tests that
    /// need to point the fake binary at a fixture directory
    /// without mutating the process environment.
    pub fn with_extra_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.extra_env.insert(key.into(), value.into());
        self
    }

    /// Executable path / command name Cabin will spawn.
    pub fn executable(&self) -> &OsStr {
        &self.executable
    }

    /// Verify the executable is reachable. Returns
    /// [`PkgConfigError::ExecutableNotFound`] when the spawn
    /// returns `NotFound`, and [`PkgConfigError::InvocationFailed`]
    /// for any other spawn / exit error. Successful invocations
    /// (any exit status) are reported as `Ok(())` so callers can
    /// move on to the actual probe.
    ///
    /// # Errors
    /// Returns [`PkgConfigError::ExecutableNotFound`] when spawning
    /// `pkg-config --version` fails with `NotFound`, and
    /// [`PkgConfigError::InvocationFailed`] for any other spawn error.
    pub fn check_available(&self) -> Result<(), PkgConfigError> {
        let mut cmd = Command::new(&self.executable);
        cmd.arg("--version");
        self.apply_extra_env(&mut cmd);
        match cmd.output() {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Err(PkgConfigError::ExecutableNotFound {
                    executable: self.executable.to_string_lossy().into_owned(),
                })
            }
            Err(err) => Err(PkgConfigError::InvocationFailed {
                executable: self.executable.to_string_lossy().into_owned(),
                stage: "--version".to_owned(),
                detail: err.to_string(),
            }),
        }
    }

    fn apply_extra_env(&self, cmd: &mut Command) {
        for (k, v) in &self.extra_env {
            cmd.env(k, v);
        }
    }
}

/// Inputs for [`probe_system_dependency`].
///
/// The orchestration layer fills this in from already-resolved
/// typed values: each system dependency from the manifest plus
/// the shared [`PkgConfigTool`] handle.
#[derive(Debug, Clone)]
pub struct SystemDependencyProbeRequest<'a> {
    /// The system dependency name. This is the package name
    /// Cabin passes to `pkg-config` — Cabin does not yet support
    /// a separate `pkg-config-name` field, so the manifest name
    /// and the pkg-config name are identical.
    pub name: &'a str,
    /// Free-form version requirement string as declared in the
    /// manifest. Empty when no version constraint was specified.
    pub version_requirement: &'a str,
    /// The shared `pkg-config` handle.
    pub tool: &'a PkgConfigTool,
}

/// Successful probe details for a single system dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemDependencyResolution {
    /// The system dependency name from the manifest.
    pub name: String,
    /// Version string reported by `pkg-config --modversion`.
    /// `None` when `pkg-config` reported no version.
    pub version: Option<String>,
    /// Typed contributions from `pkg-config --cflags` and
    /// `pkg-config --libs`, classified by purpose.
    pub flags: SystemDependencyFlags,
}

/// Typed flag contributions derived from a successful
/// `pkg-config` probe. The orchestration layer merges these into
/// the per-package [`cabin_core::ResolvedProfileFlags`] before the
/// build planner consumes them.
///
/// All vectors preserve the order `pkg-config` reported because
/// link ordering and include-path ordering are load-bearing for
/// C/C++ builds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemDependencyFlags {
    /// Include directories extracted from `-I` tokens in
    /// `--cflags`. The `-I` prefix has been stripped so the
    /// orchestration layer can hand them to
    /// [`cabin_core::ResolvedProfileFlags::include_dirs`] verbatim.
    pub include_dirs: Vec<Utf8PathBuf>,
    /// Remaining compile arguments from `--cflags` after include
    /// directories were classified out. Preserved verbatim in
    /// the order `pkg-config` emitted them.
    pub extra_compile_args: Vec<String>,
    /// Every token reported by `--libs`, in order. The link
    /// driver appends these verbatim to the link command.
    pub ldflags: Vec<String>,
}

impl SystemDependencyFlags {
    /// Whether this contribution is empty (no `-I`, no compile
    /// args, no link args).
    pub fn is_empty(&self) -> bool {
        self.include_dirs.is_empty()
            && self.extra_compile_args.is_empty()
            && self.ldflags.is_empty()
    }
}

/// Errors surfaced by the probe layer.
///
/// User-facing variants carry stable
/// `cabin::system_deps::<symbol>` diagnostic codes so external
/// tooling can match against them. Internal-only variants exist
/// for the rare "pkg-config produced output we cannot interpret"
/// case so the orchestration layer can still print a useful
/// message.
#[derive(Debug, Error, Diagnostic)]
pub enum PkgConfigError {
    /// The `pkg-config` executable Cabin tried to spawn was not
    /// found on the host.
    #[error("pkg-config executable {executable:?} was not found on PATH")]
    #[diagnostic(
        code(cabin::system_deps::executable_not_found),
        help(
            "install `pkg-config` (e.g. via your OS package manager) and re-run, or set `{}` to an absolute path",
            CABIN_PKG_CONFIG_ENV
        )
    )]
    ExecutableNotFound {
        /// Executable Cabin tried to spawn.
        executable: String,
    },

    /// `pkg-config` could not be invoked for a reason other than
    /// "not found" (permission denied, I/O error, etc.).
    #[error("failed to invoke pkg-config ({stage}): {detail}")]
    #[diagnostic(code(cabin::system_deps::invocation_failed))]
    InvocationFailed {
        /// Executable Cabin tried to spawn.
        executable: String,
        /// Which probe stage was running when the error happened.
        /// Stable strings: `--version`, `--exists`, `--modversion`,
        /// `--cflags`, `--libs`.
        stage: String,
        /// Stringified underlying error.
        detail: String,
    },

    /// `pkg-config --exists` reported the system dependency was
    /// missing.
    #[error("system dependency {name:?} was not found by pkg-config")]
    #[diagnostic(
        code(cabin::system_deps::package_not_found),
        help("install the system library or update PKG_CONFIG_PATH so pkg-config can find it")
    )]
    PackageNotFound {
        /// The system dependency name from the manifest.
        name: String,
        /// Stderr text `pkg-config` produced, trimmed of trailing
        /// whitespace. Empty when `pkg-config` printed nothing.
        stderr: String,
    },

    /// The system dependency was found but its installed version
    /// did not satisfy the requirement declared in the manifest.
    #[error(
        "system dependency {name:?} does not satisfy version requirement {requirement:?}{}",
        display_installed(installed.as_deref())
    )]
    #[diagnostic(
        code(cabin::system_deps::version_mismatch),
        help(
            "install a version that satisfies the requirement, or relax the `system = true` dependency's version constraint"
        )
    )]
    VersionMismatch {
        /// The system dependency name from the manifest.
        name: String,
        /// Cabin's view of the requirement (as written in the
        /// manifest).
        requirement: String,
        /// Installed version `pkg-config` reported, or `None`
        /// when `pkg-config` declined to report a version.
        installed: Option<String>,
    },

    /// The version requirement string in the manifest could not
    /// be interpreted as a recognized `SemVer` comparator list and
    /// `pkg-config` itself rejected it. The probe layer never
    /// rewrites the user's text; the diagnostic quotes it
    /// verbatim.
    #[error("system dependency {name:?} declares unsupported version requirement {requirement:?}")]
    #[diagnostic(code(cabin::system_deps::invalid_version_requirement))]
    InvalidVersionRequirement {
        /// The system dependency name from the manifest.
        name: String,
        /// Free-form requirement string Cabin failed to interpret.
        requirement: String,
    },

    /// `pkg-config` exited non-zero for a reason other than
    /// "package missing" or "version mismatch". Typical causes
    /// include malformed `.pc` files or environment-variable
    /// misconfiguration.
    #[error(
        "pkg-config failed while probing system dependency {name:?}{}",
        display_block(stderr)
    )]
    #[diagnostic(code(cabin::system_deps::pkg_config_failed))]
    PkgConfigFailed {
        /// The system dependency name from the manifest.
        name: String,
        /// Stage that failed (`--cflags` / `--libs` / `--modversion`).
        stage: String,
        /// Stderr text `pkg-config` produced.
        stderr: String,
    },

    /// `pkg-config` produced output that the splitter could not
    /// turn back into a sequence of argv-shaped tokens. Surfaced
    /// rather than silently dropped because compile / link
    /// behavior depends on every token.
    #[error("pkg-config produced unparsable output for system dependency {name:?}: {detail}")]
    #[diagnostic(code(cabin::system_deps::malformed_output))]
    MalformedOutput {
        /// The system dependency name from the manifest.
        name: String,
        /// Short human-readable detail.
        detail: String,
        /// Raw output `pkg-config` emitted.
        raw: String,
    },
}

fn display_installed(installed: Option<&str>) -> String {
    match installed {
        Some(v) => format!(" (installed: {v})"),
        None => String::new(),
    }
}

fn display_block(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

/// Probe a single system dependency through `pkg-config`.
///
/// The implementation always asks `pkg-config` to evaluate the
/// version constraint when one is present so the comparison
/// honors pkg-config's own debian-style version rules. Cabin
/// converts the comparator list to `pkg-config`'s argv form
/// where possible; if conversion fails because the requirement
/// is not recognizable `SemVer`, the raw requirement is forwarded
/// verbatim so pkg-config still gets a chance to interpret it.
///
/// # Errors
/// Returns [`PkgConfigError::InvalidVersionRequirement`] when the
/// version requirement is neither recognizable `SemVer` nor a token
/// pkg-config could accept. When `--exists` fails, returns
/// [`PkgConfigError::PackageNotFound`] or
/// [`PkgConfigError::VersionMismatch`] via `classify_exists_failure`.
/// Returns [`PkgConfigError::PkgConfigFailed`] when `--cflags` or
/// `--libs` exit non-zero, and [`PkgConfigError::MalformedOutput`]
/// when their output cannot be split into argv tokens (including a
/// trailing `-I` with no path). Propagates
/// [`PkgConfigError::ExecutableNotFound`] or
/// [`PkgConfigError::InvocationFailed`] when a `pkg-config` spawn
/// fails (with `NotFound` or any other error, respectively).
pub fn probe_system_dependency(
    req: &SystemDependencyProbeRequest<'_>,
) -> Result<SystemDependencyResolution, PkgConfigError> {
    let constraints = build_constraints(req.name, req.version_requirement, &req.tool.executable)?;

    // Stage 1: existence + version check. `--print-errors`
    // causes pkg-config to emit a human-readable description on
    // stderr that we can surface to the user.
    let exists_output = run_pkg_config(
        req.tool,
        "--exists",
        std::iter::once(OsString::from("--print-errors"))
            .chain(std::iter::once(OsString::from("--exists")))
            .chain(constraints.argv.iter().cloned()),
    )?;
    if !exists_output.status.success() {
        let stderr = trim_stderr(&exists_output.stderr);
        return Err(classify_exists_failure(
            req.name,
            req.version_requirement,
            constraints.had_constraint,
            stderr,
            req.tool,
        )?);
    }

    // Stage 2: modversion (best-effort; absence is not fatal).
    // Some pkg-config implementations omit Version: from .pc
    // files and exit non-zero here even when the package is
    // present. Pass only the bare module name so the response
    // is a single version string, never a list.
    let version = match run_pkg_config(
        req.tool,
        "--modversion",
        std::iter::once(OsString::from("--modversion"))
            .chain(std::iter::once(OsString::from(req.name))),
    ) {
        Ok(out) if out.status.success() => Some(trim_stdout(&out.stdout)),
        _ => None,
    };

    // Stage 3 / 4: --cflags and --libs. The version constraint
    // was already enforced by --exists, so we ask pkg-config
    // for just the bare module's flags. Real pkg-config's
    // --cflags / --libs deduplicate by module name when the
    // constraints all refer to the same module; passing the
    // bare name keeps the response unambiguous across
    // implementations.
    let cflags_output = run_pkg_config(
        req.tool,
        "--cflags",
        std::iter::once(OsString::from("--cflags"))
            .chain(std::iter::once(OsString::from(req.name))),
    )?;
    if !cflags_output.status.success() {
        return Err(PkgConfigError::PkgConfigFailed {
            name: req.name.to_owned(),
            stage: "--cflags".to_owned(),
            stderr: trim_stderr(&cflags_output.stderr),
        });
    }
    let libs_output = run_pkg_config(
        req.tool,
        "--libs",
        std::iter::once(OsString::from("--libs")).chain(std::iter::once(OsString::from(req.name))),
    )?;
    if !libs_output.status.success() {
        return Err(PkgConfigError::PkgConfigFailed {
            name: req.name.to_owned(),
            stage: "--libs".to_owned(),
            stderr: trim_stderr(&libs_output.stderr),
        });
    }

    let cflags_text = trim_stdout(&cflags_output.stdout);
    let libs_text = trim_stdout(&libs_output.stdout);
    let cflag_tokens = split_pkg_config_output(&cflags_text).map_err(|detail| {
        PkgConfigError::MalformedOutput {
            name: req.name.to_owned(),
            detail: detail.to_owned(),
            raw: cflags_text.clone(),
        }
    })?;
    let lib_tokens =
        split_pkg_config_output(&libs_text).map_err(|detail| PkgConfigError::MalformedOutput {
            name: req.name.to_owned(),
            detail: detail.to_owned(),
            raw: libs_text.clone(),
        })?;

    let mut flags = SystemDependencyFlags::default();
    let mut seen_include_dirs: BTreeSet<Utf8PathBuf> = BTreeSet::new();
    let mut iter = cflag_tokens.into_iter().peekable();
    while let Some(tok) = iter.next() {
        if let Some(rest) = tok.strip_prefix("-I") {
            let path_str = if rest.is_empty() {
                match iter.next() {
                    Some(next) => next,
                    None => {
                        // Lone `-I` with no path is malformed
                        // output. Surface it rather than dropping.
                        return Err(PkgConfigError::MalformedOutput {
                            name: req.name.to_owned(),
                            detail: "trailing `-I` with no path".to_owned(),
                            raw: cflags_text,
                        });
                    }
                }
            } else {
                rest.to_owned()
            };
            let path = Utf8PathBuf::from(path_str);
            if seen_include_dirs.insert(path.clone()) {
                flags.include_dirs.push(path);
            }
            continue;
        }
        flags.extra_compile_args.push(tok);
    }
    // Link tokens preserve order verbatim: pkg-config emits
    // `-L<path>` before the `-l<name>` tokens that depend on it,
    // and reordering would break the link.
    flags.ldflags = lib_tokens;

    Ok(SystemDependencyResolution {
        name: req.name.to_owned(),
        version,
        flags,
    })
}

/// pkg-config argv built from a Cabin version requirement.
#[derive(Debug, Clone)]
struct ConstraintArgv {
    /// Positional pkg-config tokens. Always starts with the
    /// module name; followed by zero or more `op version` pairs
    /// when a requirement was supplied.
    argv: Vec<OsString>,
    /// Whether a version constraint contributed to `argv` (used
    /// to disambiguate "not found" from "version mismatch" later).
    had_constraint: bool,
}

fn build_constraints(
    name: &str,
    requirement: &str,
    _executable: &OsStr,
) -> Result<ConstraintArgv, PkgConfigError> {
    let raw = requirement.trim();
    let mut argv: Vec<OsString> = Vec::new();
    argv.push(OsString::from(name));
    if raw.is_empty() {
        return Ok(ConstraintArgv {
            argv,
            had_constraint: false,
        });
    }
    if let Some(constraints) = convert_requirement(raw) {
        let had_constraint = !constraints.is_empty();
        for (op, ver) in constraints {
            argv.push(OsString::from(name));
            argv.push(OsString::from(op));
            argv.push(OsString::from(ver));
        }
        Ok(ConstraintArgv {
            argv,
            had_constraint,
        })
    } else {
        // The requirement is not recognizable SemVer.
        // Cabin's version field is documented as free-form,
        // so we forward the raw text directly. pkg-config
        // accepts a single positional `name op version`
        // when the whole argument is a single token; if it
        // contains whitespace, split it so each token lands
        // in a separate argv slot.
        let split: Vec<&str> = raw.split_whitespace().collect();
        if split.is_empty() {
            return Ok(ConstraintArgv {
                argv,
                had_constraint: false,
            });
        }
        // We require at least an operator and a version, or
        // a single token that pkg-config itself can interpret.
        // Treat the operator-less single token as
        // unsupported because pkg-config will reject it too.
        if split.len() == 1 && !looks_like_pkg_config_operator(split[0]) {
            return Err(PkgConfigError::InvalidVersionRequirement {
                name: name.to_owned(),
                requirement: requirement.to_owned(),
            });
        }
        let mut iter = split.into_iter();
        argv.push(OsString::from(name));
        for tok in &mut iter {
            argv.push(OsString::from(tok));
        }
        Ok(ConstraintArgv {
            argv,
            had_constraint: true,
        })
    }
}

fn looks_like_pkg_config_operator(tok: &str) -> bool {
    matches!(tok, "=" | "!=" | "<" | ">" | "<=" | ">=")
}

/// Convert a Cabin / npm-flavored `SemVer` requirement into a
/// list of `(operator, version)` pairs the pkg-config CLI
/// accepts. Returns `None` when the input cannot be parsed as
/// `SemVer` so callers can fall back to a verbatim forward.
fn convert_requirement(raw: &str) -> Option<Vec<(String, String)>> {
    let req = cabin_core::version_req::parse_lenient(raw).ok()?;
    let mut out: Vec<(String, String)> = Vec::new();
    for comp in &req.comparators {
        for pair in comparator_to_pkg_config(comp) {
            out.push(pair);
        }
    }
    // `*` parses as `VersionReq` with no comparators; the call site
    // treats `Some(empty)` as "no pkg-config version constraint" and
    // sets `had_constraint = false` so the wildcard does not flow into
    // the verbatim-fallback path that would reject `*` as unparsable.
    Some(out)
}

fn comparator_to_pkg_config(comp: &semver::Comparator) -> Vec<(String, String)> {
    // pkg-config compares string version triples segment-wise.
    // Render minor / patch when present; for partial requirements
    // (e.g. `>= 1`) drop the missing segments so the comparison
    // matches what `>= 1` means to pkg-config (any 1.x.y).
    let base = render_version(comp);
    match comp.op {
        semver::Op::Exact => vec![("=".to_owned(), base)],
        semver::Op::Greater => vec![(">".to_owned(), base)],
        semver::Op::GreaterEq => vec![(">=".to_owned(), base)],
        semver::Op::Less => vec![("<".to_owned(), base)],
        semver::Op::LessEq => vec![("<=".to_owned(), base)],
        semver::Op::Tilde => tilde_to_pkg_config(comp),
        semver::Op::Caret => caret_to_pkg_config(comp),
        semver::Op::Wildcard => {
            // `1.*` parses as Wildcard with major=1, minor=None.
            // pkg-config has no wildcard primitive; expand into a
            // `>=` / `<` range with the next bumped segment. A
            // `u64`-ceiling component carries into the next-higher one;
            // the upper bound is dropped only when the major overflows.
            let lower = render_version(comp);
            let mut out = vec![(">=".to_owned(), lower)];
            if let Some(upper) = wildcard_upper_bound(comp) {
                out.push(("<".to_owned(), upper));
            }
            out
        }
        _ => Vec::new(),
    }
}

fn render_version(comp: &semver::Comparator) -> String {
    let mut out = comp.major.to_string();
    if let Some(minor) = comp.minor {
        out.push('.');
        out.push_str(&minor.to_string());
        if let Some(patch) = comp.patch {
            out.push('.');
            out.push_str(&patch.to_string());
        }
    }
    if !comp.pre.is_empty() {
        out.push('-');
        out.push_str(comp.pre.as_str());
    }
    out
}

/// Exclusive upper bound `(major+1).0.0` — the start of the next
/// major series. `None` when the major is already saturated, so no
/// representable version exists above the lower bound and the caller
/// drops the `<` constraint.
fn next_major_series(major: u64) -> Option<String> {
    major.checked_add(1).map(|m| format!("{m}.0.0"))
}

/// Exclusive upper bound `major.(minor+1).0` — the start of the next
/// minor series. A minor at the `u64` ceiling carries into the next
/// major (`~1.MAX` ⇒ `< 2.0.0`); `None` only when the major is also
/// saturated.
fn next_minor_series(major: u64, minor: u64) -> Option<String> {
    match minor.checked_add(1) {
        Some(m) => Some(format!("{major}.{m}.0")),
        None => next_major_series(major),
    }
}

/// Exclusive upper bound `major.minor.(patch+1)` — the next patch. A
/// patch at the ceiling carries into the next minor (then major).
fn next_patch_series(major: u64, minor: u64, patch: u64) -> Option<String> {
    match patch.checked_add(1) {
        Some(p) => Some(format!("{major}.{minor}.{p}")),
        None => next_minor_series(major, minor),
    }
}

fn tilde_to_pkg_config(comp: &semver::Comparator) -> Vec<(String, String)> {
    // `~1.2.3` ≡ `>=1.2.3, <1.3.0`
    // `~1.2`   ≡ `>=1.2.0, <1.3.0`
    // `~1`     ≡ `>=1.0.0, <2.0.0`
    let base = render_version(comp);
    // The upper bound is the next series start. A component at the
    // `u64` ceiling carries into the next-higher component (`~1.MAX`
    // ⇒ `< 2.0.0`); the `<` constraint is dropped only when the major
    // itself is saturated, where no representable upper bound exists.
    let upper = match (comp.minor, comp.patch) {
        (Some(minor), _) => next_minor_series(comp.major, minor),
        (None, _) => next_major_series(comp.major),
    };
    let mut out = vec![(">=".to_owned(), base)];
    if let Some(upper) = upper {
        out.push(("<".to_owned(), upper));
    }
    out
}

fn caret_to_pkg_config(comp: &semver::Comparator) -> Vec<(String, String)> {
    // Cargo / npm caret rules: bump the leftmost non-zero
    // segment. The exact form pkg-config consumes is a
    // `>=lower, <upper` pair.
    let lower = render_version(comp);
    let upper = caret_upper_bound(comp);
    vec![(">=".to_owned(), lower), ("<".to_owned(), upper)]
}

fn caret_upper_bound(comp: &semver::Comparator) -> String {
    // `^I` widens across the whole major series (`<(I+1).0.0`,
    // including `^0` ⇒ `<1.0.0`), and `^0.0` (patch unspecified)
    // widens to `<0.1.0`. Neither is a leftmost-non-zero bump of a
    // single triple, so resolve those partial forms here and defer
    // every fully specified form to the shared kernel.
    let (major, minor, patch) = match (comp.minor, comp.patch) {
        (Some(minor), Some(patch)) => (comp.major, minor, patch),
        (Some(0), None) if comp.major == 0 => return "0.1.0".to_owned(),
        (Some(minor), None) => (comp.major, minor, 0),
        (None, _) => return format!("{}.0.0", comp.major.saturating_add(1)),
    };
    let (major, minor, patch) = cabin_core::version_req::caret_upper_bound(major, minor, patch);
    format!("{major}.{minor}.{patch}")
}

fn wildcard_upper_bound(comp: &semver::Comparator) -> Option<String> {
    // The next series start, carrying a `u64`-ceiling component into
    // the next-higher one (`1.MAX.*` ⇒ `< 2.0.0`). `None` only when
    // the major is saturated, where the caller drops the `<` bound.
    match (comp.minor, comp.patch) {
        (None, _) => next_major_series(comp.major),
        (Some(minor), None) => next_minor_series(comp.major, minor),
        // `1.2.*` is unusual but render `< 1.2.(patch+1)` to be safe.
        (Some(minor), Some(patch)) => next_patch_series(comp.major, minor, patch),
    }
}

/// Whitespace + minimal-quoting splitter for `pkg-config`
/// output. Handles single quotes, double quotes, and backslash
/// escapes. Returns the literal token sequence.
fn split_pkg_config_output(input: &str) -> Result<Vec<String>, &'static str> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = input.chars().peekable();
    let mut has_token = false;
    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
                has_token = true;
            } else {
                current.push(c);
                has_token = true;
            }
            continue;
        }
        if in_double {
            if c == '"' {
                in_double = false;
                has_token = true;
                continue;
            }
            if c == '\\' {
                if let Some(&next) = chars.peek()
                    && matches!(next, '"' | '\\' | '$' | '`')
                {
                    current.push(next);
                    chars.next();
                    has_token = true;
                    continue;
                }
                current.push(c);
                has_token = true;
                continue;
            }
            current.push(c);
            has_token = true;
            continue;
        }
        if c.is_whitespace() {
            if has_token {
                out.push(std::mem::take(&mut current));
                has_token = false;
            }
            continue;
        }
        match c {
            '\'' => {
                in_single = true;
                has_token = true;
            }
            '"' => {
                in_double = true;
                has_token = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    has_token = true;
                } else {
                    return Err("trailing backslash");
                }
            }
            _ => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if in_single || in_double {
        return Err("unterminated quoted string");
    }
    if has_token {
        out.push(current);
    }
    Ok(out)
}

struct PkgConfigOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_pkg_config<I>(
    tool: &PkgConfigTool,
    stage: &str,
    args: I,
) -> Result<PkgConfigOutput, PkgConfigError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut cmd = Command::new(&tool.executable);
    cmd.args(args);
    tool.apply_extra_env(&mut cmd);
    match cmd.output() {
        Ok(out) => Ok(PkgConfigOutput {
            status: out.status,
            stdout: out.stdout,
            stderr: out.stderr,
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err(PkgConfigError::ExecutableNotFound {
                executable: tool.executable.to_string_lossy().into_owned(),
            })
        }
        Err(err) => Err(PkgConfigError::InvocationFailed {
            executable: tool.executable.to_string_lossy().into_owned(),
            stage: stage.to_owned(),
            detail: err.to_string(),
        }),
    }
}

fn classify_exists_failure(
    name: &str,
    requirement: &str,
    had_constraint: bool,
    stderr: String,
    tool: &PkgConfigTool,
) -> Result<PkgConfigError, PkgConfigError> {
    // Disambiguate "package not found" from "version mismatch".
    // If we attached a constraint, re-run --exists without it;
    // if that succeeds, the failure is a version mismatch.
    if had_constraint {
        let bare = run_pkg_config(
            tool,
            "--exists",
            std::iter::once(OsString::from("--exists"))
                .chain(std::iter::once(OsString::from(name))),
        )?;
        if bare.status.success() {
            // Capture the installed version (best-effort) for
            // the diagnostic.
            let installed = match run_pkg_config(
                tool,
                "--modversion",
                std::iter::once(OsString::from("--modversion"))
                    .chain(std::iter::once(OsString::from(name))),
            ) {
                Ok(out) if out.status.success() => Some(trim_stdout(&out.stdout)),
                _ => None,
            };
            return Ok(PkgConfigError::VersionMismatch {
                name: name.to_owned(),
                requirement: requirement.to_owned(),
                installed,
            });
        }
    }
    Ok(PkgConfigError::PackageNotFound {
        name: name.to_owned(),
        stderr,
    })
}

fn trim_stderr(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end().to_owned()
}

fn trim_stdout(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    fn env_returning<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(*v))
        }
    }

    #[test]
    fn default_executable_is_pkg_config_when_env_unset() {
        let resolved = resolve_pkg_config_executable(|_| None);
        assert_eq!(resolved, OsString::from(DEFAULT_PKG_CONFIG_EXECUTABLE));
    }

    #[test]
    fn env_override_wins_when_non_empty() {
        let env = env_returning(&[(CABIN_PKG_CONFIG_ENV, "/opt/pkgconf/bin/pkg-config")]);
        let resolved = resolve_pkg_config_executable(env);
        assert_eq!(resolved, OsString::from("/opt/pkgconf/bin/pkg-config"));
    }

    #[test]
    fn empty_env_value_falls_back_to_default() {
        let env = env_returning(&[(CABIN_PKG_CONFIG_ENV, "")]);
        let resolved = resolve_pkg_config_executable(env);
        assert_eq!(resolved, OsString::from(DEFAULT_PKG_CONFIG_EXECUTABLE));
    }

    #[test]
    fn split_returns_empty_for_empty_input() {
        let tokens = split_pkg_config_output("").unwrap();
        assert!(tokens.is_empty());
    }

    #[test]
    fn split_handles_simple_whitespace_separated_tokens() {
        let tokens = split_pkg_config_output("-I/usr/include  -D_FOO=bar -lssl").unwrap();
        assert_eq!(tokens, vec!["-I/usr/include", "-D_FOO=bar", "-lssl"]);
    }

    #[test]
    fn split_collapses_repeated_whitespace_and_trims() {
        let tokens = split_pkg_config_output("\n\t -L/opt\t-lcrypto\n").unwrap();
        assert_eq!(tokens, vec!["-L/opt", "-lcrypto"]);
    }

    #[test]
    fn split_unwraps_single_quoted_tokens() {
        let tokens = split_pkg_config_output("'-I/path with space' -lfoo").unwrap();
        assert_eq!(tokens, vec!["-I/path with space", "-lfoo"]);
    }

    #[test]
    fn split_unwraps_double_quoted_tokens_with_escapes() {
        let tokens = split_pkg_config_output("\"-Dgreet=\\\"hi\\\"\" -lbar").unwrap();
        assert_eq!(tokens, vec!["-Dgreet=\"hi\"", "-lbar"]);
    }

    #[test]
    fn split_supports_backslash_escapes_outside_quotes() {
        let tokens = split_pkg_config_output("-I/path\\ with\\ space -lfoo").unwrap();
        assert_eq!(tokens, vec!["-I/path with space", "-lfoo"]);
    }

    #[test]
    fn split_rejects_unterminated_quotes() {
        let err = split_pkg_config_output("'unterminated").unwrap_err();
        assert_eq!(err, "unterminated quoted string");
    }

    #[test]
    fn convert_caret_to_pkg_config_pair() {
        let pairs = convert_requirement("^1.2").unwrap();
        assert_eq!(
            pairs,
            vec![
                (">=".to_owned(), "1.2".to_owned()),
                ("<".to_owned(), "2.0.0".to_owned()),
            ]
        );
    }

    #[test]
    fn convert_zero_caret_uses_minor_bump() {
        let pairs = convert_requirement("^0.2").unwrap();
        assert_eq!(
            pairs,
            vec![
                (">=".to_owned(), "0.2".to_owned()),
                ("<".to_owned(), "0.3.0".to_owned()),
            ]
        );
    }

    #[test]
    fn convert_tilde_uses_minor_bump() {
        let pairs = convert_requirement("~1.2.3").unwrap();
        assert_eq!(
            pairs,
            vec![
                (">=".to_owned(), "1.2.3".to_owned()),
                ("<".to_owned(), "1.3.0".to_owned()),
            ]
        );
    }

    #[test]
    fn convert_simple_inequality_passes_through() {
        let pairs = convert_requirement(">=1.2").unwrap();
        assert_eq!(pairs, vec![(">=".to_owned(), "1.2".to_owned())]);
    }

    #[test]
    fn convert_exact_emits_pkg_config_equals() {
        let pairs = convert_requirement("=1.0.0").unwrap();
        assert_eq!(pairs, vec![("=".to_owned(), "1.0.0".to_owned())]);
    }

    #[test]
    fn convert_space_separated_comparators() {
        let pairs = convert_requirement(">=1.2 <2").unwrap();
        assert_eq!(
            pairs,
            vec![
                (">=".to_owned(), "1.2".to_owned()),
                ("<".to_owned(), "2".to_owned()),
            ]
        );
    }

    #[test]
    fn convert_rejects_non_semver_input() {
        assert!(convert_requirement("vendor-special-1.0").is_none());
    }

    #[test]
    fn convert_version_upper_bounds_handle_u64_ceiling() {
        // A component at the u64 ceiling carries into the next-higher
        // component instead of overflowing (debug panic / release
        // wrap) or being dropped into an over-broad range. The `<`
        // bound is omitted only when the major itself is saturated.
        let max = u64::MAX;

        // Minor at the ceiling ⇒ next major: `~1.MAX` / `1.MAX.*` mean
        // `< 2.0.0`, still excluding 2.x as the SemVer range requires.
        let tilde_minor = convert_requirement(&format!("~1.{max}")).unwrap();
        assert!(
            tilde_minor.iter().any(|(op, v)| op == "<" && v == "2.0.0"),
            "{tilde_minor:?}"
        );
        let wildcard_minor = convert_requirement(&format!("1.{max}.*")).unwrap();
        assert!(
            wildcard_minor
                .iter()
                .any(|(op, v)| op == "<" && v == "2.0.0"),
            "{wildcard_minor:?}"
        );

        // Major at the ceiling ⇒ no representable upper bound, so the
        // `<` constraint is omitted (only `>=` remains).
        let wildcard_major = convert_requirement(&format!("{max}.*")).unwrap();
        assert!(
            wildcard_major.iter().all(|(op, _)| op != "<"),
            "{wildcard_major:?}"
        );
        assert!(wildcard_major.iter().any(|(op, _)| op == ">="));
        let tilde_major = convert_requirement(&format!("~{max}")).unwrap();
        assert!(
            tilde_major.iter().all(|(op, _)| op != "<"),
            "{tilde_major:?}"
        );

        // A representable tilde still emits its bounded `<` upper.
        let normal = convert_requirement("~1.2").unwrap();
        assert!(normal.iter().any(|(op, v)| op == "<" && v == "1.3.0"));
    }

    #[test]
    fn build_constraints_emits_bare_name_when_requirement_empty() {
        let tool = PkgConfigTool::new(OsString::from("pkg-config"));
        let argv = build_constraints("zlib", "", tool.executable()).unwrap();
        assert!(!argv.had_constraint);
        assert_eq!(argv.argv, vec![OsString::from("zlib")]);
    }

    #[test]
    fn build_constraints_treats_wildcard_as_unconstrained() {
        // `*` parses as a SemVer requirement with no comparators.
        // pkg-config has no equivalent primitive, so the wildcard
        // means "any installed version" — emit the bare module
        // name and flag the call as carrying no constraint.
        let tool = PkgConfigTool::new(OsString::from("pkg-config"));
        let argv = build_constraints("zlib", "*", tool.executable()).unwrap();
        assert!(!argv.had_constraint);
        assert_eq!(argv.argv, vec![OsString::from("zlib")]);
    }

    #[test]
    fn build_constraints_emits_name_op_version_for_caret() {
        let tool = PkgConfigTool::new(OsString::from("pkg-config"));
        let argv = build_constraints("zlib", "^1.2", tool.executable()).unwrap();
        assert!(argv.had_constraint);
        assert_eq!(
            argv.argv,
            vec![
                OsString::from("zlib"),
                OsString::from("zlib"),
                OsString::from(">="),
                OsString::from("1.2"),
                OsString::from("zlib"),
                OsString::from("<"),
                OsString::from("2.0.0"),
            ]
        );
    }

    #[test]
    fn build_constraints_forwards_raw_for_pkg_config_native_syntax() {
        // `>= 1.0` parses as SemVer (via parse_lenient), so it
        // uses the converted path. Use a custom-format vendor
        // string with an explicit operator instead.
        let tool = PkgConfigTool::new(OsString::from("pkg-config"));
        let argv = build_constraints("openssl", ">= 1.0.1f", tool.executable()).unwrap();
        assert!(argv.had_constraint);
        assert!(
            argv.argv.windows(2).any(|w| w[0] == "openssl"),
            "should reference the module name",
        );
    }

    #[test]
    fn build_constraints_rejects_single_unrecognized_token() {
        let tool = PkgConfigTool::new(OsString::from("pkg-config"));
        let err = build_constraints("openssl", "weird-token", tool.executable()).unwrap_err();
        match err {
            PkgConfigError::InvalidVersionRequirement { name, requirement } => {
                assert_eq!(name, "openssl");
                assert_eq!(requirement, "weird-token");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn flags_default_is_empty() {
        let f = SystemDependencyFlags::default();
        assert!(f.is_empty());
        assert!(f.include_dirs.is_empty());
        assert!(f.extra_compile_args.is_empty());
        assert!(f.ldflags.is_empty());
    }

    #[test]
    fn flags_is_not_empty_when_any_field_populated() {
        let mut f = SystemDependencyFlags::default();
        f.include_dirs.push(Utf8PathBuf::from("/usr/include"));
        assert!(!f.is_empty());
        let mut f = SystemDependencyFlags::default();
        f.extra_compile_args.push("-pthread".into());
        assert!(!f.is_empty());
        let mut f = SystemDependencyFlags::default();
        f.ldflags.push("-lssl".into());
        assert!(!f.is_empty());
    }
}
