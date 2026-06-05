//! Orchestration for invoking Ninja and post-processing its output.
//!
//! The build planner (`cabin-build`) writes a Ninja file; this
//! module spawns Ninja against it, tees the child's stdout/stderr
//! through the user's terminal while capturing every byte for
//! post-failure diagnostics, and emits the cargo-style
//! `Compiling <pkg> v<ver>` headers off Ninja's `[N/M] …` progress
//! lines.  When Ninja exits non-zero,
//! [`emit_link_diagnostic_if_applicable`] inspects the captured
//! streams and prints a one-shot link-failure hint when the
//! recognizable shape is present.
//!
//! Shared by `cabin build` / `cabin clean` (in
//! [`crate::cli`]), `cabin run` (in [`crate::run_glue`]), and
//! `cabin test` (in [`crate::test_glue`]) so each command renders
//! Ninja output the same way.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::term_verbosity_glue::Reporter;

/// Run Ninja and filter its housekeeping lines (`ninja: Entering
/// directory …`, `ninja: no work to do.`, `[N/M] …` progress)
/// from stdout so the default surface stays terse.  Compiler
/// warnings, errors, and any non-housekeeping line from Ninja or
/// the toolchain pass through unchanged on stderr.
///
/// Verbose mode (`-v`) restores the full Ninja output so users
/// who want to inspect the backend's progress have a knob.
/// Outcome of one [`run_ninja`] invocation. Both `stdout` and
/// `stderr` are captured-and-teed: every line the child wrote
/// was streamed to the user's terminal in real time AND
/// accumulated here, so post-failure diagnostics (e.g.
/// [`cabin_build::link_diagnostics::diagnose`]) have something
/// to parse.
///
/// Ninja sends *all* of a failed action's output — the
/// `FAILED:` banner, the recreated command line, and the
/// compiler/linker diagnostics — to stdout, not stderr.
/// Diagnostics that care about link failures therefore need
/// the captured stdout, not just stderr.
pub(crate) struct NinjaRun {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Path to the running `cabin` executable, used as the `cabin stamp`
/// runner the syntax-check Ninja rule invokes to run the compiler and
/// stamp its output without a shell (see [`crate::stamp`]). Falls back to
/// the bare name `cabin` (resolved via `PATH`) only if the current-exe
/// lookup fails, which should not happen in practice.
pub(crate) fn check_stamp_runner() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("cabin"))
}

/// Whether to overlay the *auto-discovered* MSVC install's
/// `INCLUDE` / `LIB` / `PATH` onto the Ninja child. Skipped when the
/// user pinned an explicit `cl` path: a separately discovered install
/// could be a different Visual Studio toolset, so its headers/libs must
/// not be forced onto the user's chosen compiler. (`VSLANG` is applied
/// regardless — see [`cabin_toolchain::msvc_environment`].)
pub(crate) fn discovered_msvc_install_applies(toolchain: &cabin_core::ResolvedToolchain) -> bool {
    !matches!(toolchain.cxx.spec, cabin_core::ToolSpec::Path(_))
}

pub(crate) fn run_ninja(
    cmd: &mut std::process::Command,
    reporter: Reporter,
    graph: &cabin_workspace::PackageGraph,
    dialect: cabin_build::Dialect,
    apply_discovered_msvc_install: bool,
) -> std::io::Result<NinjaRun> {
    use std::io::{BufRead, BufReader, Write as _};
    use std::process::Stdio;

    // Only an MSVC build graph gets the MSVC environment overlay
    // (`VSLANG`, and the auto-discovered `INCLUDE` / `LIB` / `PATH`).
    // A GNU-style toolchain on Windows must run in the environment it
    // was resolved under: overlaying MSVC headers/libs and PATH could
    // silently switch `clang++` / `g++` to MSVC behavior while Cabin is
    // still emitting `.a` archives and GNU link lines. Empty (a no-op)
    // off Windows. See `cabin_toolchain::msvc_environment`.
    if dialect == cabin_build::Dialect::Msvc {
        for (key, value) in cabin_toolchain::msvc_environment(apply_discovered_msvc_install) {
            cmd.env(key, value);
        }
    }

    // Verbose modes (`-v` / `-vv`) keep every line Ninja
    // emits — the `[N/M] …` progress prefix, the `Entering
    // directory` banner, the `no work to do.` reassurance — so
    // raising verbosity never makes the surface smaller.  The
    // `Compiling` banner is still printed in those modes from
    // the same per-package detection used at the default
    // verbosity; users opting into `-v` see the cargo-style
    // headers interleaved with the raw Ninja output.
    let keep_ninja_chatter = reporter.verbosity().shows_verbose();

    // Lookup table from package name to the workspace `WorkspacePackage`
    // entry.  We resolve each `[N/M] …` progress line back to its
    // owning package by the `/packages/<name>/` segment the
    // planner embeds in every output path, then announce the
    // `Compiling` banner the first time a given package shows
    // up.  Tying announcements to Ninja's own progress keeps the
    // banner temporally accurate — header-only libraries that
    // contribute no actions never get a `Compiling` line because
    // they never appear in Ninja's output.
    let pkg_by_name: HashMap<&str, &cabin_workspace::WorkspacePackage> = graph
        .packages
        .iter()
        .map(|pkg| (pkg.package.name.as_str(), pkg))
        .collect();
    let mut announced: HashSet<String> = HashSet::new();

    let mut child = cmd
        .stdout(Stdio::piped())
        // `stderr` is piped so cabin can tee it: every line
        // streams to the user's terminal in real time AND is
        // accumulated in a buffer so post-failure diagnostics
        // can parse it. The cost (one extra read per stderr
        // line) is invisible at typical build scale; the win
        // is that the linker output the diagnostic layer needs
        // is sitting in memory the moment ninja exits.
        .stderr(Stdio::piped())
        .spawn()?;

    let stderr_thread = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            let mut captured = String::new();
            let real_stderr = std::io::stderr();
            let mut sink = real_stderr.lock();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = writeln!(sink, "{line}");
                captured.push_str(&line);
                captured.push('\n');
            }
            captured
        })
    });

    let mut captured_stdout = String::new();
    if let Some(stdout) = child.stdout.take() {
        let stdout_handle = std::io::stdout();
        let mut sink = stdout_handle.lock();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            // Every line is captured regardless of the
            // verbose/progress filtering below, so
            // post-failure diagnostics see what ninja
            // *actually* emitted — including the `FAILED:`
            // banner and the linker's "undefined symbol"
            // block that ninja sends to stdout.
            captured_stdout.push_str(&line);
            captured_stdout.push('\n');
            if let Some(path) = ninja_progress_path(&line) {
                if let Some(pkg_name) = package_segment_from_path(path)
                    && announced.insert(pkg_name.to_owned())
                    && let Some(pkg) = pkg_by_name.get(pkg_name)
                {
                    announce_compiling(reporter, pkg);
                }
                if keep_ninja_chatter {
                    let _ = writeln!(sink, "{line}");
                }
                continue;
            }
            if is_ninja_chatter(&line) {
                if keep_ninja_chatter {
                    let _ = writeln!(sink, "{line}");
                }
                continue;
            }
            let _ = writeln!(sink, "{line}");
        }
    }
    let status = child.wait()?;
    let stderr = stderr_thread
        .and_then(|t| t.join().ok())
        .unwrap_or_default();
    Ok(NinjaRun {
        status,
        stdout: captured_stdout,
        stderr,
    })
}

/// Inspect Ninja's captured stderr after a non-zero exit and, if
/// it looks like a recognizable link failure, print a one-shot
/// `hint:` block to stderr pointing the user at the missing
/// `deps =` entry (or the un-declared bundled port).
///
/// Quiet on inputs that don't look like link failures — the
/// diagnostic is purely additive, never replaces the underlying
/// Ninja error. Failures inside the diagnostic itself (e.g. a
/// package name in the link error that isn't in the loaded
/// graph) silently do nothing rather than spew on top of the
/// real error.
pub(crate) fn emit_link_diagnostic_if_applicable(
    run: &NinjaRun,
    graph: &cabin_workspace::PackageGraph,
    feature_resolution: &cabin_feature::FeatureResolution,
    include_dev_for: &BTreeSet<String>,
    reporter: Reporter,
) {
    use cabin_build::link_diagnostics::{TargetDepInfo, diagnose, render};

    // Ninja sends the `FAILED:` banner and the failing action's
    // stdout/stderr to *its* stdout, then any wrapper diagnostics
    // (e.g. `ninja: build stopped`) also to stdout. Concatenate
    // both captured streams so the parser sees whichever stream
    // the platform's linker actually used.
    let combined = if run.stderr.is_empty() {
        run.stdout.clone()
    } else if run.stdout.is_empty() {
        run.stderr.clone()
    } else {
        format!("{}\n{}", run.stdout, run.stderr)
    };

    let host_platform = cabin_core::TargetPlatform::current();
    let lookup = |pkg_name: &str, target_name: &str| -> Option<TargetDepInfo> {
        let pkg_idx = graph.index_of(pkg_name)?;
        let wp = &graph.packages[pkg_idx];
        let target = wp
            .package
            .targets
            .iter()
            .find(|t| t.name.as_str() == target_name)?;
        // Mirror the workspace loader's active-edge filter so
        // the hint only points at deps that would actually
        // appear on the link command for this invocation:
        //
        // * Skip cfg-gated entries that do not match the host
        //   platform — they never become graph edges.
        // * Skip `[dev-dependencies]` unless the owning package
        //   activated them for this invocation
        //   (`cabin test` populates `include_dev_for` with the
        //   selected test runners; ordinary builds leave it
        //   empty, matching the loader's policy).
        // * Skip `optional = true` entries whose features are
        //   not enabled by the current resolution — suggesting
        //   "add this to target.deps" for a disabled optional
        //   dep would not change the link command.
        let dev_active = include_dev_for.contains(pkg_name);
        let features = feature_resolution.for_package(pkg_idx);
        let package_deps: BTreeSet<String> = wp
            .package
            .dependencies
            .iter()
            .filter(|d| {
                if !d.matches_platform(&host_platform) {
                    return false;
                }
                let kind_active = d.kind.is_resolved_by_default()
                    || (dev_active && d.kind == cabin_core::DependencyKind::Dev);
                if !kind_active {
                    return false;
                }
                if d.optional && !features.enabled_optional_deps.contains(d.name.as_str()) {
                    return false;
                }
                true
            })
            .map(|d| d.name.as_str().to_owned())
            .collect();
        // `target.deps` entries are either a bare name (same-package
        // target or default library of another package) or a
        // qualified `package:target` reference. We only care about
        // whether the *package* appears, so the suffix gets stripped.
        let target_deps: BTreeSet<String> = target
            .deps
            .iter()
            .map(|d| {
                d.split_once(':')
                    .map_or(d.as_str(), |(pkg, _)| pkg)
                    .to_owned()
            })
            .collect();
        Some(TargetDepInfo {
            package_deps,
            target_deps,
        })
    };

    if let Some(diag) = diagnose(&combined, lookup) {
        reporter.help(&render(&diag));
    }
}

/// Emit the cargo-style `Compiling <name> v<ver> (<dir>)`
/// header for a single package.  Local packages render their
/// manifest directory in parentheses; registry packages drop
/// the path because the workspace user did not bring them in
/// by hand.
fn announce_compiling(reporter: Reporter, pkg: &cabin_workspace::WorkspacePackage) {
    let name = pkg.package.name.as_str();
    let version = &pkg.package.version;
    match pkg.kind {
        cabin_workspace::PackageKind::Local => {
            reporter.status(
                "Compiling",
                format_args!("{} v{} ({})", name, version, pkg.manifest_dir.display()),
            );
        }
        cabin_workspace::PackageKind::Registry => {
            reporter.status("Compiling", format_args!("{name} v{version}"));
        }
    }
}

/// Return true for lines that are pure Ninja housekeeping:
/// `Entering directory` and `no work to do.`.  Any other line
/// (including compiler-emitted diagnostics, blank lines, and
/// Ninja error reports) returns `false` so it passes through
/// unchanged.  Progress lines (`[N/M] …`) are handled separately
/// — `run_ninja` extracts the package name from them before
/// dropping the line.
fn is_ninja_chatter(line: &str) -> bool {
    line == "ninja: no work to do." || line.starts_with("ninja: Entering directory")
}

/// Parse a Ninja `[<finished>/<total>] <action> <path>` progress
/// line.  Returns the trailing `<path>` slice on a successful
/// match, or `None` for any other input.  Both progress numbers
/// must be non-empty decimal integers; both are decimal positive
/// integers in practice.
fn ninja_progress_path(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let (finished, rest) = rest.split_once('/')?;
    if finished.is_empty() || !finished.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (total, after) = rest.split_once("] ")?;
    if total.is_empty() || !total.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // `after` is `<action> <path>`; the action token is the
    // single word the planner records as the description prefix
    // (`CXX`, `AR`, `LINK`, …), so the path starts immediately
    // after the first space.
    let (_action, path) = after.split_once(' ')?;
    Some(path)
}

/// Extract the package name from a planner-emitted build path.
/// Every per-package artifact lives under `<build_dir>/<profile>
/// /packages/<name>/…`, so locating the first `/packages/`
/// segment and taking the next path component yields the
/// owning package's name.  Returns `None` when the path lacks
/// the segment (a custom-command output the planner did not
/// route through the per-package tree).
fn package_segment_from_path(path: &str) -> Option<&str> {
    const SEGMENT: &str = "/packages/";
    let after = path.find(SEGMENT)?;
    let tail = &path[after + SEGMENT.len()..];
    tail.split('/').next().filter(|s| !s.is_empty())
}

/// Render the optional `-jN` token plus a trailing space for
/// the status line.  Empty when jobs is unset so the message
/// `cabin: invoking ninja -C <dir>` stays byte-identical to
/// the pre-jobs default.
pub(crate) fn ninja_jobs_echo(jobs: Option<cabin_core::BuildJobs>) -> String {
    match jobs {
        Some(j) => format!("-j{j} "),
        None => String::new(),
    }
}

/// Ninja argv fragment: a single `-jN` token.  Producing a single
/// fused argument (rather than `-j` + `N`) matches every Ninja
/// `--help` example and supports Ninja versions that historically
/// parsed only the fused form.  Backend-specific conversion lives
/// here, at the call site that spawns Ninja, rather than in
/// `cabin-core`'s [`cabin_core::BuildJobs`] model.
pub(crate) fn ninja_jobs_arg(jobs: cabin_core::BuildJobs) -> std::ffi::OsString {
    std::ffi::OsString::from(format!("-j{}", jobs.get()))
}
