//! Orchestration for `system = true` dependency probing.
//!
//! `cabin-system-deps` owns pkg-config executable resolution,
//! subprocess invocation, and flag classification; this module
//! threads the typed report back into the per-package
//! [`cabin_core::ResolvedProfileFlags`] map every Cabin pipeline
//! consumes.
//!
//! Every command that constructs a build configuration or planner
//! request reaches [`augment_build_flags_with_system_deps`] through
//! `crate::cli::build_prep::augment_build_flags`, which runs it
//! before the environment-flag layer.  The merge point sits *after*
//! `cabin_core::resolve_build_flags` and *before*
//! `BuildConfiguration::resolve`, so the build configuration
//! fingerprint observes the discovered flags.

use std::collections::{BTreeSet, HashMap};

use camino::Utf8PathBuf;

use anyhow::{Result, bail};

use cabin_core::{ResolvedProfileFlags, SystemDependency, TargetPlatform, Verbosity};
use cabin_system_deps::{
    PkgConfigError, PkgConfigTool, SystemDependencyFlags, SystemDependencyProbeRequest,
    SystemDependencyResolution, probe_system_dependency,
};
use cabin_workspace::PackageGraph;

use crate::cli::term_verbosity::Reporter;

/// Probe every active system dependency declared by every
/// primary package in the supplied graph and merge the
/// discovered flags into `build_flags`.
///
/// A no-op when no system dependency is active: no `pkg-config`
/// subprocess is spawned, so the executable is required only when
/// one is.
pub(crate) fn augment_build_flags_with_system_deps(
    graph: &PackageGraph,
    platform: &TargetPlatform,
    dev_for: &BTreeSet<String>,
    mut build_flags: HashMap<usize, ResolvedProfileFlags>,
    reporter: Reporter,
) -> Result<HashMap<usize, ResolvedProfileFlags>> {
    let active = collect_active_system_deps(graph, platform, dev_for);
    if active.is_empty() {
        return Ok(build_flags);
    }

    let tool = PkgConfigTool::from_env(|key| std::env::var_os(key));
    // A plain pkg-config reports the build machine's libraries, which
    // must not link into a build for another machine.  A triple's `env`
    // counts (a glibc host's `.pc` files do not fit a musl build), and
    // the build machine's `env` is guessed as `gnu` on Linux, so on a
    // musl host an explicit musl `--target` needs one of the overrides.
    if !platform.same_machine(&TargetPlatform::current())
        && std::env::var_os(cabin_env::CABIN_PKG_CONFIG).is_none_or(|value| value.is_empty())
        && std::env::var_os("PKG_CONFIG_LIBDIR").is_none()
    {
        bail!(
            "system dependencies of a build for {} need a pkg-config for that platform, not \
             the build machine's: set PKG_CONFIG_LIBDIR (and PKG_CONFIG_SYSROOT_DIR) to the \
             target sysroot's pkg-config directories, or CABIN_PKG_CONFIG to a pkg-config \
             for {}",
            platform.target,
            platform.target,
        );
    }
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
    // dependency.  Keep the typed error as the anyhow source so
    // the diagnostic registry can render its code and help text,
    // exactly as the probe path below does.
    if let Err(err) = tool.check_available() {
        return Err(err.into());
    }

    for (pkg_idx, deps) in active {
        let pkg_name = graph.packages[pkg_idx].package.name.as_str();
        let entry = build_flags.entry(pkg_idx).or_default();
        for dep in deps {
            let resolved = probe_dep(&tool, pkg_name, dep, reporter)?;
            merge_flags(entry, &resolved.flags);
        }
    }
    Ok(build_flags)
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
    platform: &TargetPlatform,
    dev_for: &BTreeSet<String>,
    selected: &BTreeSet<usize>,
) -> bool {
    collect_active_system_deps(graph, platform, dev_for)
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
    platform: &TargetPlatform,
    dev_for: &BTreeSet<String>,
    dialect: cabin_build::Dialect,
    selected: &BTreeSet<usize>,
) -> Result<()> {
    if dialect == cabin_build::Dialect::Msvc
        && has_active_system_deps(graph, platform, dev_for, selected)
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
            dep.name, pkg_name, dep.version,
        ));
    }
    let request = SystemDependencyProbeRequest {
        name: &dep.name,
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
        pkg_name, dep.name, err,
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

/// Refuse a build-context package that declares a system dependency
/// active on the build machine: build-context units get no pkg-config
/// probe until providers are per context (stage 06).
pub(crate) fn reject_build_context_system_deps(
    graph: &PackageGraph,
    build_machine: &TargetPlatform,
    included: &BTreeSet<usize>,
) -> Result<()> {
    for (idx, deps) in collect_active_system_deps(graph, build_machine, &BTreeSet::new()) {
        if included.contains(&idx) {
            bail!(
                "package `{}` is built as a build dependency, but declares system dependency `{}`; system dependencies of build dependencies are not supported yet",
                graph.packages[idx].package.name,
                deps[0].name,
            );
        }
    }
    Ok(())
}

fn collect_active_system_deps<'a>(
    graph: &'a PackageGraph,
    platform: &TargetPlatform,
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
            // Activation by kind matches Cabin-package dependencies.
            match dep.kind {
                DependencyKind::Normal => {}
                DependencyKind::Dev if dev_for.contains(pkg_name) => {}
                // The manifest rejects `system = true` on build edges.
                DependencyKind::Dev | DependencyKind::Build => continue,
            }
            if let Some(cond) = &dep.condition
                && !cond.evaluate(&cabin_core::ConditionContext::platform_only(platform))
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
    // Determinism: ascending package index, so the probe order, the
    // verbose output, and the first reported failure do not depend on
    // the order of the graph's primary set.
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
            }],
        }
    }

    fn zlib_dep() -> SystemDependency {
        SystemDependency {
            name: "zlib".to_owned(),
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
