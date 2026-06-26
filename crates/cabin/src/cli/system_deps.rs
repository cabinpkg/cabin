//! Orchestration for `system = true` dependency probing.
//!
//! `cabin-system-deps` owns pkg-config executable resolution,
//! subprocess invocation, and flag classification; this module
//! threads the typed report back into the per-package
//! [`cabin_core::ResolvedProfileFlags`] map every Cabin pipeline
//! consumes.  Keeping the orchestration here preserves the
//! package rule that `cabin` stays thin: no probing,
//! parsing, or flag-merge business logic lives in `cli.rs`.
//!
//! The single helper [`augment_build_flags_with_system_deps`]
//! must be called from every command that constructs a build
//! configuration or planner request - `cabin build` /
//! `cabin run` / `cabin test` / `cabin tidy` /
//! `cabin metadata`.  The merge point sits *after*
//! `cabin_core::resolve_build_flags` and *before*
//! `BuildConfiguration::resolve`, so the build configuration
//! fingerprint observes the discovered flags.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use camino::Utf8PathBuf;

use anyhow::{Result, bail};

use cabin_core::{ResolvedProfileFlags, SystemDependency, TargetPlatform, Verbosity};
use cabin_system_deps::{
    PkgConfigError, PkgConfigTool, SystemDependencyFlags, SystemDependencyProbeRequest,
    SystemDependencyResolution, probe_system_dependency,
};
use cabin_workspace::PackageGraph;

use crate::cli::term_verbosity::Reporter;

/// Per-package map of every successful `pkg-config` probe
/// produced during a workspace probe.  Keyed by package index so
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

/// Whether any active `system = true` dependency is declared by a
/// *selected* primary package for the evaluation platform.
///
/// `selected` is the index closure of the packages this command builds.
/// A workspace member's system dependency that is not part of the
/// selected build must not gate the command (e.g. `cabin build -p B`
/// when only sibling `A` declares one), so the result is restricted to
/// `selected` rather than every primary package in `graph`.
pub(crate) fn has_active_system_deps(
    graph: &PackageGraph,
    host_platform: &TargetPlatform,
    dev_for: &BTreeSet<String>,
    selected: &BTreeSet<usize>,
) -> bool {
    collect_active_system_deps(graph, host_platform, dev_for)
        .iter()
        .any(|(idx, _)| selected.contains(idx))
}

/// Reject a build whose dialect cannot consume pkg-config's GNU-style
/// `--cflags` / `--libs` output.
///
/// The MSVC backend links with `/LIBPATH:` plus `<name>.lib` file names
/// and compiles with `/`-style flags, while pkg-config emits `-L` /
/// `-lfoo` / `-pthread`.  On Windows the `.pc` files come from
/// MinGW/msys2 and reference the MinGW ABI, so even a syntactically
/// correct token translation would link the wrong libraries - worse
/// than a clear error.  Fail fast instead of emitting a command line
/// `cl` / `link` cannot run.
///
/// # Errors
/// Returns an error when `dialect` is [`cabin_build::Dialect::Msvc`] and
/// at least one *selected* package has an active system dependency.
pub(crate) fn ensure_dialect_supports_system_deps(
    graph: &PackageGraph,
    host_platform: &TargetPlatform,
    dev_for: &BTreeSet<String>,
    dialect: cabin_build::Dialect,
    selected: &BTreeSet<usize>,
) -> Result<()> {
    if dialect == cabin_build::Dialect::Msvc
        && has_active_system_deps(graph, host_platform, dev_for, selected)
    {
        bail!(
            "`system = true` dependencies are resolved with pkg-config, whose GNU-style \
             flags the MSVC backend cannot consume; system dependencies are not supported \
             with an MSVC toolchain (build with a GCC/Clang toolchain, or remove the \
             system dependency)"
        );
    }
    Ok(())
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
    // package alongside the typed error's own message.  The
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
    // The seen-set spans both buckets so a directory keeps its
    // first-seen bucket and is never spelled `-I` and `-isystem` on
    // the same command line.
    let mut seen: BTreeSet<Utf8PathBuf> = flags
        .include_dirs
        .iter()
        .chain(flags.system_include_dirs.iter())
        .cloned()
        .collect();
    for dir in &contrib.include_dirs {
        if seen.insert(dir.clone()) {
            flags.include_dirs.push(dir.clone());
        }
    }
    for dir in &contrib.system_include_dirs {
        if seen.insert(dir.clone()) {
            flags.system_include_dirs.push(dir.clone());
        }
    }
    // Extra compile args: append verbatim. pkg-config decides
    // the order; we never reshuffle it.
    flags
        .extra_compile_args
        .extend(contrib.extra_compile_args.iter().cloned());
    // Link args: append verbatim.  Order is load-bearing for
    // C/C++ linking.
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
    // packages - the local workspace members the user owns.
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
            // `Normal` → always active.
            // `Dev` → only when the command opted this
            // package into dev-dep activation
            // (`cabin test`).
            match dep.kind {
                DependencyKind::Normal => {}
                DependencyKind::Dev if dev_for.contains(pkg_name) => {}
                DependencyKind::Dev => continue,
            }
            if let Some(cond) = &dep.condition
                && !cond.evaluate(&cabin_core::ConditionContext::platform_only(host_platform))
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
    // Determinism: ascending package index.  Iteration order
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
        f.include_dirs.push(Utf8PathBuf::from("/already"));
        let contrib = SystemDependencyFlags {
            include_dirs: vec![Utf8PathBuf::from("/already"), Utf8PathBuf::from("/added")],
            ..Default::default()
        };
        merge_flags(&mut f, &contrib);
        assert_eq!(
            f.include_dirs,
            vec![Utf8PathBuf::from("/already"), Utf8PathBuf::from("/added")],
        );
    }

    #[test]
    fn merge_routes_system_include_paths_to_system_bucket() {
        let mut f = flags();
        let contrib = SystemDependencyFlags {
            system_include_dirs: vec![Utf8PathBuf::from("/opt/zlib/include")],
            ..Default::default()
        };
        merge_flags(&mut f, &contrib);
        assert_eq!(
            f.system_include_dirs,
            vec![Utf8PathBuf::from("/opt/zlib/include")],
        );
        assert!(f.include_dirs.is_empty());
    }

    #[test]
    fn merge_dedups_include_paths_across_buckets() {
        // A directory already present in the user bucket keeps it:
        // no path is ever spelled both `-I` and `-isystem` on one
        // command line.
        let mut f = flags();
        f.include_dirs.push(Utf8PathBuf::from("/already"));
        let contrib = SystemDependencyFlags {
            system_include_dirs: vec![Utf8PathBuf::from("/already"), Utf8PathBuf::from("/added")],
            ..Default::default()
        };
        merge_flags(&mut f, &contrib);
        assert_eq!(f.include_dirs, vec![Utf8PathBuf::from("/already")]);
        assert_eq!(f.system_include_dirs, vec![Utf8PathBuf::from("/added")]);
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

    fn graph_with_system_deps(deps: Vec<SystemDependency>) -> PackageGraph {
        use cabin_workspace::{PackageKind, WorkspacePackage};
        use std::path::PathBuf;
        let package = cabin_core::Package::with_config(cabin_core::PackageConfigInput {
            name: cabin_core::PackageName::new("root").unwrap(),
            version: semver::Version::parse("0.1.0").unwrap(),
            targets: Vec::new(),
            dependencies: Vec::new(),
            system_dependencies: deps,
            features: cabin_core::Features::default(),
        })
        .unwrap();
        PackageGraph {
            root_manifest_path: PathBuf::from("/tmp/cabin.toml"),
            root_dir: PathBuf::from("/tmp"),
            is_workspace_root: false,
            root_package: Some(0),
            root_settings: Default::default(),
            primary_packages: vec![0],
            default_members: vec![0],
            excluded_members: Vec::new(),
            packages: vec![WorkspacePackage {
                package,
                manifest_dir: PathBuf::from("/tmp"),
                manifest_path: PathBuf::from("/tmp/cabin.toml"),
                kind: PackageKind::Local,
                deps: Vec::new(),
                is_port: false,
            }],
        }
    }

    fn zlib_dep() -> SystemDependency {
        SystemDependency {
            name: cabin_core::PackageName::new("zlib").unwrap(),
            version: String::new(),
            kind: cabin_core::DependencyKind::Normal,
            condition: None,
        }
    }

    #[test]
    fn msvc_with_selected_active_system_dep_is_rejected_but_gnu_is_not() {
        let graph = graph_with_system_deps(vec![zlib_dep()]);
        let host = TargetPlatform::current();
        let dev_for = BTreeSet::new();
        let selected = BTreeSet::from([0usize]);

        // MSVC cannot consume pkg-config's GNU-style flags.
        let err = ensure_dialect_supports_system_deps(
            &graph,
            &host,
            &dev_for,
            cabin_build::Dialect::Msvc,
            &selected,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("MSVC"),
            "expected an MSVC-specific rejection, got: {err}"
        );

        // The GCC/Clang dialect consumes them, so it is accepted.
        ensure_dialect_supports_system_deps(
            &graph,
            &host,
            &dev_for,
            cabin_build::Dialect::GnuLike,
            &selected,
        )
        .unwrap();
    }

    #[test]
    fn msvc_without_system_deps_is_accepted() {
        let graph = graph_with_system_deps(vec![]);
        let host = TargetPlatform::current();
        let dev_for = BTreeSet::new();
        let selected = BTreeSet::from([0usize]);
        // Nothing to reject when no system dependency is active.
        ensure_dialect_supports_system_deps(
            &graph,
            &host,
            &dev_for,
            cabin_build::Dialect::Msvc,
            &selected,
        )
        .unwrap();
    }

    #[test]
    fn msvc_with_unselected_system_dep_is_accepted() {
        // The package declaring the system dependency is not in the
        // selected closure, so an MSVC build of the rest of the workspace
        // is not rejected - the rejection is scoped to the selected build.
        let graph = graph_with_system_deps(vec![zlib_dep()]);
        let host = TargetPlatform::current();
        let dev_for = BTreeSet::new();
        let selected = BTreeSet::new(); // package 0 is not selected
        ensure_dialect_supports_system_deps(
            &graph,
            &host,
            &dev_for,
            cabin_build::Dialect::Msvc,
            &selected,
        )
        .unwrap();
    }
}
