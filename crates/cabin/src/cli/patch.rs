//! Glue between [`cabin_config::EffectiveConfig`],
//! [`cabin_workspace::resolve_active_patches`], and the rest of
//! the CLI's command pipeline.
//!
//! Discovery, parsing, and merging live in `cabin-config` and
//! `cabin-workspace`.  This module owns the small amount of
//! *orchestration* the CLI needs:
//!
//! - convert the merged effective config into a typed input for
//!   [`cabin_workspace::resolve_active_patches`];
//! - resolve the source-replacement chain (with cycle detection)
//!   for whichever index source the CLI / config picked;
//! - build the lockfile records that capture active patch and
//!   source-replacement state for stale-detection;
//! - render a deterministic JSON block for `cabin metadata`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use cabin_config::EffectiveConfig;
use cabin_core::{
    PackageName, PatchProvenance, SourceLocator, SourceReplacementResolution,
    SourceReplacementSettings,
};
use cabin_lockfile::{
    LockedPatch, LockedPatchKind, LockedSourceLocatorKind, LockedSourceReplacement,
};
use cabin_workspace::{
    ActivePatchSet, ConfigPatchInput, PackageGraph, PatchResolutionInputs, resolve_active_patches,
};

/// Reload the workspace graph with active patches applied so each
/// member manifest path points at its patched working copy.
///
/// When no patch is active the original `initial_graph` is returned
/// untouched; otherwise the workspace is reloaded with an empty
/// registry, an empty strict set, and no dev edges - the read-only
/// contract the inspection commands (`metadata` / `tree` /
/// `explain`) share.
pub(crate) fn reload_for_patches(
    manifest_path: &std::path::Path,
    initial_graph: PackageGraph,
    patched_sources: &[cabin_workspace::PatchedPackageSource],
    port_sources: &[cabin_workspace::PortPackageSource],
) -> Result<PackageGraph> {
    if patched_sources.is_empty() {
        return Ok(initial_graph);
    }
    let strict_packages: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    Ok(cabin_workspace::load_workspace_with_options(
        manifest_path,
        &cabin_workspace::WorkspaceLoadOptions {
            registry: &[],
            patches: patched_sources,
            ports: port_sources,
            registry_policy: cabin_workspace::RegistryPolicy::StrictFor(&strict_packages),
            include_dev_for: &std::collections::BTreeSet::new(),
            port_policy: cabin_workspace::PortPolicy::TolerateExcept(&strict_packages),
        },
    )?)
}

/// Build the patch-resolution input the workspace layer
/// consumes.  Returns `None` and an empty active patch set when
/// `--no-patches` is set; otherwise the manifest-declared
/// patches plus the merged config-derived patches feed
/// [`resolve_active_patches`].
pub(crate) fn load_active_patches(
    graph: &PackageGraph,
    effective_config: &EffectiveConfig,
    no_patches: bool,
) -> Result<ActivePatchSet> {
    if no_patches {
        return Ok(ActivePatchSet::default());
    }
    let manifest_patches = graph.root_settings.patches.clone();
    let mut config_patches: BTreeMap<PackageName, ConfigPatchInput> = BTreeMap::new();
    for (name, entry) in &effective_config.patches {
        config_patches.insert(
            name.clone(),
            ConfigPatchInput {
                source: entry.spec.clone(),
                provenance: PatchProvenance::Config(super::config::config_value_source(
                    entry.source,
                )),
                declared_in: entry.declared_in.as_std_path().to_path_buf(),
            },
        );
    }
    let inputs = PatchResolutionInputs {
        graph,
        manifest_patches: &manifest_patches,
        config_patches: &config_patches,
    };
    let resolved = resolve_active_patches(&inputs).map_err(|err| anyhow!(err.to_string()))?;
    Ok(resolved)
}

/// Apply the source-replacement chain to `initial`.  Returns the
/// terminal source plus the chain hops so callers can record
/// them in the lockfile / metadata view. `--no-patches` disables
/// the entire local-policy layer, including source replacement.
pub(crate) fn apply_source_replacement(
    initial: SourceLocator,
    effective_config: &EffectiveConfig,
    no_patches: bool,
) -> Result<SourceReplacementResolution> {
    if no_patches {
        return Ok(SourceReplacementResolution {
            resolved: initial,
            hops: Vec::new(),
        });
    }
    effective_config
        .source_replacements
        .resolve(&initial)
        .map_err(|err| anyhow!(err.to_string()))
}

pub(crate) fn lockfile_patches(set: &ActivePatchSet) -> Vec<LockedPatch> {
    let mut out: Vec<LockedPatch> = set
        .iter()
        .map(|entry| LockedPatch {
            package: entry.name.clone(),
            version: entry.package.version.clone(),
            kind: LockedPatchKind::Path,
            provenance: entry.provenance.as_key(),
            path: entry.declared_path.clone(),
        })
        .collect();
    out.sort_by(|a, b| {
        a.package
            .as_str()
            .cmp(b.package.as_str())
            .then_with(|| a.version.cmp(&b.version))
    });
    out
}

pub(crate) fn lockfile_source_replacements(
    settings: &SourceReplacementSettings,
    no_patches: bool,
) -> Vec<LockedSourceReplacement> {
    if no_patches {
        return Vec::new();
    }
    let mut out: Vec<LockedSourceReplacement> = settings
        .entries
        .values()
        .map(|entry| LockedSourceReplacement {
            original: entry.original.display(),
            original_kind: locator_to_lock_kind(&entry.original),
            replacement: entry.replacement.display(),
            replacement_kind: locator_to_lock_kind(&entry.replacement),
            provenance: entry.provenance.as_key().to_owned(),
        })
        .collect();
    out.sort_by(|a, b| a.original.cmp(&b.original));
    out
}

fn locator_to_lock_kind(locator: &SourceLocator) -> LockedSourceLocatorKind {
    match locator {
        SourceLocator::IndexPath { .. } => LockedSourceLocatorKind::IndexPath,
        SourceLocator::IndexUrl { .. } => LockedSourceLocatorKind::IndexUrl,
    }
}

/// JSON view of the active patch set.  Returned as a sorted
/// array so consumers can rely on stable ordering.
pub(crate) fn patch_view_json(set: &ActivePatchSet) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = set
        .iter()
        .map(|entry| {
            serde_json::json!({
                "package": entry.name.as_str(),
                "version": entry.package.version.to_string(),
                "kind": entry.source.kind().as_key(),
                "path": entry.declared_path.as_str(),
                "provenance": entry.provenance.as_key(),
            })
        })
        .collect();
    serde_json::Value::Array(entries)
}

/// JSON view of the active source-replacement entries.
pub(crate) fn source_replacement_view_json(
    settings: &SourceReplacementSettings,
    no_patches: bool,
) -> serde_json::Value {
    if no_patches {
        return serde_json::Value::Array(Vec::new());
    }
    let entries: Vec<serde_json::Value> = settings
        .entries
        .values()
        .map(|entry| {
            serde_json::json!({
                "original": entry.original.display(),
                "original_kind": entry.original.kind_key(),
                "replacement": entry.replacement.display(),
                "replacement_kind": entry.replacement.kind_key(),
                "provenance": entry.provenance.as_key(),
            })
        })
        .collect();
    serde_json::Value::Array(entries)
}

/// Package a typed [`SourceLocator`] back into the
/// `(index_path, index_url)` shape Cabin's existing artifact
/// pipeline expects.  The two values are mutually exclusive - at
/// most one is `Some`.
pub(crate) fn locator_to_index_inputs(
    locator: &SourceLocator,
) -> (Option<PathBuf>, Option<String>) {
    match locator {
        SourceLocator::IndexPath { path } => (Some(path.as_std_path().to_path_buf()), None),
        SourceLocator::IndexUrl { url } => (None, Some(url.clone())),
    }
}
