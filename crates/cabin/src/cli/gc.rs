//! Build-output garbage collection: the collection `build`, `check`,
//! `run`, and `test` run after the first build of an active day, and
//! `cabin clean --gc`.  The policy and its safety rules live in
//! [`cabin_build::gc`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use cabin_build::gc::{
    BuildDirLock, CollectReport, CollectRequest, GcError, Policy, Reason, collect,
};

use crate::cli::term_verbosity::Reporter;

/// How long an automatic collection may remove files before it leaves
/// the rest to the next build.
const AUTO_BUDGET: Duration = Duration::from_secs(1);

/// The `[gc]` settings in effect.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GcSettings {
    pub auto: bool,
    pub policy: Policy,
}

impl GcSettings {
    pub(crate) fn from_config(config: &cabin_config::EffectiveConfig) -> Self {
        let defaults = Policy::default();
        Self {
            auto: config.gc.auto.as_ref().is_none_or(|auto| auto.value),
            policy: Policy {
                max_age: config
                    .gc
                    .max_age
                    .as_ref()
                    .map_or(defaults.max_age, |v| v.value),
                max_size: config.gc.max_size.as_ref().map(|v| v.value),
            },
        }
    }
}

fn wait_message(reporter: Reporter) -> impl FnOnce() {
    move || {
        reporter.status(
            "Blocking",
            format_args!("waiting for file lock on build directory"),
        );
    }
}

/// Lock `build_root` for using its outputs.
pub(crate) fn lock_shared(build_root: &Path, reporter: Reporter) -> Result<BuildDirLock> {
    Ok(BuildDirLock::shared(build_root, wait_message(reporter))?)
}

/// Lock `build_root` for removing outputs.
pub(crate) fn lock_exclusive(build_root: &Path, reporter: Reporter) -> Result<BuildDirLock> {
    Ok(BuildDirLock::exclusive(build_root, wait_message(reporter))?)
}

/// Record that the plan's products are in use.  Fails the command when
/// the record cannot be written: an unrecorded use could let a later
/// collection treat the outputs as unused.
/// Returns whether a collection is due.
pub(crate) fn record_use(build_root: &Path, plan_graph: &cabin_build::BuildGraph) -> Result<bool> {
    cabin_build::gc::record_use(
        build_root,
        cabin_build::gc::plan_products(plan_graph),
        cabin_build::gc::today(),
    )
    .with_context(|| {
        format!(
            "failed to record the use of build outputs under {}",
            build_root.display()
        )
    })
}

/// An automatic collection that came due, run once the command no
/// longer uses its outputs (see [`crate::cli::ninja::Built`]).
pub(crate) struct DueCollection {
    build_root: PathBuf,
    workspace_root: PathBuf,
    package_roots: Vec<PathBuf>,
    protected_source_paths: Vec<PathBuf>,
    policy: Policy,
    reporter: Reporter,
}

impl DueCollection {
    pub(crate) fn new(
        build_root: &Path,
        graph: &cabin_workspace::PackageGraph,
        policy: Policy,
        reporter: Reporter,
    ) -> Self {
        Self {
            build_root: build_root.to_path_buf(),
            workspace_root: graph.root_dir.clone(),
            package_roots: graph
                .packages
                .iter()
                .map(|pkg| pkg.manifest_dir.clone())
                .collect(),
            protected_source_paths: crate::cli::clean::clean_protected_source_paths(graph),
            policy,
            reporter,
        }
    }

    /// Collect unused outputs unless another process is using the build
    /// directory.  Never fails the command: a problem is a warning, and
    /// an unusable build directory is noted only under `-v`.
    pub(crate) fn run(&self, lock: &BuildDirLock) {
        let (policy, reporter) = (self.policy, self.reporter);
        let request = CollectRequest {
            build_dir: &self.build_root,
            workspace_root: &self.workspace_root,
            package_roots: &self.package_roots,
            protected_source_paths: &self.protected_source_paths,
            policy,
            dry_run: false,
            time_limit: Some(AUTO_BUDGET),
        };
        match lock.try_exclusive(|| collect(&request)) {
            Ok(Some(Ok(report))) => {
                print_summary(&report, policy, false, reporter);
                warn_about(&report, policy, reporter);
                if let Some(failed) = describe_failures(&report) {
                    reporter.warning(format_args!("{failed}"));
                }
                if report.incomplete {
                    reporter.verbose(format_args!(
                        "cabin: stopped collecting unused build outputs after {}s; the next build continues",
                        AUTO_BUDGET.as_secs()
                    ));
                }
            }
            Ok(None) => reporter.verbose(format_args!(
                "cabin: did not collect unused build outputs: {} is in use by another process, or its file system has no file locks",
                self.build_root.display()
            )),
            Ok(Some(Err(GcError::UnsafeBuildDir(err)))) => reporter.verbose(format_args!(
                "cabin: did not collect unused build outputs: {err}"
            )),
            Ok(Some(Err(err))) | Err(err) => reporter.warning(format_args!(
                "could not collect unused build outputs: {err}"
            )),
        }
    }
}

/// `cabin clean --gc`.
pub(crate) fn clean_gc(request: &CollectRequest<'_>, reporter: Reporter) -> Result<()> {
    let build_dir = request.build_dir;
    if !build_dir.exists() {
        reporter.status(
            "Removed",
            format_args!(
                "nothing under {} (build directory does not exist)",
                build_dir.display()
            ),
        );
        return Ok(());
    }
    // Refuse an unsafe directory before the lock file is created in it.
    cabin_build::gc::validate_build_dir(request)?;
    // A dry run only reads, and writes nothing - not even the lock file.
    let _lock = if request.dry_run {
        None
    } else {
        Some(lock_exclusive(build_dir, reporter)?)
    };
    let report = collect(request)?;
    if report.collected.is_empty() {
        reporter.status(
            "Removed",
            format_args!(
                "nothing under {}{}",
                build_dir.display(),
                if request.dry_run { " (dry-run)" } else { "" }
            ),
        );
    } else {
        print_summary(&report, request.policy, request.dry_run, reporter);
    }
    if request.dry_run {
        for collected in &report.collected {
            reporter.note(format_args!(
                "  {} ({})",
                collected.path.display(),
                describe(collected.reason)
            ));
        }
    }
    warn_about(&report, request.policy, reporter);
    if let Some(failed) = describe_failures(&report) {
        anyhow::bail!(failed);
    }
    Ok(())
}

fn describe_failures(report: &CollectReport) -> Option<String> {
    let (path, err) = report.failed.first()?;
    Some(format!(
        "could not remove {} unused build output{}; first: {}: {err}",
        report.failed.len(),
        crate::plural(report.failed.len()),
        path.display()
    ))
}

fn print_summary(report: &CollectReport, policy: Policy, dry_run: bool, reporter: Reporter) {
    if report.collected.is_empty() {
        return;
    }
    let count = report.collected.len();
    let by_size = report
        .collected
        .iter()
        .filter(|c| c.reason == Reason::SizeBudget)
        .count();
    let why = match (by_size, policy.max_size) {
        (0, _) | (_, None) => format!("unused for over {} active days", policy.max_age),
        (n, Some(budget)) if n == count => {
            format!("to fit the {} size budget", human_bytes(budget))
        }
        (_, Some(budget)) => format!(
            "unused for over {} active days or to fit the {} size budget",
            policy.max_age,
            human_bytes(budget)
        ),
    };
    reporter.status(
        "Removed",
        format_args!(
            "{count} build output{} {why}, {}{}",
            crate::plural(count),
            human_bytes(report.collected_bytes()),
            if dry_run {
                " (dry-run; re-run without --dry-run to apply)"
            } else {
                ""
            }
        ),
    );
}

fn warn_about(report: &CollectReport, policy: Policy, reporter: Reporter) {
    if report.unreadable > 0 {
        reporter.warning(format_args!(
            "skipped {} unreadable build-output usage record{}; the outputs they list are kept",
            report.unreadable,
            crate::plural(report.unreadable)
        ));
    }
    if let (Some(retained), Some(budget)) = (report.retained_bytes, policy.max_size)
        && retained > budget
    {
        reporter.warning(format_args!(
            "build outputs used in the last two active days take {}, more than the {} size budget; they are kept",
            human_bytes(retained),
            human_bytes(budget)
        ));
    }
}

fn describe(reason: Reason) -> String {
    match reason {
        Reason::Unused { days } => format!("unused for {days} active days"),
        Reason::SizeBudget => "size budget".to_owned(),
    }
}

/// `1.5 MiB`-style rendering of a logical size.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
