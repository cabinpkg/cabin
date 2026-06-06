use crate::error::BuildError;
use cabin_core::{SourceLanguage, Target};
use cabin_driver::Dialect;
use cabin_workspace::PackageGraph;
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::{PlanRequest, TargetId, format_target_id};

pub(super) fn resolve_target_dep(
    raw: &str,
    pkg_idx: usize,
    graph: &PackageGraph,
) -> Result<TargetId, BuildError> {
    let pkg = &graph.packages[pkg_idx];

    // Cross-package target lookups must only see Normal-kind
    // dependency edges. Dev dependencies are declaration-only as
    // far as ordinary `target.<X>.deps` resolution is concerned.
    if let Some((p_name, t_name)) = raw.split_once(':') {
        // Qualified `package:target`. The package must be either this
        // package itself or one of its declared *normal*
        // dependencies.
        let dep_idx = if p_name == pkg.package.name.as_str() {
            pkg_idx
        } else {
            pkg.deps_of_kind(cabin_core::DependencyKind::Normal)
                .find(|&di| graph.packages[di].package.name.as_str() == p_name)
                .ok_or_else(|| BuildError::UnknownPackageInTargetSelector {
                    package: p_name.to_owned(),
                    selector: raw.to_owned(),
                })?
        };
        let dep_pkg = &graph.packages[dep_idx];
        if !dep_pkg
            .package
            .targets
            .iter()
            .any(|t| t.name.as_str() == t_name)
        {
            return Err(BuildError::UnknownTargetInPackage {
                package: p_name.to_owned(),
                target: t_name.to_owned(),
            });
        }
        return Ok((dep_idx, t_name.to_owned()));
    }

    // Unqualified. Same-package match wins.
    if pkg.package.targets.iter().any(|t| t.name.as_str() == raw) {
        return Ok((pkg_idx, raw.to_owned()));
    }

    // Then, *normal-kind* package dependency name → its default
    // library or header_only target. Build / tool / dev deps are
    // intentionally skipped here so they cannot auto-link into
    // ordinary targets.
    if let Some(dep_idx) = pkg
        .deps_of_kind(cabin_core::DependencyKind::Normal)
        .find(|&di| graph.packages[di].package.name.as_str() == raw)
    {
        let dep_pkg = &graph.packages[dep_idx];
        let libs: Vec<&Target> = dep_pkg
            .package
            .targets
            .iter()
            .filter(|t| t.kind.produces_archive() || t.kind.is_header_only())
            .collect();
        return match libs.len() {
            0 => Err(BuildError::DependencyHasNoLibrary {
                dep: raw.to_owned(),
                package: dep_pkg.package.name.as_str().to_owned(),
            }),
            1 => Ok((dep_idx, libs[0].name.as_str().to_owned())),
            _ => Err(BuildError::AmbiguousDefaultLibrary {
                dep: raw.to_owned(),
                package: dep_pkg.package.name.as_str().to_owned(),
            }),
        };
    }

    Err(BuildError::UnknownTargetReference(raw.to_owned()))
}

// ---------------------------------------------------------------------------
// internal: target topological sort
// ---------------------------------------------------------------------------

pub(super) fn topo_sort_targets(
    reachable: &HashSet<TargetId>,
    resolved: &HashMap<TargetId, Vec<TargetId>>,
    graph: &PackageGraph,
) -> Result<Vec<TargetId>, BuildError> {
    #[derive(Clone, Copy)]
    enum Color {
        Visiting,
        Done,
    }

    fn visit(
        node: &TargetId,
        resolved: &HashMap<TargetId, Vec<TargetId>>,
        graph: &PackageGraph,
        state: &mut HashMap<TargetId, Color>,
        path: &mut Vec<TargetId>,
        order: &mut Vec<TargetId>,
    ) -> Result<(), BuildError> {
        match state.get(node) {
            Some(Color::Done) => return Ok(()),
            Some(Color::Visiting) => {
                let start = path.iter().position(|n| n == node).unwrap_or(0);
                let mut cycle: Vec<String> = path[start..]
                    .iter()
                    .map(|t| format_target_id(t, graph))
                    .collect();
                cycle.push(format_target_id(node, graph));
                return Err(BuildError::DependencyCycle(cycle));
            }
            None => {}
        }
        state.insert(node.clone(), Color::Visiting);
        path.push(node.clone());
        if let Some(deps) = resolved.get(node) {
            for d in deps {
                visit(d, resolved, graph, state, path, order)?;
            }
        }
        path.pop();
        state.insert(node.clone(), Color::Done);
        order.push(node.clone());
        Ok(())
    }

    let mut state: HashMap<TargetId, Color> = HashMap::new();
    let mut order = Vec::new();
    let mut path = Vec::new();

    let mut sorted: Vec<TargetId> = reachable.iter().cloned().collect();
    sorted.sort();
    for tid in sorted {
        visit(&tid, resolved, graph, &mut state, &mut path, &mut order)?;
    }
    Ok(order)
}

// ---------------------------------------------------------------------------
// internal: include dir + link lib collection
// ---------------------------------------------------------------------------

pub(super) fn collect_include_dirs(
    start: &TargetId,
    target: &Target,
    resolved: &HashMap<TargetId, Vec<TargetId>>,
    graph: &PackageGraph,
) -> Result<Vec<Utf8PathBuf>, BuildError> {
    let manifest_dir = promote_dir(&graph.packages[start.0].manifest_dir)?;
    let mut result: Vec<Utf8PathBuf> = target
        .include_dirs
        .iter()
        .map(|d| manifest_dir.join(d))
        .collect();

    let mut seen: HashSet<TargetId> = HashSet::new();
    let empty: Vec<TargetId> = Vec::new();
    let mut stack: Vec<&TargetId> = resolved.get(start).unwrap_or(&empty).iter().collect();
    while let Some(tid) = stack.pop() {
        if !seen.insert(tid.clone()) {
            continue;
        }
        let Some(dep_target) = graph.packages[tid.0]
            .package
            .targets
            .iter()
            .find(|t| t.name.as_str() == tid.1)
        else {
            continue;
        };
        if dep_target.kind.produces_archive() || dep_target.kind.is_header_only() {
            let dep_manifest = promote_dir(&graph.packages[tid.0].manifest_dir)?;
            for inc in &dep_target.include_dirs {
                let abs = dep_manifest.join(inc);
                if !result.contains(&abs) {
                    result.push(abs);
                }
            }
        }
        if let Some(deps) = resolved.get(tid) {
            for d in deps {
                stack.push(d);
            }
        }
    }

    Ok(result)
}

pub(super) fn collect_link_libs(
    start: &TargetId,
    resolved: &HashMap<TargetId, Vec<TargetId>>,
    graph: &PackageGraph,
    output_for_target: &HashMap<TargetId, Utf8PathBuf>,
) -> Vec<Utf8PathBuf> {
    fn visit(
        node: &TargetId,
        resolved: &HashMap<TargetId, Vec<TargetId>>,
        graph: &PackageGraph,
        seen: &mut HashSet<TargetId>,
        post: &mut Vec<TargetId>,
    ) {
        if !seen.insert(node.clone()) {
            return;
        }
        if let Some(deps) = resolved.get(node) {
            for d in deps {
                visit(d, resolved, graph, seen, post);
            }
        }
        let Some(target) = graph.packages[node.0]
            .package
            .targets
            .iter()
            .find(|t| t.name.as_str() == node.1)
        else {
            return;
        };
        if target.kind.produces_archive() {
            post.push(node.clone());
        }
    }

    let mut seen: HashSet<TargetId> = HashSet::new();
    let mut post: Vec<TargetId> = Vec::new();
    if let Some(deps) = resolved.get(start) {
        for d in deps {
            visit(d, resolved, graph, &mut seen, &mut post);
        }
    }
    post.iter()
        .rev()
        .filter_map(|tid| output_for_target.get(tid).cloned())
        .collect()
}

/// One per-source compile decision. Naming the components
/// (driver, flags, action kind, human tag) keeps the planner's
/// per-source loop legible: the dispatch table is *the* place
/// where a future language addition would go, and changes here
/// are mechanically reviewable.
pub(super) struct CompileDispatch<'a> {
    /// Driver executable (the compiler binary).
    pub(super) driver: &'a Utf8Path,
    /// Short human-readable tag (`CC` or `CXX`) used in Ninja
    /// `description = ...` lines.
    pub(super) description_tag: &'static str,
}

/// Failure modes for [`compile_dispatch`]. Carry only the
/// language-level reason; the planner attaches target / source
/// context via [`CompileDispatchError::attach_target_path`].
pub(super) enum CompileDispatchError {
    MissingCCompiler,
}

impl CompileDispatchError {
    pub(super) fn attach_target_path(
        self,
        tid: &TargetId,
        graph: &PackageGraph,
        path: &Utf8Path,
    ) -> BuildError {
        match self {
            Self::MissingCCompiler => BuildError::MissingCCompiler {
                target: format_target_id(tid, graph),
                path: path.to_path_buf(),
            },
        }
    }
}

/// Choose driver / flags / kind for a single compile, given the
/// classified source language and the resolved toolchain.
pub(super) fn compile_dispatch<'a>(
    language: SourceLanguage,
    req: &'a PlanRequest<'a>,
) -> Result<CompileDispatch<'a>, CompileDispatchError> {
    match language {
        SourceLanguage::Cxx => Ok(CompileDispatch {
            driver: req.toolchain.cxx.path(),
            description_tag: "CXX",
        }),
        SourceLanguage::C => {
            let cc = req
                .toolchain
                .cc
                .as_ref()
                .ok_or(CompileDispatchError::MissingCCompiler)?;
            Ok(CompileDispatch {
                driver: cc.path(),
                description_tag: "CC",
            })
        }
    }
}

pub(super) fn object_path(
    pkg_build_dir: &Utf8Path,
    target: &str,
    source: &Utf8Path,
    dialect: Dialect,
) -> Result<Utf8PathBuf, String> {
    let mut sanitized = Utf8PathBuf::new();
    for component in source.components() {
        match component {
            Utf8Component::Normal(name) => sanitized.push(name),
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                return Err("parent directory components ('..') are not supported".to_owned());
            }
            Utf8Component::RootDir | Utf8Component::Prefix(_) => {
                return Err("absolute source paths are not supported".to_owned());
            }
        }
    }
    if sanitized.as_str().is_empty() {
        return Err("source path is empty".to_owned());
    }
    let parent = sanitized
        .parent()
        .map(Utf8Path::to_path_buf)
        .unwrap_or_default();
    let name = format!(
        "{}.{}",
        sanitized.file_name().unwrap(),
        dialect.object_extension()
    );
    Ok(pkg_build_dir
        .join("obj")
        .join(target)
        .join(parent)
        .join(name))
}

pub(super) fn depfile_path(object: &Utf8Path) -> Utf8PathBuf {
    Utf8PathBuf::from(format!("{object}.d"))
}

/// Promote an OS-canonicalized directory into a [`Utf8Path`],
/// rejecting non-UTF-8 paths with [`BuildError::NonUtf8Path`]. The
/// planner anchors every source, include, and output path on these
/// directories, so they must be valid UTF-8 to enter the semantic IR.
pub(super) fn promote_dir(p: &Path) -> Result<&Utf8Path, BuildError> {
    Utf8Path::from_path(p).ok_or_else(|| BuildError::NonUtf8Path(p.to_path_buf()))
}
