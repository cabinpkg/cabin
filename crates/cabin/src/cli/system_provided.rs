//! The root's `[system-provided]` policy: packages a build takes from
//! the system's pkg-config instead of fetching and building them.
//!
//! Resolution and `cabin.lock` ignore the policy.  After resolution,
//! [`split_resolution`] leaves the provided packages, and whatever only
//! they reach, out of the fetch and puts a synthesized package in each
//! one's place; once the final graph's features are known,
//! [`probe_provided`] probes the provided packages the build includes
//! and returns the interfaces the planner gives their targets.
//!
//! The resolution walks are `cabin_resolver::ResolveOutput`'s; this
//! module joins their results with the index, the loaded graph, feature
//! resolution, and the pkg-config probe, as `system_deps` does for
//! `system = true`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context as _, Result, bail};

use cabin_core::{
    DependencyKind, DependencySource, Features, Package, PackageConfigInput, PackageName,
    SystemProvidedSettings, Target, TargetKind, TargetName, TargetPlatform,
};
use cabin_index::PackageIndex;
use cabin_resolver::{ResolveOutput, ResolvedSource};
use cabin_system_deps::{SystemDependencyProbeRequest, probe_system_dependency};
use cabin_workspace::{PackageGraph, PackageKind};

use crate::cli::term_verbosity::Reporter;

/// What a resolution leaves to the system.
#[derive(Default)]
pub(crate) struct SystemSplit {
    /// Resolved packages the build leaves out: the provided ones and
    /// every package only they reach, a `[patch]` fork included.
    pub(crate) unfetched: BTreeSet<PackageName>,
    /// One package per provided package the resolution reaches, in
    /// place of its fetched manifest: the locked version, one
    /// header-only target with no sources, the index's feature names
    /// enabling nothing, and no dependencies.
    pub(crate) stubs: Vec<Package>,
    /// Every package each stub's package reaches in the resolution, on
    /// any platform: the system library links its own copies of them.
    pub(crate) reaches: BTreeMap<PackageName, BTreeSet<PackageName>>,
}

/// Walk `output` from its root through resolved dependency edges, not
/// descending into a package `settings` provides.
///
/// A provided package that resolved to a local package gets no stub;
/// [`probe_provided`] refuses it.
///
/// # Errors
/// A provided package whose index entry does not list exactly one
/// library target.
pub(crate) fn split_resolution(
    output: &ResolveOutput,
    index: &PackageIndex,
    settings: &SystemProvidedSettings,
) -> Result<SystemSplit> {
    if settings.is_empty() {
        return Ok(SystemSplit::default());
    }
    let (fetched, provided) = output.reach_until(|name| settings.entries.contains_key(name));

    let mut stubs = Vec::new();
    let mut reaches = BTreeMap::new();
    for resolved in output
        .packages
        .iter()
        .filter(|p| p.source == ResolvedSource::Index && provided.contains(&p.name))
    {
        let name = &resolved.name;
        reaches.insert(
            name.clone(),
            output.dependencies_of(name).into_iter().cloned().collect(),
        );
        let meta = index
            .package(name)
            .and_then(|entry| entry.versions.get(&resolved.version))
            .with_context(|| {
                format!(
                    "resolver chose `{name} {}`, but the index has no entry for this version",
                    resolved.version
                )
            })?;
        let targets: Vec<&String> = meta.standards.targets.keys().collect();
        let [target] = targets.as_slice() else {
            bail!(
                "`{name} {}` is `[system-provided]` by a single pkg-config module, but the index \
                 lists {} library targets for it rather than one",
                resolved.version,
                targets.len(),
            );
        };
        let target = Target {
            name: TargetName::new(target.as_str())?,
            kind: TargetKind::HeaderOnly,
            sources: Vec::new(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            interface_defines: Vec::new(),
            deps: Vec::new(),
            required_features: Vec::new(),
            language: cabin_core::LanguageStandardSettings::default(),
            links: None,
            conditional_sources: Vec::new(),
            instruction_sets: BTreeMap::new(),
        };
        stubs.push(Package::with_config(PackageConfigInput {
            name: name.clone(),
            version: resolved.version.clone(),
            targets: vec![target],
            dependencies: Vec::new(),
            system_dependencies: Vec::new(),
            features: Features {
                default: Vec::new(),
                features: meta
                    .features
                    .features
                    .keys()
                    .map(|feature| (feature.clone(), Vec::new()))
                    .collect(),
            },
        })?);
    }
    let unfetched = output
        .packages
        .iter()
        .filter(|p| p.source != ResolvedSource::Root && !fetched.contains(&p.name))
        .map(|p| p.name.clone())
        .collect();
    Ok(SystemSplit {
        unfetched,
        stubs,
        reaches,
    })
}

/// Refuse a package that is both `[patch]`ed and `[system-provided]`.
pub(crate) fn reject_patched(
    settings: &SystemProvidedSettings,
    patched_names: &BTreeSet<String>,
) -> Result<()> {
    if let Some(name) = settings
        .entries
        .keys()
        .find(|name| patched_names.contains(name.as_str()))
    {
        bail!(
            "`{name}` is both `[patch]`ed and `[system-provided]`; a package comes from one \
             of them"
        );
    }
    Ok(())
}

/// Probe each provided package `features` includes and return the
/// interfaces of their targets, keyed for
/// [`cabin_build::PlanRequest::system_targets`].
///
/// `build_included` is the build context's reach, which cannot take a
/// provided package yet; `reaches` is [`SystemSplit::reaches`].
///
/// # Errors
/// A provided package the graph has as a local package; a provided
/// package the build context reaches; an included provided
/// package that reaches a package the build also includes, since the
/// system library links its own copy beside the one Cabin builds; any
/// provided package under the MSVC dialect, whose tools cannot consume
/// pkg-config's GNU-style flags; and a probe failure, including a
/// system version outside a requirement of an active edge.
pub(crate) fn probe_provided(
    graph: &PackageGraph,
    features: &cabin_feature::FeatureResolution,
    build_included: &BTreeSet<usize>,
    reaches: &BTreeMap<PackageName, BTreeSet<PackageName>>,
    platform: &TargetPlatform,
    dialect: cabin_build::Dialect,
    reporter: Reporter,
) -> Result<HashMap<(usize, String), cabin_build::SystemTargetInterface>> {
    let included = &features.included;
    let settings = &graph.root_settings.system_provided;
    if let Some(pkg) = graph.packages.iter().find(|pkg| {
        pkg.kind == PackageKind::Local && settings.entries.contains_key(&pkg.package.name)
    }) {
        bail!(
            "`{}` is `[system-provided]`, but is a local package; only a registry package can \
             come from the system",
            pkg.package.name,
        );
    }
    let provided: Vec<usize> = (0..graph.packages.len())
        .filter(|idx| {
            let pkg = &graph.packages[*idx];
            pkg.kind == PackageKind::Registry && settings.entries.contains_key(&pkg.package.name)
        })
        .collect();
    if let Some(idx) = provided.iter().find(|idx| build_included.contains(idx)) {
        bail!(
            "`{}` is `[system-provided]`, but a build dependency reaches it; build dependencies \
             cannot come from the system yet",
            graph.packages[*idx].package.name,
        );
    }
    let provided: Vec<usize> = provided
        .into_iter()
        .filter(|idx| included.contains(idx))
        .collect();
    let mut interfaces = HashMap::new();
    if provided.is_empty() {
        return Ok(interfaces);
    }
    for &idx in &provided {
        let name = &graph.packages[idx].package.name;
        if let Some(built) = included
            .iter()
            .map(|&i| &graph.packages[i].package.name)
            .find(|built| reaches[name].contains(*built) && !settings.entries.contains_key(*built))
        {
            bail!(
                "`{name}` is `[system-provided]`, and the system library links its own \
                 `{built}`, but the build also builds `{built}`; provide `{built}` from the \
                 system too"
            );
        }
    }
    if dialect == cabin_build::Dialect::Msvc {
        bail!(
            "`{}` is `[system-provided]` through pkg-config, whose GNU-style flags the MSVC \
             backend cannot consume",
            graph.packages[provided[0]].package.name,
        );
    }
    let tool = crate::cli::system_deps::pkg_config_for(platform)?;
    for idx in provided {
        let package = &graph.packages[idx].package;
        let module = &settings.entries[&package.name].pkg_config;
        // The comparators of every active target-context edge onto the
        // package, which pkg-config checks against the module's
        // version.  A `*` contributes none.
        let mut comparators = BTreeSet::new();
        for &i in included {
            let dependent = &graph.packages[i];
            for edge in &dependent.deps {
                // The loader adds `Dev` edges only for the packages
                // whose dev dependencies this command activates.
                if edge.index != idx
                    || edge.kind == DependencyKind::Build
                    || !edge.matches_platform(platform)
                {
                    continue;
                }
                let enabled = &features.for_package(i).enabled_optional_deps;
                for dep in &dependent.package.dependencies {
                    if dep.name == package.name
                        && dep.kind == edge.kind
                        && dep.condition == edge.condition
                        && (!dep.optional || enabled.contains(dep.name.as_str()))
                        && let DependencySource::Version(req) = &dep.source
                    {
                        comparators.extend(req.comparators.iter().map(ToString::to_string));
                    }
                }
            }
        }
        let requirement = comparators.into_iter().collect::<Vec<_>>().join(", ");
        // The typed error renders with its own help; the module it
        // names is the policy's.
        let resolution = probe_system_dependency(&SystemDependencyProbeRequest {
            name: module,
            version_requirement: &requirement,
            tool: &tool,
        })?;
        reporter.aux_verbose(format_args!(
            "cabin: `{}` comes from pkg-config module `{module}` (version {})",
            package.name,
            resolution.version.as_deref().unwrap_or("unknown"),
        ));
        let flags = resolution.flags;
        // `-D` tokens stay among the compile arguments, in pkg-config's
        // order, so a later `-U` still undoes them.
        interfaces.insert(
            (idx, package.targets[0].name.as_str().to_owned()),
            cabin_build::SystemTargetInterface {
                include_dirs: flags.include_dirs,
                system_include_dirs: flags.system_include_dirs,
                defines: Vec::new(),
                compile_args: flags.extra_compile_args,
                link_args: flags.ldflags,
            },
        );
    }
    Ok(interfaces)
}
