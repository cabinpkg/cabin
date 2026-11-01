use crate::error::{BuildError, IncompatibleLanguageStandardError, PlannedStandardError};
use crate::graph::{BuildGraph, CompileCommand, InterfaceViolationKind, StandardViolation};
use cabin_core::{AssemblySyntax, classify_assembly};
use cabin_core::{
    InterfaceStandardSource, LanguageStandard, Package, ResolvedCompilerWrapper,
    ResolvedLanguageStandards, ResolvedProfile, ResolvedProfileFlags, ResolvedToolchain,
    SourceLanguage, StandardFlagConflict, Target, TargetKind, classify_source,
    link_driver_language, macro_name,
};
use cabin_driver::{
    ArchiveAction, AssembleAction, BuildAction, CompileAction, CompileArguments, CompileMode,
    Dialect, LinkAction, compile_argv,
};
use cabin_workspace::PackageGraph;
use camino::Utf8PathBuf;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

mod lowering;
mod shadowing;
#[cfg(test)]
mod tests;

use self::lowering::{
    collect_interface, collect_link_libs, collect_link_names, collect_system_link_args,
    compile_dispatch, depfile_path, object_path, promote_dir, resolve_target_dep_edge,
    topo_sort_targets,
};

/// Reference to a manifest target - one of the `[target.<name>]`
/// declarations in a package's `cabin.toml`.  May be qualified
/// `package:target` or unqualified `target`.  Resolution against a
/// [`PackageGraph`] happens in the planner.
///
/// This is the *manifest-target* selector.  It has nothing to do
/// with the platform target a `--target` triple (e.g.
/// `x86_64-unknown-linux-gnu`) names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestTargetSelector {
    pub package: Option<String>,
    pub name: String,
}

impl ManifestTargetSelector {
    /// Parse a `package:target` or bare `target` string.  Unknown formats
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

/// What a system provider supplies for one library target of a
/// package the build takes from the system instead of building it.
/// The graph holds that target as a header-only target with no sources
/// of its own, which the planner never builds; consumers receive this
/// interface wherever they would receive the target's include dirs and
/// `interface-defines`, and every link that reaches it takes its link
/// arguments.
#[derive(Debug, Default)]
pub struct SystemTargetInterface {
    /// Include dirs passed as plain `-I`: compiler default dirs such as
    /// `/usr/include`, which a system search path would move ahead of
    /// the compiler's own headers, breaking `#include_next`.
    pub include_dirs: Vec<Utf8PathBuf>,
    /// Include dirs passed as system search paths (`-isystem` /
    /// `/external:I`), like a registry package's.
    pub system_include_dirs: Vec<Utf8PathBuf>,
    /// Preprocessor definitions for consumers' compiles, with the
    /// conflict check `interface-defines` get.
    pub defines: Vec<String>,
    /// Other arguments every consumer's compiles take, verbatim, ahead
    /// of the consumer's standard, profile, and own flags.
    pub compile_args: Vec<String>,
    /// Arguments every link that reaches the target takes, verbatim and
    /// in the provider's order, after the archives and the linking
    /// package's own `ldflags`.
    pub link_args: Vec<String>,
}

/// Inputs to the build planner.
// Independent inputs that happen to be booleans, not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct PlanRequest<'a> {
    pub graph: &'a PackageGraph,
    /// Resolved C/C++ toolchain.  The planner picks the compile
    /// driver per source language (`toolchain.cc.path()` for `.c`,
    /// `toolchain.cxx.path()` for `.cc` / `.cpp` / `.cxx` /
    /// `.c++` / `.C`) and the link driver per executable (C when its
    /// linked objects came from C sources and none from C++,
    /// otherwise C++).
    /// `toolchain.ar.path()` drives archive commands.
    pub toolchain: &'a ResolvedToolchain,
    /// Per-package resolved build flags.  Missing entries fall
    /// back to an empty [`ResolvedProfileFlags`]; the planner does
    /// not require every package to be present so consumers can
    /// resolve flags lazily for the selected closure only.
    pub build_flags: &'a HashMap<usize, ResolvedProfileFlags>,
    /// Per-package effective language standards.  Missing entries
    /// resolve to no package-level standards, mirroring
    /// `build_flags`; a compile of a language with no effective
    /// standard is then rejected at its compile site.
    pub language_standards: &'a HashMap<usize, ResolvedLanguageStandards>,
    /// Per-package standard-flag conflict candidates, detected by
    /// the CLI on the manifest-derived flags *before* env /
    /// pkg-config augmentation (so `CFLAGS` / `CXXFLAGS` stay
    /// exempt).  The planner records a violation for each planned
    /// compile a candidate's scope covers; candidates whose scope
    /// is never planned (an unbuilt sibling target) gate nothing.
    pub standard_flag_conflicts: &'a HashMap<usize, Vec<StandardFlagConflict>>,
    /// The interfaces of the system-provided targets this plan uses,
    /// keyed by `graph.packages` index and target name.
    pub system_targets: &'a HashMap<(usize, String), SystemTargetInterface>,
    /// Absolute [`crate::layout::platform_dir`] of the target context's
    /// platform, which a build-context plan shares; the profile's
    /// outputs go under it.
    pub build_dir: PathBuf,
    /// The platform the build is for - the build machine, or the
    /// `--target` triple - against which `cfg(...)` dependency edges and
    /// `[target.'cfg(...)'.sources]` select what each target compiles.
    pub platform: cabin_core::TargetPlatform,
    /// The triple every compile, assemble, and link command passes as
    /// `--target=` to a Clang-family driver; `None` keeps the drivers'
    /// default (a native build, or a compiler with a fixed target).
    pub clang_target: Option<String>,
    /// The sysroot every GNU/Clang compile, assemble, and link
    /// command passes; `None` keeps the drivers' default.
    pub sysroot: Option<cabin_core::Sysroot>,
    /// Whether this plan builds the build context: tools for the
    /// build machine, whose units land under
    /// `<profile>/build-machine/packages` instead of
    /// `<profile>/packages` so they never share products with the
    /// target context's units of the same package.
    pub build_context: bool,
    /// Resolved build profile.  Drives compile flags and the per-
    /// profile output directory.
    pub profile: ResolvedProfile,
    /// Specific manifest targets to build, plus their transitive
    /// deps.  `None` selects every default-buildable target of the
    /// selected packages whose required features are enabled.
    pub selected: Option<Vec<ManifestTargetSelector>>,
    /// Indices of `graph.packages` to build: the workspace package
    /// selection, or a build-context plan's build-dependency roots.
    /// `None` means "use the graph's primary set".  When `Some`,
    /// default-target enumeration narrows to the supplied indices
    /// and unqualified selectors in `selected` resolve only among
    /// them; a `package:target` selector resolves against the whole
    /// graph.
    pub selected_packages: Option<&'a [usize]>,
    /// Optional compiler wrapper applied to every C and C++
    /// compile command.  The Ninja `command` field is prefixed with
    /// the wrapper executable; the matching `compile_commands.json`
    /// entry stays *unwrapped* so clangd / IDE tooling
    /// keeps seeing the underlying compiler.  Link and archive
    /// commands are never wrapped.
    pub compiler_wrapper: Option<&'a ResolvedCompilerWrapper>,
    /// Compiler command-line dialect for this build.  Selected from
    /// the detected C++ compiler (MSVC drives the `cl.exe` dialect).
    /// Governs object and archive naming (`.o` vs `.obj`, `lib<x>.a`
    /// vs `<x>.lib`), executable naming together with `platform`
    /// (`<x>.exe` under MSVC or for Windows), and the spelling of
    /// every compile / assemble / archive / link command the
    /// lowering emits.
    pub dialect: Dialect,
    /// Whether `toolchain.ar` splits a response file like a command line
    /// when it runs on Windows
    /// ([`cabin_core::ArchiverIdentity::windows_rsp_quoting`]).
    pub archiver_windows_rsp_quoting: bool,
    /// Whether the MSVC-dialect compilers accept the `/external:I`
    /// block ([`crate::msvc_external_includes_supported`]).  When
    /// `false` on an MSVC build, the planner collapses the system
    /// include bucket into the plain `/I` list instead of emitting a
    /// switch an old `cl` would reject.  Ignored by the GCC/Clang
    /// dialect, where `-isystem` is part of the base command line.
    pub msvc_external_includes: bool,
    /// The languages whose MSVC-dialect compiler is `clang-cl`
    /// ([`crate::clang_cl_languages`]), which needs a `-m<name>` flag
    /// for an intrinsic past the x86 baseline; `cl.exe` exposes every
    /// intrinsic without one.  Their compiles also write a depfile
    /// ([`cabin_driver::CompileAction::clang_depfile`]).  Ignored by the
    /// GCC/Clang dialect, which always takes the flag.
    pub clang_cl: Vec<SourceLanguage>,
    /// Per-package enabled feature names from the cross-package
    /// feature resolver, keyed by `graph.packages` index.  Gates
    /// targets that declare `required-features`: default
    /// enumeration skips a target whose required features are not
    /// all enabled, while naming one explicitly (a selector or a
    /// `deps` entry) is a hard error.  `None` (and missing
    /// entries) mean "no features enabled" for gating purposes.
    pub enabled_features: Option<&'a HashMap<usize, BTreeSet<String>>>,
    /// Whether the post-resolution standard-compatibility check runs
    /// over the resolved target graph ([`crate::standard_compat`]).
    /// The build / check / run / test commands set it; `cabin tidy`
    /// leaves it `false`, and then the graph's
    /// `standard_compat_violations` stay empty.
    pub standard_compat: bool,
}

/// One manifest-declared source resolved to its absolute path and the
/// per-target object path it compiles to.
struct PreparedSource {
    abs_source: Utf8PathBuf,
    object: Utf8PathBuf,
    language: SourceLanguage,
    instruction_sets: Vec<cabin_core::InstructionSet>,
}

struct PreparedAssembly {
    abs_source: Utf8PathBuf,
    object: Utf8PathBuf,
    syntax: AssemblySyntax,
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
/// [`BuildError::UnknownTargetInPackage`],
/// [`BuildError::AmbiguousLibraryTargetInDependency`],
/// [`BuildError::NoLibraryTargetInDependency`],
/// [`BuildError::DevDependencyNotActive`]); required-feature gating
/// errors ([`BuildError::TargetRequiresFeatures`],
/// [`BuildError::TargetDepRequiresFeatures`],
/// [`BuildError::AllDefaultTargetsRequireFeatures`]);
/// [`BuildError::PackageNameCollidesWithScope`] when one planned
/// package's bare name is another planned package's scope;
/// [`BuildError::DependencyCycle`]
/// when the target dependency graph contains a cycle;
/// [`BuildError::StandardHeaderShadowing`] when a registry dependency's
/// include dir holds a file named like a standard header; and per-target
/// source errors ([`BuildError::UnrecognizedSourceExtension`],
/// [`BuildError::InvalidSourcePath`], [`BuildError::EmptyTargetSources`],
/// [`BuildError::MissingLanguageStandard`], [`BuildError::MissingCCompiler`]).
#[tracing::instrument(level = "info", skip_all, fields(actions = tracing::field::Empty))]
pub fn plan(req: &PlanRequest<'_>) -> Result<BuildGraph, BuildError> {
    let selected = select_targets(req)?;
    let resolved = resolve_dependencies(&selected, req)?;
    shadowing::reject_standard_header_shadowing(&resolved.resolved_deps, req.graph)?;
    let graph = emit_actions(&selected, resolved, req)?;
    tracing::Span::current().record("actions", graph.actions.len());
    Ok(graph)
}

/// Phase one: resolve the requested selection (or enumerate the
/// default-buildable set) into concrete target IDs, hard-erroring on
/// missing required features for explicitly named targets.
fn select_targets(req: &PlanRequest<'_>) -> Result<Vec<TargetId>, BuildError> {
    let empty_features = BTreeSet::new();
    let enabled_of = |pkg_idx: usize| -> &BTreeSet<String> {
        req.enabled_features
            .and_then(|m| m.get(&pkg_idx))
            .unwrap_or(&empty_features)
    };

    if let Some(sel) = &req.selected {
        let selected = resolve_selection(sel, req.graph, req.selected_packages)?;
        // Explicitly named targets hard-error when their
        // `required-features` are not enabled - a silent skip here
        // would turn a typo'd feature selection into "built
        // nothing".
        for tid in &selected {
            let target = lookup_target(tid, req.graph)?;
            let missing = target.missing_required_features(enabled_of(tid.0));
            if !missing.is_empty() {
                return Err(BuildError::TargetRequiresFeatures {
                    target: format_target_id(tid, req.graph),
                    package: req.graph.packages[tid.0].package.name.as_str().to_owned(),
                    missing,
                });
            }
        }
        Ok(selected)
    } else {
        let (chosen, gated) =
            default_selection(req.graph, req.selected_packages, req.enabled_features);
        if chosen.is_empty() {
            if gated.is_empty() {
                return Err(BuildError::EmptySelectedPackages);
            }
            return Err(BuildError::AllDefaultTargetsRequireFeatures {
                gated: gated
                    .into_iter()
                    .map(|(tid, missing)| (format_target_id(&tid, req.graph), missing))
                    .collect(),
            });
        }
        Ok(chosen)
    }
}

/// Everything dependency resolution hands the emission phase.
struct ResolvedTargets {
    resolved_deps: HashMap<TargetId, Vec<TargetDepEdge>>,
    reachable: HashSet<TargetId>,
    topo: Vec<TargetId>,
    standard_compat_violations: Vec<crate::standard_compat::StandardCompatViolation>,
}

/// Phase two: walk the target dep graph from the selection, resolving
/// each raw `deps` entry to a concrete [`TargetDepEdge`], then
/// topo-sort the reachable targets and run the post-resolution
/// standard-compatibility check over the resolved edges.
fn resolve_dependencies(
    selected: &[TargetId],
    req: &PlanRequest<'_>,
) -> Result<ResolvedTargets, BuildError> {
    let empty_features = BTreeSet::new();
    let enabled_of = |pkg_idx: usize| -> &BTreeSet<String> {
        req.enabled_features
            .and_then(|m| m.get(&pkg_idx))
            .unwrap_or(&empty_features)
    };
    let mut resolved_deps: HashMap<TargetId, Vec<TargetDepEdge>> = HashMap::new();
    let mut reachable: HashSet<TargetId> = HashSet::new();
    let mut to_visit: Vec<TargetId> = selected.to_vec();

    while let Some(tid) = to_visit.pop() {
        if !reachable.insert(tid.clone()) {
            continue;
        }
        let target = lookup_target(&tid, req.graph)?;
        let mut resolved = Vec::with_capacity(target.deps.len());
        // Dev-only target kinds (`test` / `example`) may reference
        // the owning package's activated `[dev-dependencies]`
        // edges; every other kind resolves through Normal edges
        // only.
        let dev_deps_visible = target.kind.is_dev_only();
        for decl in &target.deps {
            let edge =
                resolve_target_dep_edge(decl, tid.0, dev_deps_visible, req.graph, &req.platform)?;
            let dep = edge.to.clone();
            // A dep edge is an explicit request: a feature-gated
            // dep target whose required features are not enabled
            // on its package is a hard error, not a skip.
            let dep_target = lookup_target(&dep, req.graph)?;
            let missing = dep_target.missing_required_features(enabled_of(dep.0));
            if !missing.is_empty() {
                // CLI feature selection applies to the selected
                // roots only, so the actionable fix depends on how
                // the gated package entered the graph.  For a
                // cross-package reference, mirror
                // `resolve_target_dep`'s edge preference: a Normal
                // edge wins, so the help only points at
                // `[dev-dependencies]` when the dep is reachable
                // through an activated dev edge alone.
                let gated_pkg_is_root = match req.selected_packages {
                    Some(s) => s.contains(&dep.0),
                    None => req.graph.primary_packages.contains(&dep.0),
                };
                let fix = if gated_pkg_is_root {
                    crate::error::FeatureGateFix::RootSelection
                } else if dep.0 == tid.0 {
                    crate::error::FeatureGateFix::UpstreamEdge
                } else if req.graph.packages[tid.0]
                    .deps_of_kind(cabin_core::DependencyKind::Normal, &req.platform)
                    .any(|di| di == dep.0)
                {
                    crate::error::FeatureGateFix::DependencyEdge
                } else {
                    crate::error::FeatureGateFix::DevDependencyEdge
                };
                return Err(BuildError::TargetDepRequiresFeatures {
                    consumer: format_target_id(&tid, req.graph),
                    dep_target: format_target_id(&dep, req.graph),
                    dep_package: req.graph.packages[dep.0].package.name.as_str().to_owned(),
                    missing,
                    fix,
                });
            }
            to_visit.push(dep);
            resolved.push(edge);
        }
        resolved_deps.insert(tid, resolved);
    }

    reject_bare_name_scope_collisions(req.graph, &reachable)?;

    let topo = topo_sort_targets(&reachable, &resolved_deps, req.graph)?;

    // Post-resolution standard-compatibility check (D13 over
    // every resolved edge).
    let standard_compat_violations = if req.standard_compat {
        crate::standard_compat::edge_violations(&topo, &resolved_deps, req)?
    } else {
        Vec::new()
    };

    Ok(ResolvedTargets {
        resolved_deps,
        reachable,
        topo,
        standard_compat_violations,
    })
}

/// Phase three: lower every reachable target in topological order
/// into compile / assemble / archive / link actions and assemble the
/// [`BuildGraph`].
fn emit_actions(
    selected: &[TargetId],
    resolved: ResolvedTargets,
    req: &PlanRequest<'_>,
) -> Result<BuildGraph, BuildError> {
    let ResolvedTargets {
        resolved_deps,
        reachable,
        topo,
        standard_compat_violations,
    } = resolved;

    // Ninja runs every command from the profile directory, which
    // prefixes every object, archive, and executable path in the IR,
    // so a non-UTF-8 build directory is rejected here instead of
    // lossily converted.
    let profile_dir = crate::layout::profile_dir(&req.build_dir, &req.profile.name);
    let profile_dir = promote_dir(&profile_dir)?;

    let mut actions: Vec<BuildAction> = Vec::new();
    let mut compile_commands: Vec<CompileCommand> = Vec::new();
    let mut standard_violations: Vec<StandardViolation> = Vec::new();
    let mut output_for_target: HashMap<TargetId, Utf8PathBuf> = HashMap::new();
    // Per-target source-language manifest, including transitive
    // contributions through `target.deps`; picks each executable's
    // link driver.
    let mut target_languages: HashMap<TargetId, BTreeSet<SourceLanguage>> = HashMap::new();

    for tid in &topo {
        let target = lookup_target(tid, req.graph)?;

        let pkg = &req.graph.packages[tid.0];
        let pkg_build_dir = promote_dir(&crate::layout::package_dir(
            profile_dir.as_std_path(),
            req.build_context,
            &pkg.package.name,
        ))?
        .to_owned();
        // The manifest directory is an OS-canonicalized path; promote
        // it to UTF-8 (rejecting non-UTF-8) so the source and include
        // paths it anchors enter the IR as `Utf8PathBuf`.
        let manifest_dir = promote_dir(&pkg.manifest_dir)?;

        // The languages of the objects a link reaching this target
        // through its dependencies takes in: every direct target dep's
        // entry in `target_languages`.  The topo iteration guarantees
        // dependencies are populated before we visit the dependent.
        let linked_through: BTreeSet<SourceLanguage> = resolved_deps
            .get(tid)
            .into_iter()
            .flatten()
            .filter_map(|edge| target_languages.get(&edge.to))
            .flatten()
            .copied()
            .collect();

        // Header-only libraries declare include dirs but no
        // translation units.  Skip every action - `collect_link_libs`
        // and `collect_interface` already walk dep targets by their
        // declared interfaces, so consumers still pick up the
        // headers and link the archives behind them, whose languages
        // the manifest passes on.
        if target.kind.is_header_only() {
            target_languages.insert(tid.clone(), linked_through);
            continue;
        }

        let mut prepared: Vec<PreparedSource> = Vec::new();
        let mut assembly: Vec<PreparedAssembly> = Vec::new();
        // Instruction sets mean nothing off x86, like a non-matching
        // `cfg`, so one source can serve every platform.
        let x86 = matches!(req.platform.arch.as_str(), "x86" | "x86_64");
        for source in target.sources_for(&req.platform) {
            let language = classify_source(source);
            let syntax = classify_assembly(source);
            if language.is_none() && syntax.is_none() {
                return Err(BuildError::UnrecognizedSourceExtension {
                    target: format_target_id(tid, req.graph),
                    path: source.clone(),
                });
            }
            let object = object_path(&pkg_build_dir, target.name.as_str(), source, req.dialect)
                .map_err(|reason| BuildError::InvalidSourcePath {
                    target: format_target_id(tid, req.graph),
                    path: source.clone(),
                    reason,
                })?;
            let abs_source = manifest_dir.join(source);
            if let Some(language) = language {
                let flags = req.dialect == Dialect::GnuLike || req.clang_cl.contains(&language);
                let instruction_sets = match target.instruction_sets.get(source) {
                    Some(sets) if x86 && flags => sets.clone(),
                    _ => Vec::new(),
                };
                prepared.push(PreparedSource {
                    abs_source,
                    object,
                    language,
                    instruction_sets,
                });
            } else if let Some(syntax) = syntax {
                assembly.push(PreparedAssembly {
                    abs_source,
                    object,
                    syntax,
                });
            }
        }
        if prepared.is_empty() && assembly.is_empty() {
            return Err(BuildError::EmptyTargetSources(format_target_id(
                tid, req.graph,
            )));
        }

        // A C/C++ compile when the target has one: the `cabin check`
        // rewrite keeps a definition conflict only with that compile.
        let object = prepared
            .first()
            .map_or_else(|| &assembly[0].object, |source| &source.object);
        let collected = collect_interface(
            tid,
            target,
            &resolved_deps,
            req.graph,
            req.system_targets,
            object,
            &mut standard_violations,
        )?;
        let pkg_standards = req
            .language_standards
            .get(&tid.0)
            .copied()
            .unwrap_or_default();
        enforce_interface_standards(
            tid,
            target,
            &prepared,
            &collected.visible,
            pkg_standards,
            req,
            &mut standard_violations,
        )?;

        // Per-package resolved build flags from the manifest's
        // `[profile]`, `[target.'cfg(...)'.profile]`, and the active
        // `[profile.<name>]` table, plus any `pkg-config` and
        // `CPPFLAGS` / `CFLAGS` / `CXXFLAGS` / `LDFLAGS` layers the
        // CLI applies.
        let pkg_flags = req.build_flags.get(&tid.0);

        // Compose include_dirs and defines: existing target +
        // per-package build flags, partitioned into the user (`-I`)
        // and system (`-isystem` / `/external:I`) buckets.
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
        // the system bucket; fall back to plain `/I` for those dirs.
        if req.dialect == Dialect::Msvc && !req.msvc_external_includes {
            for dir in system_include_dirs.drain(..) {
                if !include_dirs.contains(&dir) {
                    include_dirs.push(dir);
                }
            }
        }
        // A target's own definition overrides the package's definition
        // of the same macro, as a target's setting overrides its package's.
        let mut defines: Vec<String> = target.defines.clone();
        if let Some(flags) = pkg_flags {
            let target_own: HashSet<&str> = target.defines.iter().map(|d| macro_name(d)).collect();
            defines.extend(
                flags
                    .defines
                    .iter()
                    .filter(|d| !target_own.contains(macro_name(d)))
                    .cloned(),
            );
        }
        // The target's own definitions override an incoming interface
        // definition of the same macro.
        let own: HashSet<&str> = defines.iter().map(|d| macro_name(d)).collect();
        let incoming: Vec<String> = collected
            .defines
            .into_iter()
            .filter(|d| !own.contains(macro_name(d)))
            .collect();
        defines.extend(incoming);
        let extra_compile_args: &[String] =
            pkg_flags.map_or(&[], |f| f.extra_compile_args.as_slice());
        let cflags: &[String] = pkg_flags.map_or(&[], |f| f.cflags.as_slice());
        let cxxflags: &[String] = pkg_flags.map_or(&[], |f| f.cxxflags.as_slice());
        let ldflags: &[String] = pkg_flags.map_or(&[], |f| f.ldflags.as_slice());

        // Strictly per-target: two targets in one build may differ in
        // both level and `gnu-extensions`.
        let gnu_extensions = cabin_core::effective_gnu_extensions(
            &req.graph.packages[tid.0].package.language,
            target,
        );
        let mut objects: Vec<Utf8PathBuf> = Vec::with_capacity(prepared.len());
        for ps in &prepared {
            let depfile = depfile_path(&ps.object);
            let dispatch = compile_dispatch(ps.language, req)
                .map_err(|err| err.attach_target_path(tid, req.graph, &ps.abs_source))?;
            // Escape-hatch flags: the language-neutral list first, then
            // the language-specific one - so a per-language override
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
            let compiler_wrapper = req.compiler_wrapper.map(|wrapper| wrapper.path.clone());
            // Manifest loading rejects targets that compile a
            // language without an effective standard, so a `None`
            // here is a package that bypassed the parser - reject it
            // with the same actionable message.
            let missing_standard = |field: &'static str| BuildError::MissingLanguageStandard {
                target: format_target_id(tid, req.graph),
                language: ps.language.human_label(),
                field,
            };
            let standard = match ps.language {
                SourceLanguage::C => LanguageStandard::C(
                    cabin_core::effective_c(&pkg_standards, target)
                        .ok_or_else(|| missing_standard("c-standard"))?
                        .standard,
                ),
                SourceLanguage::Cxx => LanguageStandard::Cxx(
                    cabin_core::effective_cxx(&pkg_standards, target)
                        .ok_or_else(|| missing_standard("cxx-standard"))?
                        .standard,
                ),
            };
            // An MSVC-dialect compile whose standard has no stable
            // `/std:` flag cannot be lowered.  Record the violation
            // instead of failing eagerly: the `cabin check` rewrite
            // prunes dependency compiles after planning, and a
            // pruned compile must not gate the command.  The CLI
            // surfaces surviving violations through
            // `validate_planned_standards` before anything is
            // lowered or written.
            let msvc_spelling_missing =
                req.dialect == Dialect::Msvc && standard.msvc_spelling().is_none();
            if msvc_spelling_missing {
                standard_violations.push(StandardViolation {
                    object: ps.object.clone(),
                    error: PlannedStandardError::StandardUnsupportedOnMsvcDialect {
                        target: format_target_id(tid, req.graph),
                        language: ps.language.human_label(),
                        standard: standard.as_str(),
                    },
                });
            }
            // `cl.exe` has no GNU dialect mode, so `gnu-extensions`
            // cannot be honored - and must never be silently
            // ignored.  Recorded (not failed eagerly) for the same
            // check-rewrite reason as the missing-spelling case.
            let msvc_gnu_extensions = req.dialect == Dialect::Msvc && gnu_extensions;
            if msvc_gnu_extensions {
                standard_violations.push(StandardViolation {
                    object: ps.object.clone(),
                    error: PlannedStandardError::GnuExtensionsUnsupportedOnMsvcDialect {
                        target: format_target_id(tid, req.graph),
                    },
                });
            }
            // Escape-hatch conflicts: a candidate covers this
            // compile when its language matches and its scope is the
            // whole package or this specific target.
            if let Some(candidates) = req.standard_flag_conflicts.get(&tid.0) {
                for candidate in candidates {
                    let covers = candidate.language == ps.language
                        && candidate
                            .target
                            .as_deref()
                            .is_none_or(|t| t == tid.1.as_str());
                    if covers {
                        standard_violations.push(StandardViolation {
                            object: ps.object.clone(),
                            error: PlannedStandardError::StandardFlagConflict(Box::new(
                                candidate.clone(),
                            )),
                        });
                    }
                }
            }
            let mut compile = CompileAction {
                standard,
                gnu_extensions,
                source: ps.abs_source.clone(),
                object: ps.object.clone(),
                mode: CompileMode::Object,
                depfile: Some(depfile),
                clang_depfile: false,
                compiler: dispatch.driver.to_path_buf(),
                target: req.clang_target.clone(),
                sysroot: req.sysroot.clone(),
                compiler_wrapper,
                arguments: CompileArguments {
                    opt_level: req.profile.opt_level,
                    debug_info: req.profile.debug,
                    define_ndebug: !req.profile.assertions,
                    include_dirs: include_dirs.clone(),
                    system_include_dirs: system_include_dirs.clone(),
                    defines: defines.clone(),
                    instruction_sets: ps.instruction_sets.clone(),
                    provider_flags: collected.args.clone(),
                    extra_flags,
                },
                description: format!("{} {}", dispatch.description_tag, ps.object),
            };
            // `compile_commands.json` records the unwrapped, object-mode
            // argv.  Deriving it from the same lowering the Ninja writer
            // uses (minus the wrapper) keeps the two in lockstep.  A
            // violating compile has no lowerable argv, so its entry is
            // omitted - the violation above makes that loud, never
            // silent.
            if !msvc_spelling_missing && !msvc_gnu_extensions {
                compile_commands.push(CompileCommand {
                    directory: profile_dir.to_path_buf(),
                    file: ps.abs_source.clone(),
                    arguments: compile_argv(req.dialect, &compile),
                    output: ps.object.clone(),
                });
            }
            // Only after the database entry, which keeps `/showIncludes`:
            // clang-tidy drops that from a database command but not
            // `/clang:-MD`, whose depfile it would write.  A compiler
            // wrapper keeps `/showIncludes` too, which its cache hit
            // replays: a hit need not write the depfile, and Ninja reads
            // a missing depfile as no dependencies.
            compile.clang_depfile =
                req.clang_cl.contains(&ps.language) && req.compiler_wrapper.is_none();
            objects.push(ps.object.clone());
            actions.push(BuildAction::Compile(compile));
        }
        for asm in &assembly {
            // The C++ compiler is the one the dialect was detected
            // from, so it is the driver that understands its syntax.
            let Some(assembler) = req.dialect.assembler(asm.syntax, req.toolchain.cxx.path())
            else {
                let (syntax, needs) = match asm.syntax {
                    AssemblySyntax::Gnu => ("GNU", "a GCC or Clang toolchain"),
                    AssemblySyntax::Masm => (
                        "MASM",
                        "MSVC's `cl.exe`, beside which `ml64.exe` is installed",
                    ),
                };
                standard_violations.push(StandardViolation {
                    object: asm.object.clone(),
                    error: PlannedStandardError::AssemblyNeedsToolchain {
                        target: format_target_id(tid, req.graph),
                        path: asm.abs_source.clone(),
                        syntax,
                        needs,
                    },
                });
                continue;
            };
            actions.push(BuildAction::Assemble(AssembleAction {
                assembler,
                target: req.clang_target.clone(),
                sysroot: req.sysroot.clone(),
                source: asm.abs_source.clone(),
                object: asm.object.clone(),
                depfile: Some(depfile_path(&asm.object)),
                include_dirs: include_dirs
                    .iter()
                    .chain(&system_include_dirs)
                    .cloned()
                    .collect(),
                defines: defines.clone(),
                description: format!("AS {}", asm.object),
            }));
            objects.push(asm.object.clone());
        }

        let mut languages_here = linked_through.clone();
        languages_here.extend(prepared.iter().map(|p| p.language));

        match target.kind {
            TargetKind::Library => {
                let lib_path =
                    pkg_build_dir.join(req.dialect.static_library_name(target.name.as_str()));
                actions.push(BuildAction::Archive(ArchiveAction {
                    archiver: req.toolchain.ar.path().to_path_buf(),
                    windows_rsp_quoting: req.archiver_windows_rsp_quoting,
                    output: lib_path.clone(),
                    inputs: objects,
                    description: format!("AR {lib_path}"),
                }));
                output_for_target.insert(tid.clone(), lib_path);
            }
            TargetKind::Executable | TargetKind::Test | TargetKind::Example => {
                let exe_path = pkg_build_dir.join(
                    req.dialect
                        .executable_name(target.name.as_str(), &req.platform),
                );
                let lib_paths =
                    collect_link_libs(tid, &resolved_deps, req.graph, &output_for_target);

                let mut inputs = objects;
                inputs.extend(lib_paths);

                // System libraries required by this executable's
                // dependency closure (e.g. a static library
                // dependency's `link-libs`).  Carried as bare names on the LinkAction
                // so the dialect lowering spells them (`-l<name>` for
                // GNU, `<name>.lib` for MSVC) and places them after the
                // archives for left-to-right resolution. `arguments`
                // is the package's own raw `ldflags`, then the link
                // arguments of the system-provided targets it reaches.
                let mut link_arguments = ldflags.to_vec();
                link_arguments.extend(collect_system_link_args(
                    tid,
                    &resolved_deps,
                    req.system_targets,
                ));
                // Apple's linker resolves archives in any order and
                // warns on a repeated `-l`, so its links name each
                // library once; other linkers search left to right.
                let apple = matches!(req.platform.os.as_str(), "macos" | "ios");
                let link_libs = collect_link_names(
                    tid,
                    &resolved_deps,
                    req.build_flags,
                    |flags| &flags.link_libs,
                    apple,
                );
                // Frameworks exist only on Apple platforms; elsewhere a
                // package's list means nothing, like a non-matching `cfg`.
                let frameworks = if apple {
                    collect_link_names(
                        tid,
                        &resolved_deps,
                        req.build_flags,
                        |flags| &flags.frameworks,
                        true,
                    )
                } else {
                    Vec::new()
                };

                // Link-driver pick: C++ if any of this target's
                // own objects came from a C++ source, or if any
                // transitively reachable object did, or if none came
                // from C either (assembly only).  Otherwise the C
                // compiler drives the link, which keeps pure-C
                // executables off the C++ runtime.
                let languages_slice: Vec<SourceLanguage> = languages_here.iter().copied().collect();
                let driver_language = link_driver_language(&languages_slice);
                let driver_path = match driver_language {
                    SourceLanguage::Cxx => req.toolchain.cxx.path(),
                    // Only a lowered C source puts C in the manifest, and
                    // `compile_dispatch` fails its compile without `cc`.
                    SourceLanguage::C => req
                        .toolchain
                        .cc
                        .as_ref()
                        .expect("a C object in the link closure was compiled with `cc`")
                        .path(),
                };
                actions.push(BuildAction::Link(LinkAction {
                    linker: driver_path.to_path_buf(),
                    target: req.clang_target.clone(),
                    sysroot: req.sysroot.clone(),
                    output: exe_path.clone(),
                    inputs,
                    arguments: link_arguments,
                    link_libs,
                    frameworks,
                    description: format!("LINK {exe_path}"),
                }));
                output_for_target.insert(tid.clone(), exe_path);
            }
            TargetKind::HeaderOnly => {
                unreachable!("header-only targets are skipped before action generation")
            }
        }
        // A dependent links this target's own objects only when they
        // are archived (`collect_link_libs` keeps libraries).
        let linked = if matches!(target.kind, TargetKind::Library) {
            languages_here
        } else {
            linked_through
        };
        target_languages.insert(tid.clone(), linked);
    }

    let default_outputs: Vec<Utf8PathBuf> = selected
        .iter()
        .filter_map(|tid| output_for_target.get(tid).cloned())
        .collect();

    let planned_packages: BTreeSet<String> = reachable
        .iter()
        .map(|tid| req.graph.packages[tid.0].package.name.as_str().to_owned())
        .collect();

    Ok(BuildGraph {
        actions,
        dialect: req.dialect,
        default_outputs,
        planned_packages,
        build_context_packages: BTreeSet::new(),
        compile_commands,
        standard_violations,
        standard_compat_violations,
    })
}

// ---------------------------------------------------------------------------
// internal: target IDs and lookups
// ---------------------------------------------------------------------------

/// Stable identifier for a target within a [`PackageGraph`]: the index of
/// its package in `graph.packages` and its target name.
pub(crate) type TargetId = (usize, String);

/// One resolved edge of the target dependency graph.  Alias
/// resolution has already happened - `to` is a concrete
/// (package, target), never a pre-alias bare name - and the edge
/// carries the visibility declared on the manifest entry.  The
/// `public` classification feeds the standard-compatibility
/// pass, which propagates interface requirements along public edges
/// (D5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetDepEdge {
    pub(crate) to: TargetId,
    pub(crate) public: bool,
}

/// Find a target by name within a package's manifest.  Target
/// names are unique within a package, so this returns the sole
/// match (or `None`).
pub(crate) fn find_target<'a>(pkg: &'a Package, name: &str) -> Option<&'a Target> {
    pkg.targets.iter().find(|t| t.name.as_str() == name)
}

pub(crate) fn lookup_target<'a>(
    tid: &TargetId,
    graph: &'a PackageGraph,
) -> Result<&'a Target, BuildError> {
    let pkg = &graph.packages[tid.0];
    find_target(&pkg.package, &tid.1).ok_or_else(|| BuildError::UnknownTargetInPackage {
        package: pkg.package.name.as_str().to_owned(),
        target: tid.1.clone(),
    })
}

pub(crate) fn format_target_id(tid: &TargetId, graph: &PackageGraph) -> String {
    format!("{}:{}", graph.packages[tid.0].package.name.as_str(), tid.1)
}

// ---------------------------------------------------------------------------
// internal: interface-standard compatibility
// ---------------------------------------------------------------------------

/// Reject a plan where one planned package's bare name equals
/// another planned package's scope.  Both would claim
/// `packages/<name>` in this build tree: the bare package writes its
/// target files there while the scoped one nests its package
/// directory inside, so the build could fail on a file/directory
/// clash and `cabin clean -p <name>` would remove the scoped
/// package's output too.  Rejecting the combination up front keeps
/// the on-disk layout unambiguous for every consumer that parses it
/// back (progress banners, link diagnostics, clean).  Only packages
/// whose targets are reachable from this plan's selection
/// participate: an unrelated bare workspace member named like some
/// dependency's scope never blocks a selection that does not build
/// it.
fn reject_bare_name_scope_collisions(
    graph: &PackageGraph,
    reachable: &HashSet<TargetId>,
) -> Result<(), BuildError> {
    // Sorted so a multi-collision graph reports the same pair every
    // run.
    let planned: BTreeSet<usize> = reachable.iter().map(|tid| tid.0).collect();
    let bare: HashSet<&str> = planned
        .iter()
        .map(|&idx| &graph.packages[idx].package.name)
        .filter(|name| !name.is_scoped())
        .map(cabin_core::PackageName::as_str)
        .collect();
    if bare.is_empty() {
        return Ok(());
    }
    for &idx in &planned {
        let name = &graph.packages[idx].package.name;
        if let Some(scope) = name.scope()
            && bare.contains(scope)
        {
            return Err(BuildError::PackageNameCollidesWithScope {
                package: scope.to_owned(),
                scoped: name.as_str().to_owned(),
            });
        }
    }
    Ok(())
}

/// Pre-build interface-standard compatibility: a consuming target's
/// effective implementation standard must fall inside the declared
/// interface range of every library-like dependency whose interface it
/// sees (a direct dependency, or one re-exported through public edges;
/// see `collect_interface`), per language the consumer compiles.
/// Range membership on the chronological chain is a compatibility
/// policy, not a proof of header validity - see
/// `docs/language-standards.md`.
///
/// Incompatibilities are *recorded* on the consumer's first compile
/// of the offending language rather than failing the plan: the
/// `cabin check` rewrite prunes dependency compiles after planning,
/// and an incompatibility between two dependencies whose compiles
/// are all pruned must not gate the command.
/// `validate_planned_standards` surfaces the survivors.
fn enforce_interface_standards(
    tid: &TargetId,
    target: &Target,
    prepared: &[PreparedSource],
    visible: &[TargetId],
    pkg_standards: ResolvedLanguageStandards,
    req: &PlanRequest<'_>,
    violations: &mut Vec<StandardViolation>,
) -> Result<(), BuildError> {
    let object_of = |language: SourceLanguage| {
        prepared
            .iter()
            .find(|p| p.language == language)
            .map(|p| p.object.clone())
    };
    for dep_tid in visible {
        let dep_target = lookup_target(dep_tid, req.graph)?;
        if !dep_target.kind.is_library_like() {
            continue;
        }
        let dep_pkg = &req.graph.packages[dep_tid.0].package;
        let dep_standards = req
            .language_standards
            .get(&dep_tid.0)
            .copied()
            .unwrap_or_default();
        // A `None` requirement or consumer standard cannot happen
        // for manifest-derived packages: `imposes_requirement` is
        // true only when some declaration (or a source file, whose
        // standard manifest loading guarantees) backs the language,
        // and the consumer compiles the language, so manifest
        // loading demanded its own standard too.
        if let Some(object) = object_of(SourceLanguage::C)
            && cabin_core::imposes_requirement(dep_target, &dep_pkg.language, SourceLanguage::C)
            && let Some(required) =
                cabin_core::interface_c(&dep_standards, &dep_pkg.language, dep_target)
            && let Some(consumer) = cabin_core::effective_c(&pkg_standards, target)
            && let Some(violation) = interface_violation(
                format_target_id(tid, req.graph),
                format_target_id(dep_tid, req.graph),
                SourceLanguage::C,
                consumer.standard,
                required.requirement,
                required.source,
                object,
            )
        {
            violations.push(violation);
        }
        if let Some(object) = object_of(SourceLanguage::Cxx)
            && cabin_core::imposes_requirement(dep_target, &dep_pkg.language, SourceLanguage::Cxx)
            && let Some(required) =
                cabin_core::interface_cxx(&dep_standards, &dep_pkg.language, dep_target)
            && let Some(consumer) = cabin_core::effective_cxx(&pkg_standards, target)
            && let Some(violation) = interface_violation(
                format_target_id(tid, req.graph),
                format_target_id(dep_tid, req.graph),
                SourceLanguage::Cxx,
                consumer.standard,
                required.requirement,
                required.source,
                object,
            )
        {
            violations.push(violation);
        }
    }
    Ok(())
}

/// The consumer's violation of one direct interface requirement,
/// or `None` when the consumer's level satisfies it.
///
/// A declared `"none"` is deliberately not enforced here: the
/// post-resolution compatibility pass owns it (D9 row 1)
/// together with the per-edge `ignore-interface-standard`
/// override, which must be able to unblock exactly that class -
/// this always-on layer enforces the range bounds, which the
/// override never silences.
fn interface_violation<S: cabin_core::StandardLevel>(
    consumer: String,
    dependency: String,
    language: SourceLanguage,
    consumer_standard: S,
    required: cabin_core::InterfaceRequirement<S>,
    source: InterfaceStandardSource,
    object: Utf8PathBuf,
) -> Option<StandardViolation> {
    let cabin_core::InterfaceRequirement::Requirement(declared) = required else {
        return None;
    };
    let requirement = cabin_core::standard_compatibility::Requirement::from_declared(declared);
    if requirement.satisfied_by(consumer_standard) {
        return None;
    }
    let kind = if consumer_standard < declared.min() {
        InterfaceViolationKind::BelowMinimum {
            min: declared.min().level_str(),
        }
    } else {
        InterfaceViolationKind::AboveMaximum {
            max: declared
                .max()
                .expect("a violated non-forbidden requirement fails one of its bounds")
                .level_str(),
        }
    };
    let requirement_source = match source {
        InterfaceStandardSource::Target => "its target-level interface standard",
        InterfaceStandardSource::Package => "its package-level interface standard",
        InterfaceStandardSource::Workspace => "its workspace-inherited interface standard",
        InterfaceStandardSource::CompileStandard => {
            "its effective implementation standard (no interface standard declared)"
        }
    };
    Some(StandardViolation {
        object,
        error: PlannedStandardError::IncompatibleLanguageStandard(Box::new(
            IncompatibleLanguageStandardError {
                consumer,
                dependency,
                language: language.human_label(),
                consumer_standard: consumer_standard.level_str(),
                required: requirement.to_string(),
                kind,
                requirement_source,
            },
        )),
    })
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
        if find_target(&pkg.package, &sel.name).is_none() {
            return Err(BuildError::UnknownTargetInPackage {
                package: pkg_name.clone(),
                target: sel.name.clone(),
            });
        }
        return Ok((pkg_idx, sel.name.clone()));
    }

    // Under an active package selection, an unqualified selector
    // searches only the selected packages, never a root package
    // outside them that the user did not select.
    let candidates: Vec<usize> = if let Some(s) = selected_packages {
        s.to_vec()
    } else {
        // Unqualified selector with no workspace selection
        // active: walk the root first, then every primary.
        let mut root_match: Option<TargetId> = None;
        if let Some(root_idx) = graph.root_package {
            let root = &graph.packages[root_idx];
            if find_target(&root.package, &sel.name).is_some() {
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
        if find_target(&pkg.package, &sel.name).is_some() {
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

/// Default-buildable targets of the selected packages, split into
/// the buildable set and the targets skipped because their
/// `required-features` are not enabled (with the missing names, for
/// the all-gated diagnostic).
fn default_selection(
    graph: &PackageGraph,
    selected_packages: Option<&[usize]>,
    enabled_features: Option<&HashMap<usize, BTreeSet<String>>>,
) -> (Vec<TargetId>, Vec<(TargetId, Vec<String>)>) {
    let empty = BTreeSet::new();
    let mut out = Vec::new();
    let mut gated = Vec::new();
    let pkg_indices: &[usize] = match selected_packages {
        Some(s) => s,
        None => graph.primary_packages.as_slice(),
    };
    for &pkg_idx in pkg_indices {
        let pkg = &graph.packages[pkg_idx];
        let enabled = enabled_features
            .and_then(|m| m.get(&pkg_idx))
            .unwrap_or(&empty);
        for target in &pkg.package.targets {
            if !target.kind.is_default_buildable() {
                continue;
            }
            let tid = (pkg_idx, target.name.as_str().to_owned());
            let missing = target.missing_required_features(enabled);
            if missing.is_empty() {
                out.push(tid);
            } else {
                gated.push((tid, missing));
            }
        }
    }
    (out, gated)
}

/// Whether the selector's target has all of its
/// `required-features` enabled under `enabled_features`.  Used by
/// enumeration-style callers (`cabin test` without `--test`,
/// `cabin tidy`) to *skip* feature-gated targets; explicitly named
/// targets go through [`plan`], which hard-errors instead.
/// Unqualified selectors, and selectors that do not resolve to a
/// target, return `true` so resolution diagnostics stay owned by
/// [`plan`].
// The map is always the std-hasher one the CLI builds from the
// feature resolver; a `BuildHasher` parameter would be dead
// flexibility.
#[allow(clippy::implicit_hasher)]
pub fn selector_required_features_met(
    sel: &ManifestTargetSelector,
    graph: &PackageGraph,
    enabled_features: &HashMap<usize, BTreeSet<String>>,
) -> bool {
    let empty = BTreeSet::new();
    let Some(pkg_name) = &sel.package else {
        return true;
    };
    let Some(pkg_idx) = graph.index_of(pkg_name) else {
        return true;
    };
    let Some(target) = graph.packages[pkg_idx]
        .package
        .targets
        .iter()
        .find(|t| t.name.as_str() == sel.name)
    else {
        return true;
    };
    let enabled = enabled_features.get(&pkg_idx).unwrap_or(&empty);
    target.missing_required_features(enabled).is_empty()
}

/// Package-qualified selectors for every target of `kind` in the
/// selected packages (the primary set when `None`), in package
/// order, then each package's target order.  `cabin test`
/// enumerates its test targets with it, `cabin tidy` every kind.
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
