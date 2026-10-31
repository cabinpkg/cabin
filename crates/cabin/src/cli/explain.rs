//! Glue layer for `cabin explain`.
//!
//! Package, target, source, and feature subcommands map onto
//! the typed explanation model in `cabin-explain`.  The
//! orchestration layer here is responsible for loading the
//! workspace + lockfile + active patches + source-replacement
//! table + (for `build-config`) the full profile / toolchain /
//! build-config preamble, then handing typed inputs to the
//! owning crates.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::cli::{
    ConfigSelectionArgs, ResolveFormat, ToolchainSelectionArgs, WorkspaceSelectionArgs,
    build_selection_request, build_workspace_selection, compiler_wrapper_override_from_args,
    compute_feature_resolution, lockfile_path_for, read_optional_lockfile,
    resolve_build_configurations, resolve_invocation_manifest, workspace_compiler_wrapper_settings,
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
    /// build configuration (`build-config`).  Defaults to `dev`.
    #[arg(long, value_name = "NAME", global = true)]
    pub profile: Option<String>,

    /// Toolchain-selection flags.  Same precedence rules as
    /// `cabin build`.
    #[command(flatten)]
    pub toolchain: ToolchainSelectionArgs,

    /// Feature selection flags.
    #[command(flatten)]
    pub selection: ConfigSelectionArgs,

    /// Workspace package-selection flags.  Restrict the closure
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
        #[arg(value_parser = crate::cli::parse_package_name)]
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
        #[arg(value_parser = crate::cli::parse_package_name)]
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
        #[arg(value_parser = crate::cli::parse_package_name)]
        name: String,
    },
}

pub(crate) fn explain(
    args: &ExplainArgs,
    reporter: crate::cli::term_verbosity::Reporter,
) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let explain_selection = build_workspace_selection(&args.workspace_selection);
    let crate::cli::workspace_prep::WorkspacePrep {
        effective_config,
        active_patches,
        graph,
    } = crate::cli::workspace_prep::load_workspace_and_patches(&manifest_path, args.no_patches)?;

    let lockfile_path = lockfile_path_for(&manifest_path);
    let lockfile = read_optional_lockfile(&lockfile_path)?;

    let request = build_selection_request(
        &args.selection.features,
        args.selection.all_features,
        args.selection.no_default_features,
    );
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &explain_selection)?;
    let platform = args.toolchain.platform();
    let feature_resolution = compute_feature_resolution(
        &graph,
        &resolved_selection,
        &request,
        Some(&platform),
        &std::collections::BTreeSet::new(),
    )?;
    let included = crate::cli::included_in_either_context(&graph, &feature_resolution, &platform)?;

    let explanation = match &args.command {
        ExplainCommand::Package { name } => {
            let exp = cabin_explain::explain_package(
                &graph,
                &resolved_selection.packages,
                name,
                Some(&active_patches),
                lockfile.as_ref(),
                Some(&included),
                Some(&platform),
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
            // Split on the last `/` to mirror `explain_feature`:
            // the package part of the query may itself be scoped.
            let pkg_name = query
                .rsplit_once('/')
                .map_or_else(|| query.clone(), |(p, _)| p.to_owned());
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
            let inspection = crate::cli::build_prep::resolve_inspection_prep(
                &graph,
                args.profile.as_deref(),
                &args.toolchain,
                &platform,
                &effective_config,
                reporter,
            )?;
            let manifest_compiler_wrapper = workspace_compiler_wrapper_settings(&graph);
            let cli_compiler_wrapper = compiler_wrapper_override_from_args(&args.toolchain)?;
            // `cabin explain` does not opt into dev-dep activation;
            // dev-kind system deps stay declaration-only here.
            let dev_for: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            let prep = crate::cli::build_prep::resolve_build_prep(
                crate::cli::build_prep::BuildConfigInputs {
                    graph: &graph,
                    platform: &platform,
                    target: args.toolchain.target.as_ref().map(|t| t.target.as_str()),
                    sysroot: inspection
                        .sysroot
                        .as_ref()
                        .and_then(|choice| choice.written.as_deref()),
                    toolchain: &inspection.toolchain,
                    detection: inspection.detection.as_ref(),
                    cli_compiler_wrapper,
                    manifest_compiler_wrapper: manifest_compiler_wrapper.as_ref(),
                    effective_config: &effective_config,
                    profile: &inspection.profile,
                    dev_for: &dev_for,
                    feature_resolution: &feature_resolution,
                    reporter,
                },
            )?;
            let configurations = resolve_build_configurations(
                &graph,
                &feature_resolution,
                &inspection.profile,
                &prep.toolchain_summary,
                &prep.build_flags,
            )?;
            let config = cabin_explain::explain_build_config(&configurations, &graph, name)?;
            // Rendered outside `Explanation` so the JSON matches
            // the `configuration` blocks of `cabin metadata`.
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
            if let Some(c) = &config.language.c {
                println!("c standard: {} ({})", c.standard, c.source.as_key());
            }
            if let Some(cxx) = &config.language.cxx {
                println!("cxx standard: {} ({})", cxx.standard, cxx.source.as_key());
            }
            println!("fingerprint: {}", config.fingerprint);
            // Stay terse - JSON is the contract for tooling.
        }
    }
    Ok(())
}
