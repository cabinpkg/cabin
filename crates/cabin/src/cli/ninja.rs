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
//! [`crate::cli`]), `cabin run` (in [`crate::cli::run`]), and
//! `cabin test` (in [`crate::cli::test`]) so each command renders
//! Ninja output the same way.

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Context as _;

use crate::cli::term_verbosity::Reporter;

/// Run Ninja and filter its housekeeping lines (`ninja: Entering
/// directory …`, `ninja: no work to do.`, `[N/M] …` progress)
/// from stdout so the default surface stays terse.  Compiler
/// warnings, errors, and any non-housekeeping line from Ninja or
/// the toolchain pass through unchanged on stdout (Ninja funnels
/// each action's own stdout/stderr onto its stdout stream).
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
/// `INCLUDE` / `LIB` / `PATH` onto the Ninja child. `cxx_kind` is the
/// detected compiler family.
///
/// - `clang-cl` ships no CRT/SDK headers of its own, so it *always*
///   borrows an installed MSVC toolset's `INCLUDE` / `LIB` / `PATH`,
///   however it was spelled — apply the discovered overlay unconditionally
///   for it. This is the same environment a bare-name `clang-cl` already
///   receives via the arm below; without it, an explicitly-*pinned*
///   `clang-cl` path silently falls through to the `Path` arm (a
///   `clang-cl.exe` can never equal the discovered `cl.exe`) and loses the
///   overlay, so the same compiler builds or fails based only on whether
///   it was named or pathed. Applying it also means `clang-cl` uses
///   Cabin's discovered toolset rather than its own auto-probe — the
///   intended unification with the other two cases, and safe because the
///   overlay only fires when no MSVC environment was supplied at all.
/// - A bare-name / auto-discovered compiler *is* the discovered install,
///   so its overlay is the right environment — apply it.
/// - An explicitly pinned `cl` path takes the overlay only when it is the
///   discovered install. Unlike `clang-cl`, `cl.exe` *is* a complete
///   toolset, so a separately discovered install could be a different
///   Visual Studio toolset whose headers/libs must not be mixed into the
///   user's chosen compiler. But when the pinned path *is* the discovered
///   install — the common case on Windows outside a Developer Command
///   Prompt — the compile still needs that install's
///   `INCLUDE` / `LIB` / `PATH`, so apply it there too.
///
/// The overlay only ever has an effect when an install was discovered
/// (Windows, outside an activated environment — see
/// [`cabin_toolchain::msvc_environment`]); inside a Developer Command
/// Prompt it is a no-op, so this never overrides a deliberately activated
/// toolset. (`VSLANG` is applied regardless.)
pub(crate) fn discovered_msvc_install_applies(
    toolchain: &cabin_core::ResolvedToolchain,
    cxx_kind: cabin_core::CompilerKind,
) -> bool {
    if cxx_kind == cabin_core::CompilerKind::ClangCl {
        return true;
    }
    match &toolchain.cxx.spec {
        cabin_core::ToolSpec::Name(_) => true,
        cabin_core::ToolSpec::Path(_) => {
            cabin_toolchain::path_is_discovered_msvc_cl(toolchain.cxx.path.as_std_path())
        }
    }
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

fn ninja_verbose_echo(verbose: bool) -> &'static str {
    if verbose { "-v " } else { "" }
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

/// Inputs for [`invoke_ninja_and_report`]: everything needed to write
/// the Ninja files for a planned graph and drive Ninja to completion.
/// Shared by `cabin build` / `cabin run` / `cabin test`, each of which
/// does its own command-specific work once the build phase is done.
pub(crate) struct NinjaInvocationRequest<'a> {
    /// Resolved, absolute build directory — the parent of the
    /// per-profile build root.
    pub build_dir: &'a std::path::Path,
    /// Active build profile; its name selects the `build/<profile>`
    /// root and labels the `Finished` banner the caller prints.
    pub profile: &'a cabin_core::ResolvedProfile,
    /// Planned build graph to lower into `build.ninja` and
    /// `compile_commands.json`.
    pub plan_graph: &'a cabin_build::BuildGraph,
    /// Loaded workspace graph, used to attribute Ninja progress lines
    /// to packages and to render a link-failure hint.
    pub graph: &'a cabin_workspace::PackageGraph,
    /// Resolved toolchain, used to decide whether the discovered MSVC
    /// environment overlay applies.
    pub toolchain: &'a cabin_core::ResolvedToolchain,
    /// Detected C++ compiler family
    /// (`detection_report.cxx.identity.kind`).
    pub cxx_kind: cabin_core::CompilerKind,
    /// Resolved feature graph, consumed by the link-failure hint.
    pub feature_resolution: &'a cabin_feature::FeatureResolution,
    /// Packages whose `[dev-dependencies]` are active for this
    /// invocation: empty for `build` / `run`, the selected runners for
    /// `test`.
    pub dev_for: &'a BTreeSet<String>,
    /// Located `ninja` executable.
    pub ninja: &'a std::path::Path,
    /// Parallelism for Ninja's `-j` flag, or `None` to let Ninja pick.
    pub jobs: Option<cabin_core::BuildJobs>,
    pub reporter: Reporter,
}

/// Write `build.ninja` + `compile_commands.json` for the planned graph
/// under `build/<profile>/`, invoke Ninja there, and surface a
/// link-failure hint before bailing on a non-zero exit. Returns the
/// wall-clock time the Ninja invocation took so callers can print
/// their own cargo-style `Finished` banner.
///
/// # Errors
/// Returns an error if the build root cannot be created, the Ninja
/// files cannot be written, Ninja cannot be spawned, or Ninja exits
/// non-zero.
pub(crate) fn invoke_ninja_and_report(
    req: &NinjaInvocationRequest<'_>,
) -> anyhow::Result<std::time::Duration> {
    let profile_build_root = req.build_dir.join(req.profile.name.as_str());
    std::fs::create_dir_all(&profile_build_root).with_context(|| {
        format!(
            "failed to create build directory {}",
            profile_build_root.display()
        )
    })?;

    let ninja_file = profile_build_root.join("build.ninja");
    cabin_ninja::write_build_ninja(&ninja_file, req.plan_graph, &check_stamp_runner())?;
    let ccmd_file = profile_build_root.join("compile_commands.json");
    cabin_ninja::write_compile_commands(&ccmd_file, req.plan_graph)?;

    // Implementation-detail status is verbose-only: under `-v` the
    // user sees which files Cabin wrote and how Ninja was invoked,
    // alongside Ninja's own raw banner.
    req.reporter
        .verbose(format_args!("cabin: wrote {}", ninja_file.display()));
    req.reporter
        .verbose(format_args!("cabin: wrote {}", ccmd_file.display()));
    let ninja_verbose = req.reporter.verbosity().shows_verbose();
    req.reporter.verbose(format_args!(
        "cabin: invoking {} {}{}-C {}",
        req.ninja.display(),
        ninja_jobs_echo(req.jobs),
        ninja_verbose_echo(ninja_verbose),
        profile_build_root.display()
    ));

    let mut ninja_cmd = std::process::Command::new(req.ninja);
    if let Some(jobs) = req.jobs {
        ninja_cmd.arg(ninja_jobs_arg(jobs));
    }
    if ninja_verbose {
        ninja_cmd.arg("-v");
    }
    let build_started = std::time::Instant::now();
    let run = run_ninja(
        ninja_cmd.arg("-C").arg(&profile_build_root),
        req.reporter,
        req.graph,
        req.plan_graph.dialect,
        discovered_msvc_install_applies(req.toolchain, req.cxx_kind),
    )
    .with_context(|| format!("failed to invoke ninja at {}", req.ninja.display()))?;
    if !run.status.success() {
        emit_link_diagnostic_if_applicable(
            &run,
            req.graph,
            req.feature_resolution,
            req.dev_for,
            req.reporter,
        );
        anyhow::bail!("ninja exited with {}", run.status);
    }
    Ok(build_started.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{
        CompilerKind, ResolvedTool, ResolvedToolchain, ToolKind, ToolSource, ToolSpec,
    };
    use camino::Utf8PathBuf;

    fn toolchain_with_pinned_cxx(path: &str) -> ResolvedToolchain {
        let pinned = |kind, p: &str| ResolvedTool {
            kind,
            path: Utf8PathBuf::from(p),
            spec: ToolSpec::Path(Utf8PathBuf::from(p)),
            source: ToolSource::Cli,
        };
        ResolvedToolchain {
            cxx: pinned(ToolKind::CxxCompiler, path),
            ar: pinned(ToolKind::Archiver, "/llvm/bin/llvm-lib.exe"),
            cc: None,
        }
    }

    #[test]
    fn explicit_clang_cl_path_takes_the_discovered_overlay() {
        // An explicitly-pinned `clang-cl` path is MSVC-dialect but is never
        // the discovered `cl.exe`, yet it still needs the discovered
        // INCLUDE/LIB/PATH because `clang-cl` ships no CRT/SDK headers. The
        // `ClangCl` early return short-circuits before
        // `path_is_discovered_msvc_cl`, so this holds with no Windows host
        // or real install — locking the regression at the only seam that
        // distinguishes `clang-cl` from a foreign-toolset `cl.exe`.
        let toolchain = toolchain_with_pinned_cxx("/llvm/bin/clang-cl.exe");
        assert!(discovered_msvc_install_applies(
            &toolchain,
            CompilerKind::ClangCl
        ));
    }

    #[test]
    fn explicit_non_clang_cl_path_still_defers_to_install_match() {
        // A pinned `cl.exe` from a *different* toolset detects as
        // `CompilerKind::Msvc`, not `ClangCl`, so it skips the early return
        // and falls through to the path comparison, which is false off a
        // matching Windows install — the eo1 "don't mix SDKs" safety stays
        // intact.
        let toolchain = toolchain_with_pinned_cxx("/some/other/vs/cl.exe");
        assert!(!discovered_msvc_install_applies(
            &toolchain,
            CompilerKind::Msvc
        ));
    }
}
