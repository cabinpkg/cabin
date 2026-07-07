use crate::error::BuildError;
use cabin_core::{SourceLanguage, Target};
use cabin_driver::Dialect;
use cabin_workspace::PackageGraph;
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::{PlanRequest, TargetDepEdge, TargetId, find_target, format_target_id};

/// Resolve one declared `deps` entry into a graph edge.  The alias
/// forms (`foo` as a same-package target or as the `foo:foo`
/// shorthand) resolve *before* the declared visibility is attached,
/// so the recorded edge always names the concrete (package, target)
/// and downstream code never sees pre-alias names.
pub(super) fn resolve_target_dep_edge(
    decl: &cabin_core::TargetDep,
    pkg_idx: usize,
    dev_deps_visible: bool,
    graph: &PackageGraph,
) -> Result<TargetDepEdge, BuildError> {
    let to = resolve_target_dep(&decl.reference, pkg_idx, dev_deps_visible, graph)?;
    Ok(TargetDepEdge {
        to,
        public: decl.public,
    })
}

pub(super) fn resolve_target_dep(
    raw: &str,
    pkg_idx: usize,
    dev_deps_visible: bool,
    graph: &PackageGraph,
) -> Result<TargetId, BuildError> {
    let pkg = &graph.packages[pkg_idx];

    // Cross-package target lookups see Normal-kind dependency
    // edges, plus Dev-kind edges when the referencing target is a
    // dev-only kind (`test` / `example`).  Dev edges only exist in
    // the graph when the loader activated them for the owning
    // package (`cabin test` does, for the selected packages), so
    // ordinary builds never resolve through them.
    let find_dep_edge = |name: &str| -> Option<usize> {
        pkg.deps_of_kind(cabin_core::DependencyKind::Normal)
            .chain(
                dev_deps_visible
                    .then(|| pkg.deps_of_kind(cabin_core::DependencyKind::Dev))
                    .into_iter()
                    .flatten(),
            )
            .find(|&di| graph.packages[di].package.name.as_str() == name)
    };
    // Whether the owning manifest declares `name` under
    // `[dev-dependencies]` at all - used to turn a failed lookup
    // into the targeted dev-dependency diagnostic instead of a
    // generic unknown-reference error.
    let declared_as_dev = |name: &str| -> bool {
        pkg.package
            .dependencies
            .iter()
            .any(|d| d.kind == cabin_core::DependencyKind::Dev && d.name.as_str() == name)
    };

    if let Some((p_name, t_name)) = raw.split_once(':') {
        // Qualified `package:target`.  The package must be either this
        // package itself or one of its visible dependency edges.
        let dep_idx = if p_name == pkg.package.name.as_str() {
            pkg_idx
        } else if let Some(di) = find_dep_edge(p_name) {
            di
        } else if declared_as_dev(p_name) {
            return Err(BuildError::DevDependencyNotActive {
                dep: p_name.to_owned(),
                package: pkg.package.name.as_str().to_owned(),
            });
        } else {
            return Err(BuildError::UnknownPackageInTargetSelector {
                package: p_name.to_owned(),
                selector: raw.to_owned(),
            });
        };
        let dep_pkg = &graph.packages[dep_idx];
        if find_target(&dep_pkg.package, t_name).is_none() {
            return Err(BuildError::UnknownTargetInPackage {
                package: p_name.to_owned(),
                target: t_name.to_owned(),
            });
        }
        return Ok((dep_idx, t_name.to_owned()));
    }

    // Unqualified.  A bare name is a same-package target reference
    // first.
    if find_target(&pkg.package, raw).is_some() {
        return Ok((pkg_idx, raw.to_owned()));
    }

    // Otherwise a bare name that matches a visible dependency
    // package is the same-name shorthand: `deps = ["foo"]` means
    // `foo:foo`.  This is pure name matching - a package never
    // exports a *default* target, so a dependency whose targets are
    // all named differently must be spelled `package:target`.  The
    // shorthand only accepts a link/interface-bearing target
    // (library / header-only): a same-named executable would build
    // but contribute no include dirs or archives, which reads as a
    // silent no-op link.  The exotic executable-dep case keeps its
    // explicit `package:target` spelling.
    if let Some(dep_idx) = find_dep_edge(raw) {
        let dep_pkg = &graph.packages[dep_idx];
        if dep_pkg
            .package
            .targets
            .iter()
            .any(|t| t.name.as_str() == raw && t.kind.is_library_like())
        {
            return Ok((dep_idx, raw.to_owned()));
        }
        let candidates: Vec<String> = dep_pkg
            .package
            .targets
            .iter()
            .filter(|t| t.kind.is_library_like())
            .map(|t| format!("{}:{}", dep_pkg.package.name.as_str(), t.name.as_str()))
            .collect();
        return Err(BuildError::NoSameNameTargetInDependency {
            dep: raw.to_owned(),
            package: dep_pkg.package.name.as_str().to_owned(),
            candidates,
        });
    }

    if declared_as_dev(raw) {
        return Err(BuildError::DevDependencyNotActive {
            dep: raw.to_owned(),
            package: pkg.package.name.as_str().to_owned(),
        });
    }
    Err(BuildError::UnknownTargetReference(raw.to_owned()))
}

// ---------------------------------------------------------------------------
// internal: target topological sort
// ---------------------------------------------------------------------------

pub(super) fn topo_sort_targets(
    reachable: &HashSet<TargetId>,
    resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
    graph: &PackageGraph,
) -> Result<Vec<TargetId>, BuildError> {
    #[derive(Clone, Copy)]
    enum Color {
        Visiting,
        Done,
    }

    fn visit(
        node: &TargetId,
        resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
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
                visit(&d.to, resolved, graph, state, path, order)?;
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

/// Include dirs for one target's compiles, partitioned into the user
/// bucket (`-I`) and the system bucket (`-isystem` / `/external:I`).
pub(super) struct CollectedIncludeDirs {
    /// The target's own include dirs plus those contributed by
    /// local, non-port dependency targets - code the user owns.
    pub(super) user: Vec<Utf8PathBuf>,
    /// Include dirs contributed by third-party dependency targets:
    /// extracted registry packages and foundation ports.  Their
    /// headers are upstream code the user cannot fix, so compiles
    /// mark them as system search paths.
    pub(super) system: Vec<Utf8PathBuf>,
}

pub(super) fn collect_include_dirs(
    start: &TargetId,
    target: &Target,
    resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
    graph: &PackageGraph,
) -> Result<CollectedIncludeDirs, BuildError> {
    let manifest_dir = promote_dir(&graph.packages[start.0].manifest_dir)?;
    let mut user: Vec<Utf8PathBuf> = target
        .include_dirs
        .iter()
        .map(|d| manifest_dir.join(d))
        .collect();
    let mut system: Vec<Utf8PathBuf> = Vec::new();

    let mut seen: HashSet<TargetId> = HashSet::new();
    let empty: Vec<TargetDepEdge> = Vec::new();
    let mut stack: Vec<&TargetId> = resolved
        .get(start)
        .unwrap_or(&empty)
        .iter()
        .map(|e| &e.to)
        .collect();
    while let Some(tid) = stack.pop() {
        if !seen.insert(tid.clone()) {
            continue;
        }
        let dep_pkg = &graph.packages[tid.0];
        let Some(dep_target) = find_target(&dep_pkg.package, &tid.1) else {
            continue;
        };
        if dep_target.kind.is_library_like() {
            // Provenance decides the bucket: registry archives and
            // foundation ports are third-party code, while workspace
            // members, plain path deps, and `[patch]`ed packages are
            // the user's own (a patched dependency intentionally
            // surfaces its warnings again).  A dir already collected
            // keeps its first-seen bucket so no path is ever spelled
            // both `-I` and `-isystem` on one command line.
            let third_party =
                dep_pkg.kind == cabin_workspace::PackageKind::Registry || dep_pkg.is_port;
            let dep_manifest = promote_dir(&dep_pkg.manifest_dir)?;
            for inc in &dep_target.include_dirs {
                let abs = dep_manifest.join(inc);
                if !user.contains(&abs) && !system.contains(&abs) {
                    if third_party {
                        system.push(abs);
                    } else {
                        user.push(abs);
                    }
                }
            }
        }
        if let Some(deps) = resolved.get(tid) {
            for d in deps {
                stack.push(&d.to);
            }
        }
    }

    Ok(CollectedIncludeDirs { user, system })
}

/// Collect the validated system-library names
/// (`ResolvedProfileFlags::link_libs`) that must be appended to the
/// final link of the executable rooted at `start`.  Walks `start`'s
/// own package plus every transitively-reachable dependency
/// package's resolved build flags, deduplicating by name while
/// preserving first-seen order (link order matters for GNU `ld`).
///
/// The names are emitted as `-l<name>` by the caller *after* the
/// archive inputs and the executable's own `ldflags`, so a static
/// library's required system libraries (e.g. sqlite's
/// `-lpthread -ldl -lm`) resolve left-to-right against the archive
/// that references them.
pub(super) fn collect_link_lib_names(
    start: &TargetId,
    resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
    build_flags: &HashMap<usize, cabin_core::ResolvedProfileFlags>,
) -> Vec<String> {
    fn add_package(
        pkg_idx: usize,
        build_flags: &HashMap<usize, cabin_core::ResolvedProfileFlags>,
        result: &mut Vec<String>,
        seen_packages: &mut HashSet<usize>,
    ) {
        if !seen_packages.insert(pkg_idx) {
            return;
        }
        if let Some(flags) = build_flags.get(&pkg_idx) {
            for lib in &flags.link_libs {
                if !result.contains(lib) {
                    result.push(lib.clone());
                }
            }
        }
    }

    let mut result: Vec<String> = Vec::new();
    let mut seen_packages: HashSet<usize> = HashSet::new();

    add_package(start.0, build_flags, &mut result, &mut seen_packages);

    let mut seen_targets: HashSet<TargetId> = HashSet::new();
    let empty: Vec<TargetDepEdge> = Vec::new();
    let mut stack: Vec<&TargetId> = resolved
        .get(start)
        .unwrap_or(&empty)
        .iter()
        .map(|e| &e.to)
        .collect();
    while let Some(tid) = stack.pop() {
        if !seen_targets.insert(tid.clone()) {
            continue;
        }
        add_package(tid.0, build_flags, &mut result, &mut seen_packages);
        if let Some(deps) = resolved.get(tid) {
            for d in deps {
                stack.push(&d.to);
            }
        }
    }

    result
}

pub(super) fn collect_link_libs(
    start: &TargetId,
    resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
    graph: &PackageGraph,
    output_for_target: &HashMap<TargetId, Utf8PathBuf>,
) -> Vec<Utf8PathBuf> {
    fn visit(
        node: &TargetId,
        resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
        graph: &PackageGraph,
        seen: &mut HashSet<TargetId>,
        post: &mut Vec<TargetId>,
    ) {
        if !seen.insert(node.clone()) {
            return;
        }
        if let Some(deps) = resolved.get(node) {
            for d in deps {
                visit(&d.to, resolved, graph, seen, post);
            }
        }
        let Some(target) = find_target(&graph.packages[node.0].package, &node.1) else {
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
            visit(&d.to, resolved, graph, &mut seen, &mut post);
        }
    }
    post.iter()
        .rev()
        .filter_map(|tid| output_for_target.get(tid).cloned())
        .collect()
}

/// One per-source compile decision.  Naming the components
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

/// Failure modes for [`compile_dispatch`].  Carry only the
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
/// rejecting non-UTF-8 paths with [`BuildError::NonUtf8Path`].  The
/// planner anchors every source, include, and output path on these
/// directories, so they must be valid UTF-8 to enter the semantic IR.
pub(super) fn promote_dir(p: &Path) -> Result<&Utf8Path, BuildError> {
    Utf8Path::from_path(p).ok_or_else(|| BuildError::NonUtf8Path(p.to_path_buf()))
}
