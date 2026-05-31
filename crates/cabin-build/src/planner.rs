use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use cabin_core::{
    ResolvedCompilerWrapper, ResolvedProfile, ResolvedProfileFlags, ResolvedToolchain,
    SourceLanguage, Target, TargetKind, classify_source, link_driver_language,
};
use cabin_workspace::PackageGraph;

use crate::error::BuildError;
use crate::graph::{Action, ActionKind, BuildGraph, CompileCommand};

/// Cabin's built-in C++ standard. Hardcoded for now; users
/// override via `[profile].cxxflags`.
pub(crate) const DEFAULT_CXX_STANDARD: &str = "-std=c++17";

/// Cabin's built-in C standard. Hardcoded for now; users
/// override via `[profile].cflags`.
///
/// Kept distinct from [`DEFAULT_CXX_STANDARD`] so the two flag
/// spaces never share state. A change here must not silently
/// alter C++ compile lines.
pub(crate) const DEFAULT_C_STANDARD: &str = "-std=c11";

/// Compose the deterministic compile flags for `profile`,
/// prefixed with the supplied language-specific `standard` flag.
///
/// The optimization / debug-info / `NDEBUG` flags
/// ([`ResolvedProfile::compile_flags`]) are language-neutral and
/// apply to both C and C++ compiles; the `standard` argument is
/// the only language-specific contribution. Pulling the two
/// `*_flags_for_profile` paths through one helper keeps the
/// per-language flag composition byte-identical except for the
/// standard flag itself, so `compile_commands.json` and
/// `build.ninja` stay deterministic.
pub(crate) fn flags_for_profile(standard: &str, profile: &ResolvedProfile) -> Vec<String> {
    let optim = profile.compile_flags();
    let mut out: Vec<String> = Vec::with_capacity(optim.len() + 1);
    out.push(standard.to_owned());
    for flag in optim {
        out.push((*flag).to_owned());
    }
    out
}

/// Convenience: the C++ standard flag plus profile flags.
pub(crate) fn cxx_flags_for_profile(profile: &ResolvedProfile) -> Vec<String> {
    flags_for_profile(DEFAULT_CXX_STANDARD, profile)
}

/// Convenience: the C standard flag plus profile flags.
pub(crate) fn c_flags_for_profile(profile: &ResolvedProfile) -> Vec<String> {
    flags_for_profile(DEFAULT_C_STANDARD, profile)
}

/// Reference to a manifest target — one of the `[target.<name>]`
/// declarations in a package's `cabin.toml`. May be qualified
/// `package:target` or unqualified `target`. Resolution against a
/// [`PackageGraph`] happens in the planner.
///
/// This is the *manifest-target* selector. It has nothing to do
/// with a platform / toolchain target (e.g. an
/// `x86_64-unknown-linux-gnu` triple); Cabin does not yet model
/// the latter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestTargetSelector {
    pub package: Option<String>,
    pub name: String,
}

impl ManifestTargetSelector {
    /// Parse a `package:target` or bare `target` string. Unknown formats
    /// (multiple `:`s) are accepted and surfaced later by resolution
    /// errors.
    pub fn parse(s: &str) -> Self {
        match s.split_once(':') {
            Some((pkg, tgt)) => Self {
                package: Some(pkg.to_owned()),
                name: tgt.to_owned(),
            },
            None => Self {
                package: None,
                name: s.to_owned(),
            },
        }
    }
}

/// Inputs to the build planner.
#[derive(Debug)]
pub struct PlanRequest<'a> {
    pub graph: &'a PackageGraph,
    /// Resolved C/C++ toolchain. The planner picks the compile
    /// driver per source language (`toolchain.cc.path()` for `.c`,
    /// `toolchain.cxx.path()` for `.cc` / `.cpp` / `.cxx` /
    /// `.c++` / `.C`) and the link driver per target (C++ if any
    /// linked object came from a C++ source, otherwise C).
    /// `toolchain.ar.path()` drives archive commands.
    pub toolchain: &'a ResolvedToolchain,
    /// Per-package resolved build flags. Missing entries fall
    /// back to an empty [`ResolvedProfileFlags`]; the planner does
    /// not require every package to be present so consumers can
    /// resolve flags lazily for the selected closure only.
    pub build_flags: &'a HashMap<usize, ResolvedProfileFlags>,
    /// Absolute path under which all build outputs are placed.
    pub build_dir: PathBuf,
    /// Resolved build profile. Drives compile flags and the per-
    /// profile output directory.
    pub profile: ResolvedProfile,
    /// Specific manifest targets to build, plus their transitive
    /// deps. `None` means "every C/C++ target in every primary
    /// package".
    pub selected: Option<Vec<ManifestTargetSelector>>,
    /// Resolved root-package configuration. Carried through
    /// the planner so future cache logic and any planner-level
    /// fingerprint comparisons see the same selection the build
    /// script and metadata observed. The planner does not yet
    /// change C++ flags based on this value.
    pub configuration: Option<&'a cabin_core::BuildConfiguration>,
    /// Indices of `graph.packages` that the user picked
    /// through workspace package-selection flags. `None` means
    /// "use the graph's primary set" (the documented default).
    /// When `Some`, default-target enumeration narrows to the
    /// supplied indices and any manifest-target selectors in
    /// `selected` are validated against them so an unrelated
    /// package never sneaks into the build.
    pub selected_packages: Option<&'a [usize]>,
    /// Optional compiler-cache wrapper applied to every C++
    /// compile command. The Ninja `command` field is prefixed with
    /// the wrapper executable; the matching `compile_commands.json`
    /// `arguments` array stays *unwrapped* so clangd / IDE tooling
    /// keeps seeing the underlying compiler. Link and archive
    /// commands are never wrapped.
    pub compiler_wrapper: Option<&'a ResolvedCompilerWrapper>,
}

/// Plan a build for the requested package graph.
pub fn plan(req: &PlanRequest<'_>) -> Result<BuildGraph, BuildError> {
    path_to_str(&req.build_dir)?;

    let selected = match &req.selected {
        Some(sel) => resolve_selection(sel, req.graph, req.selected_packages)?,
        None => {
            let chosen = default_selection(req.graph, req.selected_packages);
            if chosen.is_empty() {
                return Err(BuildError::EmptySelectedPackages);
            }
            chosen
        }
    };

    // Walk the target dep graph, resolving each raw `deps` entry to a
    // concrete (package, target) ID and recording the edges.
    let mut resolved_deps: HashMap<TargetId, Vec<TargetId>> = HashMap::new();
    let mut reachable: HashSet<TargetId> = HashSet::new();
    let mut to_visit: Vec<TargetId> = selected.clone();

    while let Some(tid) = to_visit.pop() {
        if !reachable.insert(tid.clone()) {
            continue;
        }
        let target = lookup_target(&tid, req.graph)?;
        let mut resolved = Vec::with_capacity(target.deps.len());
        for raw in &target.deps {
            let dep = resolve_target_dep(raw.as_str(), tid.0, req.graph)?;
            to_visit.push(dep.clone());
            resolved.push(dep);
        }
        resolved_deps.insert(tid, resolved);
    }

    let topo = topo_sort_targets(&reachable, &resolved_deps, req.graph)?;

    let mut actions: Vec<Action> = Vec::new();
    let mut compile_commands: Vec<CompileCommand> = Vec::new();
    let mut output_for_target: HashMap<TargetId, PathBuf> = HashMap::new();
    // Per-target source-language manifest, including transitive
    // contributions through `target.deps`. Used to pick the
    // link-driver language deterministically: a target with any
    // direct or transitive C++ object link-drives through the C++
    // compiler, every other target link-drives through the C
    // compiler. Populated in topo order so dependents inherit
    // their dependencies' contributions.
    let mut target_languages: HashMap<TargetId, BTreeSet<SourceLanguage>> = HashMap::new();

    for tid in &topo {
        let target = lookup_target(tid, req.graph)?;

        let pkg = &req.graph.packages[tid.0];
        let pkg_name = pkg.package.name.as_str();
        // Per-profile output root keeps `dev` and `release`
        // builds from overwriting each other and gives custom
        // profiles a deterministic, non-colliding output tree.
        let pkg_build_dir = req
            .build_dir
            .join(req.profile.name.as_str())
            .join("packages")
            .join(pkg_name);
        let manifest_dir = &pkg.manifest_dir;

        // Header-only libraries declare include dirs but no
        // translation units.  Skip every action — `collect_link_libs`
        // and `collect_include_dirs` already walk dep targets by
        // their declared `include_dirs`, so consumers still pick up
        // the headers; they just see no `.a` to link against.
        if target.kind.is_header_only() {
            target_languages.insert(tid.clone(), Default::default());
            continue;
        }

        // Build the per-source list. Each manifest-declared source
        // resolves to an absolute path under the manifest directory
        // and a per-target object path.
        struct PreparedSource {
            abs_source: PathBuf,
            object: PathBuf,
            language: SourceLanguage,
        }
        let mut prepared: Vec<PreparedSource> = Vec::with_capacity(target.sources.len());
        for source in &target.sources {
            let language =
                classify_source(source).ok_or_else(|| BuildError::UnrecognizedSourceExtension {
                    target: format_target_id(tid, req.graph),
                    path: source.clone(),
                })?;
            let object =
                object_path(&pkg_build_dir, target.name.as_str(), source).map_err(|reason| {
                    BuildError::InvalidSourcePath {
                        target: format_target_id(tid, req.graph),
                        path: source.clone(),
                        reason,
                    }
                })?;
            prepared.push(PreparedSource {
                abs_source: manifest_dir.join(source),
                object,
                language,
            });
        }
        if prepared.is_empty() {
            return Err(BuildError::EmptyTargetSources(format_target_id(
                tid, req.graph,
            )));
        }

        // Per-package resolved build flags from the manifest's
        // `[profile]`, `[target.'cfg(...)'.profile]`, and the active
        // `[profile.<name>]` table. Layered on top of per-target
        // defines / include dirs.
        let pkg_flags = req.build_flags.get(&tid.0);

        // Compose include_dirs and defines: existing target +
        // per-package build flags.
        let mut include_dirs = collect_include_dirs(tid, target, &resolved_deps, req.graph);
        if let Some(flags) = pkg_flags {
            for inc in &flags.include_dirs {
                let absolute = if inc.is_absolute() {
                    inc.clone()
                } else {
                    manifest_dir.join(inc)
                };
                if !include_dirs.contains(&absolute) {
                    include_dirs.push(absolute);
                }
            }
        }
        let mut defines: Vec<String> = target.defines.clone();
        if let Some(flags) = pkg_flags {
            for def in &flags.defines {
                if !defines.contains(def) {
                    defines.push(def.clone());
                }
            }
        }
        let extra_compile_args: &[String] = pkg_flags
            .map(|f| f.extra_compile_args.as_slice())
            .unwrap_or(&[]);
        let cflags: &[String] = pkg_flags.map(|f| f.cflags.as_slice()).unwrap_or(&[]);
        let cxxflags: &[String] = pkg_flags.map(|f| f.cxxflags.as_slice()).unwrap_or(&[]);
        let ldflags: &[String] = pkg_flags.map(|f| f.ldflags.as_slice()).unwrap_or(&[]);

        let mut objects: Vec<PathBuf> = Vec::with_capacity(prepared.len());
        for ps in &prepared {
            let depfile = depfile_path(&ps.object);
            // Pick the language-appropriate compiler driver, the
            // language-appropriate standard / profile flags, the
            // matching escape-hatch arg list, the action kind,
            // and the human-readable tag. Naming the components
            // here is the single point that enforces "C and C++
            // compile lines never share argv space".
            let dispatch = compile_dispatch(ps.language, req)
                .map_err(|err| err.attach_target_path(tid, req.graph, &ps.abs_source))?;
            let cmd = build_compile_command(&CompileCommandInput {
                driver: dispatch.driver,
                language_flags: &dispatch.language_flags,
                source: &ps.abs_source,
                object: &ps.object,
                depfile: &depfile,
                include_dirs: &include_dirs,
                defines: &defines,
                extra_compile_args,
                extra_language_compile_args: match ps.language {
                    SourceLanguage::C => cflags,
                    SourceLanguage::Cxx => cxxflags,
                },
            })?;
            // Ninja sees the wrapped command (`ccache cxx ...`)
            // for C++ compiles when a compiler-cache wrapper is
            // selected; C compiles stay unwrapped because the public
            // wrapper contract is C++-only today. The matching
            // `compile_commands.json` entry keeps the unwrapped
            // command so clangd / IDE tooling still sees the
            // underlying compiler. Link and archive commands are
            // deliberately never wrapped.
            let ninja_cmd = match (req.compiler_wrapper, ps.language) {
                (Some(wrapper), SourceLanguage::Cxx) => prepend_wrapper(&cmd, wrapper)?,
                _ => cmd.clone(),
            };

            actions.push(Action {
                kind: dispatch.action_kind,
                inputs: vec![ps.abs_source.clone()],
                implicit_inputs: vec![],
                outputs: vec![ps.object.clone()],
                depfile: Some(depfile),
                command: ninja_cmd,
                description: format!("{} {}", dispatch.description_tag, display(&ps.object)?),
            });
            compile_commands.push(CompileCommand {
                directory: req.build_dir.clone(),
                file: ps.abs_source.clone(),
                arguments: cmd,
                output: ps.object.clone(),
            });
            objects.push(ps.object.clone());
        }

        // Per-target language manifest: own sources' languages
        // unioned with every direct target dep's manifest. The
        // topo iteration guarantees dependencies are populated
        // before we visit the dependent.
        let mut languages_here: BTreeSet<SourceLanguage> =
            prepared.iter().map(|p| p.language).collect();
        if let Some(deps) = resolved_deps.get(tid) {
            for dep in deps {
                if let Some(dep_langs) = target_languages.get(dep) {
                    languages_here.extend(dep_langs.iter().copied());
                }
            }
        }

        match target.kind {
            TargetKind::Library => {
                let lib_path = pkg_build_dir.join(format!("lib{}.a", target.name.as_str()));
                let mut cmd = vec![
                    path_to_str(req.toolchain.ar.path())?.to_owned(),
                    "crs".to_owned(),
                    path_to_str(&lib_path)?.to_owned(),
                ];
                for o in &objects {
                    cmd.push(path_to_str(o)?.to_owned());
                }
                actions.push(Action {
                    kind: ActionKind::ArchiveStaticLibrary,
                    inputs: objects.clone(),
                    implicit_inputs: vec![],
                    outputs: vec![lib_path.clone()],
                    depfile: None,
                    command: cmd,
                    description: format!("AR {}", display(&lib_path)?),
                });
                output_for_target.insert(tid.clone(), lib_path);
            }
            // Every executable kind takes the same link path:
            // `executable` (production binaries), `test`
            // (run by `cabin test`), and `example`. The build
            // planner does not distinguish between them here because
            // the link/compile semantics are identical; the kind
            // difference is only consulted when deciding *which*
            // targets to select (default-buildable vs. dev-only) and
            // which targets `cabin test` runs. Compiler-driver
            // selection is per-source via `link_driver_language`, so
            // an `executable` that declares only `.c` sources
            // drives the link with the C compiler, while one that
            // mixes in any `.cc` / `.cpp` source — directly or
            // transitively — drives the link with the C++ compiler.
            TargetKind::Executable | TargetKind::Test | TargetKind::Example => {
                let exe_path = pkg_build_dir.join(target.name.as_str());
                let lib_paths =
                    collect_link_libs(tid, &resolved_deps, req.graph, &output_for_target);

                let mut inputs: Vec<PathBuf> = objects.clone();
                inputs.extend(lib_paths.iter().cloned());

                // Link-driver pick: C++ if any of this target's
                // own objects came from a C++ source, or if any
                // transitively reachable object did. Otherwise
                // the C compiler drives the link, which keeps
                // pure-C executables off the C++ runtime.
                let languages_slice: Vec<SourceLanguage> = languages_here.iter().copied().collect();
                let driver_language = link_driver_language(&languages_slice);
                let driver_path = match driver_language {
                    SourceLanguage::Cxx => req.toolchain.cxx.path(),
                    SourceLanguage::C => {
                        req.toolchain.cc.as_ref().map(|t| t.path()).ok_or_else(|| {
                            BuildError::MissingCCompiler {
                                target: format_target_id(tid, req.graph),
                                // Pick a representative source for the
                                // diagnostic; pure-C link errors
                                // always have at least one C source on
                                // this target.
                                path: prepared
                                    .iter()
                                    .find(|p| p.language == SourceLanguage::C)
                                    .map(|p| p.abs_source.clone())
                                    .unwrap_or_else(|| exe_path.clone()),
                            }
                        })?
                    }
                };
                let mut cmd = vec![path_to_str(driver_path)?.to_owned()];
                for inp in &inputs {
                    cmd.push(path_to_str(inp)?.to_owned());
                }
                for arg in ldflags {
                    cmd.push(arg.clone());
                }
                cmd.push("-o".to_owned());
                cmd.push(path_to_str(&exe_path)?.to_owned());

                actions.push(Action {
                    kind: ActionKind::LinkExecutable,
                    inputs,
                    implicit_inputs: vec![],
                    outputs: vec![exe_path.clone()],
                    depfile: None,
                    command: cmd,
                    description: format!("LINK {}", display(&exe_path)?),
                });
                output_for_target.insert(tid.clone(), exe_path);
            }
            TargetKind::HeaderOnly => {
                unreachable!("header-only targets are skipped before action generation")
            }
        }
        target_languages.insert(tid.clone(), languages_here);
    }

    let default_outputs: Vec<PathBuf> = selected
        .iter()
        .filter_map(|tid| output_for_target.get(tid).cloned())
        .collect();

    Ok(BuildGraph {
        actions,
        default_outputs,
        compile_commands,
    })
}

// ---------------------------------------------------------------------------
// internal: target IDs and lookups
// ---------------------------------------------------------------------------

/// Stable identifier for a target within a [`PackageGraph`]: the index of
/// its package in `graph.packages` and its target name.
type TargetId = (usize, String);

fn lookup_target<'a>(tid: &TargetId, graph: &'a PackageGraph) -> Result<&'a Target, BuildError> {
    let pkg = &graph.packages[tid.0];
    pkg.package
        .targets
        .iter()
        .find(|t| t.name.as_str() == tid.1)
        .ok_or_else(|| BuildError::UnknownTargetInPackage {
            package: pkg.package.name.as_str().to_owned(),
            target: tid.1.clone(),
        })
}

fn format_target_id(tid: &TargetId, graph: &PackageGraph) -> String {
    format!("{}:{}", graph.packages[tid.0].package.name.as_str(), tid.1)
}

// ---------------------------------------------------------------------------
// internal: manifest-target selector resolution
// ---------------------------------------------------------------------------

fn resolve_selection(
    selectors: &[ManifestTargetSelector],
    graph: &PackageGraph,
    selected_packages: Option<&[usize]>,
) -> Result<Vec<TargetId>, BuildError> {
    let mut out: Vec<TargetId> = Vec::with_capacity(selectors.len());
    for sel in selectors {
        out.push(resolve_top_level_selector(sel, graph, selected_packages)?);
    }
    Ok(out)
}

fn resolve_top_level_selector(
    sel: &ManifestTargetSelector,
    graph: &PackageGraph,
    selected_packages: Option<&[usize]>,
) -> Result<TargetId, BuildError> {
    if let Some(pkg_name) = &sel.package {
        let pkg_idx =
            graph
                .index_of(pkg_name)
                .ok_or_else(|| BuildError::UnknownPackageInTargetSelector {
                    package: pkg_name.clone(),
                    selector: format!("{}:{}", pkg_name, sel.name),
                })?;
        let pkg = &graph.packages[pkg_idx];
        if !pkg
            .package
            .targets
            .iter()
            .any(|t| t.name.as_str() == sel.name)
        {
            return Err(BuildError::UnknownTargetInPackage {
                package: pkg_name.clone(),
                target: sel.name.clone(),
            });
        }
        return Ok((pkg_idx, sel.name.clone()));
    }

    // unqualified selectors search the selected
    // package set (or the primary set when no selection is
    // active). We no longer fall back to the root package when it
    // is outside the selected set — that would silently build
    // something the user did not ask for.
    let candidates: Vec<usize> = match selected_packages {
        Some(s) => s.to_vec(),
        None => {
            // Unqualified selector with no workspace selection
            // active: walk the root first, then every primary.
            let mut root_match: Option<TargetId> = None;
            if let Some(root_idx) = graph.root_package {
                let root = &graph.packages[root_idx];
                if root
                    .package
                    .targets
                    .iter()
                    .any(|t| t.name.as_str() == sel.name)
                {
                    root_match = Some((root_idx, sel.name.clone()));
                }
            }
            if let Some(tid) = root_match {
                return Ok(tid);
            }
            graph.primary_packages.clone()
        }
    };

    let mut matches: Vec<TargetId> = Vec::new();
    for idx in candidates {
        let pkg = &graph.packages[idx];
        if pkg
            .package
            .targets
            .iter()
            .any(|t| t.name.as_str() == sel.name)
        {
            matches.push((idx, sel.name.clone()));
        }
    }
    match matches.len() {
        0 => Err(BuildError::UnknownTargetReference(sel.name.clone())),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => Err(BuildError::AmbiguousTarget(
            sel.name.clone(),
            matches
                .iter()
                .map(|(i, t)| format!("{}:{}", graph.packages[*i].package.name.as_str(), t))
                .collect(),
        )),
    }
}

fn default_selection(graph: &PackageGraph, selected_packages: Option<&[usize]>) -> Vec<TargetId> {
    let mut out = Vec::new();
    let pkg_indices: &[usize] = match selected_packages {
        Some(s) => s,
        None => graph.primary_packages.as_slice(),
    };
    for &pkg_idx in pkg_indices {
        let pkg = &graph.packages[pkg_idx];
        for target in &pkg.package.targets {
            if target.kind.is_default_buildable() {
                out.push((pkg_idx, target.name.as_str().to_owned()));
            }
        }
    }
    out
}

/// Build-time selector for `cabin test`: expand a package
/// selection into the set of targets of a specific
/// development-only kind (`test` today). Returns
/// deterministic `(package_index, target_name)` tuples in the same
/// order as the planner consumes selectors. Useful for callers that
/// want every dev-only target of a given kind without naming each
/// one explicitly.
pub fn select_targets_of_kind(
    graph: &PackageGraph,
    selected_packages: Option<&[usize]>,
    kind: TargetKind,
) -> Vec<ManifestTargetSelector> {
    let pkg_indices: &[usize] = match selected_packages {
        Some(s) => s,
        None => graph.primary_packages.as_slice(),
    };
    let mut out = Vec::new();
    for &pkg_idx in pkg_indices {
        let pkg = &graph.packages[pkg_idx];
        for target in &pkg.package.targets {
            if target.kind == kind {
                out.push(ManifestTargetSelector {
                    package: Some(pkg.package.name.as_str().to_owned()),
                    name: target.name.as_str().to_owned(),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// internal: target.deps resolution
// ---------------------------------------------------------------------------

fn resolve_target_dep(
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

fn topo_sort_targets(
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

fn collect_include_dirs(
    start: &TargetId,
    target: &Target,
    resolved: &HashMap<TargetId, Vec<TargetId>>,
    graph: &PackageGraph,
) -> Vec<PathBuf> {
    let manifest_dir = &graph.packages[start.0].manifest_dir;
    let mut result: Vec<PathBuf> = target
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
        let dep_target = match graph.packages[tid.0]
            .package
            .targets
            .iter()
            .find(|t| t.name.as_str() == tid.1)
        {
            Some(t) => t,
            None => continue,
        };
        if dep_target.kind.produces_archive() || dep_target.kind.is_header_only() {
            let dep_manifest = &graph.packages[tid.0].manifest_dir;
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

    result
}

fn collect_link_libs(
    start: &TargetId,
    resolved: &HashMap<TargetId, Vec<TargetId>>,
    graph: &PackageGraph,
    output_for_target: &HashMap<TargetId, PathBuf>,
) -> Vec<PathBuf> {
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
        let target = match graph.packages[node.0]
            .package
            .targets
            .iter()
            .find(|t| t.name.as_str() == node.1)
        {
            Some(t) => t,
            None => return,
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
struct CompileDispatch<'a> {
    /// Driver executable (the compiler binary).
    driver: &'a Path,
    /// Language-specific standard + profile flags.
    language_flags: Vec<String>,
    /// Build-graph action kind to record on the emitted
    /// [`Action`].
    action_kind: ActionKind,
    /// Short human-readable tag (`CC` or `CXX`) used in Ninja
    /// `description = ...` lines.
    description_tag: &'static str,
}

/// Failure modes for [`compile_dispatch`]. Carry only the
/// language-level reason; the planner attaches target / source
/// context via [`CompileDispatchError::attach_target_path`].
enum CompileDispatchError {
    MissingCCompiler,
}

impl CompileDispatchError {
    fn attach_target_path(self, tid: &TargetId, graph: &PackageGraph, path: &Path) -> BuildError {
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
fn compile_dispatch<'a>(
    language: SourceLanguage,
    req: &'a PlanRequest<'a>,
) -> Result<CompileDispatch<'a>, CompileDispatchError> {
    match language {
        SourceLanguage::Cxx => Ok(CompileDispatch {
            driver: req.toolchain.cxx.path(),
            language_flags: cxx_flags_for_profile(&req.profile),
            action_kind: ActionKind::CompileCpp,
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
                language_flags: c_flags_for_profile(&req.profile),
                action_kind: ActionKind::CompileC,
                description_tag: "CC",
            })
        }
    }
}

fn object_path(pkg_build_dir: &Path, target: &str, source: &Path) -> Result<PathBuf, String> {
    let mut sanitized = PathBuf::new();
    for component in source.components() {
        match component {
            Component::Normal(name) => sanitized.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err("parent directory components ('..') are not supported".to_owned());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute source paths are not supported".to_owned());
            }
        }
    }
    if sanitized.as_os_str().is_empty() {
        return Err("source path is empty".to_owned());
    }
    let parent = sanitized
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut name: OsString = sanitized.file_name().unwrap().to_owned();
    name.push(".o");
    Ok(pkg_build_dir
        .join("obj")
        .join(target)
        .join(parent)
        .join(name))
}

fn depfile_path(object: &Path) -> PathBuf {
    let mut s: OsString = object.as_os_str().to_owned();
    s.push(".d");
    PathBuf::from(s)
}

/// Prefix `cmd` with the wrapper executable. Used only on the
/// Ninja command path; `compile_commands.json` keeps the unwrapped
/// argument list so IDE tooling keeps seeing the underlying
/// compiler.
fn prepend_wrapper(
    cmd: &[String],
    wrapper: &ResolvedCompilerWrapper,
) -> Result<Vec<String>, BuildError> {
    let mut out = Vec::with_capacity(cmd.len() + 1);
    out.push(path_to_str(wrapper.path.as_path())?.to_owned());
    out.extend(cmd.iter().cloned());
    Ok(out)
}

/// Build a single compile command. The caller picks the
/// language-appropriate driver, profile flags, and language
/// escape-hatch args; `extra_compile_args` carries the
/// language-neutral escape-hatch args (applied to both C and
/// C++ compiles). The argv shape is identical across languages
/// so backends can render a single rule per language without
/// re-deriving the layout.
struct CompileCommandInput<'a> {
    driver: &'a Path,
    language_flags: &'a [String],
    source: &'a Path,
    object: &'a Path,
    depfile: &'a Path,
    include_dirs: &'a [PathBuf],
    defines: &'a [String],
    extra_compile_args: &'a [String],
    extra_language_compile_args: &'a [String],
}

fn build_compile_command(input: &CompileCommandInput<'_>) -> Result<Vec<String>, BuildError> {
    let &CompileCommandInput {
        driver,
        language_flags,
        source,
        object,
        depfile,
        include_dirs,
        defines,
        extra_compile_args,
        extra_language_compile_args,
    } = input;
    let mut cmd = Vec::new();
    cmd.push(path_to_str(driver)?.to_owned());
    for f in language_flags {
        cmd.push(f.clone());
    }
    cmd.push("-MMD".to_owned());
    cmd.push("-MF".to_owned());
    cmd.push(path_to_str(depfile)?.to_owned());
    for d in defines {
        cmd.push(format!("-D{d}"));
    }
    for i in include_dirs {
        cmd.push("-I".to_owned());
        cmd.push(path_to_str(i)?.to_owned());
    }
    // Language-neutral escape-hatch first, then the
    // language-specific list — so a per-language override always
    // appears later in argv where the compiler treats it as the
    // final word.
    for arg in extra_compile_args {
        cmd.push(arg.clone());
    }
    for arg in extra_language_compile_args {
        cmd.push(arg.clone());
    }
    cmd.push("-c".to_owned());
    cmd.push(path_to_str(source)?.to_owned());
    cmd.push("-o".to_owned());
    cmd.push(path_to_str(object)?.to_owned());
    Ok(cmd)
}

fn path_to_str(p: &Path) -> Result<&str, BuildError> {
    p.to_str()
        .ok_or_else(|| BuildError::NonUtf8Path(p.to_path_buf()))
}

fn display(p: &Path) -> Result<String, BuildError> {
    Ok(path_to_str(p)?.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::{
        Dependency, DependencySource, Package, PackageName, ProfileDefinition, ProfileName,
        ProfileSelection, ResolvedProfile, Target as CoreTarget, TargetName, resolve_profile,
    };
    use cabin_workspace::{PackageGraph, PackageKind, WorkspacePackage};
    use std::collections::BTreeMap;

    fn dev_profile() -> ResolvedProfile {
        resolve_profile(
            &ProfileSelection::default_dev(),
            &BTreeMap::<ProfileName, ProfileDefinition>::new(),
        )
        .expect("built-in dev resolves")
    }

    fn release_profile() -> ResolvedProfile {
        resolve_profile(
            &ProfileSelection::release_alias(),
            &BTreeMap::<ProfileName, ProfileDefinition>::new(),
        )
        .expect("built-in release resolves")
    }

    fn version() -> semver::Version {
        semver::Version::parse("0.1.0").unwrap()
    }

    fn pkg_name(name: &str) -> PackageName {
        PackageName::new(name).unwrap()
    }

    fn target_name(name: &str) -> TargetName {
        TargetName::new(name).unwrap()
    }

    fn target(name: &str, kind: TargetKind, sources: &[&str], deps: &[&str]) -> CoreTarget {
        CoreTarget {
            name: target_name(name),
            kind,
            sources: sources.iter().map(PathBuf::from).collect(),
            include_dirs: Vec::new(),
            defines: Vec::new(),
            deps: deps.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    fn target_with_includes(
        name: &str,
        kind: TargetKind,
        sources: &[&str],
        includes: &[&str],
        deps: &[&str],
    ) -> CoreTarget {
        CoreTarget {
            name: target_name(name),
            kind,
            sources: sources.iter().map(PathBuf::from).collect(),
            include_dirs: includes.iter().map(PathBuf::from).collect(),
            defines: Vec::new(),
            deps: deps.iter().map(|d| (*d).to_owned()).collect(),
        }
    }

    fn dep(name: &str, path: &str) -> Dependency {
        Dependency {
            name: pkg_name(name),
            source: DependencySource::Path(PathBuf::from(path)),
            kind: cabin_core::DependencyKind::Normal,
            optional: false,
            features: Vec::new(),
            default_features: true,
            condition: None,
        }
    }

    fn toolchain() -> ResolvedToolchain {
        use cabin_core::{ResolvedTool, ToolKind, ToolSource, ToolSpec};
        ResolvedToolchain {
            cxx: ResolvedTool {
                kind: ToolKind::CxxCompiler,
                path: PathBuf::from("/usr/bin/g++"),
                spec: ToolSpec::Name("g++".into()),
                source: ToolSource::Default,
            },
            ar: ResolvedTool {
                kind: ToolKind::Archiver,
                path: PathBuf::from("/usr/bin/ar"),
                spec: ToolSpec::Name("ar".into()),
                source: ToolSource::Default,
            },
            cc: None,
        }
    }

    /// Toolchain with both compilers resolved. Used by tests that
    /// exercise the C compile path or the link-driver pick.
    fn toolchain_with_cc() -> ResolvedToolchain {
        use cabin_core::{ResolvedTool, ToolKind, ToolSource, ToolSpec};
        let mut tc = toolchain();
        tc.cc = Some(ResolvedTool {
            kind: ToolKind::CCompiler,
            path: PathBuf::from("/usr/bin/cc"),
            spec: ToolSpec::Name("cc".into()),
            source: ToolSource::Default,
        });
        tc
    }

    fn empty_build_flags() -> HashMap<usize, ResolvedProfileFlags> {
        HashMap::new()
    }

    fn make_pkg(
        _name: &str,
        manifest_dir: &str,
        package: Package,
        deps: Vec<usize>,
    ) -> WorkspacePackage {
        let manifest_dir = PathBuf::from(manifest_dir);
        let manifest_path = manifest_dir.join("cabin.toml");
        WorkspacePackage {
            package,
            manifest_path,
            manifest_dir,
            deps: deps
                .into_iter()
                .map(|index| cabin_workspace::DependencyEdge {
                    index,
                    kind: cabin_core::DependencyKind::Normal,
                    condition: None,
                })
                .collect(),
            kind: PackageKind::Local,
        }
    }

    fn graph_with(
        packages: Vec<WorkspacePackage>,
        primaries: Vec<usize>,
        root: Option<usize>,
    ) -> PackageGraph {
        let root_dir = packages
            .first()
            .map(|p| p.manifest_dir.clone())
            .unwrap_or_else(|| PathBuf::from("/abs"));
        let root_manifest = root_dir.join("cabin.toml");
        PackageGraph {
            root_manifest_path: root_manifest,
            root_dir,
            is_workspace_root: false,
            root_package: root,
            root_settings: Default::default(),
            primary_packages: primaries,
            default_members: Vec::new(),
            excluded_members: Vec::new(),
            packages,
        }
    }

    fn single_package_graph(package: Package, manifest_dir: &str) -> PackageGraph {
        let name = package.name.as_str().to_owned();
        let pkg = make_pkg(&name, manifest_dir, package, vec![]);
        graph_with(vec![pkg], vec![0], Some(0))
    }

    #[test]
    fn plans_single_executable() {
        let package = Package::new(
            pkg_name("hello"),
            version(),
            vec![target(
                "hello",
                TargetKind::Executable,
                &["src/main.cc"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/proj");
        let tc = toolchain();
        let req = PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/proj/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        };
        let bg = plan(&req).unwrap();
        assert_eq!(bg.actions.len(), 2);
        assert_eq!(bg.actions[0].kind, ActionKind::CompileCpp);
        assert_eq!(bg.actions[1].kind, ActionKind::LinkExecutable);
        assert_eq!(
            bg.default_outputs,
            vec![PathBuf::from("/abs/proj/build/dev/packages/hello/hello")]
        );
        let cc = &bg.compile_commands[0];
        assert_eq!(
            cc.output,
            PathBuf::from("/abs/proj/build/dev/packages/hello/obj/hello/src/main.cc.o")
        );
        assert!(cc.arguments.iter().any(|a| a == "-std=c++17"));
    }

    #[test]
    fn compiler_wrapper_prefixes_only_the_ninja_command() {
        let package = Package::new(
            pkg_name("hello"),
            version(),
            vec![target(
                "hello",
                TargetKind::Executable,
                &["src/main.cc"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/proj");
        let tc = toolchain();
        let wrapper = ResolvedCompilerWrapper {
            kind: cabin_core::CompilerWrapperKind::Ccache,
            path: PathBuf::from("/usr/local/bin/ccache"),
            spec: "ccache".into(),
            source: cabin_core::CompilerWrapperSource::Cli,
            identity: None,
        };
        let req = PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/proj/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: Some(&wrapper),
        };
        let bg = plan(&req).unwrap();
        let compile = bg
            .actions
            .iter()
            .find(|a| a.kind == ActionKind::CompileCpp)
            .expect("compile action present");
        assert_eq!(compile.command[0], "/usr/local/bin/ccache");
        assert_eq!(compile.command[1], "/usr/bin/g++");
        let cc = &bg.compile_commands[0];
        // compile_commands.json must keep the underlying compiler
        // first so clangd / IDE tooling continues to recognize the
        // command shape.
        assert_eq!(cc.arguments[0], "/usr/bin/g++");
        // Link / archive paths are never wrapped.
        let link = bg
            .actions
            .iter()
            .find(|a| a.kind == ActionKind::LinkExecutable)
            .expect("link action present");
        assert_eq!(link.command[0], "/usr/bin/g++");
        assert!(
            !link.command.iter().any(|a| a == "/usr/local/bin/ccache"),
            "wrapper must not appear in link command"
        );
    }

    #[test]
    fn compiler_wrapper_does_not_prefix_c_compile_commands() {
        let package = Package::new(
            pkg_name("hello"),
            version(),
            vec![target(
                "hello",
                TargetKind::Executable,
                &["src/main.c"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/proj");
        let tc = toolchain_with_cc();
        let wrapper = ResolvedCompilerWrapper {
            kind: cabin_core::CompilerWrapperKind::Ccache,
            path: PathBuf::from("/usr/local/bin/ccache"),
            spec: "ccache".into(),
            source: cabin_core::CompilerWrapperSource::Cli,
            identity: None,
        };
        let req = PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/proj/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: Some(&wrapper),
        };
        let bg = plan(&req).unwrap();
        let compile = bg
            .actions
            .iter()
            .find(|a| a.kind == ActionKind::CompileC)
            .expect("C compile action present");
        assert_eq!(compile.command[0], "/usr/bin/cc");
        assert!(
            !compile.command.iter().any(|a| a == "/usr/local/bin/ccache"),
            "wrapper must not appear in C compile command"
        );
    }

    #[test]
    fn release_profile_uses_release_flags() {
        let package = Package::new(
            pkg_name("hello"),
            version(),
            vec![target(
                "hello",
                TargetKind::Executable,
                &["src/main.cc"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/proj");
        let tc = toolchain();
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/proj/build"),
            profile: release_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();
        let cc = &bg.compile_commands[0];
        assert!(cc.arguments.iter().any(|a| a == "-O3"));
        assert!(cc.arguments.iter().any(|a| a == "-DNDEBUG"));
        assert!(!cc.arguments.iter().any(|a| a == "-O0"));
    }

    #[test]
    fn plans_library_then_executable_within_one_package() {
        let package = Package::new(
            pkg_name("multi"),
            version(),
            vec![
                target_with_includes(
                    "greet",
                    TargetKind::Library,
                    &["src/greet.cc"],
                    &["include"],
                    &[],
                ),
                target(
                    "hello",
                    TargetKind::Executable,
                    &["src/main.cc"],
                    &["greet"],
                ),
            ],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/proj");
        let tc = toolchain();
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/proj/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();
        let kinds: Vec<ActionKind> = bg.actions.iter().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ActionKind::CompileCpp,
                ActionKind::ArchiveStaticLibrary,
                ActionKind::CompileCpp,
                ActionKind::LinkExecutable,
            ]
        );
        let link = &bg.actions[3];
        assert!(link.inputs.contains(&PathBuf::from(
            "/abs/proj/build/dev/packages/multi/libgreet.a"
        )));
        let hello_compile = &bg.actions[2];
        assert!(
            hello_compile
                .command
                .iter()
                .any(|a| a == "/abs/proj/include")
        );
    }

    #[test]
    fn cross_package_path_dep_links_library() {
        // greet at /abs/greet, app at /abs/app depending on greet.
        let greet_proj = Package::new(
            pkg_name("greet"),
            version(),
            vec![target_with_includes(
                "greet",
                TargetKind::Library,
                &["src/greet.cc"],
                &["include"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let app_proj = Package::new(
            pkg_name("app"),
            version(),
            vec![target(
                "app",
                TargetKind::Executable,
                &["src/main.cc"],
                &["greet"],
            )],
            vec![dep("greet", "../greet")],
        )
        .unwrap();
        let greet_pkg = make_pkg("greet", "/abs/greet", greet_proj, vec![]);
        let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
        let graph = graph_with(vec![greet_pkg, app_pkg], vec![1], Some(1));
        let tc = toolchain();
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();

        // Outputs should be namespaced by package.
        let greet_lib = PathBuf::from("/abs/build/dev/packages/greet/libgreet.a");
        let app_exe = PathBuf::from("/abs/build/dev/packages/app/app");
        // app's link action must include greet's static archive.
        let link = bg
            .actions
            .iter()
            .find(|a| a.kind == ActionKind::LinkExecutable)
            .unwrap();
        assert!(link.inputs.contains(&greet_lib));
        assert_eq!(link.outputs, vec![app_exe.clone()]);

        // Default outputs are only the primary package's targets (app).
        assert_eq!(bg.default_outputs, vec![app_exe]);

        // greet's include dir should propagate into app's compile command.
        let app_compile = bg
            .actions
            .iter()
            .find(|a| {
                a.kind == ActionKind::CompileCpp && a.outputs[0].to_string_lossy().contains("/app/")
            })
            .unwrap();
        assert!(
            app_compile
                .command
                .iter()
                .any(|a| a == "/abs/greet/include")
        );
    }

    #[test]
    fn qualified_target_selector_picks_specific_target() {
        let greet_proj = Package::new(
            pkg_name("greet"),
            version(),
            vec![target("greet", TargetKind::Library, &["src/greet.cc"], &[])],
            Vec::new(),
        )
        .unwrap();
        let app_proj = Package::new(
            pkg_name("app"),
            version(),
            vec![
                target("app", TargetKind::Executable, &["src/main.cc"], &["greet"]),
                target("other", TargetKind::Executable, &["src/other.cc"], &[]),
            ],
            vec![dep("greet", "../greet")],
        )
        .unwrap();
        let greet_pkg = make_pkg("greet", "/abs/greet", greet_proj, vec![]);
        let app_pkg = make_pkg("app", "/abs/app", app_proj, vec![0]);
        let graph = graph_with(vec![greet_pkg, app_pkg], vec![1], Some(1));
        let tc = toolchain();
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/build"),
            profile: dev_profile(),
            selected: Some(vec![ManifestTargetSelector::parse("app:app")]),
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();
        // Only app:app and greet:greet should appear; not app:other.
        let outs: Vec<String> = bg
            .actions
            .iter()
            .map(|a| a.outputs[0].display().to_string())
            .collect();
        assert!(outs.iter().any(|o| o.ends_with("/packages/app/app")));
        assert!(!outs.iter().any(|o| o.contains("/packages/app/other")));
    }

    #[test]
    fn ambiguous_unqualified_target_errors() {
        // Workspace with two member packages each having a target "build".
        let a = Package::new(
            pkg_name("a"),
            version(),
            vec![target("build", TargetKind::Executable, &["a.cc"], &[])],
            Vec::new(),
        )
        .unwrap();
        let b = Package::new(
            pkg_name("b"),
            version(),
            vec![target("build", TargetKind::Executable, &["b.cc"], &[])],
            Vec::new(),
        )
        .unwrap();
        let pkg_a = make_pkg("a", "/abs/a", a, vec![]);
        let pkg_b = make_pkg("b", "/abs/b", b, vec![]);
        let mut graph = graph_with(vec![pkg_a, pkg_b], vec![0, 1], None);
        graph.is_workspace_root = true;
        let tc = toolchain();
        let err = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/build"),
            profile: dev_profile(),
            selected: Some(vec![ManifestTargetSelector::parse("build")]),
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap_err();
        assert!(matches!(err, BuildError::AmbiguousTarget(_, _)));
    }

    #[test]
    fn unknown_package_in_qualified_selector_errors() {
        let package = Package::new(
            pkg_name("hello"),
            version(),
            vec![target(
                "hello",
                TargetKind::Executable,
                &["src/main.cc"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/proj");
        let tc = toolchain();
        let err = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/build"),
            profile: dev_profile(),
            selected: Some(vec![ManifestTargetSelector::parse("nope:thing")]),
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap_err();
        assert!(matches!(
            err,
            BuildError::UnknownPackageInTargetSelector { .. }
        ));
    }

    #[test]
    fn target_dep_cycle_within_package_is_reported() {
        let package = Package {
            name: pkg_name("cyc"),
            version: version(),
            targets: vec![
                target("a", TargetKind::Library, &["a.cc"], &["b"]),
                target("b", TargetKind::Library, &["b.cc"], &["a"]),
            ],
            dependencies: Vec::new(),
            system_dependencies: Vec::new(),
            features: Default::default(),
            profiles: Default::default(),
            toolchain: Default::default(),
            build: Default::default(),
            compiler_wrapper: Default::default(),
            patches: Default::default(),
        };
        let graph = single_package_graph(package, "/abs/proj");
        let tc = toolchain();
        let err = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap_err();
        match err {
            BuildError::DependencyCycle(cycle) => {
                assert_eq!(cycle.first(), cycle.last());
                assert!(cycle.iter().any(|s| s == "cyc:a"));
                assert!(cycle.iter().any(|s| s == "cyc:b"));
            }
            other => panic!("expected DependencyCycle, got {other:?}"),
        }
    }

    #[test]
    fn unknown_target_in_qualified_selector_errors() {
        let package = Package::new(
            pkg_name("hello"),
            version(),
            vec![target(
                "hello",
                TargetKind::Executable,
                &["src/main.cc"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/proj");
        let tc = toolchain();
        let err = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/build"),
            profile: dev_profile(),
            selected: Some(vec![ManifestTargetSelector::parse("hello:missing")]),
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap_err();
        assert!(matches!(err, BuildError::UnknownTargetInPackage { .. }));
    }

    /// Helper: extract the link-action command from a planned
    /// graph. Returns the `Vec<String>` argv of the first
    /// `LinkExecutable` action so tests can assert on `command[0]`
    /// (the chosen driver). Panics if no link action is present.
    fn link_command(bg: &BuildGraph) -> &Vec<String> {
        &bg.actions
            .iter()
            .find(|a| a.kind == ActionKind::LinkExecutable)
            .expect("link action present")
            .command
    }

    #[test]
    fn link_driver_is_c_when_target_has_only_c_sources() {
        // A pure-C executable must link through the C driver so
        // the C++ runtime is not pulled in.
        let package = Package::new(
            pkg_name("cdemo"),
            version(),
            vec![target(
                "cdemo_exe",
                TargetKind::Executable,
                &["src/main.c"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/cdemo");
        let tc = toolchain_with_cc();
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/cdemo/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();
        let link = link_command(&bg);
        assert_eq!(link[0], "/usr/bin/cc");
    }

    #[test]
    fn link_driver_is_cxx_when_target_has_any_cpp_source() {
        // Mixed C / C++ executable in a single target must link
        // through the C++ driver because the closure has C++
        // objects.
        let package = Package::new(
            pkg_name("mixed"),
            version(),
            vec![target(
                "mixed_exe",
                TargetKind::Executable,
                &["src/c_part.c", "src/cpp_part.cc"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/mixed");
        let tc = toolchain_with_cc();
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/mixed/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();
        let link = link_command(&bg);
        assert_eq!(link[0], "/usr/bin/g++");
    }

    #[test]
    fn link_driver_is_cxx_when_dependency_has_cpp_objects() {
        // Pure-C executable that links a C++ static library
        // must use the C++ driver — the runtime is required
        // because the library carries C++ objects.
        let cpp_lib = target("cppcore", TargetKind::Library, &["src/cpp_part.cc"], &[]);
        let c_exe = target(
            "c_runner",
            TargetKind::Executable,
            &["src/main.c"],
            &["cppcore"],
        );
        let package = Package::new(
            pkg_name("interop"),
            version(),
            vec![cpp_lib, c_exe],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/interop");
        let tc = toolchain_with_cc();
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/interop/build"),
            profile: dev_profile(),
            selected: Some(vec![ManifestTargetSelector::parse("c_runner")]),
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();
        let link = link_command(&bg);
        assert_eq!(link[0], "/usr/bin/g++");
    }

    #[test]
    fn link_driver_stays_c_when_dependency_is_also_pure_c() {
        // C executable + C library: still link through the C
        // driver because the closure has no C++ objects.
        let c_lib = target("ccore", TargetKind::Library, &["src/util.c"], &[]);
        let c_exe = target(
            "c_runner",
            TargetKind::Executable,
            &["src/main.c"],
            &["ccore"],
        );
        let package = Package::new(
            pkg_name("clib_only"),
            version(),
            vec![c_lib, c_exe],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/clib_only");
        let tc = toolchain_with_cc();
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/clib_only/build"),
            profile: dev_profile(),
            selected: Some(vec![ManifestTargetSelector::parse("c_runner")]),
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();
        let link = link_command(&bg);
        assert_eq!(link[0], "/usr/bin/cc");
    }

    #[test]
    fn missing_c_compiler_yields_actionable_error_with_target_id() {
        // C source + no `cc` resolved → MissingCCompiler error
        // that names both the package and the target so a
        // monorepo user can map the failure to the right
        // manifest.
        let package = Package::new(
            pkg_name("cdemo"),
            version(),
            vec![target(
                "cdemo_exe",
                TargetKind::Executable,
                &["src/main.c"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/cdemo");
        let tc = toolchain(); // no cc populated
        let err = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/cdemo/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("cdemo:cdemo_exe"),
            "error should name the package:target, got: {rendered}"
        );
        assert!(
            rendered.contains("CC") || rendered.contains("`--cc"),
            "error should suggest how to set the C compiler, got: {rendered}"
        );
    }

    #[test]
    fn unrecognized_source_extension_yields_actionable_error() {
        let package = Package::new(
            pkg_name("broken"),
            version(),
            vec![target(
                "broken",
                TargetKind::Library,
                &["src/file.txt"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/broken");
        let tc = toolchain_with_cc();
        let err = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &empty_build_flags(),
            build_dir: PathBuf::from("/abs/broken/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("broken:broken"),
            "error should name package:target, got: {rendered}"
        );
        assert!(
            rendered.contains(".c") && rendered.contains(".cc"),
            "error should enumerate the supported extensions, got: {rendered}"
        );
    }

    #[test]
    fn flags_for_profile_returns_only_standard_and_optimization_flags() {
        // The shared helper threads the standard flag in front
        // of the language-neutral optimization flags. Anchoring
        // the assertion on the helper rather than on the
        // language-specific wrappers gives one place to update
        // if the default profile flags change.
        let dev = dev_profile();
        let c = flags_for_profile(DEFAULT_C_STANDARD, &dev);
        let cxx = flags_for_profile(DEFAULT_CXX_STANDARD, &dev);
        assert_eq!(c[0], "-std=c11");
        assert_eq!(cxx[0], "-std=c++17");
        // Optimization flags appear in the same order on both
        // languages — that is the language-neutral postfix.
        assert_eq!(&c[1..], &cxx[1..]);
        // The C standard never sneaks into the C++ flag list
        // and vice versa.
        assert!(!cxx.iter().any(|f| f == "-std=c11"));
        assert!(!c.iter().any(|f| f == "-std=c++17"));
    }

    /// Build a single-package graph with a `mixed` library
    /// target carrying one C source and one C++ source. Used by
    /// the per-language flag-routing tests below.
    fn graph_with_mixed_sources() -> PackageGraph {
        let package = Package::new(
            pkg_name("mixed"),
            version(),
            vec![target(
                "mixed",
                TargetKind::Library,
                &["src/c_part.c", "src/cpp_part.cc"],
                &[],
            )],
            Vec::new(),
        )
        .unwrap();
        single_package_graph(package, "/abs/mixed")
    }

    /// Build per-package flag map carrying a single
    /// `ResolvedProfileFlags` for the mixed package.
    fn build_flags_map(flags: ResolvedProfileFlags) -> HashMap<usize, ResolvedProfileFlags> {
        let mut out = HashMap::new();
        out.insert(0usize, flags);
        out
    }

    /// Plan and return the compile actions for the mixed
    /// fixture under the supplied build flags. Used by every
    /// per-language flag-routing test below to keep the
    /// boilerplate to one place.
    fn plan_compile_actions(flags: ResolvedProfileFlags) -> Vec<Action> {
        let graph = graph_with_mixed_sources();
        let tc = toolchain_with_cc();
        let map = build_flags_map(flags);
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &map,
            build_dir: PathBuf::from("/abs/mixed/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();
        bg.actions
            .into_iter()
            .filter(|a| matches!(a.kind, ActionKind::CompileC | ActionKind::CompileCpp))
            .collect()
    }

    fn compile_action_for(actions: &[Action], kind: ActionKind) -> &Action {
        actions
            .iter()
            .find(|a| a.kind == kind)
            .unwrap_or_else(|| panic!("expected a {kind:?} compile action"))
    }

    #[test]
    fn cflags_route_to_c_compile_only() {
        // The C-only escape-hatch reaches every C compile
        // command and never reaches a C++ compile. Without this
        // routing, a flag that is invalid for C++ (`-std=c99`,
        // `-Wno-pointer-sign`) would break C++ builds.
        let flags = ResolvedProfileFlags {
            cflags: vec!["-DC_ONLY_FLAG=1".to_owned()],
            ..ResolvedProfileFlags::default()
        };
        let actions = plan_compile_actions(flags);
        let c = compile_action_for(&actions, ActionKind::CompileC);
        let cxx = compile_action_for(&actions, ActionKind::CompileCpp);
        assert!(
            c.command.iter().any(|a| a == "-DC_ONLY_FLAG=1"),
            "C compile must include the C-only define, got: {:?}",
            c.command
        );
        assert!(
            !cxx.command.iter().any(|a| a == "-DC_ONLY_FLAG=1"),
            "C-only define must NOT leak into the C++ compile, got: {:?}",
            cxx.command
        );
    }

    #[test]
    fn cxxflags_route_to_cxx_compile_only() {
        // Mirror of the C-only test: a C++-only flag never
        // reaches the C compile command. Required so a flag
        // that is invalid for C (`-fno-rtti`, `-std=c++20`)
        // does not break C builds.
        let flags = ResolvedProfileFlags {
            cxxflags: vec!["-DCXX_ONLY_FLAG=1".to_owned()],
            ..ResolvedProfileFlags::default()
        };
        let actions = plan_compile_actions(flags);
        let c = compile_action_for(&actions, ActionKind::CompileC);
        let cxx = compile_action_for(&actions, ActionKind::CompileCpp);
        assert!(
            cxx.command.iter().any(|a| a == "-DCXX_ONLY_FLAG=1"),
            "C++ compile must include the C++-only define, got: {:?}",
            cxx.command
        );
        assert!(
            !c.command.iter().any(|a| a == "-DCXX_ONLY_FLAG=1"),
            "C++-only define must NOT leak into the C compile, got: {:?}",
            c.command
        );
    }

    #[test]
    fn language_neutral_extra_compile_args_reach_both_compile_kinds() {
        // The language-neutral slot is the documented home for
        // flags that are valid for both C and C++. It must
        // appear on every compile command.
        let flags = ResolvedProfileFlags {
            extra_compile_args: vec!["-Wall".to_owned()],
            ..ResolvedProfileFlags::default()
        };
        let actions = plan_compile_actions(flags);
        let c = compile_action_for(&actions, ActionKind::CompileC);
        let cxx = compile_action_for(&actions, ActionKind::CompileCpp);
        assert!(
            c.command.iter().any(|a| a == "-Wall"),
            "C compile must include the language-neutral flag, got: {:?}",
            c.command
        );
        assert!(
            cxx.command.iter().any(|a| a == "-Wall"),
            "C++ compile must include the language-neutral flag, got: {:?}",
            cxx.command
        );
    }

    #[test]
    fn ldflags_appear_on_link_command_only() {
        // Link-only flags must reach every link command and
        // never appear on a compile command.
        let package = Package::new(
            pkg_name("mixed"),
            version(),
            vec![
                target(
                    "mixedlib",
                    TargetKind::Library,
                    &["src/c_part.c", "src/cpp_part.cc"],
                    &[],
                ),
                target(
                    "app",
                    TargetKind::Executable,
                    &["src/main.cc"],
                    &["mixedlib"],
                ),
            ],
            Vec::new(),
        )
        .unwrap();
        let graph = single_package_graph(package, "/abs/mixed");
        let tc = toolchain_with_cc();
        let mut map = HashMap::new();
        let flags = ResolvedProfileFlags {
            ldflags: vec!["-Wl,--as-needed".to_owned()],
            ..ResolvedProfileFlags::default()
        };
        map.insert(0usize, flags);
        let bg = plan(&PlanRequest {
            graph: &graph,
            toolchain: &tc,
            build_flags: &map,
            build_dir: PathBuf::from("/abs/mixed/build"),
            profile: dev_profile(),
            selected: None,
            configuration: None,
            selected_packages: None,
            compiler_wrapper: None,
        })
        .unwrap();
        let link = bg
            .actions
            .iter()
            .find(|a| a.kind == ActionKind::LinkExecutable)
            .expect("link action present");
        assert!(
            link.command.iter().any(|a| a == "-Wl,--as-needed"),
            "link command must include the link-only flag, got: {:?}",
            link.command
        );
        for compile in bg
            .actions
            .iter()
            .filter(|a| matches!(a.kind, ActionKind::CompileC | ActionKind::CompileCpp))
        {
            assert!(
                !compile.command.iter().any(|a| a == "-Wl,--as-needed"),
                "link-only flag must NOT appear on compile, got: {:?}",
                compile.command
            );
        }
    }
}
