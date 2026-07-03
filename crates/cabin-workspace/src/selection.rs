//! `WorkspacePackage` selection across a [`PackageGraph`].
//!
//! `cabin` translates user flags (`--workspace`, `--package`,
//! `--exclude`, `--default-members`) into a [`PackageSelection`]
//! and hands it to [`resolve_package_selection`], which validates
//! the request against the graph and returns the deterministic
//! ordered list of selected primary-package indices.  Centralizing
//! this here keeps CLI code free of workspace-graph algorithms.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use crate::error::WorkspaceError;
use crate::graph::PackageGraph;

/// Selection mode the user requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionMode {
    /// Default behavior:
    ///
    /// - inside a single-package package (no `[workspace]`), select
    ///   the root package;
    /// - at a workspace root, select `[workspace.default-members]`
    ///   when present, otherwise fall back to **all** workspace
    ///   members.  The fallback rule is documented in
    ///   [`docs/workspaces.md`](../../../docs/workspaces.md).
    CurrentPackage,
    /// `--default-members`.  Errors when the workspace declares no
    /// `[workspace.default-members]`.
    DefaultMembers,
    /// `--workspace`.  Selects every workspace member, then applies
    /// `--exclude` filtering.
    WholeWorkspace,
    /// `-p` / `--package`.  Selects exactly the named packages (each
    /// must be a workspace member).
    ExplicitPackages(Vec<String>),
}

/// User-facing selection request, before validation against any
/// concrete graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSelection {
    pub mode: SelectionMode,
    /// Packages to drop from the resolved selection.  Only valid in
    /// combination with `WholeWorkspace` and `DefaultMembers`.
    pub exclude: Vec<String>,
}

impl PackageSelection {
    pub fn current_package() -> Self {
        Self {
            mode: SelectionMode::CurrentPackage,
            exclude: Vec::new(),
        }
    }
}

/// Final, validated selection.  Indices are into [`PackageGraph::packages`]
/// and are ordered to match the graph's primary-package ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSelection {
    pub packages: Vec<usize>,
}

impl ResolvedSelection {
    /// Closure of the selection over local
    /// path-dependency edges.  Includes every package reachable from
    /// `self.packages` by walking `WorkspacePackage::deps` transitively, in
    /// deterministic ascending-index order.  Workspace siblings that
    /// the selection neither names nor pulls in via path deps are
    /// **not** in the closure - that is the whole point of this
    /// helper.
    pub fn closure(&self, graph: &PackageGraph) -> BTreeSet<usize> {
        let mut closure: BTreeSet<usize> = BTreeSet::new();
        let mut stack: Vec<usize> = self.packages.clone();
        while let Some(idx) = stack.pop() {
            if !closure.insert(idx) {
                continue;
            }
            for edge in &graph.packages[idx].deps {
                if !closure.contains(&edge.index) {
                    stack.push(edge.index);
                }
            }
        }
        closure
    }

    /// Names of every package in the selection's path-dependency
    /// [`closure`](Self::closure), in deterministic order.  Convenience
    /// over `closure(graph)` for the common case where a caller needs a
    /// set of package *names* - e.g. to seed a strict registry / port
    /// policy - rather than graph indices.
    pub fn closure_package_names(&self, graph: &PackageGraph) -> BTreeSet<String> {
        self.closure(graph)
            .into_iter()
            .map(|i| graph.packages[i].package.name.as_str().to_owned())
            .collect()
    }
}

/// Validate a [`PackageSelection`] against `graph` and return the
/// concrete list of selected primary-package indices.  Errors are
/// emitted with deterministic, actionable messages so the user can
/// fix typos quickly.
///
/// # Errors
/// Returns a [`WorkspaceError`] when the selection is invalid:
/// [`WorkspaceError::ExcludeWithoutWorkspaceSelection`] for
/// `--exclude` outside a workspace selection,
/// [`WorkspaceError::DefaultMembersWithoutWorkspace`] or
/// [`WorkspaceError::DefaultMemberNotInMembers`] for default-member
/// modes that don't apply, [`WorkspaceError::PackageNotInWorkspace`]
/// for an unknown or non-primary named/excluded package, and
/// [`WorkspaceError::AmbiguousPackageSelection`] when the selection
/// resolves to no packages.
pub fn resolve_package_selection(
    graph: &PackageGraph,
    selection: &PackageSelection,
) -> Result<ResolvedSelection, WorkspaceError> {
    // `--exclude` requires an explicit
    // `--workspace` or `--default-members` mode.  The
    // implicit-default `CurrentPackage` mode no longer accepts
    // `--exclude`: the user must opt into a multi-package
    // selection before excluding from it.  Stricter behavior
    // matches Cargo and stops `cabin <cmd> --exclude foo` from
    // silently doing the wrong thing on a single-package package.
    let exclusion_compatible = matches!(
        selection.mode,
        SelectionMode::WholeWorkspace | SelectionMode::DefaultMembers
    );
    if !selection.exclude.is_empty() && !exclusion_compatible {
        return Err(WorkspaceError::ExcludeWithoutWorkspaceSelection);
    }

    let exclude_indices = exclude_indices(graph, &selection.exclude)?;

    // Borrow the graph's index lists where possible; only the
    // `ExplicitPackages` arm needs an owned, freshly-built list.
    let candidates: Cow<'_, [usize]> = match &selection.mode {
        SelectionMode::CurrentPackage => Cow::Borrowed(current_package_default(graph)),
        SelectionMode::DefaultMembers => {
            if !graph.is_workspace_root {
                return Err(WorkspaceError::DefaultMembersWithoutWorkspace);
            }
            if graph.default_members.is_empty() {
                return Err(WorkspaceError::DefaultMemberNotInMembers {
                    member: "<no default-members declared>".to_owned(),
                });
            }
            Cow::Borrowed(graph.default_members.as_slice())
        }
        SelectionMode::WholeWorkspace => {
            if graph.is_workspace_root {
                Cow::Borrowed(graph.primary_packages.as_slice())
            } else {
                // `--workspace` against a single-package package
                // selects that package - keeps CI users from
                // having to special-case a non-workspace tree.
                Cow::Borrowed(current_package_default(graph))
            }
        }
        SelectionMode::ExplicitPackages(names) => {
            let mut out = Vec::with_capacity(names.len());
            for name in names {
                let idx =
                    graph
                        .index_of(name)
                        .ok_or_else(|| WorkspaceError::PackageNotInWorkspace {
                            name: name.clone(),
                            members: workspace_member_names(graph),
                        })?;
                if !graph.primary_packages.contains(&idx) {
                    return Err(WorkspaceError::PackageNotInWorkspace {
                        name: name.clone(),
                        members: workspace_member_names(graph),
                    });
                }
                if !out.contains(&idx) {
                    out.push(idx);
                }
            }
            Cow::Owned(out)
        }
    };

    let mut packages: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|i| !exclude_indices.contains(i))
        .collect();
    // Stable, deterministic ordering: by package name.
    packages.sort_by(|a, b| {
        graph.packages[*a]
            .package
            .name
            .as_str()
            .cmp(graph.packages[*b].package.name.as_str())
    });
    if packages.is_empty() {
        return Err(WorkspaceError::AmbiguousPackageSelection);
    }
    Ok(ResolvedSelection { packages })
}

fn current_package_default(graph: &PackageGraph) -> &[usize] {
    if graph.is_workspace_root {
        if graph.default_members.is_empty() {
            // Documented fallback: all workspace members
            // when default-members is absent.
            &graph.primary_packages
        } else {
            &graph.default_members
        }
    } else if let Some(root) = &graph.root_package {
        std::slice::from_ref(root)
    } else {
        &graph.primary_packages
    }
}

fn exclude_indices(
    graph: &PackageGraph,
    excludes: &[String],
) -> Result<BTreeSet<usize>, WorkspaceError> {
    let mut out = BTreeSet::new();
    for name in excludes {
        let idx = graph
            .index_of(name)
            .ok_or_else(|| WorkspaceError::PackageNotInWorkspace {
                name: name.clone(),
                members: workspace_member_names(graph),
            })?;
        if !graph.primary_packages.contains(&idx) {
            return Err(WorkspaceError::PackageNotInWorkspace {
                name: name.clone(),
                members: workspace_member_names(graph),
            });
        }
        out.insert(idx);
    }
    Ok(out)
}

/// Combine several version-requirement strings into one
/// [`semver::VersionReq`] by joining them with `, ` (the comma form
/// semver reads as an AND of comparators) and re-parsing.  On a parse
/// failure the joined string is returned alongside the error so each
/// caller can build its own diagnostic.  This is the single
/// join-on-collision kernel shared by the closure and patch
/// requirement aggregators and the CLI's root-dep merge.
///
/// # Errors
/// Returns `Err((joined, source))` - the comma-joined requirement
/// string paired with the [`semver::Error`] - when the joined form
/// is not a valid [`semver::VersionReq`] (the requirements are
/// mutually incompatible).
pub fn combine_version_reqs(
    reqs: &[String],
) -> Result<semver::VersionReq, (String, semver::Error)> {
    let joined = reqs.join(", ");
    match semver::VersionReq::parse(&joined) {
        Ok(req) => Ok(req),
        Err(source) => Err((joined, source)),
    }
}

/// The per-dependency eligibility predicate shared by the versioned-dep
/// aggregators.  Returns the [`semver::VersionReq`] when `dep` is an active
/// registry-versioned dependency for this invocation - right kind (normal
/// kinds, plus `Dev` when `dev_active_here`), matches the host platform,
/// optional only if enabled, and not excluded - otherwise `None`. `idx` is
/// the declaring package's closure index, threaded so the optional gate can
/// consult `is_optional_dep_enabled`.
fn versioned_dep_active<'a, F>(
    dep: &'a cabin_core::Dependency,
    idx: usize,
    dev_active_here: bool,
    host: &cabin_core::TargetPlatform,
    is_optional_dep_enabled: &F,
    excluded_names: &BTreeSet<String>,
) -> Option<&'a semver::VersionReq>
where
    F: Fn(usize, &str) -> bool,
{
    use cabin_core::{DependencyKind, DependencySource};
    let kind_active =
        dep.kind.is_resolved_by_default() || (dev_active_here && dep.kind == DependencyKind::Dev);
    if !kind_active {
        return None;
    }
    if !dep.matches_platform(host) {
        return None;
    }
    if dep.optional && !is_optional_dep_enabled(idx, dep.name.as_str()) {
        return None;
    }
    if excluded_names.contains(dep.name.as_str()) {
        return None;
    }
    match &dep.source {
        DependencySource::Version(req) => Some(req),
        _ => None,
    }
}

/// Enumerate the versioned dependencies that drive
/// resolve / fetch / update for a selected package set.  Walks the
/// closure (selection + transitive local path deps) so a registry
/// dep declared by a path-dep `lib` is visible when the user
/// selected `app`.
///
/// Only dependency kinds that participate in ordinary resolution
/// (`Normal`, `Build`, `Tool`) are included.  Dev dependencies are
/// excluded so a workspace member's dev-only requirement cannot
/// break an ordinary `cabin build` / `cabin fetch`.  System
/// dependencies never reach this path because they are never
/// stored as `DependencySource::Version`.
///
/// Optional dependencies are filtered using `is_optional_dep_enabled`:
/// the closure is `(declaring_package_index, dep_name) -> included`.
/// Pass `|_, _| false` to include only non-optional deps; pass a
/// closure backed by a feature resolution to include only optional
/// deps the user asked for.
///
/// Conflicting requirements for the same name (across different
/// packages or kinds) are joined with `, ` - a form
/// `semver::VersionReq` accepts - and re-parsed;
/// incompatible requirements surface as a clear parse error
/// rather than silent unification.
///
/// `excluded_names` drops every dependency name in the set -
/// typically used by the artifact pipeline to skip patched
/// packages that ship from a local working copy and never need
/// to be fetched from the index.
///
/// `dev_active_for` opts in `[dev-dependencies]` for the named
/// packages (typically the `cabin test` selection).  Dev deps for
/// packages not in this set stay declaration-only, matching the
/// `cabin build` policy.
///
/// # Errors
/// Returns [`WorkspaceError::IncompatibleWorkspaceRequirements`]
/// when the requirements collected for a single dependency name
/// cannot be combined into one [`semver::VersionReq`] (the joined
/// requirement string fails to parse).
///
/// # Panics
/// Panics only if the name-lookup invariant were violated: every
/// dependency name pushed into `combined` is inserted into
/// `name_lookup` in the same loop iteration, so the `.unwrap()` on
/// `name_lookup.remove(&name)` always finds the key.
pub fn collect_closure_versioned_deps_excluding_with_dev<F>(
    graph: &PackageGraph,
    closure: &BTreeSet<usize>,
    is_optional_dep_enabled: F,
    excluded_names: &BTreeSet<String>,
    dev_active_for: &BTreeSet<String>,
) -> Result<BTreeMap<cabin_core::PackageName, semver::VersionReq>, WorkspaceError>
where
    F: Fn(usize, &str) -> bool,
{
    // Conditional dependencies are evaluated against the host
    // platform - Cabin does not yet support cross-compilation.
    let host_platform = cabin_core::TargetPlatform::current();
    let mut combined: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut name_lookup: BTreeMap<String, cabin_core::PackageName> = BTreeMap::new();
    for &idx in closure {
        let pkg = &graph.packages[idx];
        // Skip registry packages - their declared deps are already
        // covered by the registry's own metadata, not by the
        // workspace user's manifests.
        if !matches!(pkg.kind, crate::graph::PackageKind::Local) {
            continue;
        }
        let dev_active_here = dev_active_for.contains(pkg.package.name.as_str());
        for dep in &pkg.package.dependencies {
            if let Some(req) = versioned_dep_active(
                dep,
                idx,
                dev_active_here,
                &host_platform,
                &is_optional_dep_enabled,
                excluded_names,
            ) {
                let key = dep.name.as_str().to_owned();
                combined
                    .entry(key.clone())
                    .or_default()
                    .push(req.to_string());
                name_lookup.insert(key, dep.name.clone());
            }
        }
    }
    let mut out = BTreeMap::new();
    for (name, mut reqs) in combined {
        reqs.sort();
        reqs.dedup();
        let parsed = combine_version_reqs(&reqs).map_err(|(requirements, source)| {
            WorkspaceError::IncompatibleWorkspaceRequirements {
                name: name.clone(),
                requirements,
                source,
            }
        })?;
        out.insert(name_lookup.remove(&name).unwrap(), parsed);
    }
    Ok(out)
}

/// Whether the supplied closure carries any versioned
/// (registry-bound) dependency that the artifact pipeline would
/// need to fetch.  Mirrors
/// [`collect_closure_versioned_deps_excluding_with_dev`] but
/// returns a `bool` so the CLI can short-circuit before opening
/// an index.
///
/// `dev_active_for` follows the same opt-in policy as
/// [`collect_closure_versioned_deps_excluding_with_dev`].
pub fn closure_has_versioned_deps_excluding_with_dev<F>(
    graph: &PackageGraph,
    closure: &BTreeSet<usize>,
    is_optional_dep_enabled: F,
    excluded_names: &BTreeSet<String>,
    dev_active_for: &BTreeSet<String>,
) -> bool
where
    F: Fn(usize, &str) -> bool,
{
    let host_platform = cabin_core::TargetPlatform::current();
    closure.iter().any(|&idx| {
        let pkg = &graph.packages[idx];
        if !matches!(pkg.kind, crate::graph::PackageKind::Local) {
            return false;
        }
        let dev_active_here = dev_active_for.contains(pkg.package.name.as_str());
        pkg.package.dependencies.iter().any(|dep| {
            versioned_dep_active(
                dep,
                idx,
                dev_active_here,
                &host_platform,
                &is_optional_dep_enabled,
                excluded_names,
            )
            .is_some()
        })
    })
}

fn workspace_member_names(graph: &PackageGraph) -> Vec<String> {
    let mut names: Vec<String> = graph
        .primary_packages
        .iter()
        .map(|i| graph.packages[*i].package.name.as_str().to_owned())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;
    use crate::loader::load_workspace;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    fn workspace_with_two_members(default_members: Option<&str>) -> TempDir {
        let dir = TempDir::new().unwrap();
        let mut root = String::from("[workspace]\nmembers = [\"packages/*\"]\n");
        if let Some(dm) = default_members {
            writeln!(root, "default-members = [\"packages/{dm}\"]").unwrap();
        }
        dir.child("cabin.toml").write_str(&root).unwrap();
        dir.child("packages/a/cabin.toml")
            .write_str("[package]\nname = \"a\"\nversion = \"0.1.0\"\n")
            .unwrap();
        dir.child("packages/b/cabin.toml")
            .write_str("[package]\nname = \"b\"\nversion = \"0.1.0\"\n")
            .unwrap();
        dir
    }

    fn names(graph: &PackageGraph, sel: &ResolvedSelection) -> Vec<String> {
        sel.packages
            .iter()
            .map(|i| graph.packages[*i].package.name.as_str().to_owned())
            .collect()
    }

    #[test]
    fn current_package_falls_back_to_all_members_without_defaults() {
        let dir = workspace_with_two_members(None);
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(&graph, &PackageSelection::current_package()).unwrap();
        assert_eq!(names(&graph, &sel), vec!["a", "b"]);
    }

    #[test]
    fn current_package_uses_declared_defaults() {
        let dir = workspace_with_two_members(Some("a"));
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(&graph, &PackageSelection::current_package()).unwrap();
        assert_eq!(names(&graph, &sel), vec!["a"]);
    }

    #[test]
    fn whole_workspace_selects_all_members() {
        let dir = workspace_with_two_members(Some("a"));
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::WholeWorkspace,
                exclude: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(names(&graph, &sel), vec!["a", "b"]);
    }

    #[test]
    fn whole_workspace_with_exclude_drops_member() {
        let dir = workspace_with_two_members(None);
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::WholeWorkspace,
                exclude: vec!["b".into()],
            },
        )
        .unwrap();
        assert_eq!(names(&graph, &sel), vec!["a"]);
    }

    #[test]
    fn explicit_package_selects_named_member() {
        let dir = workspace_with_two_members(None);
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["a".into()]),
                exclude: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(names(&graph, &sel), vec!["a"]);
    }

    #[test]
    fn explicit_package_unknown_errors() {
        let dir = workspace_with_two_members(None);
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let err = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["nope".into()]),
                exclude: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, WorkspaceError::PackageNotInWorkspace { .. }));
    }

    #[test]
    fn default_members_mode_errors_when_none_declared() {
        let dir = workspace_with_two_members(None);
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let err = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::DefaultMembers,
                exclude: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            WorkspaceError::DefaultMemberNotInMembers { .. }
        ));
    }

    #[test]
    fn exclude_with_explicit_packages_errors() {
        let dir = workspace_with_two_members(None);
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let err = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["a".into()]),
                exclude: vec!["b".into()],
            },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            WorkspaceError::ExcludeWithoutWorkspaceSelection
        ));
    }

    // -----------------------------------------------------------------
    // closure + versioned-deps helpers.
    // -----------------------------------------------------------------

    /// Workspace where `app` depends on `lib` via path.  Selecting
    /// `app` must include `lib` in the closure; `unrelated` must
    /// not be in the closure.
    fn three_member_workspace_app_lib_unrelated() -> TempDir {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/*"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
lib = { path = "../lib" }
"#,
            )
            .unwrap();
        dir.child("packages/lib/cabin.toml")
            .write_str(
                r#"[package]
name = "lib"
version = "0.1.0"

[dependencies]
fmt = ">=10 <11"
"#,
            )
            .unwrap();
        dir.child("packages/unrelated/cabin.toml")
            .write_str(
                r#"[package]
name = "unrelated"
version = "0.1.0"

[dependencies]
spdlog = "^1"
"#,
            )
            .unwrap();
        dir
    }

    #[test]
    fn closure_includes_local_path_dependency() {
        let dir = three_member_workspace_app_lib_unrelated();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["app".into()]),
                exclude: Vec::new(),
            },
        )
        .unwrap();
        let closure = sel.closure(&graph);
        let names: Vec<&str> = closure
            .iter()
            .map(|i| graph.packages[*i].package.name.as_str())
            .collect();
        assert!(names.contains(&"app"), "closure missing app: {names:?}");
        assert!(names.contains(&"lib"), "closure missing lib: {names:?}");
        assert!(
            !names.contains(&"unrelated"),
            "closure leaked unrelated: {names:?}"
        );
    }

    #[test]
    fn versioned_deps_walks_path_dep_closure() {
        let dir = three_member_workspace_app_lib_unrelated();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["app".into()]),
                exclude: Vec::new(),
            },
        )
        .unwrap();
        let closure = sel.closure(&graph);
        let deps = collect_closure_versioned_deps_excluding_with_dev(
            &graph,
            &closure,
            |_, _| false,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap();
        let keys: Vec<&str> = deps.keys().map(cabin_core::PackageName::as_str).collect();
        assert_eq!(keys, vec!["fmt"], "expected only fmt, got {keys:?}");
    }

    #[test]
    fn versioned_deps_skip_unrelated_workspace_members() {
        let dir = three_member_workspace_app_lib_unrelated();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["app".into()]),
                exclude: Vec::new(),
            },
        )
        .unwrap();
        let closure = sel.closure(&graph);
        let deps = collect_closure_versioned_deps_excluding_with_dev(
            &graph,
            &closure,
            |_, _| false,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap();
        assert!(
            !deps.contains_key(&cabin_core::PackageName::new("spdlog").unwrap()),
            "unrelated spdlog leaked into closure deps"
        );
    }

    /// Dev dependencies are excluded from ordinary resolution.
    /// The closure walker must respect that policy so a workspace
    /// member's `[dev-dependencies]` requirement cannot block an
    /// ordinary `cabin build` / `cabin fetch`.
    #[test]
    fn versioned_deps_excludes_dev_kind() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"

[dev-dependencies]
gtest = "^1.14"
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["app".into()]),
                exclude: Vec::new(),
            },
        )
        .unwrap();
        let closure = sel.closure(&graph);
        let deps = collect_closure_versioned_deps_excluding_with_dev(
            &graph,
            &closure,
            |_, _| false,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap();
        let keys: Vec<&str> = deps.keys().map(cabin_core::PackageName::as_str).collect();
        assert_eq!(keys, vec!["fmt"]);
    }

    #[test]
    fn excluded_names_are_dropped_from_versioned_deps() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"
spdlog = "^1"
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["app".into()]),
                exclude: Vec::new(),
            },
        )
        .unwrap();
        let closure = sel.closure(&graph);
        let mut excluded: BTreeSet<String> = BTreeSet::new();
        excluded.insert("fmt".into());
        let deps = collect_closure_versioned_deps_excluding_with_dev(
            &graph,
            &closure,
            |_, _| false,
            &excluded,
            &BTreeSet::new(),
        )
        .unwrap();
        let keys: Vec<&str> = deps.keys().map(cabin_core::PackageName::as_str).collect();
        assert_eq!(keys, vec!["spdlog"]);
    }

    #[test]
    fn closure_has_versioned_deps_excluding_returns_false_when_only_dep_is_excluded() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["app".into()]),
                exclude: Vec::new(),
            },
        )
        .unwrap();
        let closure = sel.closure(&graph);
        let mut excluded: BTreeSet<String> = BTreeSet::new();
        excluded.insert("fmt".into());
        assert!(!closure_has_versioned_deps_excluding_with_dev(
            &graph,
            &closure,
            |_, _| false,
            &excluded,
            &BTreeSet::new(),
        ));
        // Empty exclusion set leaves the original positive
        // result in place.
        assert!(closure_has_versioned_deps_excluding_with_dev(
            &graph,
            &closure,
            |_, _| false,
            &BTreeSet::new(),
            &BTreeSet::new(),
        ));
    }

    #[test]
    fn versioned_deps_excludes_dev_dependencies() {
        let dir = TempDir::new().unwrap();
        dir.child("cabin.toml")
            .write_str(
                r#"[workspace]
members = ["packages/app"]
"#,
            )
            .unwrap();
        dir.child("packages/app/cabin.toml")
            .write_str(
                r#"[package]
name = "app"
version = "0.1.0"

[dependencies]
fmt = ">=10"

[dev-dependencies]
gtest = "^1.14"
"#,
            )
            .unwrap();
        let graph = load_workspace(dir.path().join("cabin.toml")).unwrap();
        let sel = resolve_package_selection(
            &graph,
            &PackageSelection {
                mode: SelectionMode::ExplicitPackages(vec!["app".into()]),
                exclude: Vec::new(),
            },
        )
        .unwrap();
        let closure = sel.closure(&graph);
        let deps = collect_closure_versioned_deps_excluding_with_dev(
            &graph,
            &closure,
            |_, _| false,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap();
        let keys: Vec<&str> = deps.keys().map(cabin_core::PackageName::as_str).collect();
        assert_eq!(keys, vec!["fmt"]);
        assert!(
            !deps.contains_key(&cabin_core::PackageName::new("gtest").unwrap()),
            "dev-dep gtest must not enter ordinary resolution"
        );
    }
}
