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
//! Shared by `cabin build` / `cabin check` (in `crate::cli::build`),
//! `cabin run` (in [`crate::cli::run`]), and `cabin test` (in
//! [`crate::cli::test`]) so each command renders Ninja output the same
//! way.

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Context as _;

use crate::cli::term_verbosity::Reporter;

/// Outcome of one [`run_ninja`] invocation.  `stdout` and `stderr`
/// hold every line the child wrote, including the lines the verbosity
/// filter kept off the terminal, so post-failure diagnostics (e.g.
/// [`cabin_build::link_diagnostics::diagnose`]) have something
/// to parse.
///
/// Ninja sends *all* of a failed action's output - the
/// `FAILED:` banner, the recreated command line, and the
/// compiler/linker diagnostics - to stdout, not stderr.
/// Link diagnostics therefore parse stdout, not only stderr.
pub(crate) struct NinjaRun {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Path to the running `cabin` executable, which the generated Ninja
/// rules invoke as `cabin stamp` (syntax check), `cabin recreate`
/// (GNU-dialect archive), and `cabin depfile` (a compile whose depfile
/// Ninja would misread); see [`crate::stamp`].  Falls back to
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
///   however it was spelled - apply the discovered overlay unconditionally
///   for it.  Otherwise a pinned `clang-cl` path would reach the `Path`
///   arm, which never matches it (a `clang-cl.exe` is never the
///   discovered `cl.exe`), and the same compiler would build or fail
///   depending on whether it was named or pathed.  `clang-cl` therefore
///   uses Cabin's discovered toolset rather than its own auto-probe.
/// - A bare-name / auto-discovered compiler *is* the discovered install,
///   so its overlay is the right environment - apply it.
/// - An explicitly pinned `cl` path takes the overlay only when it is the
///   discovered install.  Unlike `clang-cl`, `cl.exe` *is* a complete
///   toolset, so a separately discovered install could be a different
///   Visual Studio toolset whose headers/libs must not be mixed into the
///   user's chosen compiler.  But when the pinned path *is* the discovered
///   install - the common case on Windows outside a Developer Command
///   Prompt - the compile still needs that install's
///   `INCLUDE` / `LIB` / `PATH`, so apply it there too.
///
/// The overlay only ever has an effect when an install was discovered
/// (Windows, outside an activated environment - see
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

/// Run Ninja and filter its housekeeping lines (`ninja: Entering
/// directory …`, `ninja: no work to do.`, `[N/M] …` progress)
/// from stdout so the default surface stays terse.  Compiler
/// warnings, errors, and any non-housekeeping line from Ninja or
/// the toolchain pass through unchanged on stdout (Ninja funnels
/// each action's own stdout/stderr onto its stdout stream).
///
/// Verbose mode (`-v` / `-vv`) keeps every line; the `Compiling`
/// banners still print, interleaved with Ninja's output.
#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn run_ninja(
    cmd: &mut std::process::Command,
    profile_build_root: &std::path::Path,
    reporter: Reporter,
    graph: &cabin_workspace::PackageGraph,
    plan_graph: &cabin_build::BuildGraph,
) -> std::io::Result<NinjaRun> {
    use std::io::{BufRead, BufReader, Write as _};
    use std::process::Stdio;

    let keep_ninja_chatter = reporter.verbosity().shows_verbose();

    // Each `[N/M] …` progress line is attributed to its package by
    // the `packages/` segment of its output path (see
    // `package_segment_candidates`), and the `Compiling` banner
    // prints the first time a package shows up.  A package with no
    // actions (e.g. header-only) therefore never gets a banner.
    // Restricted to the packages this plan actually builds: an
    // unplanned workspace member whose bare name equals a planned
    // package's scope must never win the path-candidate match.
    let pkg_by_name: HashMap<&str, &cabin_workspace::WorkspacePackage> = graph
        .packages
        .iter()
        .filter(|pkg| {
            plan_graph
                .planned_packages
                .contains(pkg.package.name.as_str())
        })
        .map(|pkg| (pkg.package.name.as_str(), pkg))
        .collect();
    let mut announced: HashSet<String> = HashSet::new();
    // Non-UTF-8 build roots fall back to the unanchored search;
    // the planner rejects non-UTF-8 paths long before Ninja runs.
    let profile_root_str = profile_build_root.to_str();

    let mut child = cmd
        .stdout(Stdio::piped())
        // Streamed to the terminal and captured (see `NinjaRun`).
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
            captured_stdout.push_str(&line);
            captured_stdout.push('\n');
            if let Some(path) = ninja_progress_path(&line) {
                // Bare candidate first, scoped second: only a
                // candidate naming a known package is announced, so
                // a scope directory never wins over a real package.
                if let Some((pkg_name, pkg)) = package_segment_candidates(path, profile_root_str)
                    .into_iter()
                    .flatten()
                    .find_map(|candidate| pkg_by_name.get(candidate).map(|pkg| (candidate, *pkg)))
                    && announced.insert(pkg_name.to_owned())
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

/// Inspect Ninja's captured output after a non-zero exit and, if
/// it looks like a recognizable link failure or missing header, print
/// a one-shot `help:` block to stderr pointing the user at the missing
/// `deps =` entry.
///
/// Quiet on inputs that don't look like link failures - the
/// diagnostic is purely additive, never replaces the underlying
/// Ninja error.  Failures inside the diagnostic itself (e.g. a
/// package name in the link error that isn't in the loaded
/// graph) silently do nothing rather than spew on top of the
/// real error.
pub(crate) fn emit_link_diagnostic_if_applicable(run: &NinjaRun, req: &NinjaInvocationRequest<'_>) {
    use cabin_build::link_diagnostics::{TargetDepInfo, diagnose, render};

    let graph = req.graph;
    let planned_packages = &req.plan_graph.planned_packages;
    let include_dev_for = req.dev_for;

    // The failing action's output is on stdout (see `NinjaRun`);
    // parse both captured streams.
    let combined = if run.stderr.is_empty() {
        run.stdout.clone()
    } else if run.stdout.is_empty() {
        run.stderr.clone()
    } else {
        format!("{}\n{}", run.stdout, run.stderr)
    };

    let lookup = |pkg_name: &str, target_name: &str| -> Option<TargetDepInfo> {
        // A candidate reading only counts when the package was part
        // of this plan: the failing path came out of this plan's
        // Ninja file, so an unplanned bare member named like a
        // scope must not absorb the failure.
        if !planned_packages.contains(pkg_name) {
            return None;
        }
        let pkg_idx = graph.index_of(pkg_name)?;
        let wp = &graph.packages[pkg_idx];
        let target = wp
            .package
            .targets
            .iter()
            .find(|t| t.name.as_str() == target_name)?;
        // Mirror the planner's active-edge filter so
        // the hint only points at deps that would
        // appear on the link command for this invocation:
        //
        // * Skip cfg-gated entries that do not match the build's
        //   platform - they never reach its link command.
        // * Skip `[dev-dependencies]` unless the owning package
        //   activated them for this invocation
        //   (`cabin test` populates `include_dev_for` with the
        //   selected test runners; ordinary builds leave it
        //   empty, matching the loader's policy).
        // * Skip `optional = true` entries whose features are
        //   not enabled by the current resolution - suggesting
        //   "add this to target.deps" for a disabled optional
        //   dep would not change the link command.
        let dev_active = include_dev_for.contains(pkg_name);
        let features = req.feature_resolution.for_package(pkg_idx);
        let package_deps: BTreeSet<String> = wp
            .package
            .dependencies
            .iter()
            .filter(|d| {
                if !d.matches_platform(req.platform) {
                    return false;
                }
                let kind_active = d.kind == cabin_core::DependencyKind::Normal
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
        // target or a dependency package's sole library-like target)
        // or a qualified `package:target` reference.  We only care
        // about whether the *package* appears, so the suffix gets
        // stripped.
        let target_deps: BTreeSet<String> = target
            .deps
            .iter()
            .map(|d| {
                d.reference
                    .split_once(':')
                    .map_or(d.reference.as_str(), |(pkg, _)| pkg)
                    .to_owned()
            })
            .collect();
        Some(TargetDepInfo {
            package_deps,
            target_deps,
        })
    };

    if let Some(diag) = diagnose(&combined, req.plan_graph.dialect, lookup) {
        req.reporter.help(&render(&diag));
    }
}

/// After a failed build against a configured sysroot, probe whether a
/// minimal program builds against it with each link driver and the
/// flags the plan uses, so a sysroot that compiles but cannot link
/// reads as such rather than as the package's own error.  Quiet when
/// the probe builds or cannot run, like the link hint.
fn emit_sysroot_diagnostic(req: &NinjaInvocationRequest<'_>, profile_build_root: &std::path::Path) {
    use cabin_build::BuildAction;
    use cabin_toolchain::{Answer, Check, Execution, ProbeRequest, ProbeToolchain, Step};

    // Ninja runs every command from the profile root.
    let Some(dir) = camino::Utf8Path::from_path(profile_build_root) else {
        return;
    };
    let actions = &req.plan_graph.actions;
    let mut probed = BTreeSet::new();
    for (link, sysroot) in actions.iter().filter_map(|action| match action {
        BuildAction::Link(link) => link.sysroot.as_ref().map(|sysroot| (link, sysroot)),
        _ => None,
    }) {
        if !probed.insert(&link.linker) {
            continue;
        }
        let Some(compile) = actions.iter().find_map(|action| match action {
            // The target context's compile, not a build-context one.
            BuildAction::Compile(compile)
                if compile.compiler == link.linker && compile.sysroot.as_ref() == Some(sysroot) =>
            {
                Some(compile)
            }
            _ => None,
        }) else {
            continue;
        };
        let toolchain = ProbeToolchain {
            dialect: req.plan_graph.dialect,
            compiler: &link.linker,
            target: link.target.as_deref(),
            sysroot: Some(sysroot),
            env: &[],
            dir,
            execution: Execution::Unavailable,
        };
        let request = ProbeRequest {
            standard: compile.standard,
            gnu_extensions: compile.gnu_extensions,
            check: Check::Links("int main(void) { return 0; }\n".to_owned()),
            arguments: compile.arguments.clone(),
            link_flags: link.arguments.clone(),
            link_libs: Vec::new(),
        };
        let answer = cabin_toolchain::probe(&toolchain, &request, &cabin_toolchain::ProcessRunner);
        let language = compile.standard.language().human_label();
        let sysroot = &sysroot.path;
        match answer {
            Ok(Answer::No { step, output }) => {
                let outcome = if step == Step::Compile {
                    format!("does not compile against the configured sysroot {sysroot}")
                } else {
                    format!("compiles against the configured sysroot {sysroot} but does not link")
                };
                req.reporter.help(&format!(
                    "a minimal {language} program {outcome}:\n{}",
                    output.trim_end()
                ));
            }
            Ok(_) => {}
            Err(error) => tracing::debug!(%error, "the sysroot probe could not run"),
        }
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
/// `Entering directory` and `no work to do.`.  Progress lines
/// (`[N/M] …`) are recognized separately by [`ninja_progress_path`].
fn is_ninja_chatter(line: &str) -> bool {
    line == "ninja: no work to do." || line.starts_with("ninja: Entering directory")
}

/// Parse a Ninja `[<finished>/<total>] <action> <path>` progress
/// line.  Returns the trailing `<path>` slice on a successful
/// match, or `None` for any other input.  Both progress numbers
/// must be non-empty decimal integers.
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

/// Extract the owning package's name candidates from a
/// planner-emitted build path.  Every per-package artifact lives
/// under `<profile-dir>/packages/<name>/…` (target context) or
/// `<profile-dir>/build-machine/packages/<name>/…` (build context),
/// with a scoped package nested as `packages/<scope>/<name>/…` (see
/// [`cabin_build::layout::package_dir`]), so locating the first
/// `/packages/` segment after the profile build root yields the
/// candidates.  The parse alone cannot tell a scope
/// directory from a bare package directory, so both readings are
/// returned - bare (one component) first, scoped (two components
/// re-joined with `/`) second - and the caller picks whichever
/// matches a known package.  The search is anchored past
/// `profile_root` (when it prefixes the path) so a `packages/`
/// directory in the user's own build path - e.g. a project checked
/// out under `~/packages/` - cannot shadow the planner's
/// per-package segment.  Both candidates are `None` when the path
/// lacks the segment.
fn package_segment_candidates<'a>(
    path: &'a str,
    profile_root: Option<&str>,
) -> [Option<&'a str>; 2] {
    const SEGMENT: &str = "/packages/";
    let search = profile_root
        .and_then(|root| path.strip_prefix(root))
        .unwrap_or(path);
    let Some(after) = search.find(SEGMENT) else {
        return [None, None];
    };
    let tail = &search[after + SEGMENT.len()..];
    let mut parts = tail.split('/');
    let first = parts.next().filter(|s| !s.is_empty());
    let second = parts.next().filter(|s| !s.is_empty());
    let scoped = match (first, second) {
        // Re-slice `tail` so the scoped candidate is one contiguous
        // `<scope>/<name>` borrow rather than an allocation.
        (Some(f), Some(s)) => Some(&tail[..f.len() + 1 + s.len()]),
        _ => None,
    };
    [first, scoped]
}

/// Render the optional `-jN` token plus a trailing space for
/// the status line; empty when `jobs` is `None`.
pub(crate) fn ninja_jobs_echo(jobs: Option<cabin_core::BuildJobs>) -> String {
    match jobs {
        Some(j) => format!("-j{j} "),
        None => String::new(),
    }
}

fn ninja_verbose_echo(verbose: bool) -> &'static str {
    if verbose { "-v " } else { "" }
}

/// Ninja argv fragment: a single `-jN` token.  Backend-specific
/// conversion lives here, at the call site that spawns Ninja, rather
/// than in `cabin-core`'s [`cabin_core::BuildJobs`] model.
pub(crate) fn ninja_jobs_arg(jobs: cabin_core::BuildJobs) -> std::ffi::OsString {
    std::ffi::OsString::from(format!("-j{}", jobs.get()))
}

/// Inputs for [`invoke_ninja_and_report`]: everything needed to write
/// the Ninja files for a planned graph and drive Ninja to completion.
/// Shared by `cabin build` / `check` / `run` / `test`, each of which
/// does its own command-specific work once the build phase is done.
pub(crate) struct NinjaInvocationRequest<'a> {
    /// Resolved, absolute [`cabin_build::layout::platform_dir`] - the
    /// parent of the per-profile build root.
    pub build_dir: &'a std::path::Path,
    /// The build directory itself, which holds the build-directory lock
    /// and the usage catalog.
    pub build_root: &'a std::path::Path,
    /// Active build profile; its name selects the profile directory
    /// under `build_dir` and labels the `Finished` banner the caller
    /// prints.
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
    /// Detected toolchain: its C++ compiler family decides the MSVC
    /// environment overlay, and its tools' identities are what every
    /// Ninja edge depends on.
    pub detection: &'a cabin_core::ToolchainDetectionReport,
    /// The build context's own toolchain in a `--target` build, whose
    /// identities every Ninja edge depends on too.
    pub build_machine_detection: Option<&'a cabin_core::ToolchainDetectionReport>,
    /// Resolved feature graph, consumed by the link-failure hint.
    pub feature_resolution: &'a cabin_feature::FeatureResolution,
    /// The platform the build targets, consumed by the link-failure
    /// hint.
    pub platform: &'a cabin_core::TargetPlatform,
    /// Packages whose `[dev-dependencies]` are active for this
    /// invocation: empty for `build` / `run`, the selected runners for
    /// `test`.
    pub dev_for: &'a BTreeSet<String>,
    /// Located `ninja` executable.
    pub ninja: &'a std::path::Path,
    /// Parallelism for Ninja's `-j` flag, or `None` to let Ninja pick.
    pub jobs: Option<cabin_core::BuildJobs>,
    /// The `[gc]` settings: whether and how to collect unused outputs
    /// after the build.
    pub gc: crate::cli::gc::GcSettings,
    /// Whether the plan's sysroot was configured rather than detected,
    /// which a failed build probes.
    pub configured_sysroot: bool,
    pub reporter: Reporter,
}

/// A finished build: how long Ninja took, and the build-directory lock,
/// held shared for as long as the caller still uses the outputs (the
/// program `cabin run` starts, the tests `cabin test` runs).  A
/// collection that came due runs when this is dropped, once the outputs
/// are no longer in use: collecting takes the lock exclusively, which
/// releases it for a moment.
pub(crate) struct Built {
    pub elapsed: std::time::Duration,
    lock: cabin_build::gc::BuildDirLock,
    due: Option<crate::cli::gc::DueCollection>,
}

impl Drop for Built {
    fn drop(&mut self) {
        if let Some(due) = &self.due {
            due.run(&self.lock);
        }
    }
}

/// Write `build.ninja` + `compile_commands.json` for the planned graph
/// in the profile directory under `build_dir`, invoke Ninja there, and
/// surface a link-failure hint before bailing on a non-zero exit.  The
/// build directory is locked shared and the plan's products recorded as used
/// before Ninja runs; after a successful build that starts a new active
/// day, unused outputs are collected when the returned [`Built`] is
/// dropped (see [`crate::cli::gc`]).  Returns the wall-clock time the
/// Ninja invocation took, for the caller's cargo-style `Finished`
/// banner, and the lock.
///
/// # Errors
/// Returns an error if the build directory cannot be locked, the use of
/// its outputs cannot be recorded, the build root cannot be created,
/// the Ninja files cannot be written, Ninja cannot be spawned, or Ninja
/// exits non-zero.
pub(crate) fn invoke_ninja_and_report(req: &NinjaInvocationRequest<'_>) -> anyhow::Result<Built> {
    let lock = crate::cli::gc::lock_shared(req.build_root, req.reporter)?;
    let profile_build_root = cabin_build::layout::profile_dir(req.build_dir, &req.profile.name);
    // Once locked, a new build directory holds the lock file, an entry
    // to tell its filesystem's case sensitivity by.
    cabin_build::validate_output_paths(req.plan_graph, &profile_build_root)?;
    let collection_due = crate::cli::gc::record_use(req.build_root, req.plan_graph)?;
    std::fs::create_dir_all(&profile_build_root).with_context(|| {
        format!(
            "failed to create build directory {}",
            profile_build_root.display()
        )
    })?;

    let ninja_file = profile_build_root.join("build.ninja");
    let stamp = profile_build_root.join(cabin_ninja::writer::TOOLCHAIN_STAMP);
    // Only an MSVC build graph gets the MSVC environment overlay
    // (`VSLANG`, and the auto-discovered `INCLUDE` / `LIB` / `PATH`).
    // A GNU-style toolchain on Windows must run in the environment it
    // was resolved under: overlaying MSVC headers/libs and PATH could
    // silently switch `clang++` / `g++` to MSVC behavior while Cabin is
    // still emitting `.a` archives and GNU link lines.  Empty (a no-op)
    // off Windows.  See `cabin_toolchain::msvc_environment`.
    let ninja_env = if req.plan_graph.dialect == cabin_build::Dialect::Msvc {
        cabin_toolchain::msvc_environment(discovered_msvc_install_applies(
            req.toolchain,
            req.detection.cxx.identity.kind,
        ))
    } else {
        Vec::new()
    };
    let mut identity = req.detection.build_identity();
    // `cl`, `clang-cl`, and `link` search `INCLUDE` and `LIB` for what no
    // command names, so a change to either rebuilds every edge, as a new
    // compiler does.
    for key in ["INCLUDE", "LIB"] {
        let value = ninja_env
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var_os(key));
        if let Some(value) = value {
            identity[key] = value.to_string_lossy().into();
        }
    }
    // A run that plans no build context (`cabin check`, a narrower
    // `-p`) keeps the last build machine identity rather than
    // rebuilding every edge.
    let build_machine = match req.build_machine_detection {
        Some(detection) => Some(detection.build_identity()),
        None => std::fs::read(&stamp)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|recorded| recorded.get("build_machine").cloned()),
    };
    if let Some(build_machine) = build_machine {
        identity["build_machine"] = build_machine;
    }
    let identity = serde_json::to_string_pretty(&identity)?;
    let ccmd_file = profile_build_root.join("compile_commands.json");
    // Rendering dominates both writes and neither file depends on the
    // other, so they render side by side.  The trace capture is scoped
    // to this thread, so the helper thread is handed its dispatcher.
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let (ninja_changed, ccmd_changed) = std::thread::scope(|scope| {
        let ccmd = scope.spawn(|| {
            tracing::dispatcher::with_default(&dispatch, || {
                cabin_ninja::write_compile_commands(&ccmd_file, req.plan_graph)
            })
        });
        let ninja = cabin_ninja::write_build_ninja(
            &ninja_file,
            req.plan_graph,
            &check_stamp_runner(),
            &identity,
        );
        (
            ninja,
            ccmd.join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        )
    });
    let (ninja_changed, ccmd_changed) = (ninja_changed?, ccmd_changed?);

    // Implementation-detail status is verbose-only: under `-v` the
    // user sees which files Cabin wrote and how Ninja was invoked,
    // alongside Ninja's own raw banner.
    for (file, changed) in [(&ninja_file, ninja_changed), (&ccmd_file, ccmd_changed)] {
        if changed {
            req.reporter
                .verbose(format_args!("cabin: wrote {}", file.display()));
        } else {
            req.reporter
                .verbose(format_args!("cabin: {} is up to date", file.display()));
        }
    }
    let ninja_verbose = req.reporter.verbosity().shows_verbose();
    req.reporter.verbose(format_args!(
        "cabin: invoking {} {}{}-C {}",
        req.ninja.display(),
        ninja_jobs_echo(req.jobs),
        ninja_verbose_echo(ninja_verbose),
        profile_build_root.display()
    ));

    let mut ninja_cmd = std::process::Command::new(req.ninja);
    ninja_cmd.envs(ninja_env);
    // Registry credentials (the registry token and the GitHub
    // Actions OIDC request token) are Cabin's input, not the build
    // backend's: scrub them so Ninja and every compile / wrapper
    // command it spawns can never read them.
    ninja_cmd.env_remove(cabin_env::CABIN_REGISTRY_TOKEN);
    ninja_cmd.env_remove(cabin_env::ACTIONS_ID_TOKEN_REQUEST_TOKEN);
    if let Some(jobs) = req.jobs {
        ninja_cmd.arg(ninja_jobs_arg(jobs));
    }
    if ninja_verbose {
        ninja_cmd.arg("-v");
    }
    let ninja_log = crate::trace::NinjaLog::snapshot(&profile_build_root);
    let build_started = std::time::Instant::now();
    let run = run_ninja(
        ninja_cmd.arg("-C").arg(&profile_build_root),
        &profile_build_root,
        req.reporter,
        req.graph,
        req.plan_graph,
    )
    .with_context(|| format!("failed to invoke ninja at {}", req.ninja.display()))?;
    if let Some(ninja_log) = ninja_log {
        ninja_log.record(build_started);
    }
    if !run.status.success() {
        emit_link_diagnostic_if_applicable(&run, req);
        if req.configured_sysroot {
            emit_sysroot_diagnostic(req, &profile_build_root);
        }
        anyhow::bail!("ninja exited with {}", run.status);
    }
    Ok(Built {
        elapsed: build_started.elapsed(),
        lock,
        due: (req.gc.auto && collection_due).then(|| {
            crate::cli::gc::DueCollection::new(
                req.build_root,
                req.graph,
                req.gc.policy,
                req.reporter,
            )
        }),
    })
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
        // A pinned `clang-cl` path is never the discovered `cl.exe`, yet
        // still needs the discovered INCLUDE/LIB/PATH because `clang-cl`
        // ships no CRT/SDK headers.  The `ClangCl` early return precedes
        // `path_is_discovered_msvc_cl`, so this holds without a Windows
        // host or a real install.
        let toolchain = toolchain_with_pinned_cxx("/llvm/bin/clang-cl.exe");
        assert!(discovered_msvc_install_applies(
            &toolchain,
            CompilerKind::ClangCl
        ));
    }

    #[test]
    fn explicit_non_clang_cl_path_still_defers_to_install_match() {
        // A pinned `cl.exe` from a *different* toolset detects as
        // `CompilerKind::Msvc`, not `ClangCl`, so it reaches the path
        // comparison, which is false without a matching discovered
        // install: two toolsets' SDKs are never mixed.
        let toolchain = toolchain_with_pinned_cxx("/some/other/vs/cl.exe");
        assert!(!discovered_msvc_install_applies(
            &toolchain,
            CompilerKind::Msvc
        ));
    }

    #[test]
    fn package_segment_anchors_after_the_profile_build_root() {
        // A project checked out under a `packages/` directory: the
        // unanchored search would report `proj` (and mislabel the
        // `Compiling` banner); anchoring past the profile root
        // finds the planner's own segment.
        let path = "/home/u/packages/proj/build/dev/packages/foo/src_main.cc.o";
        assert_eq!(
            package_segment_candidates(path, Some("/home/u/packages/proj/build/dev")),
            [Some("foo"), Some("foo/src_main.cc.o")]
        );
        assert_eq!(
            package_segment_candidates(path, None),
            [Some("proj"), Some("proj/build")]
        );
        // Paths the planner did not route through the per-package
        // tree stay unattributed.
        assert_eq!(
            package_segment_candidates(
                "/home/u/proj/build/dev/stamp",
                Some("/home/u/proj/build/dev")
            ),
            [None, None]
        );
    }

    /// A scoped package's artifacts live two components below
    /// `packages/`; the scoped candidate re-joins them so the caller
    /// can match `<scope>/<name>` against the workspace package set.
    #[test]
    fn package_segment_yields_the_scoped_candidate() {
        let path = "/w/build/dev/packages/fmtlib/fmt/src_lib.cc.o";
        assert_eq!(
            package_segment_candidates(path, Some("/w/build/dev")),
            [Some("fmtlib"), Some("fmtlib/fmt")]
        );
        // An output file directly under `packages/<name>` (a bare
        // package's executable) has no second component.
        assert_eq!(
            package_segment_candidates("/w/build/dev/packages/tool", Some("/w/build/dev")),
            [Some("tool"), None]
        );
    }
}
