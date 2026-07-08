//! Resolve the active patch set for one Cabin invocation.
//!
//! The CLI calls [`resolve_active_patches`] after loading the
//! initial workspace graph and the merged effective config.  The
//! returned [`ActivePatchSet`] is the typed input the rest of
//! the pipeline consumes:
//!
//! - the artifact pipeline filters out patched names so a
//!   patched dep is never re-fetched from the registry;
//! - the workspace loader stitches the patched manifests in via
//!   [`crate::PatchedPackageSource`];
//! - the lockfile records each entry so `--locked` can detect
//!   stale patch policy;
//! - the metadata view reports each entry deterministically.
//!
//! Validation runs eagerly: missing paths, missing `cabin.toml`,
//! package-name mismatches, and version-requirement mismatches
//! all surface as [`PatchResolutionError`] before any consumer
//! sees the resolved set.  Wording is stable so integration tests
//! can match substrings.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cabin_core::{
    DependencySource, Package, PackageName, PatchProvenance, PatchSource, PatchValidationError,
};
use camino::Utf8PathBuf;
use thiserror::Error;

use crate::graph::PackageGraph;
use crate::loader::PatchedPackageSource;

/// One fully-resolved patch entry.  Pairs the typed source with
/// the loaded patch [`Package`] so downstream consumers do not
/// need to re-parse the patched `cabin.toml`.
#[derive(Debug, Clone)]
pub struct ActivePatch {
    pub name: PackageName,
    pub source: PatchSource,
    pub provenance: PatchProvenance,
    /// Absolute path of the patched package's `cabin.toml`.
    pub manifest_path: PathBuf,
    /// Absolute path of the patched package's directory.
    pub manifest_dir: PathBuf,
    /// The path *as written* in the declaring file.  Useful for
    /// metadata / lockfile output where we prefer to show the
    /// user-visible relative form rather than the absolute
    /// canonical path.  Cabin-owned model data, so kept UTF-8.
    pub declared_path: Utf8PathBuf,
    /// Parsed patched [`Package`].  Carried through so the
    /// loader does not have to re-parse the manifest.
    pub package: Package,
}

/// Container for the active patch set.  Ordered by package name
/// for deterministic iteration.
#[derive(Debug, Clone, Default)]
pub struct ActivePatchSet {
    entries: Vec<ActivePatch>,
}

impl ActivePatchSet {
    /// Iterate entries in deterministic (package-name) order.
    pub fn iter(&self) -> std::slice::Iter<'_, ActivePatch> {
        self.entries.iter()
    }
}

impl<'a> IntoIterator for &'a ActivePatchSet {
    type Item = &'a ActivePatch;
    type IntoIter = std::slice::Iter<'a, ActivePatch>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl ActivePatchSet {
    /// Whether the set carries any entries.  Used by the CLI to
    /// short-circuit the no-patches path.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of active patches.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Lookup by package name.
    pub fn get(&self, name: &PackageName) -> Option<&ActivePatch> {
        self.entries.iter().find(|p| &p.name == name)
    }

    /// Set of patched package names.  Useful for callers that
    /// need to filter the registry list / closure detection by
    /// name.
    pub fn patched_names(&self) -> BTreeSet<&str> {
        self.entries.iter().map(|p| p.name.as_str()).collect()
    }

    /// Patched package names as owned strings.  Convenient for
    /// callers that need to hold the set across the loader /
    /// artifact-pipeline boundary without lifetime juggling.
    pub fn owned_patched_names(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .map(|p| p.name.as_str().to_owned())
            .collect()
    }

    /// Adapt this set into the workspace loader's
    /// [`PatchedPackageSource`] shape.  The loader stitches each
    /// entry as a local-kind package whose `(name, version,
    /// manifest_path)` is supplied here, so the mapping
    /// belongs next to the loader's input type rather than in
    /// CLI orchestration code.
    pub fn workspace_sources(&self) -> Vec<PatchedPackageSource> {
        self.entries
            .iter()
            .map(|entry| PatchedPackageSource {
                name: entry.name.clone(),
                version: entry.package.version.clone(),
                manifest_path: entry.manifest_path.clone(),
            })
            .collect()
    }
}

/// The per-dependency filter shared by the patch-side collectors
/// below: active kind (dev deps are declaration-only here),
/// host-platform match, non-optional, and a registry `Version`
/// source.  Returns the requirement when `dep` passes, keeping
/// [`collect_patched_versioned_deps`] and
/// [`collect_version_requirements`] on the same policy.
fn active_versioned_req<'a>(
    dep: &'a cabin_core::Dependency,
    host: &cabin_core::TargetPlatform,
) -> Option<&'a semver::VersionReq> {
    if !dep.kind.is_resolved_by_default() {
        return None;
    }
    if !dep.matches_platform(host) {
        return None;
    }
    if dep.optional {
        return None;
    }
    match &dep.source {
        DependencySource::Version(req) => Some(req),
        DependencySource::Path(_) | DependencySource::Port(_) | DependencySource::Workspace => None,
    }
}

/// Versioned dependencies declared by the patched manifests
/// themselves.
///
/// The normal workspace-closure walker only sees packages that
/// have already been stitched into the [`PackageGraph`].  Patch
/// resolution happens earlier, so callers use this helper to add
/// registry dependencies introduced by patched packages to the
/// resolver input.  The filtering policy matches
/// `collect_closure_versioned_deps_excluding_with_dev` for a
/// non-test build: normal deps are active, dev deps are
/// declaration-only, optional deps are skipped until a feature
/// resolver can prove them enabled, target predicates are evaluated
/// against the host, and patched names are excluded.
///
/// # Errors
/// Returns [`crate::WorkspaceError::IncompatibleWorkspaceRequirements`]
/// when the requirements collected for a single dependency name
/// cannot be combined into one [`semver::VersionReq`] (the joined
/// requirement string fails to parse).
pub fn collect_patched_versioned_deps(
    active_patches: &ActivePatchSet,
    excluded_names: &BTreeSet<String>,
) -> Result<BTreeMap<PackageName, semver::VersionReq>, crate::WorkspaceError> {
    let host_platform = cabin_core::TargetPlatform::current();
    let mut combined: BTreeMap<PackageName, Vec<String>> = BTreeMap::new();

    for patch in active_patches {
        for dep in &patch.package.dependencies {
            let Some(req) = active_versioned_req(dep, &host_platform) else {
                continue;
            };
            if excluded_names.contains(dep.name.as_str()) {
                continue;
            }
            combined
                .entry(dep.name.clone())
                .or_default()
                .push(req.to_string());
        }
    }

    let mut out = BTreeMap::new();
    for (name, mut reqs) in combined {
        reqs.sort();
        reqs.dedup();
        let parsed =
            crate::selection::combine_version_reqs(&reqs).map_err(|(requirements, source)| {
                crate::WorkspaceError::IncompatibleWorkspaceRequirements {
                    name: name.as_str().to_owned(),
                    requirements,
                    source,
                }
            })?;
        out.insert(name, parsed);
    }
    Ok(out)
}

/// Inputs to [`resolve_active_patches`].  Bundling them keeps the
/// call site readable and the function signature stable as new
/// inputs land.
pub struct PatchResolutionInputs<'a> {
    /// Loaded initial workspace graph.  Used to find the
    /// workspace root manifest and its `[patch]` table, and to
    /// look up version requirements for patched names.
    pub graph: &'a PackageGraph,
    /// Manifest-declared patches plus the directory the manifest
    /// lives in (for relative path resolution).
    pub manifest_patches: &'a cabin_core::PatchManifestSettings,
    /// Config-derived patches keyed by package name.  Each entry
    /// carries the directory of the config file that declared
    /// it so relative paths resolve against the right base.
    pub config_patches: &'a BTreeMap<PackageName, ConfigPatchInput>,
}

/// One config-derived patch entry as the orchestration layer
/// hands it off to the resolver.  The orchestration layer maps
/// `cabin_config::EffectivePatch` into this shape so this crate
/// stays free of `cabin-config` dependency.
#[derive(Debug, Clone)]
pub struct ConfigPatchInput {
    pub source: PatchSource,
    pub provenance: PatchProvenance,
    /// Directory of the config file that declared this patch.
    pub declared_in: PathBuf,
}

/// Resolve the active patch set deterministically.
///
/// Precedence: config patches override manifest patches on
/// overlap.  Within a single layer (one manifest table or one
/// merged config map) duplicates are impossible because both
/// inputs are keyed by [`PackageName`]; across layers the higher
/// layer wins and the lower one is dropped silently - this
/// matches the rest of the config-vs-manifest precedence ladder.
///
/// # Errors
/// Returns a [`PatchResolutionError`] when a merged patch fails to
/// resolve: [`PatchResolutionError::Validation`] when the patched
/// `cabin.toml` is missing, declares no `[package]`, names a
/// different package, or carries a version no active requirement
/// accepts; and [`PatchResolutionError::ManifestParse`] when the
/// patched manifest cannot be loaded or its path canonicalized.
pub fn resolve_active_patches(
    inputs: &PatchResolutionInputs<'_>,
) -> Result<ActivePatchSet, PatchResolutionError> {
    let root_dir = inputs.graph.root_dir.clone();

    // Merge: start with manifest patches, overlay config
    // patches.  The overlay drops the manifest entry; we record
    // the winning provenance so metadata can show it.
    let mut merged: BTreeMap<PackageName, MergedEntry> = BTreeMap::new();
    for (name, source) in &inputs.manifest_patches.entries {
        merged.insert(
            name.clone(),
            MergedEntry {
                source: source.clone(),
                provenance: PatchProvenance::Manifest,
                base_dir: root_dir.clone(),
            },
        );
    }
    for (name, entry) in inputs.config_patches {
        let base_dir = entry
            .declared_in
            .parent()
            .map_or_else(|| root_dir.clone(), Path::to_path_buf);
        merged.insert(
            name.clone(),
            MergedEntry {
                source: entry.source.clone(),
                provenance: entry.provenance,
                base_dir,
            },
        );
    }

    // Collect version requirements per patched name from the
    // initial graph so we can validate patched versions before
    // returning.  We iterate every dep edge in every loaded
    // package; only Version-source deps contribute requirements.
    let requirements = collect_version_requirements(inputs.graph, &merged);

    // Resolve each merged entry into an `ActivePatch`.
    let mut entries: Vec<ActivePatch> = Vec::with_capacity(merged.len());
    for (name, entry) in merged {
        let resolved = resolve_one_patch(&name, entry, &requirements)?;
        entries.push(resolved);
    }
    entries.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));

    Ok(ActivePatchSet { entries })
}

struct MergedEntry {
    source: PatchSource,
    provenance: PatchProvenance,
    base_dir: PathBuf,
}

/// Walk every loaded package and collect, per patched name,
/// the set of `Version`-source requirements that are
/// *active* for the current invocation.  Inactive declarations
/// (dev / system kinds, target-conditioned deps that do not
/// match the host platform, optional deps regardless of feature
/// state) are skipped so a patch on a dormant dependency does
/// not cause spurious version-mismatch errors.
///
/// Optional deps are conservative: even if a feature would
/// enable them, this function lacks the cross-package feature
/// resolution result and so cannot decide membership precisely.
/// The orchestration layer handles enabled optional patches via
/// the resolver / loader path, where the patched manifest is
/// used directly and any version mismatch surfaces against the
/// real resolver input.
fn collect_version_requirements(
    graph: &PackageGraph,
    merged: &BTreeMap<PackageName, MergedEntry>,
) -> BTreeMap<PackageName, Vec<semver::VersionReq>> {
    let host_platform = cabin_core::TargetPlatform::current();
    let mut out: BTreeMap<PackageName, Vec<semver::VersionReq>> = BTreeMap::new();
    for pkg in &graph.packages {
        for dep in &pkg.package.dependencies {
            if !merged.contains_key(&dep.name) {
                continue;
            }
            let Some(req) = active_versioned_req(dep, &host_platform) else {
                continue;
            };
            out.entry(dep.name.clone()).or_default().push(req.clone());
        }
    }
    for reqs in out.values_mut() {
        reqs.sort_by_cached_key(std::string::ToString::to_string);
        reqs.dedup_by(|a, b| a.to_string() == b.to_string());
    }
    out
}

fn resolve_one_patch(
    name: &PackageName,
    entry: MergedEntry,
    requirements: &BTreeMap<PackageName, Vec<semver::VersionReq>>,
) -> Result<ActivePatch, PatchResolutionError> {
    let MergedEntry {
        source,
        provenance,
        base_dir,
    } = entry;
    match source {
        PatchSource::Path {
            path: declared_path,
        } => {
            // `declared_path` is the Cabin-owned UTF-8 path as
            // written in the declaration.  Patch resolution from here
            // is a filesystem operation, so `absolute_dir` (used to
            // locate `cabin.toml` on disk) is a `std::path` value.
            let absolute_dir = if declared_path.is_absolute() {
                declared_path.as_std_path().to_path_buf()
            } else {
                base_dir.join(declared_path.as_std_path())
            };
            let manifest_path = absolute_dir.join("cabin.toml");
            if !manifest_path.is_file() {
                return Err(PatchResolutionError::Validation {
                    package: name.as_str().to_owned(),
                    source: PatchValidationError::MissingManifest {
                        package: name.as_str().to_owned(),
                        path: declared_path.as_str().to_owned(),
                    },
                });
            }
            let parsed = cabin_manifest::load_manifest(&manifest_path).map_err(|err| {
                PatchResolutionError::ManifestParse {
                    package: name.as_str().to_owned(),
                    path: manifest_path.clone(),
                    source: PatchManifestLoadError::Parse(Box::new(err)),
                }
            })?;
            let package = parsed
                .package
                .ok_or_else(|| PatchResolutionError::Validation {
                    package: name.as_str().to_owned(),
                    source: PatchValidationError::ManifestHasNoPackage {
                        package: name.as_str().to_owned(),
                        path: declared_path.as_str().to_owned(),
                    },
                })?;
            if &package.name != name {
                return Err(PatchResolutionError::Validation {
                    package: name.as_str().to_owned(),
                    source: PatchValidationError::PackageNameMismatch {
                        package: name.as_str().to_owned(),
                        actual: package.name.as_str().to_owned(),
                    },
                });
            }
            // Version-requirement validation.  Each requirement
            // collected from active dep edges must accept the
            // patched version; otherwise we surface a clear error
            // before the loader stitches the wrong manifest.
            if let Some(reqs) = requirements.get(name) {
                for req in reqs {
                    if !req.matches(&package.version) {
                        return Err(PatchResolutionError::Validation {
                            package: name.as_str().to_owned(),
                            source: PatchValidationError::VersionMismatch {
                                package: name.as_str().to_owned(),
                                version: package.version.to_string(),
                                requirement: req.to_string(),
                            },
                        });
                    }
                }
            }
            // Canonicalize the manifest path so the workspace
            // loader's dedup-by-canonical-path machinery sees a
            // consistent value - via the project's single canonicalize
            // boundary, so the two never disagree on Windows (where
            // `\\?\` would otherwise leak in only on this path).
            let canonical_manifest = cabin_fs::canonicalize(&manifest_path).map_err(|err| {
                PatchResolutionError::ManifestParse {
                    package: name.as_str().to_owned(),
                    path: manifest_path.clone(),
                    source: PatchManifestLoadError::Canonicalize(err),
                }
            })?;
            let canonical_dir = canonical_manifest
                .parent()
                .map_or(absolute_dir, Path::to_path_buf);
            Ok(ActivePatch {
                name: name.clone(),
                source: PatchSource::Path {
                    path: declared_path.clone(),
                },
                provenance,
                manifest_path: canonical_manifest,
                manifest_dir: canonical_dir,
                declared_path,
                package,
            })
        }
    }
}

/// Errors produced by [`resolve_active_patches`].  Wording is
/// stable so integration tests can match substrings.
#[derive(Debug, Error)]
pub enum PatchResolutionError {
    /// A patch failed structural validation (missing source,
    /// missing cabin.toml, name mismatch, version mismatch, …).
    /// Wraps the typed [`PatchValidationError`] from `cabin-core`
    /// so the inner error carries its own user-readable wording.
    #[error("invalid patch for `{package}`: {source}")]
    Validation {
        package: String,
        #[source]
        source: PatchValidationError,
    },

    /// Cabin could not load the patched manifest.  Wraps the
    /// typed [`PatchManifestLoadError`] so callers can inspect
    /// the failure kind; the inline `{source}` keeps the display
    /// message identical to the historical string form.
    #[error(
        "failed to parse patch manifest for `{package}` at {path}: {source}",
        path = path.display()
    )]
    ManifestParse {
        package: String,
        path: PathBuf,
        #[source]
        source: PatchManifestLoadError,
    },
}

/// Why a patched manifest failed to load: the manifest did not
/// parse, or its path could not be canonicalized.  The parse error
/// is boxed to keep this enum (and the containing
/// [`PatchResolutionError`]) small.
#[derive(Debug, Error)]
pub enum PatchManifestLoadError {
    #[error(transparent)]
    Parse(Box<cabin_manifest::ManifestError>),
    #[error(transparent)]
    Canonicalize(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_workspace;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    /// Build a workspace where `app` references `fmt` through
    /// `dep_block` and patches it with a local fork at version
    /// `0.1.0`.  The block is the user's chance to switch dep kind
    /// / optional / target condition while keeping the rest of the
    /// fixture identical.
    fn fixture(parent: &TempDir, dep_block: &str) -> PackageGraph {
        parent
            .child("fmt/cabin.toml")
            .write_str("[package]\nname = \"fmt\"\nversion = \"0.1.0\"\n")
            .unwrap();
        let manifest = format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n{dep_block}\n\n[patch]\nfmt = {{ path = \"../fmt\" }}\n",
        );
        parent.child("app/cabin.toml").write_str(&manifest).unwrap();
        load_workspace(parent.path().join("app/cabin.toml")).unwrap()
    }

    fn resolve_with(graph: &PackageGraph) -> Result<ActivePatchSet, PatchResolutionError> {
        let manifest_patches = &graph.root_settings.patches;
        let empty: BTreeMap<PackageName, ConfigPatchInput> = BTreeMap::new();
        resolve_active_patches(&PatchResolutionInputs {
            graph,
            manifest_patches,
            config_patches: &empty,
        })
    }

    #[test]
    fn patch_target_without_package_table_reports_no_package() {
        // The patched directory's `cabin.toml` exists and parses
        // but is a pure `[workspace]` root with no `[package]`.
        // The error must say the manifest declares no `[package]`,
        // not the misleading "does not contain a cabin.toml".
        let dir = TempDir::new().unwrap();
        let graph = fixture(&dir, "[dependencies]\nfmt = \">=0.1\"");
        // Overwrite the patched `fmt` manifest with a workspace-only
        // table: it parses fine but exposes no package to patch in.
        dir.child("fmt/cabin.toml")
            .write_str("[workspace]\nmembers = []\n")
            .unwrap();
        let err = resolve_with(&graph).expect_err("workspace-only patch target must be rejected");
        match err {
            PatchResolutionError::Validation { source, .. } => {
                assert!(
                    matches!(source, PatchValidationError::ManifestHasNoPackage { .. }),
                    "expected ManifestHasNoPackage, got {source:?}"
                );
            }
            PatchResolutionError::ManifestParse { .. } => {
                panic!("expected Validation error, got ManifestParse")
            }
        }
    }

    #[test]
    fn dev_only_dep_does_not_block_patch_version() {
        // `fmt` is referenced only as a dev dep with an
        // unsatisfiable requirement.  The patch's `0.1.0` would
        // never satisfy `>= 99`, but dev deps are not active for
        // the default build, so validation must skip the edge.
        let dir = TempDir::new().unwrap();
        let graph = fixture(&dir, "[dev-dependencies]\nfmt = \">=99\"");
        let resolved = resolve_with(&graph).expect("dev-only requirement must not gate patch");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved.iter().next().unwrap().name.as_str(), "fmt");
    }

    #[test]
    fn optional_dep_does_not_block_patch_version() {
        // Optional deps are conservatively skipped: their
        // activation depends on feature resolution we don't
        // perform here.  If the feature later enables them, the
        // resolver path surfaces any version mismatch using the
        // patched manifest directly.
        let dir = TempDir::new().unwrap();
        let graph = fixture(
            &dir,
            "[dependencies]\nfmt = { version = \">=99\", optional = true }",
        );
        let resolved = resolve_with(&graph).expect("optional requirement must not gate patch");
        assert_eq!(resolved.len(), 1);
    }

    #[test]
    fn target_mismatched_dep_does_not_block_patch_version() {
        // Pick a target the host can never match so the
        // requirement is dormant on this invocation.
        let dir = TempDir::new().unwrap();
        let graph = fixture(
            &dir,
            "[target.'cfg(os = \"never-an-os\")'.dependencies]\nfmt = \">=99\"",
        );
        let resolved =
            resolve_with(&graph).expect("non-matching target requirement must not gate patch");
        assert_eq!(resolved.len(), 1);
    }

    #[test]
    fn active_normal_dep_still_validates_patch_version() {
        // Negative control: an active normal dep with an
        // unsatisfiable requirement must still surface
        // `VersionMismatch`.  Without this, the gating change
        // could silently regress the original validation.
        let dir = TempDir::new().unwrap();
        let graph = fixture(&dir, "[dependencies]\nfmt = \">=99\"");
        let err = resolve_with(&graph).expect_err("active requirement must reject patch");
        match err {
            PatchResolutionError::Validation { source, .. } => {
                assert!(
                    matches!(source, PatchValidationError::VersionMismatch { .. }),
                    "expected VersionMismatch, got {source:?}"
                );
            }
            PatchResolutionError::ManifestParse { .. } => {
                panic!("expected Validation error, got ManifestParse")
            }
        }
    }

    #[test]
    fn patched_manifest_versioned_deps_follow_workspace_policy() {
        let dir = TempDir::new().unwrap();
        dir.child("fmt/cabin.toml")
            .write_str(
                r#"[package]
name = "fmt"
version = "0.1.0"

[dependencies]
spdlog = "^1.13"
fmt = "^99"
optional-lib = { version = "^2", optional = true }

[dev-dependencies]
testkit = "^1"

[target.'cfg(os = "never-an-os")'.dependencies]
target-only = "^1"
"#,
            )
            .unwrap();
        dir.child("app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=0.1"

[patch]
fmt = { path = "../fmt" }
"#,
            )
            .unwrap();

        let graph = load_workspace(dir.path().join("app/cabin.toml")).unwrap();
        let patches = resolve_with(&graph).unwrap();
        let excluded = patches.owned_patched_names();

        let deps = collect_patched_versioned_deps(&patches, &excluded).unwrap();
        let rendered: BTreeMap<_, _> = deps
            .iter()
            .map(|(name, req)| (name.as_str().to_owned(), req.to_string()))
            .collect();

        assert_eq!(
            rendered,
            BTreeMap::from([("spdlog".to_owned(), "^1.13".to_owned())])
        );
    }
}
