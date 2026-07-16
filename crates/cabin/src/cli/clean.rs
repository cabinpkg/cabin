use super::{
    BTreeMap, BTreeSet, CleanArgs, Context, PathBuf, Reporter, Result, absolutise,
    build_workspace_selection, profile_selection_from_flags, resolve_invocation_manifest,
    workspace_profile_definitions,
};

pub(super) fn clean(args: &CleanArgs, reporter: Reporter) -> Result<()> {
    use cabin_build::clean::{CleanRequest, CleanScope, execute_clean, plan_clean};

    // Manifest discovery, build-dir resolution, and profile
    // selection share helpers with `cabin build` so the user
    // sees the same precedence rules across both commands.
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    // must never reach the network.  Foundation-port edges are
    // skipped so a fresh checkout with an HTTP-backed port (no
    // archive cached yet) still cleans without erroring.
    let graph = cabin_workspace::load_workspace_skip_ports(&manifest_path)?;
    let effective_config = crate::cli::config::load_effective_config(&graph)?;

    let (build_dir_input, _build_dir_source) = crate::cli::config::resolve_build_dir_with_env(
        args.build_dir.as_deref(),
        &effective_config,
    );
    let build_dir = absolutise(&build_dir_input)
        .with_context(|| format!("failed to resolve build dir {}", build_dir_input.display()))?;

    let package_roots: Vec<PathBuf> = graph
        .packages
        .iter()
        .map(|pkg| pkg.manifest_dir.clone())
        .collect();
    let protected_source_paths = clean_protected_source_paths(&graph);

    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;
    let selected_explicitly = !args.workspace_selection.package.is_empty()
        || !args.workspace_selection.exclude.is_empty();

    let profile_selection =
        profile_selection_from_flags(args.profile.as_deref(), args.release, &effective_config)?;
    let manifest_profiles = workspace_profile_definitions(&graph);
    let resolved_profile = cabin_core::resolve_profile(&profile_selection, &manifest_profiles)?;
    let profile_was_chosen = args.profile.is_some() || args.release;

    let scope = if selected_explicitly {
        let packages: Vec<cabin_core::PackageName> = resolved_selection
            .packages
            .iter()
            .map(|&idx| graph.packages[idx].package.name.clone())
            .collect();
        // `packages/<bare>` doubles as the scope directory of every
        // `<bare>/<name>` package, and removal is recursive: refuse
        // to clean a bare package whose name is also a scope in this
        // workspace, instead of silently deleting scoped outputs
        // that were never selected.  (Scoped residue from graphs no
        // longer loaded is out of reach of this check; cleaning the
        // profile or the whole build dir always works.)
        for pkg in &packages {
            if pkg.is_scoped() {
                continue;
            }
            if let Some(scoped) = graph
                .packages
                .iter()
                .find(|p| p.package.name.scope() == Some(pkg.as_str()))
            {
                anyhow::bail!(
                    "cannot clean package `{}`: its build directory `packages/{}` also holds \
                     the output of scoped package `{}`; clean the scoped package directly, or \
                     use `--profile` / no selection to clean a whole tree",
                    pkg.as_str(),
                    pkg.as_str(),
                    scoped.package.name.as_str(),
                );
            }
        }
        let profiles = if profile_was_chosen {
            vec![resolved_profile.name]
        } else {
            known_profile_names(&manifest_profiles)
        };
        CleanScope::Packages { profiles, packages }
    } else if profile_was_chosen {
        CleanScope::Profile(resolved_profile.name)
    } else {
        CleanScope::Whole
    };

    let plan = plan_clean(&CleanRequest {
        build_dir: &build_dir,
        workspace_root: &graph.root_dir,
        package_roots: &package_roots,
        protected_source_paths: &protected_source_paths,
        scope,
    })
    .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    if plan.removals.is_empty() {
        if args.dry_run {
            reporter.status(
                "Removed",
                format_args!("nothing under {} (dry-run)", build_dir.display()),
            );
        } else {
            reporter.status(
                "Removed",
                format_args!(
                    "nothing under {} (build directory does not exist)",
                    build_dir.display()
                ),
            );
        }
        return Ok(());
    }

    if args.dry_run {
        reporter.status(
            "Removed",
            format_args!(
                "{} path{} under {} (dry-run; re-run without --dry-run to apply)",
                plan.removals.len(),
                crate::plural(plan.removals.len()),
                build_dir.display(),
            ),
        );
        print_plan_paths(&plan, reporter);
        return Ok(());
    }

    let report = execute_clean(&plan).map_err(|err| anyhow::anyhow!(err.to_string()))?;
    reporter.status(
        "Removed",
        format_args!(
            "{} path{} under {}",
            report.removed.len(),
            crate::plural(report.removed.len()),
            build_dir.display()
        ),
    );
    Ok(())
}

pub(super) fn clean_protected_source_paths(graph: &cabin_workspace::PackageGraph) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for pkg in &graph.packages {
        for target in &pkg.package.targets {
            paths.extend(
                target
                    .sources
                    .iter()
                    .map(|source| pkg.manifest_dir.join(source)),
            );
            paths.extend(
                target
                    .include_dirs
                    .iter()
                    .map(|include_dir| pkg.manifest_dir.join(include_dir)),
            );
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

pub(super) fn print_plan_paths(plan: &cabin_build::clean::CleanPlan, reporter: Reporter) {
    // Dry-run plan enumeration is the user-requested payload of
    // `cabin clean --dry-run`.  Routed through `Reporter::note`
    // so it stays visible at default verbosity, paired with the
    // `Removed … (dry-run)` banner above, and disappears
    // alongside the banner under `--quiet`.
    for path in &plan.removals {
        reporter.note(format_args!("  {}", path.display()));
    }
}

/// Names of every profile this workspace knows about: the two
/// built-ins (`dev`, `release`) plus every user-declared
/// `[profile.<name>]` table on the workspace root manifest.
/// The set is sorted and deduplicated so the resulting clean
/// scope is stable across invocations.
pub(super) fn known_profile_names(
    manifest_profiles: &BTreeMap<cabin_core::ProfileName, cabin_core::ProfileDefinition>,
) -> Vec<cabin_core::ProfileName> {
    let mut out: BTreeSet<cabin_core::ProfileName> = BTreeSet::new();
    for builtin in cabin_core::BuiltinProfile::all() {
        out.insert(cabin_core::ProfileName::builtin(builtin));
    }
    for name in manifest_profiles.keys() {
        out.insert(name.clone());
    }
    out.into_iter().collect()
}
