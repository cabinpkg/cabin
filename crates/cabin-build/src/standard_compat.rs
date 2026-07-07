//! Post-resolution language-standard compatibility checking.
//!
//! Evaluates the edge-compatibility model of
//! `docs/design/standard-compatibility/spec.md` over the planner's
//! resolved target graph: per-target `ReqOf` attributes are mapped
//! per the spec's D6 population contract, `cabin_workspace::standards`
//! composes the effective requirement `R_L` along public edges (D10),
//! and every resolved dependency edge is checked per D13 for every
//! language the consumer compiles.  Violated edges become
//! [`StandardCompatViolation`] records on the [`crate::BuildGraph`];
//! the CLI renders them as errors that fail the command.  An edge
//! whose consuming package declares `ignore-interface-standard =
//! true` on the matching `[dependencies]` entry is still evaluated,
//! but its violations are marked [`StandardCompatViolation::ignored`]
//! so the CLI downgrades them to unchecked-edge notes.
//!
//! The pass runs strictly after resolution (fresh or
//! lockfile-seeded - both produce the same loaded graph) and never
//! feeds back into version selection.  Its defaults deliberately
//! differ from the build-time interface enforcement in
//! `planner::enforce_interface_standards`: a compiled dependency
//! with no interface declaration imposes nothing here (spec D9 row
//! 4 - no implementation-standard fallback), while an explicit
//! `"none"` is unsatisfiable (row 1).

use std::collections::HashMap;
use std::path::PathBuf;

use cabin_core::standard_compatibility::{DependencyAttributes, ReqOfSource, Requirement};
use cabin_core::{
    LanguageStandardSettings, LanguageStandardSource, ResolvedLanguageStandards, SourceLanguage,
    StandardDeclaration, Target, classify_source, effective_c, effective_cxx,
};
use cabin_workspace::PackageKind;
use cabin_workspace::standards::{
    DeclarationSite, DeclarationSites, Provenance, TargetEdge, TargetNode, effective_requirements,
    provenance_c, provenance_cxx,
};

use crate::error::BuildError;
use crate::planner::{PlanRequest, TargetDepEdge, TargetId, format_target_id, lookup_target};

/// Which manifest table a cited declaration lives in.  Together
/// with [`DeclSite::field`] this is enough for a renderer to
/// re-locate the declaration's span in the manifest text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclScope {
    /// The `[package]` table.
    Package,
    /// The `[target.<name>]` table of the carried target.
    Target(String),
    /// The `[workspace]` table of the workspace root manifest
    /// (the value was inherited via `{ workspace = true }`).
    Workspace,
}

/// A manifest declaration a violation cites: file, table, and field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclSite {
    /// Manifest that carries the declaration.  The workspace root
    /// manifest for [`DeclScope::Workspace`], the declaring
    /// package's manifest otherwise.
    pub manifest_path: PathBuf,
    pub scope: DeclScope,
    /// Manifest field name (`c-standard`, `interface-cxx-standard`, ...).
    pub field: &'static str,
}

/// Why a dependency's effective requirement is what it is - the D9
/// row that originates the join, with the declaration to cite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequirementOrigin {
    /// Row 2: an explicit `interface-*-standard` minimum.
    Declared { site: DeclSite },
    /// Row 1: `interface-*-standard = "none"` - consumption from
    /// this language was disabled by the origin target's author.
    DeclaredNone { site: DeclSite },
    /// Row 3: a header-only target's minimum inferred from its
    /// implementation standard.
    HeaderOnlyInference { site: DeclSite },
    /// Row 6: the strict C++-to-C default - the origin target
    /// implements no C and declares no C interface, so there is no
    /// declaration to cite.
    CrossLanguageDefault,
}

/// The violated effective requirement `R_L(d)` of spec D13.
/// `Unconstrained` cannot be violated, so it has no variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeRequirement {
    /// A minimum consumer level (`[m]` of spec D3).
    Min(&'static str),
    /// Unsatisfiable at every consumer level.
    Forbidden,
}

/// One resolved dependency edge that violates spec D13 for one
/// consumer language.  A mixed-language consumer failing both
/// languages on the same edge yields two records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardCompatViolation {
    /// `package:target` of the consuming side of the edge.
    pub consumer: String,
    /// The consumer language whose conjunct of D13 failed.
    pub language: SourceLanguage,
    /// The consumer's effective compile level for `language`.
    pub consumer_standard: &'static str,
    /// Where the consumer's standard is declared.
    pub consumer_site: DeclSite,
    /// `package:target` of the direct dependency the edge points at.
    pub dependency: String,
    /// The dependency's package name, for the pin remedy.
    pub dependency_package: String,
    /// The dependency's package version, for the pin remedy.
    pub dependency_version: String,
    /// Whether the dependency came from a registry - the pin
    /// remedy only applies to versioned dependencies.
    pub dependency_is_registry: bool,
    /// The violated effective requirement `R_L(dependency)`.
    pub requirement: EdgeRequirement,
    /// `package:target` whose own `ReqOf` originates the
    /// requirement (spec T1's closed form); equals `dependency`
    /// when the dependency itself declares it.
    pub origin_target: String,
    /// The D9 row and declaration behind `origin_target`'s `ReqOf`.
    pub origin: RequirementOrigin,
    /// Public-edge provenance chain from `dependency` down to
    /// `origin_target`, `package:target` names inclusive on both
    /// ends; a single entry when the dependency's own declaration
    /// attains the join.
    pub chain: Vec<String>,
    /// Manifest of the consuming target's own package - where a
    /// per-edge `ignore-interface-standard = true` override would
    /// be declared.  Distinct from
    /// [`StandardCompatViolation::consumer_site`], which points at
    /// the workspace root manifest for inherited standards.
    pub consumer_manifest_path: PathBuf,
    /// Whether the consuming package opted this edge out of the
    /// check with `ignore-interface-standard = true` on the
    /// selected dependency entry.  The CLI renders ignored
    /// violations as unchecked-edge notes instead of errors.
    pub ignored: bool,
    /// Manifest table of the package-dependency entry that
    /// resolved this edge - where a per-edge override lives or
    /// would live: `[dependencies]`, `[dev-dependencies]`, or the
    /// condition-qualified `[target.'cfg(...)'.<kind>]` form the
    /// entry was declared under.  `None` for intra-package edges,
    /// which have no entry to carry one.
    pub override_section: Option<String>,
}

/// Per-node declaration-site bookkeeping, parallel to the
/// [`TargetNode`] slice: the [`DeclSite`] behind each populated D6
/// attribute, for [`RequirementOrigin`] construction.
struct NodeSites {
    decl_c: Option<DeclSite>,
    decl_cxx: Option<DeclSite>,
    impl_c: Option<DeclSite>,
    impl_cxx: Option<DeclSite>,
}

/// Evaluate spec D13 over every resolved dependency edge and return
/// the violated (edge, language) pairs, sorted by consumer,
/// dependency, then language for deterministic output.
///
/// # Errors
/// Returns [`BuildError::UnknownTargetInPackage`] when an edge
/// references a target its package does not declare - unreachable
/// after the planner's own resolution walk, kept as an error for
/// symmetry with the surrounding planner code.
pub(crate) fn edge_violations(
    topo: &[TargetId],
    resolved_deps: &HashMap<TargetId, Vec<TargetDepEdge>>,
    req: &PlanRequest<'_>,
) -> Result<Vec<StandardCompatViolation>, BuildError> {
    let index_of: HashMap<&TargetId, usize> = topo
        .iter()
        .enumerate()
        .map(|(index, tid)| (tid, index))
        .collect();

    let mut nodes: Vec<TargetNode> = Vec::with_capacity(topo.len());
    let mut sites: Vec<NodeSites> = Vec::with_capacity(topo.len());
    for tid in topo {
        let target = lookup_target(tid, req.graph)?;
        let pkg = &req.graph.packages[tid.0];
        let pkg_standards = req
            .language_standards
            .get(&tid.0)
            .copied()
            .unwrap_or_default();
        let (attributes, node_sites) =
            dependency_attributes(tid, target, pkg_standards, &pkg.package.language, req);
        let deps = resolved_deps
            .get(tid)
            .map(|edges| {
                edges
                    .iter()
                    .map(|edge| TargetEdge {
                        to: index_of[&edge.to],
                        public: edge.public,
                    })
                    .collect()
            })
            .unwrap_or_default();
        nodes.push(TargetNode {
            name: format_target_id(tid, req.graph),
            manifest_path: pkg.manifest_path.clone(),
            attributes,
            sites: declaration_sites(&node_sites),
            deps,
        });
        sites.push(node_sites);
    }

    let effective = effective_requirements(&nodes);

    let mut violations = Vec::new();
    for (consumer_index, tid) in topo.iter().enumerate() {
        let consumer_target = lookup_target(tid, req.graph)?;
        // A header-only consumer compiles no language: every edge
        // out of it is compatible vacuously (spec D13); its
        // requirements still propagated through `effective`.
        if consumer_target.kind.is_header_only() {
            continue;
        }
        let pkg_standards = req
            .language_standards
            .get(&tid.0)
            .copied()
            .unwrap_or_default();
        let Some(edges) = resolved_deps.get(tid) else {
            continue;
        };
        let compiles_c = has_sources_of(consumer_target, SourceLanguage::C);
        let compiles_cxx = has_sources_of(consumer_target, SourceLanguage::Cxx);
        // Which package-dependency edge resolved this target's
        // deps - the planner's own lookup rule
        // (`resolve_target_dep`): Normal-kind edges win, and
        // Dev-kind edges participate only for dev-only target
        // kinds (`test` / `example`) and only when no Normal edge
        // to the package exists.  The opt-out must sit on the
        // entry resolution actually selected: a flag on an
        // unselected (or invisible) edge suppresses nothing.
        let dev_edges_visible = consumer_target.kind.is_dev_only();
        let selected_pkg_edge = |dep_pkg: usize| -> Option<&cabin_workspace::DependencyEdge> {
            let pkg_edges = &req.graph.packages[tid.0].deps;
            pkg_edges
                .iter()
                .find(|pkg_edge| {
                    pkg_edge.kind == cabin_core::DependencyKind::Normal && pkg_edge.index == dep_pkg
                })
                .or_else(|| {
                    dev_edges_visible
                        .then(|| {
                            pkg_edges.iter().find(|pkg_edge| {
                                pkg_edge.kind == cabin_core::DependencyKind::Dev
                                    && pkg_edge.index == dep_pkg
                            })
                        })
                        .flatten()
                })
        };
        for edge in edges {
            let dep_index = index_of[&edge.to];
            // Per-edge opt-out: the consuming package's own
            // dependency entry for the dependency's package
            // carries `ignore-interface-standard = true`.  The
            // check still runs so the CLI can report the edge as
            // unchecked; only cross-package edges are overridable
            // (an intra-package edge has no `[dependencies]`
            // entry to carry the flag).  The selected entry's
            // manifest section travels on the violation so the
            // remedies name the table resolution actually read.
            let selected = if tid.0 == edge.to.0 {
                None
            } else {
                selected_pkg_edge(edge.to.0)
            };
            let edge_ignored = selected.is_some_and(|pkg_edge| pkg_edge.ignore_interface_standard);
            let override_section = selected.map(section_of);
            // D13's conjunction ranges over the languages the
            // consumer compiles; an absent effective standard for a
            // compiled language is a manifest error surfaced at the
            // compile site, not here.
            if compiles_c
                && let Some(consumer) = effective_c(&pkg_standards, consumer_target)
                && !effective[dep_index]
                    .c
                    .requirement
                    .satisfied_by(consumer.standard)
            {
                let provenance = provenance_c(&effective, dep_index);
                if !interface_less_default_origin(&provenance, topo, req)? {
                    violations.push(violation(
                        SourceLanguage::C,
                        consumer.standard.as_str(),
                        consumer_site(consumer.source, "c-standard", consumer_index, &nodes, req),
                        requirement_of(effective[dep_index].c.requirement),
                        &provenance,
                        tid,
                        edge,
                        edge_ignored,
                        override_section.clone(),
                        &nodes,
                        &sites,
                        req,
                    ));
                }
            }
            if compiles_cxx
                && let Some(consumer) = effective_cxx(&pkg_standards, consumer_target)
                && !effective[dep_index]
                    .cxx
                    .requirement
                    .satisfied_by(consumer.standard)
            {
                let provenance = provenance_cxx(&effective, dep_index);
                if !interface_less_default_origin(&provenance, topo, req)? {
                    violations.push(violation(
                        SourceLanguage::Cxx,
                        consumer.standard.as_str(),
                        consumer_site(consumer.source, "cxx-standard", consumer_index, &nodes, req),
                        requirement_of(effective[dep_index].cxx.requirement),
                        &provenance,
                        tid,
                        edge,
                        edge_ignored,
                        override_section.clone(),
                        &nodes,
                        &sites,
                        req,
                    ));
                }
            }
        }
    }

    // Topo iteration is already deterministic for a fixed graph;
    // the explicit sort pins the reading order to something a user
    // can predict regardless of the topo tie-breaks.
    violations.sort_by(|a, b| {
        (&a.consumer, &a.dependency, a.language.human_label()).cmp(&(
            &b.consumer,
            &b.dependency,
            b.language.human_label(),
        ))
    });
    Ok(violations)
}

/// The manifest table a package-dependency edge was declared
/// under, exactly as a user would spell it: the plain kind table
/// for unconditional entries, the condition-qualified
/// `[target.'cfg(...)'.<kind>]` form otherwise.  Remedies must
/// name the declaring table - `Package::validate_dependencies`
/// rejects a second same-kind entry for the same package, so
/// pointing a conditional edge's user at the top-level table
/// would send them into a rejected manifest.
fn section_of(pkg_edge: &cabin_workspace::DependencyEdge) -> String {
    let table = match pkg_edge.kind {
        cabin_core::DependencyKind::Normal => "dependencies",
        cabin_core::DependencyKind::Dev => "dev-dependencies",
    };
    match &pkg_edge.condition {
        Some(condition) => format!("[target.'cfg({condition})'.{table}]"),
        None => format!("[{table}]"),
    }
}

/// Whether a violated requirement originates at an interface-less
/// target's cross-language default and should be suppressed.
///
/// An executable-like target has no consumable interface: the
/// manifest rejects `interface-*` fields on those kinds, and the
/// planner's include-dir walk never takes headers from them.  The
/// strict C++-to-C default (spec D9 row 6) presumes dependency
/// headers a C consumer could fail to compile, so a requirement the
/// default originates *at* such a target warns about headers that
/// do not exist.  Requirements merely passing through it from a
/// library behind it keep warning (their origin is the library),
/// and the always-on build-time enforcement independently checks
/// the transitive closure, so nothing real is lost.
fn interface_less_default_origin(
    provenance: &Provenance<'_>,
    topo: &[TargetId],
    req: &PlanRequest<'_>,
) -> Result<bool, BuildError> {
    // Rows 1-3 cannot originate at an executable-like target (the
    // parser rejects its interface fields, the package-level
    // default is gated to library-like kinds, and it is not
    // header-only), so the cross-language default is the only
    // origin to inspect.
    if provenance.origin.source != ReqOfSource::CrossLanguageDefault {
        return Ok(false);
    }
    let origin_tid = &topo[*provenance
        .path
        .last()
        .expect("a provenance chain is never empty")];
    let origin = lookup_target(origin_tid, req.graph)?;
    Ok(!origin.kind.is_library_like())
}

/// Spec D6 attribute mapping for one target, with the declaration
/// sites behind each populated attribute.
///
/// Population contract (D6): `impl_L` is `Some` exactly when the
/// target itself implements `L` - a compiled target implements `L`
/// when it has sources of `L` (level via target-over-package
/// precedence), a header-only target only via a target-level
/// implementation declaration.  `decl_L` is the explicit interface
/// declaration only (target over package tier, workspace-inherited
/// counts as declared) - never the build-time implementation-
/// standard fallback.
fn dependency_attributes(
    tid: &TargetId,
    target: &Target,
    pkg_standards: ResolvedLanguageStandards,
    pkg_settings: &LanguageStandardSettings,
    req: &PlanRequest<'_>,
) -> (DependencyAttributes, NodeSites) {
    // The D6 attribute mapping (population contract included) is shared
    // with the published-index derivation so the two cannot drift; only
    // the declaration-site bookkeeping for diagnostics is local here.
    let attributes = cabin_core::standard_compatibility::dependency_attributes(
        target,
        &pkg_standards,
        pkg_settings,
    );
    let header_only = target.kind.is_header_only();

    let pkg = &req.graph.packages[tid.0];
    let target_name = target.name.as_str();

    let node_sites = NodeSites {
        decl_c: attributes.decl_c.is_some().then(|| {
            interface_decl_site(
                "interface-c-standard",
                target.language.interface_c_standard.is_some(),
                matches!(
                    pkg_settings.interface_c_standard,
                    Some(StandardDeclaration::Inherited(_))
                ),
                target_name,
                pkg,
                req,
            )
        }),
        decl_cxx: attributes.decl_cxx.is_some().then(|| {
            interface_decl_site(
                "interface-cxx-standard",
                target.language.interface_cxx_standard.is_some(),
                matches!(
                    pkg_settings.interface_cxx_standard,
                    Some(StandardDeclaration::Inherited(_))
                ),
                target_name,
                pkg,
                req,
            )
        }),
        // Header-only inference (D9 row 3) cites the target-level
        // implementation declaration the inference read; a compiled
        // target's implementation standard is never cited (row 4
        // imposes nothing).
        impl_c: (header_only && attributes.impl_c.is_some()).then(|| DeclSite {
            manifest_path: pkg.manifest_path.clone(),
            scope: DeclScope::Target(target_name.to_owned()),
            field: "c-standard",
        }),
        impl_cxx: (header_only && attributes.impl_cxx.is_some()).then(|| DeclSite {
            manifest_path: pkg.manifest_path.clone(),
            scope: DeclScope::Target(target_name.to_owned()),
            field: "cxx-standard",
        }),
    };

    (attributes, node_sites)
}

/// The [`DeclSite`] of a populated interface declaration: the
/// target-level field when present, otherwise the package-level
/// field (which points at the workspace root when inherited).
fn interface_decl_site(
    field: &'static str,
    target_declares: bool,
    pkg_inherited: bool,
    target_name: &str,
    pkg: &cabin_workspace::WorkspacePackage,
    req: &PlanRequest<'_>,
) -> DeclSite {
    if target_declares {
        DeclSite {
            manifest_path: pkg.manifest_path.clone(),
            scope: DeclScope::Target(target_name.to_owned()),
            field,
        }
    } else if pkg_inherited {
        DeclSite {
            manifest_path: req.graph.root_manifest_path.clone(),
            scope: DeclScope::Workspace,
            field,
        }
    } else {
        DeclSite {
            manifest_path: pkg.manifest_path.clone(),
            scope: DeclScope::Package,
            field,
        }
    }
}

/// Project [`NodeSites`] into the spanless [`DeclarationSites`] the
/// effective-requirement composition records provenance with.
fn declaration_sites(sites: &NodeSites) -> DeclarationSites {
    let site = |decl: &Option<DeclSite>| {
        decl.as_ref().map(|s| DeclarationSite {
            manifest_path: s.manifest_path.clone(),
            span: None,
        })
    };
    DeclarationSites {
        decl_c: site(&sites.decl_c),
        decl_cxx: site(&sites.decl_cxx),
        impl_c: site(&sites.impl_c),
        impl_cxx: site(&sites.impl_cxx),
    }
}

/// The consumer-side [`DeclSite`] for the violated language's
/// effective compile standard.
fn consumer_site(
    source: LanguageStandardSource,
    field: &'static str,
    consumer_index: usize,
    nodes: &[TargetNode],
    req: &PlanRequest<'_>,
) -> DeclSite {
    let manifest_path = nodes[consumer_index].manifest_path.clone();
    match source {
        LanguageStandardSource::Target => DeclSite {
            manifest_path,
            scope: DeclScope::Target(target_name_of(&nodes[consumer_index].name)),
            field,
        },
        LanguageStandardSource::Package => DeclSite {
            manifest_path,
            scope: DeclScope::Package,
            field,
        },
        LanguageStandardSource::Workspace => DeclSite {
            manifest_path: req.graph.root_manifest_path.clone(),
            scope: DeclScope::Workspace,
            field,
        },
    }
}

/// The `target` half of a `package:target` display name.
fn target_name_of(display: &str) -> String {
    display
        .split_once(':')
        .map_or(display, |(_, target)| target)
        .to_owned()
}

fn requirement_of<S: Copy + Ord + AsStandardStr>(requirement: Requirement<S>) -> EdgeRequirement {
    match requirement {
        Requirement::Min(min) => EdgeRequirement::Min(min.standard_str()),
        Requirement::Forbidden => EdgeRequirement::Forbidden,
        // A satisfied requirement never reaches violation
        // construction: `unconstrained` is satisfied at every level
        // (spec D11).
        Requirement::Unconstrained => {
            unreachable!("an unconstrained requirement cannot be violated (spec D11)")
        }
    }
}

/// `as_str` unification for the two level chains, local to this
/// module so `requirement_of` can stay generic like the spec's `L`.
trait AsStandardStr {
    fn standard_str(self) -> &'static str;
}
impl AsStandardStr for cabin_core::CStandard {
    fn standard_str(self) -> &'static str {
        self.as_str()
    }
}
impl AsStandardStr for cabin_core::CxxStandard {
    fn standard_str(self) -> &'static str {
        self.as_str()
    }
}

#[allow(clippy::too_many_arguments)]
fn violation(
    language: SourceLanguage,
    consumer_standard: &'static str,
    consumer_site: DeclSite,
    requirement: EdgeRequirement,
    provenance: &Provenance<'_>,
    consumer_tid: &TargetId,
    edge: &TargetDepEdge,
    ignored: bool,
    override_section: Option<String>,
    nodes: &[TargetNode],
    sites: &[NodeSites],
    req: &PlanRequest<'_>,
) -> StandardCompatViolation {
    let origin_index = *provenance
        .path
        .last()
        .expect("a provenance chain is never empty");
    let origin_sites = &sites[origin_index];
    let origin = match provenance.origin.source {
        ReqOfSource::Declared => RequirementOrigin::Declared {
            site: decl_site_for(origin_sites, language),
        },
        ReqOfSource::DeclaredNone => RequirementOrigin::DeclaredNone {
            site: decl_site_for(origin_sites, language),
        },
        ReqOfSource::HeaderOnlyInference => RequirementOrigin::HeaderOnlyInference {
            site: impl_site_for(origin_sites, language),
        },
        ReqOfSource::CrossLanguageDefault => RequirementOrigin::CrossLanguageDefault,
        // Row 4 yields `unconstrained`, which satisfies every
        // consumer (spec D11): it can never originate a violated
        // join.
        ReqOfSource::CompiledNoDeclaration => unreachable!(
            "a compiled target without a declaration imposes no constraint (spec D9 row 4)"
        ),
    };
    let dep_pkg = &req.graph.packages[edge.to.0];
    StandardCompatViolation {
        consumer: format_target_id(consumer_tid, req.graph),
        language,
        consumer_standard,
        consumer_site,
        dependency: nodes[*provenance
            .path
            .first()
            .expect("a provenance chain is never empty")]
        .name
        .clone(),
        dependency_package: dep_pkg.package.name.as_str().to_owned(),
        dependency_version: dep_pkg.package.version.to_string(),
        dependency_is_registry: matches!(dep_pkg.kind, PackageKind::Registry),
        requirement,
        origin_target: nodes[origin_index].name.clone(),
        origin,
        chain: provenance
            .path
            .iter()
            .map(|&index| nodes[index].name.clone())
            .collect(),
        consumer_manifest_path: req.graph.packages[consumer_tid.0].manifest_path.clone(),
        ignored,
        override_section,
    }
}

/// The interface-declaration site of the origin target for
/// `language`.  Present whenever the origin's `ReqOf` came from D9
/// rows 1-2, which is exactly when this is called.
fn decl_site_for(sites: &NodeSites, language: SourceLanguage) -> DeclSite {
    match language {
        SourceLanguage::C => sites.decl_c.clone(),
        SourceLanguage::Cxx => sites.decl_cxx.clone(),
    }
    .expect("a declared requirement records its declaration site")
}

/// The implementation-declaration site anchoring header-only
/// inference (D9 row 3).
fn impl_site_for(sites: &NodeSites, language: SourceLanguage) -> DeclSite {
    match language {
        SourceLanguage::C => sites.impl_c.clone(),
        SourceLanguage::Cxx => sites.impl_cxx.clone(),
    }
    .expect("header-only inference records its implementation site")
}

fn has_sources_of(target: &Target, language: SourceLanguage) -> bool {
    target
        .sources
        .iter()
        .any(|source| classify_source(source) == Some(language))
}
