//! Glue layer for `cabin explain`.
//!
//! Package, target, source, and feature subcommands map onto
//! the typed explanation model in `cabin-explain`. The
//! orchestration layer here is responsible for loading the
//! workspace + lockfile + active patches + source-replacement
//! table + (for `build-config`) the full profile / toolchain /
//! build-config preamble, then handing typed inputs to the
//! owning crates.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use crate::cli::{
    ConfigSelectionArgs, ResolveFormat, ToolchainSelectionArgs, WorkspaceSelectionArgs,
    augment_build_flags, build_selection_request, build_workspace_selection,
    compiler_wrapper_override_from_args, compute_feature_resolution, lockfile_path_for,
    profile_selection_for_metadata, resolve_build_configurations, resolve_invocation_manifest,
    resolve_per_package_build_flags, toolchain_selection_from_args,
    workspace_compiler_wrapper_settings, workspace_profile_definitions,
};

#[derive(Debug, Args)]
pub(crate) struct ExplainArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Output format. `human` is a concise summary (the
    /// default); `json` is a structured document for tooling.
    #[arg(long, value_name = "FORMAT", default_value = "human", global = true)]
    pub format: ResolveFormat,

    /// Profile to evaluate, when the explanation depends on the
    /// build configuration (`build-config`). Defaults to `dev`.
    #[arg(long, value_name = "NAME", global = true)]
    pub profile: Option<String>,

    /// Toolchain-selection flags. Same precedence rules as
    /// `cabin build`.
    #[command(flatten)]
    pub toolchain: ToolchainSelectionArgs,

    /// Feature selection flags.
    #[command(flatten)]
    pub selection: ConfigSelectionArgs,

    /// Workspace package-selection flags. Restrict the closure
    /// the explanation considers (the same `--package` /
    /// `--workspace` flags `cabin metadata` accepts).
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Disable every active patch / source-replacement entry.
    #[arg(long, global = true)]
    pub no_patches: bool,

    #[command(subcommand)]
    pub command: ExplainCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ExplainCommand {
    /// Explain why a package is selected and which selected root
    /// pulls it in.
    Package {
        /// Package name to explain.
        name: String,
    },
    /// Explain a target's owning package, kind, deps, and
    /// language summary.
    Target {
        /// Target name to explain.
        name: String,
    },
    /// Explain where a package's source bytes come from.
    Source {
        /// Package name to explain.
        name: String,
    },
    /// Explain a feature's enablement on the named package.
    Feature {
        /// Query in the form `package/feature`.
        query: String,
    },
    /// Explain the resolved [`cabin_core::BuildConfiguration`]
    /// for a package (profile, toolchain, flags, options,
    /// variants, condition trace, fingerprint).
    BuildConfig {
        /// Package name to explain.
        name: String,
    },
}

pub(crate) fn explain(
    args: &ExplainArgs,
    reporter: crate::term_verbosity_glue::Reporter,
) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    // `cabin explain` is read-only inspection: never auto-
    // download foundation ports. The cache short-circuit serves
    // an already-prepared workspace.
    let explain_selection = build_workspace_selection(&args.workspace_selection);
    let (prepared_ports, initial_graph) = crate::port_glue::prepare_ports_and_load_initial_graph(
        &manifest_path,
        None,
        true,
        false,
        false,
        &explain_selection,
        args.no_patches,
    )?;
    let port_sources: Vec<cabin_workspace::PortPackageSource> = prepared_ports
        .iter()
        .map(crate::port_glue::workspace_source)
        .collect();
    let effective_config = crate::config_glue::load_effective_config(&initial_graph)?;
    let active_patches =
        crate::patch_glue::load_active_patches(&initial_graph, &effective_config, args.no_patches)?;
    let patched_sources = active_patches.workspace_sources();
    let graph = crate::patch_glue::reload_for_patches(
        &manifest_path,
        initial_graph,
        &patched_sources,
        &port_sources,
    )?;

    let lockfile_path = lockfile_path_for(&manifest_path);
    let lockfile = if lockfile_path.is_file() {
        Some(
            cabin_lockfile::read_lockfile(&lockfile_path)
                .with_context(|| format!("failed to read {}", lockfile_path.display()))?,
        )
    } else {
        None
    };

    let request = build_selection_request(
        &args.selection.features,
        args.selection.all_features,
        args.selection.no_default_features,
    );
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;
    let feature_resolution = compute_feature_resolution(&graph, &resolved_selection, &request)?;

    let explanation = match &args.command {
        ExplainCommand::Package { name } => {
            let exp = cabin_explain::explain_package(
                &graph,
                &resolved_selection.packages,
                name,
                Some(&active_patches),
                lockfile.as_ref(),
            )?;
            cabin_explain::Explanation::Package(exp)
        }
        ExplainCommand::Target { name } => {
            let exp = cabin_explain::explain_target(&graph, &resolved_selection.packages, name)?;
            cabin_explain::Explanation::Target(exp)
        }
        ExplainCommand::Source { name } => {
            let exp = cabin_explain::explain_source(
                &graph,
                name,
                Some(&active_patches),
                lockfile.as_ref(),
                &effective_config.source_replacements,
            )?;
            cabin_explain::Explanation::Source(exp)
        }
        ExplainCommand::Feature { query } => {
            // Build a per-package feature view limited to the
            // package the user named. We look up the package up
            // front so we can map its enabled features into the
            // typed view the cabin-explain crate consumes.
            let pkg_name = query
                .split_once('/')
                .map(|(p, _)| p.to_owned())
                .unwrap_or_else(|| query.clone());
            let view = if let Some(idx) = graph.index_of(&pkg_name) {
                let enabled = feature_resolution.for_package(idx).enabled_features.clone();
                Some(cabin_explain::cabin_feature_per_package_view::FeatureView { enabled })
            } else {
                None
            };
            let exp = cabin_explain::explain_feature(&graph, view.as_ref(), query)?;
            cabin_explain::Explanation::Feature(exp)
        }
        ExplainCommand::BuildConfig { name } => {
            // Build-config explanations need the same preamble
            // as `cabin metadata`. We compute it inline rather
            // than refactoring `metadata()` itself so the
            // existing path stays untouched.
            let manifest_profiles = workspace_profile_definitions(&graph);
            let profile_selection =
                profile_selection_for_metadata(args.profile.as_deref(), &effective_config)?;
            let profile = cabin_core::resolve_profile(&profile_selection, &manifest_profiles)
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            let host_platform = cabin_core::TargetPlatform::current();
            let toolchain_selection = toolchain_selection_from_args(&args.toolchain)?;
            let toolchain = crate::cli::resolve_toolchain_layered(
                &graph,
                &toolchain_selection,
                &effective_config,
                &host_platform,
            )?;
            let manifest_compiler_wrapper = workspace_compiler_wrapper_settings(&graph);
            let cli_compiler_wrapper = compiler_wrapper_override_from_args(&args.toolchain)?;
            let compiler_wrapper = crate::cli::resolve_compiler_wrapper_layered(
                cli_compiler_wrapper,
                &manifest_compiler_wrapper,
                &effective_config,
                &host_platform,
            )?;
            let toolchain_summary = cabin_core::ToolchainSummary::from_resolved_parts(
                &toolchain,
                compiler_wrapper.as_ref(),
            );
            let profile_build = profile.build.as_ref();
            let build_flags =
                resolve_per_package_build_flags(&graph, profile_build, &host_platform);
            // `cabin explain` does not opt into dev-dep
            // activation; dev-kind system deps stay
            // declaration-only here.
            let dev_for: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            let build_flags =
                augment_build_flags(&graph, &host_platform, &dev_for, build_flags, reporter)?;
            let configurations = resolve_build_configurations(
                &graph,
                &request,
                &resolved_selection.packages,
                &profile,
                &toolchain_summary,
                &build_flags,
            )?;
            let config = cabin_explain::explain_build_config(&configurations, &graph, name)?;
            // BuildConfiguration already has its own JSON shape
            // documented by `cabin metadata`. We render it
            // directly rather than wrapping it in our `Explanation`
            // enum so users see exactly the same shape they see
            // in metadata's `configuration` blocks.
            return render_build_config(args.format, name, config);
        }
    };

    match args.format {
        ResolveFormat::Human => {
            let rendered = cabin_explain::render_explanation_human(&explanation);
            print!("{rendered}");
        }
        ResolveFormat::Json => {
            let value = cabin_explain::render_explanation_json(&explanation);
            crate::print_pretty_json(&value, "failed to serialize explanation as JSON")?;
        }
    }
    Ok(())
}

fn render_build_config(
    format: ResolveFormat,
    name: &str,
    config: &cabin_core::BuildConfiguration,
) -> Result<()> {
    match format {
        ResolveFormat::Json => {
            let mut map = serde_json::Map::new();
            map.insert(
                "kind".to_owned(),
                serde_json::Value::String("build-config".to_owned()),
            );
            map.insert(
                "package".to_owned(),
                serde_json::Value::String(name.to_owned()),
            );
            map.insert("configuration".to_owned(), config.as_json());
            crate::print_pretty_json(
                &serde_json::Value::Object(map),
                "failed to serialize build-config explanation",
            )?;
        }
        ResolveFormat::Human => {
            println!("package: {name}");
            println!("profile: {}", config.profile.name);
            println!("fingerprint: {}", config.fingerprint);
            // Stay terse — JSON is the contract for tooling.
        }
    }
    Ok(())
}
