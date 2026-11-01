use crate::error::BuildError;
use crate::{PlannedStandardError, StandardViolation};
use cabin_core::{SourceLanguage, Target, macro_name};
use cabin_driver::Dialect;
use cabin_workspace::PackageGraph;
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::{
    PlanRequest, SystemTargetInterface, TargetDepEdge, TargetId, find_target, format_target_id,
};

/// Resolve one declared `deps` entry into a graph edge.  The alias
/// forms (`foo` as a same-package target or as a dependency
/// package's sole library-like target) resolve *before* the declared
/// visibility is attached, so the recorded edge always names the
/// concrete (package, target) and downstream code never sees
/// pre-alias names.
pub(super) fn resolve_target_dep_edge(
    decl: &cabin_core::TargetDep,
    pkg_idx: usize,
    dev_deps_visible: bool,
    graph: &PackageGraph,
    platform: &cabin_core::TargetPlatform,
) -> Result<TargetDepEdge, BuildError> {
    let to = resolve_target_dep(&decl.reference, pkg_idx, dev_deps_visible, graph, platform)?;
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
    platform: &cabin_core::TargetPlatform,
) -> Result<TargetId, BuildError> {
    let pkg = &graph.packages[pkg_idx];

    // Cross-package target lookups see Normal-kind dependency
    // edges, plus Dev-kind edges when the referencing target is a
    // dev-only kind (`test` / `example`), each active on the
    // platform being built for.  Dev edges only exist in
    // the graph when the loader activated them for the owning
    // package (`cabin test` does, for the selected packages), so
    // ordinary builds never resolve through them.
    let find_dep_edge = |name: &str| -> Option<usize> {
        pkg.deps_of_kind(cabin_core::DependencyKind::Normal, platform)
            .chain(
                dev_deps_visible
                    .then(|| pkg.deps_of_kind(cabin_core::DependencyKind::Dev, platform))
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
    // package resolves to that package's *sole* library-like target
    // (library / header-only), whatever it is named
    // (`deps = ["zlib"]` links `zlib:z`).  An executable contributes
    // no include dirs or archives, so depending on one needs the
    // explicit `package:target` spelling.  Several candidates are an
    // error listing them in the package's target order (name-sorted
    // for parsed manifests).
    if let Some(dep_idx) = find_dep_edge(raw) {
        let dep_pkg = &graph.packages[dep_idx];
        let candidates: Vec<&Target> = dep_pkg
            .package
            .targets
            .iter()
            .filter(|t| t.kind.is_library_like())
            .collect();
        return match candidates.as_slice() {
            [sole] => Ok((dep_idx, sole.name.as_str().to_owned())),
            [] => Err(BuildError::NoLibraryTargetInDependency {
                package: dep_pkg.package.name.as_str().to_owned(),
            }),
            _ => Err(BuildError::AmbiguousLibraryTargetInDependency {
                package: dep_pkg.package.name.as_str().to_owned(),
                candidates: candidates
                    .iter()
                    .map(|t| format!("{}:{}", dep_pkg.package.name.as_str(), t.name.as_str()))
                    .collect(),
            }),
        };
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

/// What one target's compiles receive: its own include dirs plus the
/// consumer interfaces of the dependency targets it sees.
pub(super) struct CollectedInterface {
    /// The target's own include dirs plus those contributed by
    /// local dependency targets - code the user owns.
    pub(super) user: Vec<Utf8PathBuf>,
    /// Include dirs contributed by third-party dependency targets:
    /// extracted registry packages.  Their headers are upstream code
    /// the user cannot fix, so compiles mark them as system search
    /// paths.
    pub(super) system: Vec<Utf8PathBuf>,
    /// The visible dependencies' `interface-defines`, in visit order.
    pub(super) defines: Vec<String>,
    /// The visible system-provided targets' compile arguments, in
    /// visit order.
    pub(super) args: Vec<String>,
    /// The visible library-like dependency targets, in visit order.
    pub(super) visible: Vec<TargetId>,
}

/// Collect the consumer interface `start` compiles against: each
/// direct dependency's, plus what that dependency re-exports through
/// public edges, transitively.  A private edge's own dependencies stay
/// invisible.  Visits pop a stack pushed in declaration order, so the
/// last-declared dependency is visited first: the include search order
/// Cabin has always emitted.  Two visible dependencies defining one
/// macro differently are recorded against `object`, one of `start`'s
/// compiles, which the `cabin check` rewrite prunes the conflict with.
#[tracing::instrument(level = "debug", skip_all)]
pub(super) fn collect_interface(
    start: &TargetId,
    target: &Target,
    resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
    graph: &PackageGraph,
    system_targets: &HashMap<TargetId, SystemTargetInterface>,
    object: &Utf8Path,
    violations: &mut Vec<StandardViolation>,
) -> Result<CollectedInterface, BuildError> {
    let manifest_dir = promote_dir(&graph.packages[start.0].manifest_dir)?;
    let mut collected = CollectedInterface {
        user: target
            .include_dirs
            .iter()
            .map(|d| manifest_dir.join(d))
            .collect(),
        system: Vec::new(),
        defines: Vec::new(),
        args: Vec::new(),
        visible: Vec::new(),
    };
    // Each visited target and the target it was reached through, for
    // the paths a conflicting definition names.
    let mut reached_via: HashMap<&TargetId, &TargetId> = HashMap::new();
    let mut define_origin: HashMap<&str, (&str, &TargetId)> = HashMap::new();
    let empty: Vec<TargetDepEdge> = Vec::new();
    let mut stack: Vec<(&TargetId, &TargetId)> = resolved
        .get(start)
        .unwrap_or(&empty)
        .iter()
        .map(|e| (&e.to, start))
        .collect();
    while let Some((tid, via)) = stack.pop() {
        if tid == start || reached_via.contains_key(tid) {
            continue;
        }
        reached_via.insert(tid, via);
        let dep_pkg = &graph.packages[tid.0];
        let Some(dep_target) = find_target(&dep_pkg.package, &tid.1) else {
            continue;
        };
        if dep_target.kind.is_library_like() {
            collected.visible.push(tid.clone());
            // Provenance decides the bucket: registry archives are
            // third-party code, while workspace
            // members, plain path deps, and `[patch]`ed packages are
            // the user's own (a patched dependency intentionally
            // surfaces its warnings again).  A dir already collected
            // keeps its first-seen bucket so no path is ever spelled
            // both `-I` and `-isystem` on one command line.
            let third_party = dep_pkg.kind == cabin_workspace::PackageKind::Registry;
            let dep_manifest = promote_dir(&dep_pkg.manifest_dir)?;
            let provided = system_targets.get(tid);
            let declared = dep_target
                .include_dirs
                .iter()
                .map(|inc| (dep_manifest.join(inc), third_party));
            let supplied = provided.into_iter().flat_map(|system| {
                let plain = system.include_dirs.iter().map(|dir| (dir.clone(), false));
                plain.chain(
                    system
                        .system_include_dirs
                        .iter()
                        .map(|dir| (dir.clone(), true)),
                )
            });
            for (abs, system) in declared.chain(supplied) {
                if !collected.user.contains(&abs) && !collected.system.contains(&abs) {
                    if system {
                        collected.system.push(abs);
                    } else {
                        collected.user.push(abs);
                    }
                }
            }
            if let Some(system) = provided {
                collected.args.extend(system.compile_args.iter().cloned());
            }
            let supplied_defines = provided.into_iter().flat_map(|system| &system.defines);
            for define in dep_target.interface_defines.iter().chain(supplied_defines) {
                let name = macro_name(define);
                match define_origin.get(name) {
                    None => {
                        define_origin.insert(name, (define, tid));
                        collected.defines.push(define.clone());
                    }
                    Some((first, _)) if macro_value(first) == macro_value(define) => {}
                    Some((first, origin)) => {
                        let path_to = |end: &TargetId| {
                            let mut hops = vec![format_target_id(end, graph)];
                            let mut at = end;
                            while let Some(prev) = reached_via.get(at) {
                                hops.push(format_target_id(prev, graph));
                                at = prev;
                            }
                            hops.reverse();
                            hops.join(" -> ")
                        };
                        // Registry-supplied values: escape them before
                        // they reach the terminal.
                        violations.push(StandardViolation {
                            object: object.to_owned(),
                            error: PlannedStandardError::ConflictingInterfaceDefines {
                                target: format_target_id(start, graph),
                                name: name.to_owned(),
                                first: format!(
                                    "`{}` from {}",
                                    cabin_core::escape_control_chars(first),
                                    path_to(origin)
                                ),
                                second: format!(
                                    "`{}` from {}",
                                    cabin_core::escape_control_chars(define),
                                    path_to(tid)
                                ),
                            },
                        });
                    }
                }
            }
        }
        if let Some(deps) = resolved.get(tid) {
            for edge in deps.iter().filter(|edge| edge.public) {
                stack.push((&edge.to, tid));
            }
        }
    }
    Ok(collected)
}

/// The value a `-D` entry gives its macro: a bare `NAME` means `1`.
fn macro_value(define: &str) -> &str {
    define.split_once('=').map_or("1", |(_, value)| value)
}

/// Every node reachable from `roots`, each before the nodes it
/// reaches (the reverse postorder of a depth-first walk) as long as
/// the walked graph is acyclic.
fn reverse_postorder<T: Copy + Eq + std::hash::Hash>(
    roots: impl IntoIterator<Item = T>,
    successors: &dyn Fn(T) -> Vec<T>,
) -> Vec<T> {
    fn visit<T: Copy + Eq + std::hash::Hash>(
        node: T,
        successors: &dyn Fn(T) -> Vec<T>,
        seen: &mut HashSet<T>,
        post: &mut Vec<T>,
    ) {
        if !seen.insert(node) {
            return;
        }
        for next in successors(node) {
            visit(next, successors, seen, post);
        }
        post.push(node);
    }

    let mut seen = HashSet::new();
    let mut post = Vec::new();
    for root in roots {
        visit(root, successors, &mut seen, &mut post);
    }
    post.reverse();
    post
}

/// Every target the link of `start` reaches through any edge, private
/// ones included since the final link ignores visibility, dependents
/// before dependencies.  The target graph is acyclic, so this order
/// resolves every archive in one left-to-right pass and no linker
/// group is needed.
fn link_closure<'a>(
    start: &'a TargetId,
    resolved: &'a HashMap<TargetId, Vec<TargetDepEdge>>,
) -> Vec<&'a TargetId> {
    let deps = |tid: &'a TargetId| -> Vec<&'a TargetId> {
        resolved
            .get(tid)
            .into_iter()
            .flatten()
            .map(|edge| &edge.to)
            .collect()
    };
    reverse_postorder(deps(start), &deps)
}

/// The bare names one field of the resolved build flags
/// (`link_libs` or `frameworks`) contributes to the final link of
/// `start`: each package the link reaches, dependents before the
/// packages their linked targets depend on, so `start`'s own comes
/// first.  The caller emits them after the archives, so a static
/// library's system libraries (sqlite's `-lpthread -ldl -lm`)
/// resolve against the archive that needs them.  `dedup` keeps only
/// each name's first occurrence; otherwise a name two packages list
/// repeats, as an order-sensitive linker needs.
pub(super) fn collect_link_names(
    start: &TargetId,
    resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
    build_flags: &HashMap<usize, cabin_core::ResolvedProfileFlags>,
    field: fn(&cabin_core::ResolvedProfileFlags) -> &[String],
    dedup: bool,
) -> Vec<String> {
    let linked: Vec<&TargetId> = std::iter::once(start)
        .chain(link_closure(start, resolved))
        .collect();
    // Package edges come from linked targets only: a target the link
    // never reaches must not reorder its package.
    let packages = reverse_postorder([start.0], &|pkg| {
        linked
            .iter()
            .filter(|tid| tid.0 == pkg)
            .flat_map(|tid| resolved.get(*tid).into_iter().flatten())
            .map(|edge| edge.to.0)
            .filter(|&to| to != pkg)
            .collect()
    });
    let mut names: Vec<String> = packages
        .iter()
        .filter_map(|pkg| build_flags.get(pkg))
        .flat_map(|flags| field(flags).iter().cloned())
        .collect();
    if dedup {
        let mut seen = HashSet::new();
        names.retain(|name| seen.insert(name.clone()));
    }
    names
}

/// The link arguments of the system-provided targets the link of
/// `start` reaches, dependents before dependencies, each target's in
/// its provider's order.  A system library never depends on a
/// Cabin-built archive, so placing them all after the archives keeps
/// the left-to-right resolution a GNU linker needs.
pub(super) fn collect_system_link_args(
    start: &TargetId,
    resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
    system_targets: &HashMap<TargetId, SystemTargetInterface>,
) -> Vec<String> {
    link_closure(start, resolved)
        .into_iter()
        .filter_map(|tid| system_targets.get(tid))
        .flat_map(|system| system.link_args.iter().cloned())
        .collect()
}

#[tracing::instrument(level = "debug", skip_all)]
pub(super) fn collect_link_libs(
    start: &TargetId,
    resolved: &HashMap<TargetId, Vec<TargetDepEdge>>,
    graph: &PackageGraph,
    output_for_target: &HashMap<TargetId, Utf8PathBuf>,
) -> Vec<Utf8PathBuf> {
    link_closure(start, resolved)
        .into_iter()
        .filter(|tid| {
            find_target(&graph.packages[tid.0].package, &tid.1)
                .is_some_and(|target| target.kind.produces_archive())
        })
        .filter_map(|tid| output_for_target.get(tid).cloned())
        .collect()
}

/// The per-language parts of one source's compile.
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

/// Choose the driver and description tag for a single compile, given
/// the classified source language and the resolved toolchain.
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
            // No file name holds a NUL, and the output-path check
            // compares keys in which NUL stands for a separator.
            Utf8Component::Normal(name) if name.contains('\0') => {
                return Err("NUL characters are not supported".to_owned());
            }
            // Windows reads `C:` as a drive, which `push` would take for
            // a new root outside the build directory.
            Utf8Component::Normal(name) if cfg!(windows) && name.contains(':') => {
                return Err("':' is not supported on Windows".to_owned());
            }
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
