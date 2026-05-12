//! Orchestration glue for `cabin vendor`.
//!
//! The command resolves the selected external registry
//! dependency closure through the existing artifact pipeline
//! (workspace load → patch / source-replacement → resolver →
//! fetch into the artifact cache), then asks `cabin-vendor` to
//! materialise a
//! deterministic file-registry directory at `--vendor-dir`.
//!
//! The output is a Cabin file registry whose layout the rest of
//! the read path already understands. To consume it offline,
//! point any subsequent command at the directory:
//!
//! ```text
//! cabin vendor                                      # populate ./vendor
//! cabin build  --offline --index-path ./vendor
//! ```
//!
//! `cabin test --offline --index-path ./vendor` is valid only
//! when the selected tests do not introduce additional
//! registry-backed dev dependencies; `cabin vendor` currently
//! mirrors the ordinary build closure.
//!
//! This module is orchestration only. Resolution lives in
//! `cabin-resolver`, the artifact pipeline lives in `cabin-cli`'s
//! existing `run_artifact_pipeline` helper, and the deterministic
//! write is owned by `cabin-vendor`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;

use cabin_artifact::FetchedPackage;
use cabin_vendor::{
    DEFAULT_VENDOR_DIRNAME, VendorEntry, VendorOptions, VendorPlan,
    materialise as vendor_materialise,
};
use cabin_workspace::collect_patched_versioned_deps;

use crate::cli::{
    ArtifactPipelineRequest, WorkspaceSelectionArgs, absolutise, build_selection_request,
    build_workspace_selection, cache_dir_for, closure_has_versioned_deps_excluding_patches,
    compute_feature_resolution, lock_mode_for_flags, resolve_invocation_manifest,
    run_artifact_pipeline,
};
use crate::plural;

/// `cabin vendor` arguments. Mirrors the flag surface of
/// `cabin fetch` because the two commands share the workspace /
/// patch / index / cache preamble.
#[derive(Debug, Args)]
pub(crate) struct VendorArgs {
    /// Path to the cabin.toml manifest.
    #[arg(long, value_name = "PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Output directory for the vendored file registry.
    /// Defaults to `vendor` next to the workspace root.
    #[arg(long, value_name = "PATH")]
    pub vendor_dir: Option<PathBuf>,

    /// Path to a directory containing the local JSON package
    /// index. Required when the manifest declares any versioned
    /// dependencies. `cabin vendor` reads per-package metadata
    /// directly off disk to build a byte-stable vendor directory,
    /// so the index source must be local.
    #[arg(long, value_name = "PATH")]
    pub index_path: Option<PathBuf>,

    /// Override the default artifact cache directory.
    #[arg(long, value_name = "PATH")]
    pub cache_dir: Option<PathBuf>,

    /// Require an existing, current `cabin.lock`.
    #[arg(long, conflicts_with = "frozen")]
    pub locked: bool,

    /// Like `--locked`, but also rejects state-writing side
    /// effects on the lockfile and the artifact cache. The
    /// vendor directory is the explicit user-requested output
    /// of the command and is still written under `--frozen`.
    #[arg(long)]
    pub frozen: bool,

    /// Forbid network access. Cabin refuses to use an HTTP
    /// index URL (`--index-url` or a `[registry] index-url`
    /// config setting) and expects every needed artifact to
    /// already be available in the artifact cache.
    #[arg(long)]
    pub offline: bool,

    /// Workspace package-selection flags.
    #[command(flatten)]
    pub workspace_selection: WorkspaceSelectionArgs,

    /// Enable named features for the selected packages.
    #[arg(long, value_name = "FEATURES")]
    pub features: Vec<String>,

    /// Enable every declared feature on selected packages.
    #[arg(long)]
    pub all_features: bool,

    /// Disable each selected package's default features.
    #[arg(long)]
    pub no_default_features: bool,

    /// Disable every active patch and source-replacement entry
    /// for this invocation.
    #[arg(long)]
    pub no_patches: bool,
}

/// Run `cabin vendor`: resolve the selected external registry
/// dependency closure, fetch its archives into the artifact
/// cache, then materialise a deterministic file-registry
/// directory at `--vendor-dir`.
pub(crate) fn vendor(
    args: &VendorArgs,
    reporter: crate::term_verbosity_glue::Reporter,
) -> Result<()> {
    let manifest_path = resolve_invocation_manifest(args.manifest_path.as_deref())?;
    let initial_graph = cabin_workspace::load_workspace(&manifest_path)?;
    let effective_config = crate::config_glue::load_effective_config(&initial_graph)?;
    let active_patches =
        crate::patch_glue::load_active_patches(&initial_graph, &effective_config, args.no_patches)?;
    let patched_names = active_patches.owned_patched_names();

    // Compute the resolved selection so we can scope the index
    // requirement to the user's chosen packages, exactly like
    // `cabin fetch` does.
    let workspace_selection = build_workspace_selection(&args.workspace_selection);
    let resolved_selection =
        cabin_workspace::resolve_package_selection(&initial_graph, &workspace_selection)?;
    let selection_request =
        build_selection_request(&args.features, args.all_features, args.no_default_features);
    let initial_features =
        compute_feature_resolution(&initial_graph, &resolved_selection, &selection_request)?;

    let dev_for: BTreeSet<String> = BTreeSet::new();
    let patched_root_deps_preview =
        collect_patched_versioned_deps(&active_patches, &patched_names)?;
    let has_versioned = !patched_root_deps_preview.is_empty()
        || closure_has_versioned_deps_excluding_patches(
            &initial_graph,
            &resolved_selection,
            &initial_features,
            &patched_names,
            &dev_for,
        );

    let vendor_dir = resolve_vendor_dir(args, &manifest_path)?;

    if !has_versioned {
        // Empty plan: still write the file-registry skeleton
        // and the summary so a follow-up `cabin build
        // --offline --index-path ./vendor` has a valid target.
        let plan = VendorPlan::default();
        let report = vendor_materialise(
            &plan,
            &vendor_dir,
            &VendorOptions {
                frozen: args.frozen,
            },
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        emit_vendor_summary(&report, reporter);
        return Ok(());
    }

    let resolved_index_source = crate::config_glue::resolve_index_source(
        args.index_path.as_deref(),
        None,
        &effective_config,
    )?;
    let offline = crate::config_glue::effective_offline(args.offline)?;
    crate::config_glue::enforce_offline_index_source(offline, resolved_index_source.as_ref())?;
    let resolved_cache_dir =
        crate::config_glue::resolve_cache_dir(args.cache_dir.as_deref(), &effective_config);
    let Some(index_source) = resolved_index_source.as_ref() else {
        bail!(
            "versioned dependencies require --index-path or a `[registry] index-path` config setting"
        );
    };
    // Vendoring reads per-package metadata directly off disk so
    // the vendor directory ends up byte-stable. The only index
    // source shape that satisfies that requirement is a local
    // file index — reject a URL terminal up front instead of
    // letting the artifact pipeline reach for the network and
    // surface a less specific error.
    if matches!(
        index_source.kind,
        crate::config_glue::IndexSourceKind::Url(_)
    ) {
        bail!(
            "`cabin vendor` requires a local `--index-path` source so per-package metadata can be copied verbatim into the vendor directory"
        );
    }

    let mode = lock_mode_for_flags(args.locked, args.frozen);
    let allow_write = !(args.locked || args.frozen);
    let cache_dir = match resolved_cache_dir.as_ref() {
        Some((path, _)) => path.clone(),
        None => cache_dir_for(&manifest_path, args.cache_dir.as_deref())?,
    };
    let initial_locator = match &index_source.kind {
        crate::config_glue::IndexSourceKind::Path(p) => {
            cabin_core::SourceLocator::IndexPath { path: p.clone() }
        }
        crate::config_glue::IndexSourceKind::Url(u) => {
            cabin_core::SourceLocator::IndexUrl { url: u.clone() }
        }
    };
    let resolved_locator = crate::patch_glue::apply_source_replacement(
        initial_locator,
        &effective_config,
        args.no_patches,
    )?;
    crate::config_glue::enforce_offline_post_replacement(offline, &resolved_locator)?;
    crate::config_glue::enforce_vendor_local_index_post_replacement(&resolved_locator)?;
    let (replaced_path, replaced_url) =
        crate::patch_glue::locator_to_index_inputs(&resolved_locator.resolved);

    let pipeline = run_artifact_pipeline(&ArtifactPipelineRequest {
        manifest_path: &manifest_path,
        initial_graph: &initial_graph,
        index_path: replaced_path.as_deref(),
        index_url: replaced_url.as_deref(),
        mode,
        allow_write,
        frozen: args.frozen,
        cache_dir: &cache_dir,
        reporter,
        selection: workspace_selection,
        selection_request: &selection_request,
        patched_names: &patched_names,
        active_patches: &active_patches,
        source_replacements: &effective_config.source_replacements,
        no_patches: args.no_patches,
        dev_for: &dev_for,
    })?;

    // Vendoring copies `packages/<name>.json` files verbatim
    // into the output, so the index source must be a local
    // directory the vendor crate can read off disk.
    let index_dir = match replaced_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => bail!(
            "`cabin vendor` requires a local `--index-path` source so per-package metadata can be copied verbatim into the vendor directory"
        ),
    };

    let plan = build_vendor_plan(&pipeline.fetched, &index_dir)?;
    let report = vendor_materialise(
        &plan,
        &vendor_dir,
        &VendorOptions {
            frozen: args.frozen,
        },
    )
    .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    emit_vendor_summary(&report, reporter);
    Ok(())
}

fn resolve_vendor_dir(args: &VendorArgs, manifest_path: &std::path::Path) -> Result<PathBuf> {
    let candidate = match args.vendor_dir.as_deref() {
        Some(p) => p.to_path_buf(),
        None => manifest_path
            .parent()
            .map(|p| p.join(DEFAULT_VENDOR_DIRNAME))
            .unwrap_or_else(|| PathBuf::from(DEFAULT_VENDOR_DIRNAME)),
    };
    absolutise(&candidate)
        .with_context(|| format!("failed to resolve vendor dir {}", candidate.display()))
}

/// Build a [`VendorPlan`] from the pipeline's fetched packages
/// plus the source index's per-package JSON files. The function
/// reads each `<index>/packages/<name>.json` once, picks the
/// resolved version's entry, and pairs it with the verified
/// archive in the cache.
fn build_vendor_plan(
    fetched: &[FetchedPackage],
    index_dir: &std::path::Path,
) -> Result<VendorPlan> {
    let mut entries: Vec<VendorEntry> = Vec::with_capacity(fetched.len());
    // Cache index reads so a workspace that resolves multiple
    // versions of the same name does not re-parse the same file.
    let mut by_name: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    for pkg in fetched {
        let name = pkg.name.as_str().to_owned();
        let parsed = match by_name.get(&name) {
            Some(v) => v.clone(),
            None => {
                let path = index_dir.join("packages").join(format!("{name}.json"));
                let body = std::fs::read_to_string(&path).with_context(|| {
                    format!(
                        "vendoring requires the source index to expose `packages/{name}.json` at `{}`",
                        path.display()
                    )
                })?;
                let parsed: serde_json::Value = serde_json::from_str(&body)
                    .with_context(|| format!("failed to parse {}", path.display()))?;
                by_name.insert(name.clone(), parsed.clone());
                parsed
            }
        };
        let version_entry = parsed
            .get("versions")
            .and_then(|v| v.get(pkg.version.to_string()))
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "source index has no `{name}` version `{}` to vendor; the index file may be stale",
                    pkg.version
                )
            })?;
        entries.push(VendorEntry {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            checksum: pkg.checksum.clone(),
            archive_source: pkg.archive_path.clone(),
            index_entry: version_entry,
        });
    }
    VendorPlan::new(entries).map_err(|err| anyhow::anyhow!(err.to_string()))
}

fn emit_vendor_summary(
    report: &cabin_vendor::VendorReport,
    reporter: crate::term_verbosity_glue::Reporter,
) {
    reporter.status(format_args!(
        "cabin: vendored to {}",
        report.vendor_dir.display()
    ));
    if report.written.is_empty() {
        reporter.status(format_args!(
            "cabin: no versioned dependencies in the selected closure"
        ));
    } else {
        reporter.status(format_args!(
            "cabin: wrote {} package{}",
            report.written.len(),
            plural(report.written.len())
        ));
        for entry in &report.written {
            let action = if entry.artifact_was_written {
                "wrote"
            } else {
                "verified"
            };
            reporter.status(format_args!(
                "  {action} {} {} -> {}",
                entry.name.as_str(),
                entry.version,
                entry.artifact_relative_path
            ));
        }
    }
    reporter.status(format_args!(
        "cabin: build offline with `cabin build --offline --index-path {}`",
        report.vendor_dir.display()
    ));
    if report.frozen {
        reporter.status(format_args!(
            "cabin: --frozen: lockfile and artifact cache were not modified"
        ));
    }
}
