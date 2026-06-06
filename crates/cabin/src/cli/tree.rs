//! Glue layer for `cabin tree`.
//!
//! `cabin tree` walks the same resolved [`cabin_workspace::PackageGraph`] +
//! lockfile + active-patch state that `cabin metadata` already
//! exposes, and renders it either as a Unicode-drawing tree
//! (`--format human`, the default) or as a JSON document
//! (`--format json`). All domain logic lives in `cabin-explain`;
//! this module orchestrates the typed inputs.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, ValueEnum};

use cabin_core::DependencyKind;

use crate::cli::{
    ConfigSelectionArgs, ResolveFormat, WorkspaceSelectionArgs, build_selection_request,
    build_workspace_selection, compute_feature_resolution, lockfile_path_for,
    read_optional_lockfile, resolve_invocation_manifest,
};

/// Dependency-kind filter used by the `--kind` flag.
//
// Dev edges are intentionally not exposed here: tree/explain build their
// view through the ordinary workspace loader, which keeps dev deps
// declaration-only — only `cabin run` / `cabin test` opt them into the
// graph. A `--kind dev` filter would walk an empty edge set.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub(crate) enum TreeKindFilter {
    /// Walk every kind (default).
    All,
    /// `dependencies` edges only.
    Normal,
}

impl TreeKindFilter {
    fn to_filter(self) -> Option<DependencyKind> {
        match self {
            Self::All => None,
            Self::Normal => Some(DependencyKind::Normal),
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct TreeArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Output format. `human` is a Unicode-drawing tree (the
    /// default); `json` is a structured document for tooling.
    #[arg(long, value_name = "FORMAT", default_value = "human")]
    pub format: ResolveFormat,

    /// Restrict the walk to one dependency kind. Defaults to
    /// every kind.
    #[arg(long, value_name = "KIND", default_value = "all")]
    pub kind: TreeKindFilter,

    /// Workspace package-selection flags. Same semantics as
    /// `cabin metadata` and `cabin build`.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Feature selection flags.
    #[command(flatten)]
    pub selection: ConfigSelectionArgs,

    /// Disable every active patch and source-replacement entry
    /// for this invocation, mirroring `cabin metadata
    /// --no-patches`.
    #[arg(long)]
    pub no_patches: bool,
}

pub(crate) fn tree(args: &TreeArgs) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    // `cabin tree` is read-only inspection: never auto-download
    // foundation ports. The cache short-circuit still lets a
    // workspace whose ports were prepared by an earlier `cabin
    // build` run unchanged.
    let tree_selection = build_workspace_selection(&args.workspace_selection);
    let (prepared_ports, initial_graph) = crate::cli::port::prepare_ports_and_load_initial_graph(
        &manifest_path,
        None,
        true,
        false,
        false,
        &tree_selection,
        args.no_patches,
    )?;
    let port_sources: Vec<cabin_workspace::PortPackageSource> = prepared_ports
        .iter()
        .map(crate::cli::port::workspace_source)
        .collect();
    let effective_config = crate::cli::config::load_effective_config(&initial_graph)?;
    let active_patches =
        crate::cli::patch::load_active_patches(&initial_graph, &effective_config, args.no_patches)?;
    let patched_sources = active_patches.workspace_sources();
    let graph = crate::cli::patch::reload_for_patches(
        &manifest_path,
        initial_graph,
        &patched_sources,
        &port_sources,
    )?;

    let lockfile_path = lockfile_path_for(&manifest_path);
    let lockfile = read_optional_lockfile(&lockfile_path)?;

    // Run the same selection / feature resolver `cabin metadata`
    // runs so unknown features / `dep:` errors surface here too.
    let request = build_selection_request(
        &args.selection.features,
        args.selection.all_features,
        args.selection.no_default_features,
    );
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&graph, &workspace_selection)?;
    let _feature_resolution = compute_feature_resolution(&graph, &resolved_selection, &request)?;

    let inputs = cabin_explain::TreeInputs {
        graph: &graph,
        roots: &resolved_selection.packages,
        lockfile: lockfile.as_ref(),
        active_patches: Some(&active_patches),
        kind_filter: args.kind.to_filter(),
    };
    let forest = cabin_explain::build_tree(&inputs);

    match args.format {
        ResolveFormat::Human => {
            let rendered = cabin_explain::render_tree_human(&forest);
            print!("{rendered}");
        }
        ResolveFormat::Json => {
            let value = cabin_explain::render_tree_json(&forest);
            crate::print_pretty_json(&value, "failed to serialize tree as JSON")?;
        }
    }
    Ok(())
}
