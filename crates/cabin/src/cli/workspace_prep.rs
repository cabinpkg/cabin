//! Shared workspace loading for the commands that operate on a
//! resolved graph.
//!
//! Loading is two-pass because `[patch]` entries are themselves
//! resolved against a graph: the first pass reads the unpatched
//! workspace so the patch table can be evaluated, and the second
//! re-loads with the patched manifests linked in, so member paths
//! point at the patched working copies rather than the upstream
//! packages.  Patches are then re-resolved against the final graph,
//! which carries version requirements the first pass could not see.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;
use cabin_workspace::PackageGraph;

/// The loaded workspace plus the inputs every consumer needs
/// alongside it.
pub(crate) struct WorkspacePrep {
    pub effective_config: cabin_config::EffectiveConfig,
    /// Active patches resolved against the final `graph`, so the
    /// patched-version requirement validation sees the same edges
    /// the command itself operates on.
    pub active_patches: cabin_workspace::ActivePatchSet,
    pub graph: PackageGraph,
}

/// Load the workspace with its `[patch]` table applied.
///
/// # Errors
/// Returns manifest, workspace-load, config-discovery, or patch
/// resolution failures.
pub(crate) fn load_workspace_and_patches(
    manifest_path: &Path,
    no_patches: bool,
) -> Result<WorkspacePrep> {
    let unpatched = cabin_workspace::load_workspace(manifest_path)?;
    // Graph-keyed config discovery: the loader canonicalizes the
    // manifest path, so under a symlinked `--manifest-path` the
    // graph's root dir (and therefore the discovered workspace
    // config files) can differ from the raw manifest parent.  Both
    // loads share the same canonical root, so one discovery serves
    // the first patch pass, the final patch resolution, and the
    // returned bundle.
    let effective_config = crate::cli::config::load_effective_config(&unpatched)?;
    let patches =
        crate::cli::patch::load_active_patches(&unpatched, &effective_config, no_patches)?;
    let patched_sources = patches.workspace_sources();
    let graph = if patched_sources.is_empty() {
        unpatched
    } else {
        cabin_workspace::load_workspace_with_options(
            manifest_path,
            &cabin_workspace::WorkspaceLoadOptions {
                registry: &[],
                patches: &patched_sources,
                registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&BTreeSet::new()),
                include_dev_for: &BTreeSet::new(),
            },
        )?
    };
    // Re-resolve against the final graph: patched manifests
    // contribute version requirements the first pass could not see.
    let active_patches =
        crate::cli::patch::load_active_patches(&graph, &effective_config, no_patches)?;
    Ok(WorkspacePrep {
        effective_config,
        active_patches,
        graph,
    })
}
