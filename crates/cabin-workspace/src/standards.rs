//! Effective-requirement composition over the target dependency
//! graph.
//!
//! Implements the recursion `R_L` of
//! `docs/design/standard-compatibility/spec.md` D10 on top of the
//! per-target `ReqOf` values of
//! `cabin_core::standard_compatibility`: for each language,
//! `R_L(t) = ReqOf(t, L) ⊔ ⨆ { R_L(d) : (t, d) ∈ E_pub }`, computed
//! for every target in one topological pass - `O(|V| + |E|)` per
//! language (spec T3(2)).  Spec T1 proves the recursion has exactly
//! one solution on the finite DAG and that every topological order
//! computes it (confluence); the tests exercise that lemma directly.
//!
//! Alongside each value the pass records *provenance*: which
//! declaration (or header-only inference, or cross-language default)
//! attains the join, and through which chain of public edges it is
//! reached, carrying manifest paths and optional `miette` spans so a
//! later diagnostic can render, e.g., "requires c++20 via public
//! dependency baz:baz (path/to/manifest, line N)".  Only a parent
//! pointer is stored per target and language, keeping the pass
//! linear; [`provenance_c`] / [`provenance_cxx`] materialize a full
//! chain on demand.
//!
//! The node slice is the caller's index space: resolving raw
//! manifest `deps` references to concrete targets stays in
//! `cabin-build`, and enumerating resolver candidates stays in
//! `cabin-resolver`; both hand this module an already-resolved
//! target graph.

use std::path::PathBuf;

use cabin_core::standard_compatibility::{
    DependencyAttributes, EffectiveRequirements, ReqOfSource, Requirement, req_of_c_with_source,
    req_of_cxx_with_source,
};
use cabin_core::{CStandard, CxxStandard};

/// One target node of the compatibility graph.  [`TargetEdge::to`]
/// indices refer to positions in the slice handed to
/// [`effective_requirements`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetNode {
    /// Display name for diagnostics, conventionally `package:target`.
    pub name: String,
    /// Manifest that declares this target - the provenance site for
    /// requirements with no declaration to point at (D9 rows 4-6).
    pub manifest_path: PathBuf,
    /// Spec D6 resolved attributes, the inputs to `ReqOf`.
    pub attributes: DependencyAttributes,
    /// Where the populated fields of `attributes` were declared.
    pub sites: DeclarationSites,
    /// Dependency edges in declaration order.  Only public edges
    /// propagate requirements (spec D10); private edges may be
    /// passed along unfiltered and are ignored here.
    pub deps: Vec<TargetEdge>,
}

/// One resolved dependency edge of the compatibility graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetEdge {
    /// Index of the depended-on target in the node slice.
    pub to: usize,
    /// Spec D5: whether the edge is in `E_pub`.
    pub public: bool,
}

/// A manifest location a provenance origin points at.  The manifest
/// path may differ from the target's own when the value was
/// inherited from a workspace root.  The span is optional because
/// the typed manifest model does not retain per-field spans today;
/// carrying `miette::SourceSpan` here means a span-aware loader
/// plugs in without changing any type downstream of this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationSite {
    pub manifest_path: PathBuf,
    pub span: Option<miette::SourceSpan>,
}

/// Declaration sites for provenance, one per [`DependencyAttributes`]
/// field that can anchor a requirement.  Absent sites degrade
/// gracefully: provenance then falls back to the target's manifest
/// path with no span.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclarationSites {
    /// Site of the `interface-c-standard` declaration (D9 rows 1-2).
    pub decl_c: Option<DeclarationSite>,
    /// Site of the `interface-cxx-standard` declaration (D9 rows 1-2).
    pub decl_cxx: Option<DeclarationSite>,
    /// Site of the effective C implementation standard, anchoring
    /// header-only inference (D9 row 3).
    pub impl_c: Option<DeclarationSite>,
    /// Site of the effective C++ implementation standard (D9 row 3).
    pub impl_cxx: Option<DeclarationSite>,
}

/// `R_L` for one target across both languages, with provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveTarget {
    /// Spec D10: `R_C(t)`.
    pub c: Effective<CStandard>,
    /// Spec D10: `R_C++(t)`.
    pub cxx: Effective<CxxStandard>,
}

impl EffectiveTarget {
    /// The bare requirement pair for spec D13 edge-compatibility
    /// checks (`cabin_core::standard_compatibility::edge_compatible`).
    #[must_use]
    pub fn requirements(&self) -> EffectiveRequirements {
        EffectiveRequirements {
            c: self.c.requirement,
            cxx: self.cxx.requirement,
        }
    }
}

/// `R_L(t)` for one language, with the parent pointer anchoring its
/// provenance chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective<S> {
    /// Spec D10: the effective requirement `R_L(t)`.
    pub requirement: Requirement<S>,
    /// How the join is attained at this target.
    pub attained: Attained,
}

/// Parent pointer of a provenance chain.  Spec T1 makes the *value*
/// of `R_L` unique, but several sources may attain the same join;
/// keeping any one chain is acceptable.  For deterministic
/// diagnostics the pass keeps the target's own `ReqOf` when it
/// attains the join, and otherwise the first attaining public
/// dependency in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attained {
    /// The target's own `ReqOf` (spec D9) attains the join; the
    /// chain ends here.
    Own(Origin),
    /// `R_L` of the public dependency at this node index attains the
    /// join; follow that target's pointer to reach the origin.
    Via(usize),
}

/// The declaration-shaped end of a provenance chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// Which D9 row produced the requirement.
    pub source: ReqOfSource,
    /// The manifest location a diagnostic points at: the interface
    /// declaration for D9 rows 1-2, the implementation standard for
    /// row 3 (header-only inference), and the target's own manifest
    /// (spanless) for rows 4-6, which impose without a declaration
    /// to cite.
    pub site: DeclarationSite,
}

/// Compute `R_L` for every target and both languages (spec D10) in
/// one topological pass over the public-edge subgraph - `O(|V| +
/// |E|)` per language, spec T3(2).  The returned vector is parallel
/// to `targets`.
///
/// # Panics
/// Panics on a public dependency cycle or an out-of-bounds edge
/// index.  Spec D5 guarantees the graph handed to this model is a
/// finite DAG (a dependency cycle is an error upstream of
/// compatibility filtering), so either is a caller bug, not a
/// user-facing error.
#[must_use]
pub fn effective_requirements(targets: &[TargetNode]) -> Vec<EffectiveTarget> {
    let mut results: Vec<Option<EffectiveTarget>> = vec![None; targets.len()];
    for index in topo_order(targets) {
        let node = &targets[index];
        let (own_c, source_c) = req_of_c_with_source(&node.attributes);
        let (own_cxx, source_cxx) = req_of_cxx_with_source(&node.attributes);
        let computed = EffectiveTarget {
            c: compose(
                node,
                own_c,
                source_c,
                node.sites.decl_c.as_ref(),
                node.sites.impl_c.as_ref(),
                &results,
                |dep| &dep.c,
            ),
            cxx: compose(
                node,
                own_cxx,
                source_cxx,
                node.sites.decl_cxx.as_ref(),
                node.sites.impl_cxx.as_ref(),
                &results,
                |dep| &dep.cxx,
            ),
        };
        results[index] = Some(computed);
    }
    results
        .into_iter()
        .map(|result| result.expect("topological order covers every target"))
        .collect()
}

/// Materialize the C-side provenance chain for `target` from the
/// results of [`effective_requirements`].  Walks the stored parent
/// pointers, so the cost is the chain length, not the graph size.
#[must_use]
pub fn provenance_c(results: &[EffectiveTarget], target: usize) -> Provenance<'_> {
    provenance(results, target, |dep| &dep.c.attained)
}

/// Materialize the C++-side provenance chain for `target`.
#[must_use]
pub fn provenance_cxx(results: &[EffectiveTarget], target: usize) -> Provenance<'_> {
    provenance(results, target, |dep| &dep.cxx.attained)
}

/// A materialized provenance chain for one target and language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance<'a> {
    /// Node indices from the queried target down to the origin
    /// target, inclusive on both ends; each consecutive pair is a
    /// public edge.  A single entry means the target's own `ReqOf`
    /// attains its `R_L`.
    pub path: Vec<usize>,
    /// The declaration (or inference, or default) the chain ends at.
    pub origin: &'a Origin,
}

fn provenance(
    results: &[EffectiveTarget],
    target: usize,
    attained: impl Fn(&EffectiveTarget) -> &Attained,
) -> Provenance<'_> {
    let mut path = vec![target];
    let mut current = target;
    loop {
        match attained(&results[current]) {
            Attained::Own(origin) => return Provenance { path, origin },
            Attained::Via(next) => {
                current = *next;
                path.push(current);
            }
        }
    }
}

/// Fold one target's own `ReqOf` with `R_L` of its public
/// dependencies (the join of spec D10), recording the parent
/// pointer.  The strictly-greater comparison keeps the target's own
/// declaration on ties and the first attaining public dependency in
/// declaration order otherwise (see [`Attained`]); the derived `Ord`
/// on `Requirement` is D3's chain, so `max` is D4's join.
fn compose<S: Copy + Ord>(
    node: &TargetNode,
    own: Requirement<S>,
    own_source: ReqOfSource,
    decl_site: Option<&DeclarationSite>,
    impl_site: Option<&DeclarationSite>,
    results: &[Option<EffectiveTarget>],
    of: impl Fn(&EffectiveTarget) -> &Effective<S>,
) -> Effective<S> {
    let mut requirement = own;
    let mut via: Option<usize> = None;
    for edge in &node.deps {
        if !edge.public {
            continue;
        }
        let dep = of(results[edge.to]
            .as_ref()
            .expect("topological order visits dependencies first"));
        if dep.requirement > requirement {
            requirement = dep.requirement;
            via = Some(edge.to);
        }
    }
    let attained = match via {
        Some(to) => Attained::Via(to),
        None => Attained::Own(origin(node, own_source, decl_site, impl_site)),
    };
    Effective {
        requirement,
        attained,
    }
}

/// The provenance origin for a target whose own `ReqOf` attains the
/// join: route the D9 row to the manifest site that declared it.
fn origin(
    node: &TargetNode,
    source: ReqOfSource,
    decl_site: Option<&DeclarationSite>,
    impl_site: Option<&DeclarationSite>,
) -> Origin {
    let site = match source {
        ReqOfSource::DeclaredNone | ReqOfSource::Declared => decl_site,
        ReqOfSource::HeaderOnlyInference => impl_site,
        // Rows 4-6 impose without a declaration; point at the
        // target's own manifest.
        ReqOfSource::CompiledNoDeclaration | ReqOfSource::CrossLanguageDefault => None,
    };
    Origin {
        source,
        site: site.cloned().unwrap_or_else(|| DeclarationSite {
            manifest_path: node.manifest_path.clone(),
            span: None,
        }),
    }
}

/// Topological order of the public-edge subgraph `(T, E_pub)`,
/// dependencies before dependents.  Mirrors the loader's package
/// topo sort; cycles panic per [`effective_requirements`]'s
/// contract.
fn topo_order(targets: &[TargetNode]) -> Vec<usize> {
    #[derive(Clone, Copy)]
    enum Color {
        Visiting,
        Done,
    }

    fn visit(
        node: usize,
        targets: &[TargetNode],
        state: &mut [Option<Color>],
        order: &mut Vec<usize>,
    ) {
        match state[node] {
            Some(Color::Done) => return,
            Some(Color::Visiting) => panic!(
                "public dependency cycle through `{}`: the target graph must be a DAG (spec D5)",
                targets[node].name
            ),
            None => {}
        }
        state[node] = Some(Color::Visiting);
        for edge in &targets[node].deps {
            if edge.public {
                visit(edge.to, targets, state, order);
            }
        }
        state[node] = Some(Color::Done);
        order.push(node);
    }

    let mut state = vec![None; targets.len()];
    let mut order = Vec::with_capacity(targets.len());
    for index in 0..targets.len() {
        visit(index, targets, &mut state, &mut order);
    }
    order
}
