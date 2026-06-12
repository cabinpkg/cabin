use crate::error::BuildError;
use crate::graph::{BuildGraph, CompileCommand, MsvcStandardViolation};
use cabin_core::{
    InterfaceStandardSource, LanguageStandard, ResolvedCompilerWrapper, ResolvedLanguageStandards,
    ResolvedProfile, ResolvedProfileFlags, ResolvedToolchain, SourceLanguage, Target, TargetKind,
    classify_source, link_driver_language,
};
use cabin_driver::{
    ArchiveAction, BuildAction, CompileAction, CompileArguments, CompileMode, Dialect, LinkAction,
    compile_argv,
};
use cabin_workspace::PackageGraph;
use camino::Utf8PathBuf;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

mod lowering;
#[cfg(test)]
mod tests;

use self::lowering::{
    collect_include_dirs, collect_link_lib_names, collect_link_libs, compile_dispatch,
    depfile_path, object_path, promote_dir, resolve_target_dep, topo_sort_targets,
};

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
    /// Per-package effective language standards. Missing entries
    /// fall back to the built-in defaults, mirroring `build_flags`.
    pub language_standards: &'a HashMap<usize, ResolvedLanguageStandards>,
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
    /// Compiler command-line dialect for this build. Selected from
    /// the detected C++ compiler (MSVC drives the `cl.exe` dialect).
    /// Governs artifact naming (`.o` vs `.obj`, `lib<x>.a` vs
    /// `<x>.lib`, `<x>` vs `<x>.exe`) and the spelling of every
    /// compile / archive / link command the lowering emits.
    pub dialect: Dialect,
    /// Whether the MSVC-dialect compilers accept the `/external:I`
    /// block ([`crate::msvc_external_includes_supported`]). When
    /// `false` on an MSVC build, the planner collapses the system
    /// include bucket into the plain `/I` list instead of emitting a
    /// switch an old `cl` would reject. Ignored by the GCC/Clang
    /// dialect, where `-isystem` is part of the base command line.
    pub msvc_external_includes: bool,
}

/// One manifest-declared source resolved to its absolute path and the
/// per-target object path it compiles to.
struct PreparedSource {
    abs_source: Utf8PathBuf,
    object: Utf8PathBuf,
    language: SourceLanguage,
}

/// Plan a build for the requested package graph.
///
/// # Errors
/// Returns a [`BuildError`] when the request cannot be turned into
/// a valid graph: [`BuildError::NonUtf8Path`] when the build directory
/// or a package's manifest directory is not valid UTF-8 and so cannot
/// anchor the UTF-8 build model; [`BuildError::EmptySelectedPackages`]
/// when the default selection yields no C/C++ targets; selection
/// and dependency-resolution errors ([`BuildError::UnknownTargetReference`],
/// [`BuildError::AmbiguousTarget`], [`BuildError::UnknownPackageInTargetSelector`],
/// [`BuildError::UnknownTargetInPackage`], [`BuildError::DependencyHasNoLibrary`],
/// [`BuildError::AmbiguousDefaultLibrary`]); [`BuildError::DependencyCycle`]
/// when the target dependency graph contains a cycle; and per-target
/// source errors ([`BuildError::UnrecognizedSourceExtension`],
/// [`BuildError::InvalidSourcePath`], [`BuildError::EmptyTargetSources`],
/// [`BuildError::MissingCCompiler`]).
pub fn plan(req: &PlanRequest<'_>) -> Result<BuildGraph, BuildError> {
    let selected = if let Some(sel) = &req.selected {
        resolve_selection(sel, req.graph, req.selected_packages)?
    } else {
        let chosen = default_selection(req.graph, req.selected_packages);
        if chosen.is_empty() {
            return Err(BuildError::EmptySelectedPackages);
        }
        chosen
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

    // Promote the OS-supplied build directory to UTF-8 once: it
    // prefixes every object, archive, and executable path in the
    // semantic IR, so it must be valid UTF-8 to be embedded in build
    // commands. A non-UTF-8 build directory is rejected here rather
    // than silently lossily converted downstream.
    let build_dir = promote_dir(&req.build_dir)?;

    let mut actions: Vec<BuildAction> = Vec::new();
    let mut compile_commands: Vec<CompileCommand> = Vec::new();
    let mut msvc_standard_violations: Vec<MsvcStandardViolation> = Vec::new();
    let mut output_for_target: HashMap<TargetId, Utf8PathBuf> = HashMap::new();
    // Per-target source-language manifest, including transitive
    // contributions through `target.deps`. Used to pick the
    // link-driver language deterministically: a target with any
    // direct or transitive C++ object link-drives through the C++
    // compiler, every other target link-drives through the C
    // compiler. Populated in topo order so dependents inherit
    // their dependencies' contributions.
    let mut target_languages: HashMap<TargetId, BTreeSet<SourceLanguage>> = HashMap::new();
    // Transitively reachable dependency targets per target, in
    // first-occurrence order, populated in topo order (direct deps
    // plus their reachable sets). Drives the interface-standard
    // compatibility check.
    let mut transitive_deps: HashMap<TargetId, Vec<TargetId>> = HashMap::new();

    for tid in &topo {
        let target = lookup_target(tid, req.graph)?;
        let mut dep_closure: Vec<TargetId> = Vec::new();
        if let Some(deps) = resolved_deps.get(tid) {
            for dep in deps {
                if !dep_closure.contains(dep) {
                    dep_closure.push(dep.clone());
                }
                if let Some(transitive) = transitive_deps.get(dep) {
                    for transitive_dep in transitive {
                        if !dep_closure.contains(transitive_dep) {
                            dep_closure.push(transitive_dep.clone());
                        }
                    }
                }
            }
        }
        transitive_deps.insert(tid.clone(), dep_closure.clone());

        let pkg = &req.graph.packages[tid.0];
        let pkg_name = pkg.package.name.as_str();
        // Per-profile output root keeps `dev` and `release`
        // builds from overwriting each other and gives custom
        // profiles a deterministic, non-colliding output tree.
        let pkg_build_dir = build_dir
            .join(req.profile.name.as_str())
            .join("packages")
            .join(pkg_name);
        // The manifest directory is an OS-canonicalized path; promote
        // it to UTF-8 (rejecting non-UTF-8) so the source and include
        // paths it anchors enter the IR as `Utf8PathBuf`.
        let manifest_dir = promote_dir(&pkg.manifest_dir)?;

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
        let mut prepared: Vec<PreparedSource> = Vec::with_capacity(target.sources.len());
        for source in &target.sources {
            let language =
                classify_source(source).ok_or_else(|| BuildError::UnrecognizedSourceExtension {
                    target: format_target_id(tid, req.graph),
                    path: source.clone(),
                })?;
            let object = object_path(&pkg_build_dir, target.name.as_str(), source, req.dialect)
                .map_err(|reason| BuildError::InvalidSourcePath {
                    target: format_target_id(tid, req.graph),
                    path: source.clone(),
                    reason,
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

        let pkg_standards = req
            .language_standards
            .get(&tid.0)
            .copied()
            .unwrap_or_default();
        enforce_interface_standards(tid, target, &prepared, &dep_closure, pkg_standards, req)?;

        // Per-package resolved build flags from the manifest's
        // `[profile]`, `[target.'cfg(...)'.profile]`, and the active
        // `[profile.<name>]` table. Layered on top of per-target
        // defines / include dirs.
        let pkg_flags = req.build_flags.get(&tid.0);

        // Compose include_dirs and defines: existing target +
        // per-package build flags, partitioned into the user (`-I`)
        // and system (`-isystem` / `/external:I`) buckets.
        let collected = collect_include_dirs(tid, target, &resolved_deps, req.graph)?;
        let mut include_dirs = collected.user;
        let mut system_include_dirs = collected.system;
        if let Some(flags) = pkg_flags {
            for inc in &flags.include_dirs {
                let absolute = if inc.is_absolute() {
                    inc.clone()
                } else {
                    manifest_dir.join(inc)
                };
                if !include_dirs.contains(&absolute) && !system_include_dirs.contains(&absolute) {
                    include_dirs.push(absolute);
                }
            }
            for inc in &flags.system_include_dirs {
                let absolute = if inc.is_absolute() {
                    inc.clone()
                } else {
                    manifest_dir.join(inc)
                };
                if !include_dirs.contains(&absolute) && !system_include_dirs.contains(&absolute) {
                    system_include_dirs.push(absolute);
                }
            }
        }
        // An MSVC toolchain that predates `/external:I` cannot spell
        // the system bucket; fall back to plain `/I` for those dirs —
        // exactly the pre-system-include command shape.
        if req.dialect == Dialect::Msvc && !req.msvc_external_includes {
            for dir in system_include_dirs.drain(..) {
                if !include_dirs.contains(&dir) {
                    include_dirs.push(dir);
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
        let extra_compile_args: &[String] =
            pkg_flags.map_or(&[], |f| f.extra_compile_args.as_slice());
        let cflags: &[String] = pkg_flags.map_or(&[], |f| f.cflags.as_slice());
        let cxxflags: &[String] = pkg_flags.map_or(&[], |f| f.cxxflags.as_slice());
        let ldflags: &[String] = pkg_flags.map_or(&[], |f| f.ldflags.as_slice());

        let mut objects: Vec<Utf8PathBuf> = Vec::with_capacity(prepared.len());
        for ps in &prepared {
            let depfile = depfile_path(&ps.object);
            // Pick the language-appropriate compiler driver, the
            // language-appropriate standard / profile flags, the
            // matching escape-hatch arg list, and the human-readable
            // tag. Naming the components here is the single point that
            // enforces "C/C++ compile lines never share argv space".
            let dispatch = compile_dispatch(ps.language, req)
                .map_err(|err| err.attach_target_path(tid, req.graph, &ps.abs_source))?;
            // Escape-hatch flags: the language-neutral list first, then
            // the language-specific one — so a per-language override
            // always appears later in argv, where the compiler treats
            // it as the final word.
            let extra_language_compile_args = match ps.language {
                SourceLanguage::C => cflags,
                SourceLanguage::Cxx => cxxflags,
            };
            let mut extra_flags =
                Vec::with_capacity(extra_compile_args.len() + extra_language_compile_args.len());
            extra_flags.extend(extra_compile_args.iter().cloned());
            extra_flags.extend(extra_language_compile_args.iter().cloned());
            // Ninja runs the wrapped command (`ccache cxx ...`) for C++
            // compiles when a compiler-cache wrapper is selected; C
            // compiles stay unwrapped because the public wrapper
            // contract is C++-only today. The wrapper lives on the
            // semantic action and is applied only when lowering the
            // *run* command; `compile_commands.json` below is derived
            // from the unwrapped lowering so clangd / IDE tooling still
            // sees the underlying compiler. Link and archive commands
            // are deliberately never wrapped.
            let compiler_wrapper = match (req.compiler_wrapper, ps.language) {
                (Some(wrapper), SourceLanguage::Cxx) => Some(wrapper.path.clone()),
                _ => None,
            };
            let standard = match ps.language {
                SourceLanguage::C => {
                    LanguageStandard::C(cabin_core::effective_c(&pkg_standards, target).standard)
                }
                SourceLanguage::Cxx => LanguageStandard::Cxx(
                    cabin_core::effective_cxx(&pkg_standards, target).standard,
                ),
            };
            // An MSVC-dialect compile whose standard has no stable
            // `/std:` flag cannot be lowered. Record the violation
            // instead of failing eagerly: the `cabin check` rewrite
            // prunes dependency compiles after planning, and a
            // pruned compile must not gate the command. The CLI
            // surfaces surviving violations through
            // `validate_planned_standards` before anything is
            // lowered or written.
            let msvc_spelling_missing =
                req.dialect == Dialect::Msvc && standard.msvc_spelling().is_none();
            if msvc_spelling_missing {
                msvc_standard_violations.push(MsvcStandardViolation {
                    target: format_target_id(tid, req.graph),
                    language: ps.language.human_label(),
                    standard: standard.as_str(),
                    object: ps.object.clone(),
                });
            }
            let compile = CompileAction {
                standard,
                source: ps.abs_source.clone(),
                object: ps.object.clone(),
                mode: CompileMode::Object,
                implicit_inputs: Vec::new(),
                depfile: Some(depfile),
                compiler: dispatch.driver.to_path_buf(),
                compiler_wrapper,
                arguments: CompileArguments {
                    opt_level: req.profile.opt_level,
                    debug_info: req.profile.debug,
                    define_ndebug: !req.profile.assertions,
                    include_dirs: include_dirs.clone(),
                    system_include_dirs: system_include_dirs.clone(),
                    defines: defines.clone(),
                    extra_flags,
                },
                description: format!("{} {}", dispatch.description_tag, ps.object),
            };
            // `compile_commands.json` records the unwrapped, object-mode
            // argv. Deriving it from the same lowering the Ninja writer
            // uses (minus the wrapper) keeps the two in lockstep. A
            // violating compile has no lowerable argv, so its entry is
            // omitted — the violation above makes that loud, never
            // silent.
            if !msvc_spelling_missing {
                compile_commands.push(CompileCommand {
                    directory: build_dir.to_path_buf(),
                    file: ps.abs_source.clone(),
                    arguments: compile_argv(req.dialect, &compile),
                    output: ps.object.clone(),
                });
            }
            objects.push(ps.object.clone());
            actions.push(BuildAction::Compile(compile));
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
                let lib_path =
                    pkg_build_dir.join(req.dialect.static_library_name(target.name.as_str()));
                actions.push(BuildAction::Archive(ArchiveAction {
                    archiver: req.toolchain.ar.path().to_path_buf(),
                    output: lib_path.clone(),
                    inputs: objects.clone(),
                    description: format!("AR {lib_path}"),
                }));
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
                let exe_path =
                    pkg_build_dir.join(req.dialect.executable_name(target.name.as_str()));
                let lib_paths =
                    collect_link_libs(tid, &resolved_deps, req.graph, &output_for_target);

                let mut inputs: Vec<Utf8PathBuf> = objects.clone();
                inputs.extend(lib_paths.iter().cloned());

                // System libraries required by this executable's
                // dependency closure (e.g. a static library port's
                // `link-libs`). Carried as bare names on the LinkAction
                // so the dialect lowering spells them (`-l<name>` for
                // GNU, `<name>.lib` for MSVC) and places them after the
                // archives for left-to-right resolution. `arguments`
                // stays the package's own raw `ldflags`.
                let link_arguments = ldflags.to_vec();
                let link_libs = collect_link_lib_names(tid, &resolved_deps, req.build_flags);

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
                        req.toolchain
                            .cc
                            .as_ref()
                            .map(cabin_core::ResolvedTool::path)
                            .ok_or_else(|| {
                                BuildError::MissingCCompiler {
                                    target: format_target_id(tid, req.graph),
                                    // Pick a representative source for the
                                    // diagnostic; pure-C link errors
                                    // always have at least one C source on
                                    // this target.
                                    path: prepared
                                        .iter()
                                        .find(|p| p.language == SourceLanguage::C)
                                        .map_or_else(|| exe_path.clone(), |p| p.abs_source.clone()),
                                }
                            })?
                    }
                };
                actions.push(BuildAction::Link(LinkAction {
                    linker: driver_path.to_path_buf(),
                    output: exe_path.clone(),
                    inputs,
                    implicit_inputs: Vec::new(),
                    arguments: link_arguments,
                    link_libs,
                    description: format!("LINK {exe_path}"),
                }));
                output_for_target.insert(tid.clone(), exe_path);
            }
            TargetKind::HeaderOnly => {
                unreachable!("header-only targets are skipped before action generation")
            }
        }
        target_languages.insert(tid.clone(), languages_here);
    }

    let default_outputs: Vec<Utf8PathBuf> = selected
        .iter()
        .filter_map(|tid| output_for_target.get(tid).cloned())
        .collect();

    Ok(BuildGraph {
        actions,
        dialect: req.dialect,
        default_outputs,
        compile_commands,
        msvc_standard_violations,
    })
}

// ---------------------------------------------------------------------------
// internal: target IDs and lookups
// ---------------------------------------------------------------------------

/// Stable identifier for a target within a [`PackageGraph`]: the index of
/// its package in `graph.packages` and its target name.
pub(super) type TargetId = (usize, String);

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

pub(super) fn format_target_id(tid: &TargetId, graph: &PackageGraph) -> String {
    format!("{}:{}", graph.packages[tid.0].package.name.as_str(), tid.1)
}

// ---------------------------------------------------------------------------
// internal: interface-standard compatibility
// ---------------------------------------------------------------------------

/// Pre-build interface-standard compatibility: a consuming target's
/// effective implementation standard must be at least every reachable
/// library-like dependency's interface requirement, per language the
/// consumer actually compiles. The chronological `>=` comparison is a
/// compatibility policy, not a proof of header validity — see
/// `docs/language-standards.md`.
fn enforce_interface_standards(
    tid: &TargetId,
    target: &Target,
    prepared: &[PreparedSource],
    dep_closure: &[TargetId],
    pkg_standards: ResolvedLanguageStandards,
    req: &PlanRequest<'_>,
) -> Result<(), BuildError> {
    let compiles_c = prepared.iter().any(|p| p.language == SourceLanguage::C);
    let compiles_cxx = prepared.iter().any(|p| p.language == SourceLanguage::Cxx);
    for dep_tid in dep_closure {
        let dep_target = lookup_target(dep_tid, req.graph)?;
        if !(dep_target.kind.produces_archive() || dep_target.kind.is_header_only()) {
            continue;
        }
        let dep_pkg = &req.graph.packages[dep_tid.0].package;
        let dep_standards = req
            .language_standards
            .get(&dep_tid.0)
            .copied()
            .unwrap_or_default();
        if compiles_c
            && cabin_core::imposes_requirement(dep_target, &dep_pkg.language, SourceLanguage::C)
        {
            let required = cabin_core::interface_c(&dep_standards, &dep_pkg.language, dep_target);
            let consumer = cabin_core::effective_c(&pkg_standards, target);
            if consumer.standard < required.standard {
                return Err(incompatible_standard_error(
                    tid,
                    dep_tid,
                    req,
                    SourceLanguage::C,
                    consumer.standard.as_str(),
                    required.standard.as_str(),
                    required.source,
                ));
            }
        }
        if compiles_cxx
            && cabin_core::imposes_requirement(dep_target, &dep_pkg.language, SourceLanguage::Cxx)
        {
            let required = cabin_core::interface_cxx(&dep_standards, &dep_pkg.language, dep_target);
            let consumer = cabin_core::effective_cxx(&pkg_standards, target);
            if consumer.standard < required.standard {
                return Err(incompatible_standard_error(
                    tid,
                    dep_tid,
                    req,
                    SourceLanguage::Cxx,
                    consumer.standard.as_str(),
                    required.standard.as_str(),
                    required.source,
                ));
            }
        }
    }
    Ok(())
}

fn incompatible_standard_error(
    consumer: &TargetId,
    dependency: &TargetId,
    req: &PlanRequest<'_>,
    language: SourceLanguage,
    consumer_standard: &'static str,
    required: &'static str,
    source: InterfaceStandardSource,
) -> BuildError {
    let requirement_source = match source {
        InterfaceStandardSource::Target => "its target-level interface standard",
        InterfaceStandardSource::Package => "its package-level interface standard",
        InterfaceStandardSource::CompileStandard => {
            "its effective implementation standard (no interface standard declared)"
        }
    };
    BuildError::IncompatibleLanguageStandard {
        consumer: format_target_id(consumer, req.graph),
        dependency: format_target_id(dependency, req.graph),
        language: language.human_label(),
        consumer_standard,
        required,
        requirement_source,
    }
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
    let candidates: Vec<usize> = if let Some(s) = selected_packages {
        s.to_vec()
    } else {
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
