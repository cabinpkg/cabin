//! Orchestration for `system = true` dependency probing.
//!
//! `cabin-system-deps` owns pkg-config executable resolution,
//! subprocess invocation, and flag classification; this module
//! threads the typed report back into the per-package
//! [`cabin_core::ResolvedProfileFlags`] map every Cabin pipeline
//! consumes. Keeping the orchestration here preserves the
//! package rule that `cabin` stays thin: no probing,
//! parsing, or flag-merge business logic lives in `cli.rs`.
//!
//! The single helper [`augment_build_flags_with_system_deps`]
//! must be called from every command that constructs a build
//! configuration or planner request — `cabin build` /
//! `cabin run` / `cabin test` / `cabin tidy` /
//! `cabin metadata`. The merge point sits *after*
//! `cabin_core::resolve_build_flags` and *before*
//! `BuildConfiguration::resolve`, so the build configuration
//! fingerprint observes the discovered flags.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use anyhow::{Result, bail};

use cabin_core::{ResolvedProfileFlags, SystemDependency, TargetPlatform, Verbosity};
use cabin_system_deps::{
    PkgConfigError, PkgConfigTool, SystemDependencyFlags, SystemDependencyProbeRequest,
    SystemDependencyResolution, probe_system_dependency,
};
use cabin_workspace::PackageGraph;

use crate::term_verbosity_glue::Reporter;

/// Per-package map of every successful `pkg-config` probe
/// produced during a workspace probe. Keyed by package index so
/// callers can correlate the resolution back to the originating
/// manifest.
type SystemDependencyReports = BTreeMap<usize, Vec<SystemDependencyResolution>>;

/// Return shape of [`augment_build_flags_with_system_deps`]:
/// the augmented per-package flag map plus the deterministic
/// probe reports.
type AugmentedBuildFlags = (
    HashMap<usize, ResolvedProfileFlags>,
    SystemDependencyReports,
);

/// Probe every active system dependency declared by every
/// primary package in the supplied graph and merge the
/// discovered flags into `build_flags`.  Returns the same map,
/// augmented, plus a deterministic per-package probe report so
/// the caller can render verbose output or extend a metadata
/// view.
///
/// The function is a no-op when no system dependency is active:
/// no `pkg-config` subprocess is spawned and the supplied
/// `build_flags` map is returned unchanged.  The pkg-config
/// executable is only required when at least one package
/// contributes an active system dependency.
pub(crate) fn augment_build_flags_with_system_deps(
    graph: &PackageGraph,
    host_platform: &TargetPlatform,
    dev_for: &BTreeSet<String>,
    mut build_flags: HashMap<usize, ResolvedProfileFlags>,
    reporter: Reporter,
) -> Result<AugmentedBuildFlags> {
    let active = collect_active_system_deps(graph, host_platform, dev_for);
    if active.is_empty() {
        return Ok((build_flags, BTreeMap::new()));
    }

    let tool = PkgConfigTool::from_env(|key| std::env::var_os(key));
    let count = total_count(&active);
    let noun = if count == 1 {
        "dependency"
    } else {
        "dependencies"
    };
    // Probe chatter always lands on stderr because `cabin
    // metadata` reserves stdout for its JSON document.
    reporter.aux_verbose(format_args!(
        "cabin: probing {count} system {noun} via {}",
        tool.executable().to_string_lossy(),
    ));

    // Check availability up-front so the user gets a single
    // actionable diagnostic when pkg-config is missing,
    // regardless of which package declares the first system
    // dependency.
    if let Err(err) = tool.check_available() {
        return Err(anyhow::anyhow!(err));
    }

    let mut reports: BTreeMap<usize, Vec<SystemDependencyResolution>> = BTreeMap::new();
    for (pkg_idx, deps) in active {
        let pkg_name = graph.packages[pkg_idx].package.name.as_str();
        let entry = build_flags.entry(pkg_idx).or_default();
        let mut pkg_reports: Vec<SystemDependencyResolution> = Vec::with_capacity(deps.len());
        for dep in deps {
            let resolved = probe_dep(&tool, pkg_name, dep, reporter)?;
            merge_flags(entry, &resolved.flags);
            pkg_reports.push(resolved);
        }
        reports.insert(pkg_idx, pkg_reports);
    }
    Ok((build_flags, reports))
}

fn probe_dep(
    tool: &PkgConfigTool,
    pkg_name: &str,
    dep: &SystemDependency,
    reporter: Reporter,
) -> Result<SystemDependencyResolution> {
    let verbosity = reporter.verbosity();
    if verbosity == Verbosity::VeryVerbose {
        reporter.aux_very_verbose(format_args!(
            "cabin: probing `{}` for package `{}` (version = {:?})",
            dep.name.as_str(),
            pkg_name,
            dep.version,
        ));
    }
    let request = SystemDependencyProbeRequest {
        name: dep.name.as_str(),
        version_requirement: &dep.version,
        tool,
    };
    match probe_system_dependency(&request) {
        Ok(resolved) => {
            if verbosity.shows_verbose() {
                let version_suffix = match resolved.version.as_deref() {
                    Some(v) => format!(" (version {v})"),
                    None => String::new(),
                };
                reporter.aux_verbose(format_args!(
                    "cabin: system dependency `{}` ok{}",
                    resolved.name, version_suffix,
                ));
            }
            Ok(resolved)
        }
        Err(err) => bail!(format_probe_error(pkg_name, dep, err)),
    }
}

fn format_probe_error(
    pkg_name: &str,
    dep: &SystemDependency,
    err: PkgConfigError,
) -> anyhow::Error {
    // Surface a single sentence that identifies the declaring
    // package alongside the typed error's own message. The
    // typed error keeps its diagnostic code and help text so
    // `cabin-diagnostics::render` can pick it up upstream.
    let message = format!(
        "package `{}` failed to probe system dependency `{}`: {}",
        pkg_name,
        dep.name.as_str(),
        err,
    );
    anyhow::Error::new(err).context(message)
}

fn merge_flags(flags: &mut ResolvedProfileFlags, contrib: &SystemDependencyFlags) {
    // Include paths: dedupe by exact value while preserving
    // first-seen order, mirroring `cabin_core::resolve_build_flags`.
    let mut seen: BTreeSet<PathBuf> = flags.include_dirs.iter().cloned().collect();
    for dir in &contrib.include_dirs {
        if seen.insert(dir.clone()) {
            flags.include_dirs.push(dir.clone());
        }
    }
    // Extra compile args: append verbatim. pkg-config decides
    // the order; we never reshuffle it.
    flags
        .extra_compile_args
        .extend(contrib.extra_compile_args.iter().cloned());
    // Link args: append verbatim. Order is load-bearing for
    // C / C++ linking.
    flags.ldflags.extend(contrib.ldflags.iter().cloned());
}

fn collect_active_system_deps<'a>(
    graph: &'a PackageGraph,
    host_platform: &TargetPlatform,
    dev_for: &BTreeSet<String>,
) -> Vec<(usize, Vec<&'a SystemDependency>)> {
    use cabin_core::DependencyKind;
    let mut out: Vec<(usize, Vec<&'a SystemDependency>)> = Vec::new();
    // System dependencies are only probed for *primary*
    // packages — the local workspace members the user owns.
    // Registry / extracted dependencies do not contribute
    // system deps; their canonical metadata round-trips
    // declarations only.
    for &idx in &graph.primary_packages {
        let package = &graph.packages[idx].package;
        if package.system_dependencies.is_empty() {
            continue;
        }
        let pkg_name = package.name.as_str();
        let mut deps: Vec<&SystemDependency> = Vec::new();
        for dep in &package.system_dependencies {
            // Per-kind activation matches Cabin-package deps:
            //   `Normal` → always active.
            //   `Dev`    → only when the command opted this
            //              package into dev-dep activation
            //              (`cabin test`).
            match dep.kind {
                DependencyKind::Normal => {}
                DependencyKind::Dev if dev_for.contains(pkg_name) => {}
                DependencyKind::Dev => continue,
            }
            if let Some(cond) = &dep.condition
                && !cond.evaluate(host_platform)
            {
                continue;
            }
            deps.push(dep);
        }
        if !deps.is_empty() {
            // Determinism: order by name so identical workspaces
            // always probe in the same sequence (and the
            // resulting flag append order is stable).
            deps.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
            out.push((idx, deps));
        }
    }
    // Determinism: ascending package index. Iteration order
    // shows up in the deterministic flag append sequence and in
    // any verbose output we emit, so we pin it here rather than
    // relying on the graph's primary set being already sorted.
    out.sort_by_key(|(idx, _)| *idx);
    out
}

fn total_count(active: &[(usize, Vec<&SystemDependency>)]) -> usize {
    active.iter().map(|(_, v)| v.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::ResolvedProfileFlags;

    fn flags() -> ResolvedProfileFlags {
        ResolvedProfileFlags::default()
    }

    #[test]
    fn merge_appends_include_paths_uniquely() {
        let mut f = flags();
        f.include_dirs.push(PathBuf::from("/already"));
        let contrib = SystemDependencyFlags {
            include_dirs: vec![PathBuf::from("/already"), PathBuf::from("/added")],
            ..Default::default()
        };
        merge_flags(&mut f, &contrib);
        assert_eq!(
            f.include_dirs,
            vec![PathBuf::from("/already"), PathBuf::from("/added")],
        );
    }

    #[test]
    fn merge_preserves_link_args_order() {
        let mut f = flags();
        f.ldflags.push("-lpriv".into());
        let contrib = SystemDependencyFlags {
            ldflags: vec![
                "-L/lib".into(),
                "-lssl".into(),
                "-lcrypto".into(),
                "-lssl".into(),
            ],
            ..Default::default()
        };
        merge_flags(&mut f, &contrib);
        assert_eq!(
            f.ldflags,
            vec![
                "-lpriv".to_owned(),
                "-L/lib".to_owned(),
                "-lssl".to_owned(),
                "-lcrypto".to_owned(),
                "-lssl".to_owned(),
            ],
        );
    }

    #[test]
    fn merge_appends_compile_args_in_order() {
        let mut f = flags();
        let contrib = SystemDependencyFlags {
            extra_compile_args: vec!["-pthread".into(), "-fPIC".into()],
            ..Default::default()
        };
        merge_flags(&mut f, &contrib);
        assert_eq!(
            f.extra_compile_args,
            vec!["-pthread".to_owned(), "-fPIC".to_owned()],
        );
    }
}
