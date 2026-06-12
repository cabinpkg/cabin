//! Typed dependency-tree and explain models for `cabin tree` and
//! `cabin explain`.
//!
//! `cabin metadata` already exposes the loaded package state as
//! a deterministic JSON document. This crate adds two
//! complementary, lower-bandwidth views on the same loaded
//! `PackageGraph` + lockfile + active patch / source-replacement
//! state:
//!
//! - [`build_tree`] returns a [`TreeNode`] showing every package
//!   in the loaded [`PackageGraph`] reachable from the selected
//!   primary packages,
//!   with edges tagged by [`cabin_core::DependencyKind`] and
//!   provenance pulled from the lockfile / active patch set.
//!   Renderers ([`render_tree_human`] /
//!   [`render_tree_json`]) turn the typed tree into either a
//!   stable text drawing or a structured JSON document; the JSON
//!   document shares its package shape with `cabin metadata`.
//!
//! - [`explain_package`] / [`explain_target`] /
//!   [`explain_source`] / [`explain_feature`] /
//!   [`explain_build_config`] each return a typed
//!   [`Explanation`] answering "why is X selected", "where does
//!   X come from", "which feature lit up X", and "what does the
//!   build configuration look like for X". Each variant carries
//!   only structured data so callers can render either a
//!   human-readable summary ([`render_explanation_human`]) or a
//!   stable JSON document ([`render_explanation_json`]).
//!
//! Crate boundaries:
//! - this crate must not run the resolver, parse manifests, or
//!   plan builds; it consumes the typed values the orchestration
//!   layer hands it;
//! - it must not perform I/O. The orchestration layer in
//!   `cabin` is responsible for loading the workspace, the
//!   lockfile, and the active patch set; this crate works
//!   purely on those typed inputs;
//! - it must not invent new identity for packages. Provenance
//!   comes from `cabin_workspace::PackageKind`, the lockfile, the
//!   patch set, and the source-replacement settings.
//!
//! ## Determinism contract
//!
//! Every output produced by this crate is deterministic across
//! runs:
//!
//! - tree children are sorted by `(dependency_kind, package_name,
//!   package_version)`;
//! - explanation paths are sorted by `(length, joined name
//!   sequence)`;
//! - JSON keys are emitted in struct-declaration order;
//! - paths surfaced through the API are *not* normalized here —
//!   that is the orchestration layer's job.
//!
//! Anything that mutates the inputs is the orchestration layer's
//! responsibility; this crate only reads them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::BuildHasher;
use std::path::PathBuf;

use cabin_core::DependencyKind;
use cabin_lockfile::Lockfile;
use cabin_workspace::{PackageGraph, PackageKind, WorkspacePackage};
use serde::Serialize;
use thiserror::Error;

/// Provenance label for one node in a [`TreeNode`] or one
/// step in an [`Explanation::Package`] chain.
///
/// The variants reflect the load-bearing distinctions Cabin
/// already makes elsewhere: a workspace member, a local path
/// dependency, a patched local working copy, or a registry
/// package the artifact pipeline materialized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum SourceProvenance {
    /// A workspace member declared by the root manifest's
    /// `[workspace.members]` table.
    WorkspaceMember,
    /// A local `path = "..."` dependency that lives outside the
    /// workspace.
    LocalPath,
    /// A prepared foundation port: its source tree was materialized
    /// from a `port.toml` recipe (downloaded, checksum-verified,
    /// extracted) and overlaid with a Cabin manifest. Rendered as
    /// `[port]`.
    Port,
    /// A registry package that an active `[patch]` entry pinned
    /// to a local working copy. The `path` is the patched
    /// directory's `manifest_dir`.
    Patched {
        /// Filesystem path of the patched working copy.
        path: PathBuf,
        /// Origin layer of the patch (`manifest`, `user-config`,
        /// `workspace-config`, etc.).
        provenance: String,
    },
    /// A registry package whose source bytes were materialized
    /// by the artifact pipeline. Carries the recorded checksum
    /// when the lockfile pinned one.
    Registry {
        /// `sha256:<hex>` checksum recorded for this version, if
        /// any. `None` when the lockfile predates checksum
        /// recording.
        #[serde(skip_serializing_if = "Option::is_none")]
        checksum: Option<String>,
    },
}

/// One node in a dependency tree rooted at a selected primary
/// package. Children are deduplicated per traversal: the first
/// occurrence of a `(name, version)` carries the full subtree;
/// subsequent occurrences are marked with [`TreeNode::repeated`].
#[derive(Debug, Clone, Serialize)]
pub struct TreeNode {
    /// `WorkspacePackage` name.
    pub name: String,
    /// Resolved package version.
    pub version: String,
    /// How the parent reached this node. `None` for the tree's
    /// roots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_kind: Option<&'static str>,
    /// Provenance of this package's source bytes.
    pub source: SourceProvenance,
    /// `true` when this node was already expanded earlier in
    /// the traversal — children were pruned to keep the tree
    /// finite.
    #[serde(skip_serializing_if = "is_false")]
    pub repeated: bool,
    /// Children, sorted by `(dependency_kind, name, version)`.
    pub children: Vec<TreeNode>,
}

fn is_false<T>(value: &T) -> bool
where
    T: PartialEq + Default,
{
    *value == T::default()
}

/// Per-call options for [`build_tree`]. Mirrors the same
/// dependency-kind filter `cabin tree --kind ...` exposes, plus
/// the optional [`Lockfile`] / patch / vendor / source-replacement
/// inputs used to color provenance.
pub struct TreeInputs<'a> {
    /// Resolved package graph.
    pub graph: &'a PackageGraph,
    /// Indices of packages the user selected as roots.
    /// Children are walked from these. Empty falls back to the
    /// graph's primary set so callers do not have to special-case
    /// "no selection" themselves.
    pub roots: &'a [usize],
    /// Optional lockfile contributing version-pinned checksums
    /// to the provenance label.
    pub lockfile: Option<&'a Lockfile>,
    /// Active patch entries.  Patched packages are flagged
    /// with [`SourceProvenance::Patched`] regardless of the
    /// lockfile.
    pub active_patches: Option<&'a cabin_workspace::ActivePatchSet>,
    /// Restrict the walk to a single dependency-kind edge.
    /// `None` walks every kind the graph carries.
    pub kind_filter: Option<DependencyKind>,
}

/// Build a deterministic [`TreeNode`] forest rooted at every
/// index in `roots`. Returned as a single root-less synthetic
/// vector with the documented sort key applied at every level
/// so renderers can iterate without re-sorting.
pub fn build_tree(inputs: &TreeInputs<'_>) -> Vec<TreeNode> {
    let roots: Vec<usize> = if inputs.roots.is_empty() {
        inputs.graph.primary_packages.clone()
    } else {
        let mut owned = inputs.roots.to_vec();
        owned.sort_by(|a, b| {
            inputs.graph.packages[*a]
                .package
                .name
                .as_str()
                .cmp(inputs.graph.packages[*b].package.name.as_str())
        });
        owned.dedup();
        owned
    };
    let mut out: Vec<TreeNode> = roots
        .iter()
        .map(|&idx| {
            let mut visited: HashSet<usize> = HashSet::new();
            build_node(idx, None, inputs, &mut visited)
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));
    out
}

fn build_node(
    idx: usize,
    edge_kind: Option<DependencyKind>,
    inputs: &TreeInputs<'_>,
    visited: &mut HashSet<usize>,
) -> TreeNode {
    let pkg = &inputs.graph.packages[idx];
    let name = pkg.package.name.as_str().to_owned();
    let version = pkg.package.version.to_string();
    let source = source_provenance_for(pkg, inputs);
    let edge_kind_label = edge_kind.map(dep_kind_key);

    let already_visited = !visited.insert(idx);
    if already_visited {
        return TreeNode {
            name,
            version,
            edge_kind: edge_kind_label,
            source,
            repeated: true,
            children: Vec::new(),
        };
    }

    let mut children: Vec<TreeNode> = Vec::new();
    for edge in &pkg.deps {
        if let Some(filter) = inputs.kind_filter
            && edge.kind != filter
        {
            continue;
        }
        children.push(build_node(edge.index, Some(edge.kind), inputs, visited));
    }
    children.sort_by(|a, b| {
        edge_kind_sort_key(a.edge_kind)
            .cmp(&edge_kind_sort_key(b.edge_kind))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.version.cmp(&b.version))
    });

    TreeNode {
        name,
        version,
        edge_kind: edge_kind_label,
        source,
        repeated: false,
        children,
    }
}

fn dep_kind_key(kind: DependencyKind) -> &'static str {
    kind.as_str()
}

fn edge_kind_sort_key(label: Option<&'static str>) -> u8 {
    // Same canonical order `cabin metadata` already documents:
    // normal → dev. Roots have no edge kind; sort them first.
    match label {
        None => 0,
        Some("normal") => 1,
        Some("dev") => 2,
        Some(_) => 99,
    }
}

fn source_provenance_for(pkg: &WorkspacePackage, inputs: &TreeInputs<'_>) -> SourceProvenance {
    if let Some(set) = inputs.active_patches
        && let Some(active) = set.get(&pkg.package.name)
    {
        return SourceProvenance::Patched {
            path: active.manifest_dir.clone(),
            provenance: active.provenance.as_key(),
        };
    }
    // Ports are `PackageKind::Local`, so this flag — not `kind` — is
    // what distinguishes a prepared foundation port from an ordinary
    // path dependency.
    if pkg.is_port {
        return SourceProvenance::Port;
    }
    match pkg.kind {
        PackageKind::Local => {
            // The graph's primary set carries every workspace
            // member; anything else marked `Local` is a bare
            // `path = "..."` dependency.
            if inputs
                .graph
                .index_of(pkg.package.name.as_str())
                .is_some_and(|idx| inputs.graph.primary_packages.contains(&idx))
            {
                SourceProvenance::WorkspaceMember
            } else {
                SourceProvenance::LocalPath
            }
        }
        PackageKind::Registry => {
            let checksum = inputs
                .lockfile
                .and_then(|lock| lock.find(&pkg.package.name))
                .and_then(|locked| {
                    if locked.version == pkg.package.version {
                        locked.checksum.clone()
                    } else {
                        None
                    }
                });
            SourceProvenance::Registry { checksum }
        }
    }
}

/// Render a [`TreeNode`] forest as a human-readable Unicode
/// drawing. Output is deterministic: every level sorts by
/// `(edge_kind, name, version)` before this rendering runs, so
/// callers can compare two renderings byte-for-byte.
///
/// The first column is the package identifier (`name vX.Y.Z`),
/// followed by an annotation describing the dependency kind for
/// non-root nodes and the source provenance for every node.
pub fn render_tree_human(forest: &[TreeNode]) -> String {
    let mut out = String::new();
    for (i, node) in forest.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        render_human_node(&mut out, node, "", true, true);
    }
    out
}

fn render_human_node(
    out: &mut String,
    node: &TreeNode,
    prefix: &str,
    is_last: bool,
    is_root: bool,
) {
    let connector = if is_root {
        ""
    } else if is_last {
        "└── "
    } else {
        "├── "
    };
    out.push_str(prefix);
    out.push_str(connector);
    push_name_version_kind(out, &node.name, &node.version, node.edge_kind);
    out.push(' ');
    out.push('(');
    out.push_str(&render_source_label(&node.source));
    out.push(')');
    if node.repeated {
        out.push_str(" (*)");
    }
    out.push('\n');
    let child_prefix = if is_root {
        String::new()
    } else if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };
    let count = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        render_human_node(out, child, &child_prefix, i + 1 == count, false);
    }
}

/// Append the shared `name vVERSION [kind]` fragment used by both
/// the tree renderer and the explanation path steps. `edge_kind`
/// is the dependency-kind annotation rendered as ` [<kind>]`
/// (`None` for roots, which carry no incoming edge).
fn push_name_version_kind(
    out: &mut String,
    name: &str,
    version: &str,
    edge_kind: Option<&'static str>,
) {
    out.push_str(name);
    out.push(' ');
    out.push('v');
    out.push_str(version);
    if let Some(label) = edge_kind {
        out.push_str(" [");
        out.push_str(label);
        out.push(']');
    }
}

/// Append the shared `<name> v<version>  (<source>)` header line used
/// by both the package and source human renderers.
fn push_name_version_source(
    out: &mut String,
    name: &str,
    version: &str,
    source: &SourceProvenance,
) {
    use std::fmt::Write as _;
    let _ = writeln!(out, "{name} v{version}  ({})", render_source_label(source));
}

fn render_source_label(source: &SourceProvenance) -> String {
    match source {
        SourceProvenance::WorkspaceMember => "workspace".to_owned(),
        SourceProvenance::LocalPath => "local path".to_owned(),
        SourceProvenance::Port => "port".to_owned(),
        SourceProvenance::Patched { provenance, .. } => format!("patched via {provenance}"),
        SourceProvenance::Registry { checksum: Some(c) } => format!("registry, {c}"),
        SourceProvenance::Registry { checksum: None } => "registry".to_owned(),
    }
}

/// Render the forest as a stable JSON document.
///
/// # Panics
/// Panics if a [`TreeNode`] fails to serialize via `serde_json::to_value`,
/// which cannot happen because [`TreeNode`] derives [`Serialize`] with no
/// fallible custom serializer.
pub fn render_tree_json(forest: &[TreeNode]) -> serde_json::Value {
    serde_json::Value::Array(
        forest
            .iter()
            .map(|n| serde_json::to_value(n).expect("TreeNode is Serialize"))
            .collect(),
    )
}

/// Typed explanation chain returned by every `cabin explain`
/// query. Renderers read the variant to choose the layout; the
/// JSON output is a tagged union so downstream tooling sees the
/// query kind without re-parsing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Explanation {
    /// `cabin explain package <name>`.
    Package(PackageExplanation),
    /// `cabin explain target <name>`.
    Target(TargetExplanation),
    /// `cabin explain source <package>`.
    Source(SourceExplanation),
    /// `cabin explain feature <package/feature>`.
    Feature(FeatureExplanation),
}

/// Explain why a package is in the resolved graph, who pulled
/// it in, and which dependency edge introduced it.
#[derive(Debug, Clone, Serialize)]
pub struct PackageExplanation {
    pub name: String,
    pub version: String,
    pub source: SourceProvenance,
    /// Every minimal path from a selected root to this package,
    /// sorted by `(length, joined name sequence)` for stable
    /// output. Each element of the inner vec is one
    /// `(name, version, edge_kind)` step; the first element is a
    /// selected root and the last is the queried package.
    pub paths: Vec<Vec<ExplainStep>>,
    /// Whether this package is itself a selected root.
    pub is_selected_root: bool,
}

/// One step in a [`PackageExplanation::paths`] chain.
#[derive(Debug, Clone, Serialize)]
pub struct ExplainStep {
    pub name: String,
    pub version: String,
    /// Dependency kind under which this step was reached. `None`
    /// for the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_kind: Option<&'static str>,
}

/// Explain a target's owning package, kind, language summary,
/// and dependency edges. `cabin explain target <name>` only
/// considers targets in the selected package closure.
#[derive(Debug, Clone, Serialize)]
pub struct TargetExplanation {
    pub package: String,
    pub target: String,
    /// Target kind as the stable string the rest of Cabin uses
    /// (`library`, `executable`, `test`, …). Named `target_kind`
    /// rather than `kind` to avoid colliding with the
    /// [`Explanation`] tag field in the JSON shape.
    #[serde(rename = "target_kind")]
    pub target_kind: String,
    /// Names of source-language families the target carries
    /// (`c`, `cxx`, `rust`). Sorted alphabetically.
    pub languages: Vec<String>,
    /// Manifest-declared deps for this target, in declaration
    /// order. The orchestration layer normalizes each entry's
    /// rendering.
    pub deps: Vec<String>,
    /// `true` for every kind that produces a Ninja action
    /// (`library`, `executable`, `test`, `example`). `header-only`
    /// is the only buildable=false kind. `is_test` and
    /// `is_dev_only` further classify whether the target reaches
    /// the default `cabin build` selection.
    pub is_buildable: bool,
    /// `true` for `test` only.
    pub is_test: bool,
    /// `true` for the dev-only kinds (`test`, `example`).
    pub is_dev_only: bool,
}

/// Explain where a package's source bytes came from.
#[derive(Debug, Clone, Serialize)]
pub struct SourceExplanation {
    pub name: String,
    pub version: String,
    pub source: SourceProvenance,
    /// Active source-replacement entries the orchestration
    /// layer surfaced as relevant to this query (typically
    /// every entry in the merged config since one chain may
    /// rewrite many packages). Empty when no replacements are
    /// active.
    pub source_replacements: Vec<String>,
}

/// Explain a feature's enablement: declared, enabled, what it
/// implies, and which root pulled it in if any.
#[derive(Debug, Clone, Serialize)]
pub struct FeatureExplanation {
    pub package: String,
    pub feature: String,
    pub enabled: bool,
    /// Other features this feature implies, in declaration order.
    pub implies: Vec<String>,
    /// Whether this feature is a member of the package's
    /// `default` group.
    pub is_default: bool,
}

/// Build a [`PackageExplanation`] for `name`. Returns
/// [`ExplainError::PackageNotFound`] when the name is not in the
/// resolved graph; returns
/// [`ExplainError::AmbiguousPackageName`] if a future graph
/// gains multiple packages with the same name from distinct
/// sources (today the resolver enforces unique names).
///
/// # Errors
/// Returns [`ExplainError::PackageNotFound`] (with the known package
/// names as candidates) when no package in `graph` matches `name`, and
/// [`ExplainError::AmbiguousPackageName`] when more than one package
/// shares that name.
pub fn explain_package(
    graph: &PackageGraph,
    roots: &[usize],
    name: &str,
    active_patches: Option<&cabin_workspace::ActivePatchSet>,
    lockfile: Option<&Lockfile>,
) -> Result<PackageExplanation, ExplainError> {
    let target_idx = locate_package(graph, name)?;
    let pkg = &graph.packages[target_idx];
    let inputs = TreeInputs {
        graph,
        roots,
        lockfile,
        active_patches,
        kind_filter: None,
    };
    let source = source_provenance_for(pkg, &inputs);

    let effective_roots: Vec<usize> = if roots.is_empty() {
        graph.primary_packages.clone()
    } else {
        roots.to_vec()
    };
    let is_selected_root = effective_roots.contains(&target_idx);

    let mut paths: Vec<Vec<ExplainStep>> = Vec::new();
    for &root in &effective_roots {
        for path in shortest_paths_to(graph, root, target_idx) {
            paths.push(materialize_path(graph, &path));
        }
    }
    paths.sort_by(|a, b| {
        a.len()
            .cmp(&b.len())
            .then_with(|| join_path_names(a).cmp(&join_path_names(b)))
    });
    paths.dedup_by(|a, b| {
        a.len() == b.len()
            && a.iter()
                .zip(b.iter())
                .all(|(x, y)| x.name == y.name && x.version == y.version)
    });

    Ok(PackageExplanation {
        name: pkg.package.name.as_str().to_owned(),
        version: pkg.package.version.to_string(),
        source,
        paths,
        is_selected_root,
    })
}

fn join_path_names(steps: &[ExplainStep]) -> String {
    steps
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn locate_package(graph: &PackageGraph, name: &str) -> Result<usize, ExplainError> {
    let matches: Vec<usize> = graph
        .packages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.package.name.as_str() == name)
        .map(|(i, _)| i)
        .collect();
    match matches.len() {
        0 => Err(ExplainError::PackageNotFound {
            name: name.to_owned(),
            candidates: known_package_names(graph),
        }),
        1 => Ok(matches[0]),
        _ => {
            let mut versions: Vec<String> = matches
                .iter()
                .map(|&i| graph.packages[i].package.version.to_string())
                .collect();
            versions.sort();
            Err(ExplainError::AmbiguousPackageName {
                name: name.to_owned(),
                versions,
            })
        }
    }
}

/// The known package names, sorted, capped at 10. Surfaced as the
/// `candidates` list in `PackageNotFound`; it lists what *is* known
/// rather than ranking by similarity to the query (the resolver
/// package count is small enough that edit-distance would be
/// overkill), so the rendered error reads "known packages: …".
fn known_package_names(graph: &PackageGraph) -> Vec<String> {
    let mut names: Vec<String> = graph
        .packages
        .iter()
        .map(|p| p.package.name.as_str().to_owned())
        .collect();
    names.sort();
    names
        .into_iter()
        .filter(|n| !n.is_empty())
        .take(10)
        .collect()
}

/// Walk the graph from `start` to `target` and return every
/// shortest path of package indices. Edge kinds are recorded by
/// the caller through [`materialize_path`] so the resulting
/// [`ExplainStep`]s carry the right edge label.
fn shortest_paths_to(graph: &PackageGraph, start: usize, target: usize) -> Vec<Vec<usize>> {
    if start == target {
        return vec![vec![start]];
    }
    // BFS by depth to find the *shortest* paths first; collect
    // every parent at the depth where the target was reached.
    let mut depth: BTreeMap<usize, usize> = BTreeMap::new();
    let mut parents: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    depth.insert(start, 0);
    let mut frontier: Vec<usize> = vec![start];
    let mut found: bool = false;
    let mut level = 0usize;
    while !frontier.is_empty() && !found {
        let mut next: Vec<usize> = Vec::new();
        for &node in &frontier {
            for edge in &graph.packages[node].deps {
                let child = edge.index;
                let new_depth = level + 1;
                if let Some(&existing) = depth.get(&child) {
                    if existing == new_depth {
                        parents.entry(child).or_default().push(node);
                    }
                    continue;
                }
                depth.insert(child, new_depth);
                parents.entry(child).or_default().push(node);
                if child == target {
                    found = true;
                }
                next.push(child);
            }
        }
        frontier = next;
        level += 1;
    }
    if !depth.contains_key(&target) {
        return Vec::new();
    }
    // Reconstruct every shortest path by walking parents
    // backwards from target.
    let mut paths: Vec<Vec<usize>> = vec![vec![target]];
    loop {
        let mut next: Vec<Vec<usize>> = Vec::new();
        let mut grew = false;
        for path in &paths {
            let head = *path.first().expect("path is non-empty");
            if head == start {
                next.push(path.clone());
                continue;
            }
            let Some(parent_list) = parents.get(&head) else {
                continue;
            };
            for &p in parent_list {
                let mut extended = vec![p];
                extended.extend(path.iter().copied());
                next.push(extended);
                grew = true;
            }
        }
        paths = next;
        if !grew {
            break;
        }
    }
    paths
        .into_iter()
        .filter(|p| p.first().copied() == Some(start))
        .collect()
}

fn materialize_path(graph: &PackageGraph, path: &[usize]) -> Vec<ExplainStep> {
    let mut out: Vec<ExplainStep> = Vec::with_capacity(path.len());
    for (i, &idx) in path.iter().enumerate() {
        let pkg = &graph.packages[idx];
        let edge_kind = if i == 0 {
            None
        } else {
            let parent = &graph.packages[path[i - 1]];
            parent
                .deps
                .iter()
                .find(|e| e.index == idx)
                .map(|e| dep_kind_key(e.kind))
        };
        out.push(ExplainStep {
            name: pkg.package.name.as_str().to_owned(),
            version: pkg.package.version.to_string(),
            edge_kind,
        });
    }
    out
}

/// Build a [`TargetExplanation`] for `target_name`, scoped to
/// the selected packages. Returns
/// [`ExplainError::TargetNotFound`] if the name does not exist
/// in any selected package, with a list of candidate names for
/// the diagnostic.
///
/// # Errors
/// Returns [`ExplainError::TargetNotFound`] (with the available target
/// names as candidates) when no selected package declares a target named
/// `target_name`, and [`ExplainError::AmbiguousTargetName`] when more than
/// one selected package declares it.
pub fn explain_target(
    graph: &PackageGraph,
    selected_packages: &[usize],
    target_name: &str,
) -> Result<TargetExplanation, ExplainError> {
    let pool: Vec<usize> = if selected_packages.is_empty() {
        (0..graph.packages.len()).collect()
    } else {
        selected_packages.to_vec()
    };
    let mut hits: Vec<(usize, &cabin_core::Target)> = Vec::new();
    for idx in &pool {
        let pkg = &graph.packages[*idx];
        for target in &pkg.package.targets {
            if target.name.as_str() == target_name {
                hits.push((*idx, target));
            }
        }
    }
    if hits.is_empty() {
        let mut candidates: BTreeSet<String> = BTreeSet::new();
        for idx in &pool {
            for target in &graph.packages[*idx].package.targets {
                candidates.insert(target.name.as_str().to_owned());
            }
        }
        return Err(ExplainError::TargetNotFound {
            name: target_name.to_owned(),
            candidates: candidates.into_iter().collect(),
        });
    }
    if hits.len() > 1 {
        let owners: Vec<String> = hits
            .iter()
            .map(|(idx, _)| graph.packages[*idx].package.name.as_str().to_owned())
            .collect();
        return Err(ExplainError::AmbiguousTargetName {
            name: target_name.to_owned(),
            owners,
        });
    }
    let (pkg_idx, target) = hits[0];
    let pkg = &graph.packages[pkg_idx];
    let mut languages: BTreeSet<&'static str> = BTreeSet::new();
    for source in &target.sources {
        if let Some(lang) = cabin_core::classify_source(source) {
            languages.insert(lang.as_key());
        }
    }
    let kind = target.kind;
    Ok(TargetExplanation {
        package: pkg.package.name.as_str().to_owned(),
        target: target.name.as_str().to_owned(),
        target_kind: kind.as_str().to_owned(),
        languages: languages.into_iter().map(str::to_owned).collect(),
        deps: target.deps.clone(),
        // Buildable = anything that emits compile/archive/link
        // actions. Excludes the header-only kinds because they
        // contribute no translation units of their own.
        is_buildable: kind.produces_archive() || kind.produces_executable(),
        is_test: kind.is_test(),
        is_dev_only: kind.is_dev_only(),
    })
}

/// Build a [`SourceExplanation`] for the named package.
///
/// # Errors
/// Propagates [`ExplainError::PackageNotFound`] or
/// [`ExplainError::AmbiguousPackageName`] from `locate_package` when `name`
/// matches no package, or more than one package, in `graph`.
pub fn explain_source(
    graph: &PackageGraph,
    name: &str,
    active_patches: Option<&cabin_workspace::ActivePatchSet>,
    lockfile: Option<&Lockfile>,
    source_replacements: &cabin_core::SourceReplacementSettings,
) -> Result<SourceExplanation, ExplainError> {
    let idx = locate_package(graph, name)?;
    let pkg = &graph.packages[idx];
    let inputs = TreeInputs {
        graph,
        roots: &[],
        lockfile,
        active_patches,
        kind_filter: None,
    };
    let source = source_provenance_for(pkg, &inputs);
    let mut replacements: Vec<String> = source_replacements
        .entries
        .values()
        .map(|entry| {
            format!(
                "{} -> {} ({})",
                entry.original.display(),
                entry.replacement.display(),
                entry.provenance.as_key()
            )
        })
        .collect();
    replacements.sort();
    Ok(SourceExplanation {
        name: pkg.package.name.as_str().to_owned(),
        version: pkg.package.version.to_string(),
        source,
        source_replacements: replacements,
    })
}

/// Build a [`FeatureExplanation`] for `package/feature`. The
/// query string must contain a single `/` separating the package
/// name from the feature name; an unrecognized shape is rejected
/// with [`ExplainError::InvalidFeatureQuery`].
///
/// # Errors
/// Returns [`ExplainError::InvalidFeatureQuery`] when `query` lacks a `/`
/// separator; propagates [`ExplainError::PackageNotFound`] or
/// [`ExplainError::AmbiguousPackageName`] from `locate_package`; and
/// returns [`ExplainError::FeatureNotFound`] when the package does not
/// declare the named feature (and it is not the `default` group).
pub fn explain_feature(
    graph: &PackageGraph,
    feature_resolution: Option<&cabin_feature_per_package_view::FeatureView>,
    query: &str,
) -> Result<FeatureExplanation, ExplainError> {
    let (pkg_name, feature_name) =
        query
            .split_once('/')
            .ok_or_else(|| ExplainError::InvalidFeatureQuery {
                query: query.to_owned(),
            })?;
    let idx = locate_package(graph, pkg_name)?;
    let pkg = &graph.packages[idx];
    let package = &pkg.package;
    if !package.features.features.contains_key(feature_name)
        && feature_name != cabin_core::DEFAULT_FEATURE_KEY
    {
        let mut candidates: Vec<String> = package.features.features.keys().cloned().collect();
        candidates.sort();
        return Err(ExplainError::FeatureNotFound {
            package: pkg_name.to_owned(),
            feature: feature_name.to_owned(),
            candidates,
        });
    }
    let implies = if feature_name == cabin_core::DEFAULT_FEATURE_KEY {
        package.features.default.clone()
    } else {
        package
            .features
            .features
            .get(feature_name)
            .cloned()
            .unwrap_or_default()
    };
    let enabled = feature_resolution.is_some_and(|fv| fv.enabled.contains(feature_name));
    let is_default = package.features.default.iter().any(|n| n == feature_name);
    Ok(FeatureExplanation {
        package: pkg_name.to_owned(),
        feature: feature_name.to_owned(),
        enabled,
        implies,
        is_default,
    })
}

/// `cabin explain build-config <package>` returns the package's
/// resolved [`cabin_core::BuildConfiguration`]. The orchestration
/// layer already knows how to render it through
/// `BuildConfiguration::as_json`, so this crate just looks it up.
///
/// # Errors
/// Propagates [`ExplainError::PackageNotFound`] or
/// [`ExplainError::AmbiguousPackageName`] from `locate_package`, and
/// returns [`ExplainError::NoBuildConfiguration`] when `configurations`
/// holds no entry for the located package (typically because it lies
/// outside the selected closure).
pub fn explain_build_config<'a, S: BuildHasher>(
    configurations: &'a HashMap<usize, cabin_core::BuildConfiguration, S>,
    graph: &PackageGraph,
    name: &str,
) -> Result<&'a cabin_core::BuildConfiguration, ExplainError> {
    let idx = locate_package(graph, name)?;
    configurations
        .get(&idx)
        .ok_or_else(|| ExplainError::NoBuildConfiguration {
            name: name.to_owned(),
        })
}

/// Render an [`Explanation`] as a concise human-readable
/// summary suitable for terminal output.
pub fn render_explanation_human(exp: &Explanation) -> String {
    use std::fmt::Write as _;
    match exp {
        Explanation::Package(p) => {
            let mut out = String::new();
            push_name_version_source(&mut out, &p.name, &p.version, &p.source);
            if p.is_selected_root {
                out.push_str("  selected as a root package\n");
            }
            if p.paths.is_empty() {
                out.push_str("  no dependency path from any selected root reaches this package\n");
            } else {
                out.push_str("  dependency paths from selected roots:\n");
                for path in &p.paths {
                    out.push_str("    ");
                    for (i, step) in path.iter().enumerate() {
                        if i > 0 {
                            out.push_str(" -> ");
                        }
                        push_name_version_kind(&mut out, &step.name, &step.version, step.edge_kind);
                    }
                    out.push('\n');
                }
            }
            out
        }
        Explanation::Target(t) => {
            let mut out = String::new();
            let _ = writeln!(out, "{}:{}  kind = {}", t.package, t.target, t.target_kind);
            if !t.languages.is_empty() {
                let _ = writeln!(out, "  languages: {}", t.languages.join(", "));
            }
            if !t.deps.is_empty() {
                let _ = writeln!(out, "  deps: {}", t.deps.join(", "));
            }
            let _ = writeln!(
                out,
                "  flags: buildable={}, test={}, dev-only={}",
                t.is_buildable, t.is_test, t.is_dev_only
            );
            out
        }
        Explanation::Source(s) => {
            let mut out = String::new();
            push_name_version_source(&mut out, &s.name, &s.version, &s.source);
            if !s.source_replacements.is_empty() {
                out.push_str("  active source-replacement entries:\n");
                for entry in &s.source_replacements {
                    let _ = writeln!(out, "    {entry}");
                }
            }
            out
        }
        Explanation::Feature(f) => {
            let mut out = String::new();
            let _ = writeln!(
                out,
                "{}/{}  enabled={}, default={}",
                f.package, f.feature, f.enabled, f.is_default
            );
            if !f.implies.is_empty() {
                let _ = writeln!(out, "  implies: {}", f.implies.join(", "));
            }
            out
        }
    }
}

/// Render an [`Explanation`] as a stable JSON document.
///
/// # Panics
/// Panics if the [`Explanation`] fails to serialize via `serde_json::to_value`,
/// which cannot happen because [`Explanation`] derives [`Serialize`] with no
/// fallible custom serializer.
pub fn render_explanation_json(exp: &Explanation) -> serde_json::Value {
    serde_json::to_value(exp).expect("Explanation is Serialize")
}

/// Errors produced by the explain queries. Wording is stable
/// so integration tests can match on substrings.
#[derive(Debug, Error)]
pub enum ExplainError {
    /// The named package is not in the resolved graph.
    #[error(
        "package `{name}` was not found in the resolved graph; known packages: {}",
        candidates.join(", ")
    )]
    PackageNotFound {
        name: String,
        candidates: Vec<String>,
    },
    /// More than one package shares the same name. This is
    /// rejected by the resolver today, but the variant is
    /// retained so future graph changes have a clear failure
    /// mode.
    #[error(
        "package name `{name}` matches multiple packages with versions: {}",
        versions.join(", ")
    )]
    AmbiguousPackageName { name: String, versions: Vec<String> },
    /// The named target does not exist in any package the user
    /// selected.
    #[error(
        "target `{name}` was not found in the selected packages; available: {}",
        candidates.join(", ")
    )]
    TargetNotFound {
        name: String,
        candidates: Vec<String>,
    },
    /// Multiple selected packages declare a target with the
    /// same name.
    #[error(
        "target name `{name}` is ambiguous; declared by packages: {}",
        owners.join(", ")
    )]
    AmbiguousTargetName { name: String, owners: Vec<String> },
    /// `cabin explain feature <package/feature>` query string
    /// did not contain a `/` separator.
    #[error(
        "feature query `{query}` must use the `package/feature` form (use `default` to ask about the default feature group)"
    )]
    InvalidFeatureQuery { query: String },
    /// The named feature does not exist on the named package.
    #[error(
        "feature `{feature}` was not declared by package `{package}`; available: {}",
        candidates.join(", ")
    )]
    FeatureNotFound {
        package: String,
        feature: String,
        candidates: Vec<String>,
    },
    /// The orchestration layer did not compute a build
    /// configuration for this package. Today this happens only
    /// when the user asks about a package outside the selected
    /// closure.
    #[error(
        "no build configuration was resolved for package `{name}`; check the workspace selection"
    )]
    NoBuildConfiguration { name: String },
}

/// Stand-in module that names the per-package feature view this
/// crate consumes through a thin `&FeatureView` parameter. The
/// orchestration layer (`cabin`) already builds the typed
/// `cabin_feature::FeatureResolution`; rather than depending on
/// the full crate from a query-only library, we accept an
/// abstract view object so the orchestration layer can adapt the
/// existing types into our shape without leaking the resolver
/// crate boundary.
pub mod cabin_feature_per_package_view {
    use std::collections::BTreeSet;

    /// Per-package feature view consumed by [`super::explain_feature`].
    pub struct FeatureView {
        /// Names of features enabled on this package by the
        /// resolver. Empty when the resolver did not visit the
        /// package.
        pub enabled: BTreeSet<String>,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{Dependency, DependencyKind, DependencySource, Package, PackageName};
    use cabin_workspace::{DependencyEdge, PackageGraph, PackageKind, WorkspacePackage};
    use camino::Utf8PathBuf;

    fn pkg_name(s: &str) -> PackageName {
        PackageName::new(s.to_owned()).unwrap()
    }

    fn make_pkg(name: &str, version: &str, deps: &[(&str, DependencyKind)]) -> WorkspacePackage {
        let package = Package::new(
            pkg_name(name),
            semver::Version::parse(version).unwrap(),
            Vec::new(),
            deps.iter()
                .map(|(n, k)| Dependency {
                    name: pkg_name(n),
                    source: DependencySource::Path(Utf8PathBuf::from(format!("../{n}"))),
                    kind: *k,
                    optional: false,
                    features: Vec::new(),
                    default_features: true,
                    condition: None,
                })
                .collect(),
        )
        .unwrap();
        WorkspacePackage {
            package,
            manifest_path: PathBuf::from(format!("/abs/{name}/cabin.toml")),
            manifest_dir: PathBuf::from(format!("/abs/{name}")),
            deps: Vec::new(),
            kind: PackageKind::Local,
            is_port: false,
        }
    }

    fn three_pkg_graph() -> PackageGraph {
        // app -> lib -> util
        // Indices: app=0, lib=1, util=2.
        let mut app = make_pkg("app", "0.1.0", &[("lib", DependencyKind::Normal)]);
        let mut lib = make_pkg("lib", "0.2.0", &[("util", DependencyKind::Normal)]);
        let util = make_pkg("util", "0.3.0", &[]);
        app.deps = vec![DependencyEdge {
            index: 1,
            kind: DependencyKind::Normal,
            condition: None,
        }];
        lib.deps = vec![DependencyEdge {
            index: 2,
            kind: DependencyKind::Normal,
            condition: None,
        }];
        let packages = vec![app, lib, util];
        PackageGraph {
            root_manifest_path: PathBuf::from("/abs/app/cabin.toml"),
            root_dir: PathBuf::from("/abs/app"),
            is_workspace_root: false,
            root_package: Some(0),
            root_settings: cabin_workspace::RootSettings::default(),
            primary_packages: vec![0],
            default_members: Vec::new(),
            excluded_members: Vec::new(),
            packages,
        }
    }

    #[test]
    fn build_tree_orders_children_by_kind_then_name() {
        let graph = three_pkg_graph();
        let forest = build_tree(&TreeInputs {
            graph: &graph,
            roots: &[],
            lockfile: None,
            active_patches: None,
            kind_filter: None,
        });
        assert_eq!(forest.len(), 1);
        let root = &forest[0];
        assert_eq!(root.name, "app");
        let kinds: Vec<&'static str> = root.children.iter().map(|c| c.edge_kind.unwrap()).collect();
        assert_eq!(kinds, vec!["normal"]);
        // lib's child appears under lib.
        assert_eq!(root.children[0].children[0].name, "util");
    }

    #[test]
    fn build_tree_filters_by_dependency_kind() {
        let graph = three_pkg_graph();
        let forest = build_tree(&TreeInputs {
            graph: &graph,
            roots: &[],
            lockfile: None,
            active_patches: None,
            kind_filter: Some(DependencyKind::Normal),
        });
        let root = &forest[0];
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].name, "lib");
    }

    #[test]
    fn port_packages_render_with_port_provenance_and_nest() {
        // app -> libport (a prepared foundation port). The port must
        // nest under its consumer and carry `SourceProvenance::Port`,
        // rendered as `(port)`.
        let mut app = make_pkg("app", "0.1.0", &[]);
        app.deps = vec![DependencyEdge {
            index: 1,
            kind: DependencyKind::Normal,
            condition: None,
        }];
        let mut libport = make_pkg("libport", "1.2.3", &[]);
        libport.is_port = true;
        let graph = PackageGraph {
            root_manifest_path: PathBuf::from("/abs/app/cabin.toml"),
            root_dir: PathBuf::from("/abs/app"),
            is_workspace_root: false,
            root_package: Some(0),
            root_settings: cabin_workspace::RootSettings::default(),
            primary_packages: vec![0],
            default_members: Vec::new(),
            excluded_members: Vec::new(),
            packages: vec![app, libport],
        };
        let forest = build_tree(&TreeInputs {
            graph: &graph,
            roots: &[],
            lockfile: None,
            active_patches: None,
            kind_filter: None,
        });
        let root = &forest[0];
        assert_eq!(root.children.len(), 1, "port should nest under app");
        let port_node = &root.children[0];
        assert_eq!(port_node.name, "libport");
        assert_eq!(port_node.source, SourceProvenance::Port);
        let rendered = render_tree_human(&forest);
        assert!(
            rendered.contains("└── libport v1.2.3") && rendered.contains("(port)"),
            "rendered:\n{rendered}"
        );
    }

    #[test]
    fn render_tree_human_is_deterministic_and_uses_box_chars() {
        let graph = three_pkg_graph();
        let forest = build_tree(&TreeInputs {
            graph: &graph,
            roots: &[],
            lockfile: None,
            active_patches: None,
            kind_filter: None,
        });
        let a = render_tree_human(&forest);
        let b = render_tree_human(&forest);
        assert_eq!(a, b, "render must be deterministic");
        assert!(a.contains("app v0.1.0"));
        assert!(a.contains("lib v0.2.0 [normal]"));
        // The second-to-last child uses `└──` because there's
        // exactly one normal-kind dep under app.lib.
        assert!(a.contains("└── util"));
    }

    #[test]
    fn explain_package_returns_dep_path_from_root() {
        let graph = three_pkg_graph();
        let exp = explain_package(&graph, &[0], "util", None, None).unwrap();
        assert_eq!(exp.name, "util");
        assert!(!exp.is_selected_root);
        assert_eq!(exp.paths.len(), 1);
        let path = &exp.paths[0];
        assert_eq!(
            path.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["app", "lib", "util"]
        );
        assert_eq!(path[1].edge_kind, Some("normal"));
        assert_eq!(path[2].edge_kind, Some("normal"));
    }

    #[test]
    fn explain_package_marks_selected_root() {
        let graph = three_pkg_graph();
        let exp = explain_package(&graph, &[0], "app", None, None).unwrap();
        assert!(exp.is_selected_root);
        // Path from root to root has a single step.
        assert_eq!(exp.paths.len(), 1);
        assert_eq!(exp.paths[0].len(), 1);
    }

    #[test]
    fn explain_package_returns_actionable_error_for_unknown_name() {
        let graph = three_pkg_graph();
        let err = explain_package(&graph, &[0], "missing", None, None).unwrap_err();
        match err {
            ExplainError::PackageNotFound { name, candidates } => {
                assert_eq!(name, "missing");
                assert!(candidates.contains(&"app".to_owned()));
                assert!(candidates.contains(&"lib".to_owned()));
            }
            other => panic!("expected PackageNotFound, got {other:?}"),
        }
    }

    #[test]
    fn explain_target_returns_owning_package_and_kind_flags() {
        let graph = three_pkg_graph();
        // Build a small fixture with one target so we can hit
        // the explain_target path. Re-use the helper graph and
        // append a target by mutating a clone.
        let mut graph = graph;
        let target = cabin_core::Target {
            name: cabin_core::TargetName::new("util").unwrap(),
            kind: cabin_core::TargetKind::Library,
            sources: vec![
                Utf8PathBuf::from("src/util.c"),
                Utf8PathBuf::from("src/util.cc"),
            ],
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            language: cabin_core::LanguageStandardSettings::default(),
        };
        graph.packages[2].package.targets.push(target);
        let exp = explain_target(&graph, &[2], "util").unwrap();
        assert_eq!(exp.package, "util");
        assert_eq!(exp.target, "util");
        assert_eq!(exp.target_kind, "library");
        assert_eq!(exp.languages, vec!["c".to_owned(), "cxx".to_owned()]);
        assert!(exp.is_buildable);
        assert!(!exp.is_test);
        assert!(!exp.is_dev_only);
    }

    #[test]
    fn explain_target_unknown_lists_available_candidates() {
        let mut graph = three_pkg_graph();
        let lib_target = cabin_core::Target {
            name: cabin_core::TargetName::new("lib_lib").unwrap(),
            kind: cabin_core::TargetKind::Library,
            sources: vec![Utf8PathBuf::from("src/lib.cc")],
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: Vec::new(),
            language: cabin_core::LanguageStandardSettings::default(),
        };
        graph.packages[1].package.targets.push(lib_target);
        let err = explain_target(&graph, &[1], "missing").unwrap_err();
        match err {
            ExplainError::TargetNotFound { name, candidates } => {
                assert_eq!(name, "missing");
                assert_eq!(candidates, vec!["lib_lib".to_owned()]);
            }
            other => panic!("expected TargetNotFound, got {other:?}"),
        }
    }

    #[test]
    fn explain_feature_invalid_query_form_is_rejected() {
        let graph = three_pkg_graph();
        let err = explain_feature(&graph, None, "noseparator").unwrap_err();
        assert!(matches!(err, ExplainError::InvalidFeatureQuery { .. }));
    }
}
