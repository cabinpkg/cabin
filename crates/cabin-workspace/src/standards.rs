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
//! Alongside each value the pass records *provenance* per bound:
//! which declaration (or header-only inference, or cross-language
//! default) attains the composed lower bound, and which the upper
//! bound - the two may come from different targets, because the
//! join intersects ranges (spec D4) and no single source has to
//! determine a composed requirement.  Each bound stores only a
//! parent pointer, keeping the pass linear;
//! [`provenance_c`] / [`provenance_cxx`] materialize full chains on
//! demand, carrying manifest paths and optional `miette` spans so a
//! later diagnostic can render, e.g., "requires c++20 or newer via
//! public dependency baz:baz (path/to/manifest, line N)" - or, for
//! an empty intersection, both clashing chains.
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

/// `R_L(t)` for one language, with per-bound parent pointers
/// anchoring its provenance chains.
///
/// Pointer population, by requirement shape:
/// - `Unconstrained`: both pointers `None`.
/// - `Min`: `min_attained` only.
/// - `Bounded`: both pointers - the two bounds may be attained by
///   different contributions.
/// - `Forbidden` from a single contribution (a declared `"none"`,
///   the strict cross-language default, or a propagated forbidden):
///   `min_attained` only.
/// - `Forbidden` from an **empty intersection** at this target:
///   both pointers, naming the clashing lower- and upper-bound
///   contributions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective<S> {
    /// Spec D10: the effective requirement `R_L(t)`.
    pub requirement: Requirement<S>,
    /// How the composed lower bound (or the forbidden verdict) is
    /// attained at this target.
    pub min_attained: Option<Attained>,
    /// How the composed upper bound is attained at this target.
    pub max_attained: Option<Attained>,
}

/// Parent pointer of one bound's provenance chain.  Spec T1 makes
/// the *value* of `R_L` unique, but several sources may attain the
/// same bound; keeping any one chain is acceptable.  For
/// deterministic diagnostics the pass keeps the target's own
/// `ReqOf` when it attains the bound, and otherwise the first
/// attaining public dependency in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attained {
    /// The target's own `ReqOf` (spec D9) attains the bound; the
    /// chain ends here.
    Own(Origin),
    /// The bound of the public dependency at this node index
    /// attains it; follow that target's matching bound pointer to
    /// reach the origin.
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

/// Materialize the C-side provenance for `target` from the results
/// of [`effective_requirements`].  Walks the stored parent
/// pointers, so the cost is the chain length, not the graph size.
#[must_use]
pub fn provenance_c(results: &[EffectiveTarget], target: usize) -> RequirementProvenance<'_> {
    provenance(results, target, |dep| &dep.c)
}

/// Materialize the C++-side provenance for `target`.
#[must_use]
pub fn provenance_cxx(results: &[EffectiveTarget], target: usize) -> RequirementProvenance<'_> {
    provenance(results, target, |dep| &dep.cxx)
}

/// A materialized provenance chain for one bound of one target and
/// language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance<'a> {
    /// Node indices from the queried target down to the origin
    /// target, inclusive on both ends; each consecutive pair is a
    /// public edge.  A single entry means the target's own `ReqOf`
    /// attains the bound.
    pub path: Vec<usize>,
    /// The declaration (or inference, or default) the chain ends at.
    pub origin: &'a Origin,
}

/// The materialized provenance of one composed requirement.  A
/// composed value has up to two independently attributed bounds, so
/// no single chain can always explain it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequirementProvenance<'a> {
    /// `Unconstrained`: nothing to attribute.
    Unconstrained,
    /// A single chain explains the value: a minimum-only
    /// requirement, or a forbidden that originates at one
    /// declaration (`"none"`), inference, or cross-language
    /// default.
    Single(Provenance<'a>),
    /// A bounded requirement: the lower and upper bound chains,
    /// possibly ending at different targets.
    Bounds {
        min: Provenance<'a>,
        max: Provenance<'a>,
    },
    /// Forbidden because two contributions intersect to the empty
    /// range: the clashing lower- and upper-bound chains.  The
    /// chains share their prefix down to the target where the
    /// intersection collapsed.
    EmptyIntersection {
        min: Provenance<'a>,
        max: Provenance<'a>,
    },
}

impl<'a> RequirementProvenance<'a> {
    /// The chain explaining the lower bound (or single forbidden
    /// origin), when one exists.
    #[must_use]
    pub fn min(&self) -> Option<&Provenance<'a>> {
        match self {
            Self::Unconstrained => None,
            Self::Single(min) | Self::Bounds { min, .. } | Self::EmptyIntersection { min, .. } => {
                Some(min)
            }
        }
    }
}

fn provenance<'a, S: Copy + 'a>(
    results: &'a [EffectiveTarget],
    target: usize,
    of: impl Fn(&'a EffectiveTarget) -> &'a Effective<S> + Copy,
) -> RequirementProvenance<'a> {
    let start = of(&results[target]);
    match (&start.min_attained, &start.max_attained) {
        (None, None) => RequirementProvenance::Unconstrained,
        (Some(_), Some(_)) => {
            let min = walk_bound(results, vec![target], of, |e| &e.min_attained);
            let max = walk_bound(results, vec![target], of, |e| &e.max_attained);
            if matches!(start.requirement, Requirement::Forbidden) {
                RequirementProvenance::EmptyIntersection { min, max }
            } else {
                RequirementProvenance::Bounds { min, max }
            }
        }
        (Some(_), None) => {
            if matches!(start.requirement, Requirement::Forbidden) {
                walk_forbidden(results, target, of)
            } else {
                RequirementProvenance::Single(walk_bound(results, vec![target], of, |e| {
                    &e.min_attained
                }))
            }
        }
        (None, Some(_)) => unreachable!(
            "an upper-bound pointer only exists alongside a lower-bound pointer (Bounded or empty intersection)"
        ),
    }
}

/// Follow one bound's parent pointers from the last node of `path`
/// to its `Own` origin.  Every node on a bound chain stores a
/// pointer for that bound (the bound propagated through it).
fn walk_bound<'a, S: 'a>(
    results: &'a [EffectiveTarget],
    mut path: Vec<usize>,
    of: impl Fn(&'a EffectiveTarget) -> &'a Effective<S>,
    pick: impl Fn(&'a Effective<S>) -> &'a Option<Attained>,
) -> Provenance<'a> {
    let mut current = *path.last().expect("bound walks start non-empty");
    loop {
        match pick(of(&results[current]))
            .as_ref()
            .expect("every node on a bound chain stores that bound's pointer")
        {
            Attained::Own(origin) => return Provenance { path, origin },
            Attained::Via(next) => {
                current = *next;
                path.push(current);
            }
        }
    }
}

/// Follow a single-pointer forbidden chain.  It either ends at an
/// `Own` origin (a declared `"none"` or the strict cross-language
/// default) or reaches a node whose forbidden arose from an empty
/// intersection - the walk then forks into that node's two bound
/// chains, keeping the shared prefix.
fn walk_forbidden<'a, S: Copy + 'a>(
    results: &'a [EffectiveTarget],
    target: usize,
    of: impl Fn(&'a EffectiveTarget) -> &'a Effective<S> + Copy,
) -> RequirementProvenance<'a> {
    let mut path = vec![target];
    let mut current = target;
    loop {
        let node = of(&results[current]);
        if path.len() > 1 && node.max_attained.is_some() {
            // A forbidden node with both pointers is an
            // empty-intersection site (reached via propagation).
            let min = walk_bound(results, path.clone(), of, |e| &e.min_attained);
            let max = walk_bound(results, path, of, |e| &e.max_attained);
            return RequirementProvenance::EmptyIntersection { min, max };
        }
        match node
            .min_attained
            .as_ref()
            .expect("a forbidden requirement stores its origin pointer")
        {
            Attained::Own(origin) => {
                return RequirementProvenance::Single(Provenance { path, origin });
            }
            Attained::Via(next) => {
                current = *next;
                path.push(current);
            }
        }
    }
}

/// One contribution to a target's join: its own `ReqOf` or a public
/// dependency's `R_L`.
struct Contribution<S> {
    requirement: Requirement<S>,
    attained: Attained,
}

/// Fold one target's own `ReqOf` with `R_L` of its public
/// dependencies (the join of spec D10), recording a parent pointer
/// per bound.  The join intersects ranges, so the lower and upper
/// bound may be attained by different contributions; a
/// strictly-better comparison keeps the target's own declaration on
/// ties and the first attaining public dependency in declaration
/// order otherwise (see [`Attained`]).
fn compose<S: Copy + Ord>(
    node: &TargetNode,
    own: Requirement<S>,
    own_source: ReqOfSource,
    decl_site: Option<&DeclarationSite>,
    impl_site: Option<&DeclarationSite>,
    results: &[Option<EffectiveTarget>],
    of: impl Fn(&EffectiveTarget) -> &Effective<S>,
) -> Effective<S> {
    let contributions = std::iter::once(Contribution {
        requirement: own,
        attained: Attained::Own(origin(node, own_source, decl_site, impl_site)),
    })
    .chain(node.deps.iter().filter(|edge| edge.public).map(|edge| {
        let dep = of(results[edge.to]
            .as_ref()
            .expect("topological order visits dependencies first"));
        Contribution {
            requirement: dep.requirement,
            attained: Attained::Via(edge.to),
        }
    }));

    let mut forbidden: Option<Attained> = None;
    let mut lower: Option<(S, Attained)> = None;
    let mut upper: Option<(S, Attained)> = None;
    for contribution in contributions {
        let (min, max) = match contribution.requirement {
            Requirement::Forbidden => {
                if forbidden.is_none() {
                    forbidden = Some(contribution.attained);
                }
                continue;
            }
            Requirement::Unconstrained => continue,
            Requirement::Min(min) => (min, None),
            Requirement::Bounded(range) => (range.min(), Some(range.max())),
        };
        if lower.as_ref().is_none_or(|(current, _)| min > *current) {
            lower = Some((min, contribution.attained.clone()));
        }
        if let Some(max) = max
            && upper.as_ref().is_none_or(|(current, _)| max < *current)
        {
            upper = Some((max, contribution.attained));
        }
    }

    // A forbidden contribution absorbs (spec L2): its single origin
    // chain explains the result regardless of any accumulated
    // bounds.
    if let Some(attained) = forbidden {
        return Effective {
            requirement: Requirement::Forbidden,
            min_attained: Some(attained),
            max_attained: None,
        };
    }
    match (lower, upper) {
        (None, None) => Effective {
            requirement: Requirement::Unconstrained,
            min_attained: None,
            max_attained: None,
        },
        (Some((min, attained)), None) => Effective {
            requirement: Requirement::Min(min),
            min_attained: Some(attained),
            max_attained: None,
        },
        (Some((min, min_attained)), Some((max, max_attained))) => Effective {
            // An empty intersection collapses to forbidden; both
            // pointers survive so diagnostics can name the two
            // clashing origins.
            requirement: Requirement::bounded(min, max).unwrap_or(Requirement::Forbidden),
            min_attained: Some(min_attained),
            max_attained: Some(max_attained),
        },
        (None, Some(_)) => unreachable!(
            "an upper bound only exists on Bounded contributions, which also carry a lower bound"
        ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::standard_compatibility::DependencyKind;
    use cabin_core::{InterfaceRequirement, StandardLevel, StandardRequirement};

    fn interface_min<S>(min: S) -> InterfaceRequirement<S> {
        InterfaceRequirement::Requirement(StandardRequirement::at_least(min))
    }

    fn interface_range<S: StandardLevel>(min: S, max: S) -> InterfaceRequirement<S> {
        InterfaceRequirement::Requirement(StandardRequirement::bounded(min, Some(max)).unwrap())
    }

    /// Unwrap a provenance expected to be a single chain.
    fn single(provenance: RequirementProvenance<'_>) -> Provenance<'_> {
        match provenance {
            RequirementProvenance::Single(chain) => chain,
            other => panic!("expected a single provenance chain, got {other:?}"),
        }
    }

    /// A compiled target with no declarations - D9 row 4 for
    /// languages it implements, rows 5-6 otherwise.
    fn compiled(impl_c: Option<CStandard>, impl_cxx: Option<CxxStandard>) -> DependencyAttributes {
        DependencyAttributes {
            kind: DependencyKind::Compiled,
            impl_c,
            impl_cxx,
            decl_c: None,
            decl_cxx: None,
        }
    }

    fn site(path: &str, offset: usize) -> DeclarationSite {
        DeclarationSite {
            manifest_path: PathBuf::from(path),
            span: Some(miette::SourceSpan::new(offset.into(), 8)),
        }
    }

    /// Fake relative manifest paths are fine here: these are pure
    /// model tests that never touch the filesystem.
    fn node(name: &str, attributes: DependencyAttributes, deps: &[(usize, bool)]) -> TargetNode {
        TargetNode {
            name: name.to_owned(),
            manifest_path: PathBuf::from(format!("{name}/cabin.toml")),
            attributes,
            sites: DeclarationSites::default(),
            deps: deps
                .iter()
                .map(|&(to, public)| TargetEdge { to, public })
                .collect(),
        }
    }

    /// Linear chain root → a → b over public edges: b's explicit
    /// interface declarations (D9 row 2) propagate to the root by
    /// D10's join, in both languages, and the provenance chain
    /// carries b's manifest path and span - everything a diagnostic
    /// needs to render "requires c++20 via public dependency b:b
    /// (b/cabin.toml, line N)".
    #[test]
    fn linear_chain_propagates_declarations_with_provenance() {
        let mut b = node(
            "b:b",
            compiled(Some(CStandard::C17), Some(CxxStandard::Cxx23)),
            &[],
        );
        b.attributes.decl_c = Some(interface_min(CStandard::C17));
        b.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx20));
        b.sites.decl_c = Some(site("b/cabin.toml", 100));
        b.sites.decl_cxx = Some(site("b/cabin.toml", 200));
        let targets = vec![
            node(
                "root:root",
                compiled(Some(CStandard::C23), Some(CxxStandard::Cxx23)),
                &[(1, true)],
            ),
            node(
                "a:a",
                compiled(Some(CStandard::C23), Some(CxxStandard::Cxx23)),
                &[(2, true)],
            ),
            b,
        ];
        let results = effective_requirements(&targets);

        assert_eq!(
            results[0].cxx.requirement,
            Requirement::Min(CxxStandard::Cxx20)
        );
        assert_eq!(results[0].c.requirement, Requirement::Min(CStandard::C17));
        assert_eq!(
            results[1].cxx.requirement,
            Requirement::Min(CxxStandard::Cxx20)
        );

        let cxx = single(provenance_cxx(&results, 0));
        assert_eq!(cxx.path, [0, 1, 2]);
        assert_eq!(cxx.origin.source, ReqOfSource::Declared);
        assert_eq!(cxx.origin.site, site("b/cabin.toml", 200));

        let c = single(provenance_c(&results, 0));
        assert_eq!(c.path, [0, 1, 2]);
        assert_eq!(c.origin.source, ReqOfSource::Declared);
        assert_eq!(c.origin.site, site("b/cabin.toml", 100));

        // The chain's names and site suffice for the rendered form.
        assert_eq!(targets[*cxx.path.last().unwrap()].name, "b:b");
    }

    /// Diamond root → {a, b} → z: z's requirement is joined into
    /// the root once even though z is publicly reachable along two
    /// paths, because idempotence of D4's join means path
    /// multiplicity cannot double-count (T1's closed form is over
    /// the *set* `PubReach`).  The kept chain is the first attaining
    /// one in declaration order (any one chain is acceptable; see
    /// [`Attained`]).
    #[test]
    fn diamond_joins_once_and_keeps_one_chain() {
        let mut a = node(
            "a:a",
            compiled(Some(CStandard::C11), Some(CxxStandard::Cxx23)),
            &[(3, true)],
        );
        a.attributes.decl_c = Some(interface_min(CStandard::C99));
        a.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx17));
        let mut z = node(
            "z:z",
            compiled(Some(CStandard::C11), Some(CxxStandard::Cxx23)),
            &[],
        );
        z.attributes.decl_c = Some(interface_min(CStandard::C11));
        z.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx20));
        let targets = vec![
            node(
                "root:root",
                compiled(Some(CStandard::C23), Some(CxxStandard::Cxx23)),
                &[(1, true), (2, true)],
            ),
            a,
            node(
                "b:b",
                compiled(Some(CStandard::C11), Some(CxxStandard::Cxx23)),
                &[(3, true)],
            ),
            z,
        ];
        let results = effective_requirements(&targets);

        // The join over PubReach(root) = {root, a, b, z}: z's
        // declarations are the maximum in both languages.
        assert_eq!(
            results[0].cxx.requirement,
            Requirement::Min(CxxStandard::Cxx20)
        );
        assert_eq!(results[0].c.requirement, Requirement::Min(CStandard::C11));

        // Both root → a → z and root → b → z attain the join; the
        // first declared edge (a) is kept, deterministically.
        assert_eq!(single(provenance_cxx(&results, 0)).path, [0, 1, 3]);
        assert_eq!(single(provenance_c(&results, 0)).path, [0, 1, 3]);

        // a's own c++17 declaration is exceeded by z's c++20.
        assert_eq!(
            results[1].cxx.requirement,
            Requirement::Min(CxxStandard::Cxx20)
        );
        assert_eq!(single(provenance_cxx(&results, 1)).path, [1, 3]);
    }

    /// When the target's own declaration attains the join, the chain
    /// ends at the target itself even if a public dependency imposes
    /// the same requirement (the documented tie preference).
    #[test]
    fn own_declaration_wins_ties_over_dependencies() {
        let mut root = node(
            "root:root",
            compiled(None, Some(CxxStandard::Cxx23)),
            &[(1, true)],
        );
        root.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx20));
        root.sites.decl_cxx = Some(site("root/cabin.toml", 40));
        let mut dep = node("dep:dep", compiled(None, Some(CxxStandard::Cxx23)), &[]);
        dep.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx20));
        let targets = vec![root, dep];
        let results = effective_requirements(&targets);

        let cxx = single(provenance_cxx(&results, 0));
        assert_eq!(
            results[0].cxx.requirement,
            Requirement::Min(CxxStandard::Cxx20)
        );
        assert_eq!(cxx.path, [0]);
        assert_eq!(cxx.origin.site, site("root/cabin.toml", 40));
    }

    /// Private edges do not propagate (D10 folds public dependencies
    /// only): even a forbidden requirement behind a private edge
    /// leaves the consumer and its ancestors untouched - Example 3's
    /// private variant.
    #[test]
    fn private_edges_do_not_propagate() {
        let mut b = node(
            "b:b",
            compiled(Some(CStandard::C23), Some(CxxStandard::Cxx23)),
            &[],
        );
        b.attributes.decl_cxx = Some(InterfaceRequirement::None);
        b.attributes.decl_c = Some(interface_min(CStandard::C23));
        let targets = vec![
            node(
                "root:root",
                compiled(None, Some(CxxStandard::Cxx17)),
                &[(1, true)],
            ),
            node(
                "a:a",
                compiled(None, Some(CxxStandard::Cxx17)),
                &[(2, false)],
            ),
            b,
        ];
        let results = effective_requirements(&targets);

        // b itself is forbidden for C++ (D9 row 1) ...
        assert_eq!(results[2].cxx.requirement, Requirement::Forbidden);
        // ... but a's private edge keeps it out of the join: a and
        // the root stay at their own row-4 / row-6 requirements.
        assert_eq!(results[1].cxx.requirement, Requirement::Unconstrained);
        assert_eq!(results[0].cxx.requirement, Requirement::Unconstrained);
        assert_eq!(results[1].c.requirement, Requirement::Forbidden);
        // Unconstrained composed values have nothing to attribute.
        assert_eq!(
            provenance_cxx(&results, 1),
            RequirementProvenance::Unconstrained
        );
        assert_eq!(
            provenance_cxx(&results, 0),
            RequirementProvenance::Unconstrained
        );
        // The forbidden C side of `a` (D9 row 6) keeps its single
        // origin chain.
        assert_eq!(single(provenance_c(&results, 1)).path, [1]);
    }

    /// Example 3: a declared `"none"` on a transitive public
    /// dependency poisons every ancestor for that language, and the
    /// provenance chain points at the origin of the `none` - its
    /// manifest path and span included.
    #[test]
    fn declared_none_poisons_public_ancestors_with_provenance() {
        let mut b = node("b:b", compiled(None, Some(CxxStandard::Cxx17)), &[]);
        b.attributes.decl_cxx = Some(InterfaceRequirement::None);
        b.sites.decl_cxx = Some(site("b/cabin.toml", 64));
        let targets = vec![
            node(
                "root:root",
                compiled(None, Some(CxxStandard::Cxx26)),
                &[(1, true)],
            ),
            node(
                "a:a",
                compiled(None, Some(CxxStandard::Cxx17)),
                &[(2, true)],
            ),
            b,
        ];
        let results = effective_requirements(&targets);

        // The absorbing element of L2: once forbidden enters the
        // join, nothing recovers, at any consumer level.
        assert_eq!(results[0].cxx.requirement, Requirement::Forbidden);
        assert_eq!(results[1].cxx.requirement, Requirement::Forbidden);

        let cxx = single(provenance_cxx(&results, 0));
        assert_eq!(cxx.path, [0, 1, 2]);
        assert_eq!(cxx.origin.source, ReqOfSource::DeclaredNone);
        assert_eq!(cxx.origin.site, site("b/cabin.toml", 64));
    }

    /// The strict C++-to-C default (D9 row 6) poisons C consumers
    /// through public chains the same way an explicit `"none"` does,
    /// with provenance pointing at the C++-only dependency - there
    /// is no declaration to cite, so the site is its manifest,
    /// spanless.
    #[test]
    fn strict_cxx_to_c_default_poisons_c_ancestors() {
        let targets = vec![
            node(
                "root:root",
                compiled(Some(CStandard::C17), None),
                &[(1, true)],
            ),
            node(
                "mid:mid",
                compiled(Some(CStandard::C17), None),
                &[(2, true)],
            ),
            node(
                "cxxlib:cxxlib",
                compiled(None, Some(CxxStandard::Cxx20)),
                &[],
            ),
        ];
        let results = effective_requirements(&targets);

        assert_eq!(results[0].c.requirement, Requirement::Forbidden);
        let c = single(provenance_c(&results, 0));
        assert_eq!(c.path, [0, 1, 2]);
        assert_eq!(c.origin.source, ReqOfSource::CrossLanguageDefault);
        assert_eq!(
            c.origin.site,
            DeclarationSite {
                manifest_path: PathBuf::from("cxxlib:cxxlib/cabin.toml"),
                span: None,
            }
        );

        // The permissive C-to-C++ default (row 5) leaves the C++
        // side of the same chain unconstrained below cxxlib.
        assert_eq!(results[1].cxx.requirement, Requirement::Unconstrained);
    }

    /// Header-only inference (D9 row 3) feeds composition: the
    /// inferred minimum joins into every public ancestor, with the
    /// origin pointing at the implementation-standard site that the
    /// inference read.
    #[test]
    fn header_only_inference_feeds_composition() {
        let mut h = TargetNode {
            name: "h:h".to_owned(),
            manifest_path: PathBuf::from("h/cabin.toml"),
            attributes: DependencyAttributes {
                kind: DependencyKind::HeaderOnly,
                impl_c: Some(CStandard::C17),
                impl_cxx: Some(CxxStandard::Cxx20),
                decl_c: None,
                decl_cxx: None,
            },
            sites: DeclarationSites::default(),
            deps: Vec::new(),
        };
        h.sites.impl_c = Some(site("h/cabin.toml", 10));
        h.sites.impl_cxx = Some(site("h/cabin.toml", 20));
        let targets = vec![
            node(
                "root:root",
                compiled(Some(CStandard::C23), Some(CxxStandard::Cxx23)),
                &[(1, true)],
            ),
            node(
                "a:a",
                compiled(Some(CStandard::C23), Some(CxxStandard::Cxx23)),
                &[(2, true)],
            ),
            h,
        ];
        let results = effective_requirements(&targets);

        assert_eq!(
            results[0].cxx.requirement,
            Requirement::Min(CxxStandard::Cxx20)
        );
        assert_eq!(results[0].c.requirement, Requirement::Min(CStandard::C17));

        let cxx = single(provenance_cxx(&results, 0));
        assert_eq!(cxx.path, [0, 1, 2]);
        assert_eq!(cxx.origin.source, ReqOfSource::HeaderOnlyInference);
        assert_eq!(cxx.origin.site, site("h/cabin.toml", 20));
        assert_eq!(
            single(provenance_c(&results, 0)).origin.site,
            site("h/cabin.toml", 10)
        );
    }

    /// The two bounds of a composed range may come from different
    /// targets: no single source determines the requirement, and
    /// each bound carries its own chain.
    #[test]
    fn bounds_attributed_to_different_dependencies() {
        let mut floor = node("floor:floor", compiled(None, Some(CxxStandard::Cxx23)), &[]);
        floor.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx17));
        floor.sites.decl_cxx = Some(site("floor/cabin.toml", 10));
        let mut cap = node("cap:cap", compiled(None, Some(CxxStandard::Cxx17)), &[]);
        cap.attributes.decl_cxx = Some(interface_range(CxxStandard::Cxx11, CxxStandard::Cxx20));
        cap.sites.decl_cxx = Some(site("cap/cabin.toml", 20));
        let targets = vec![
            node(
                "root:root",
                compiled(None, Some(CxxStandard::Cxx17)),
                &[(1, true), (2, true)],
            ),
            floor,
            cap,
        ];
        let results = effective_requirements(&targets);

        assert_eq!(
            results[0].cxx.requirement,
            Requirement::bounded(CxxStandard::Cxx17, CxxStandard::Cxx20).unwrap()
        );
        let RequirementProvenance::Bounds { min, max } = provenance_cxx(&results, 0) else {
            panic!("expected per-bound provenance");
        };
        assert_eq!(min.path, [0, 1]);
        assert_eq!(min.origin.site, site("floor/cabin.toml", 10));
        assert_eq!(max.path, [0, 2]);
        assert_eq!(max.origin.site, site("cap/cabin.toml", 20));
    }

    /// Two public dependencies whose accepted ranges do not overlap
    /// forbid the consumer outright, and the provenance names both
    /// clashing chains.
    #[test]
    fn empty_intersection_forbids_with_both_chains() {
        let mut modern = node(
            "modern:modern",
            compiled(None, Some(CxxStandard::Cxx23)),
            &[],
        );
        modern.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx23));
        modern.sites.decl_cxx = Some(site("modern/cabin.toml", 30));
        let mut legacy = node(
            "legacy:legacy",
            compiled(None, Some(CxxStandard::Cxx14)),
            &[],
        );
        legacy.attributes.decl_cxx = Some(interface_range(CxxStandard::Cxx11, CxxStandard::Cxx17));
        legacy.sites.decl_cxx = Some(site("legacy/cabin.toml", 40));
        let targets = vec![
            node(
                "root:root",
                compiled(None, Some(CxxStandard::Cxx17)),
                &[(1, true), (2, true)],
            ),
            modern,
            legacy,
        ];
        let results = effective_requirements(&targets);

        assert_eq!(results[0].cxx.requirement, Requirement::Forbidden);
        let RequirementProvenance::EmptyIntersection { min, max } = provenance_cxx(&results, 0)
        else {
            panic!("expected empty-intersection provenance");
        };
        assert_eq!(min.path, [0, 1]);
        assert_eq!(min.origin.site, site("modern/cabin.toml", 30));
        assert_eq!(max.path, [0, 2]);
        assert_eq!(max.origin.site, site("legacy/cabin.toml", 40));
    }

    /// A forbidden born from an empty intersection propagates like
    /// any forbidden; querying an ancestor keeps the shared prefix
    /// and forks at the target where the ranges collapsed.
    #[test]
    fn propagated_empty_intersection_keeps_the_fork() {
        let mut modern = node(
            "modern:modern",
            compiled(None, Some(CxxStandard::Cxx23)),
            &[],
        );
        modern.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx23));
        let mut legacy = node(
            "legacy:legacy",
            compiled(None, Some(CxxStandard::Cxx14)),
            &[],
        );
        legacy.attributes.decl_cxx = Some(interface_range(CxxStandard::Cxx11, CxxStandard::Cxx17));
        let targets = vec![
            node(
                "root:root",
                compiled(None, Some(CxxStandard::Cxx26)),
                &[(1, true)],
            ),
            node(
                "mid:mid",
                compiled(None, Some(CxxStandard::Cxx17)),
                &[(2, true), (3, true)],
            ),
            modern,
            legacy,
        ];
        let results = effective_requirements(&targets);

        assert_eq!(results[0].cxx.requirement, Requirement::Forbidden);
        assert_eq!(results[1].cxx.requirement, Requirement::Forbidden);
        let RequirementProvenance::EmptyIntersection { min, max } = provenance_cxx(&results, 0)
        else {
            panic!("expected empty-intersection provenance");
        };
        assert_eq!(min.path, [0, 1, 2]);
        assert_eq!(max.path, [0, 1, 3]);
    }

    /// A declared `"none"` absorbs even when bounded contributions
    /// are present: the single forbidden chain explains the result.
    #[test]
    fn declared_none_absorbs_over_bounds() {
        let mut none = node("none:none", compiled(None, Some(CxxStandard::Cxx17)), &[]);
        none.attributes.decl_cxx = Some(InterfaceRequirement::None);
        none.sites.decl_cxx = Some(site("none/cabin.toml", 50));
        let mut cap = node("cap:cap", compiled(None, Some(CxxStandard::Cxx17)), &[]);
        cap.attributes.decl_cxx = Some(interface_range(CxxStandard::Cxx11, CxxStandard::Cxx20));
        let targets = vec![
            node(
                "root:root",
                compiled(None, Some(CxxStandard::Cxx17)),
                &[(1, true), (2, true)],
            ),
            none,
            cap,
        ];
        let results = effective_requirements(&targets);

        assert_eq!(results[0].cxx.requirement, Requirement::Forbidden);
        let chain = single(provenance_cxx(&results, 0));
        assert_eq!(chain.path, [0, 1]);
        assert_eq!(chain.origin.source, ReqOfSource::DeclaredNone);
        assert_eq!(chain.origin.site, site("none/cabin.toml", 50));
    }

    /// T1 (confluence): computing `R_L` along two different
    /// topological orders of the same DAG yields identical results.
    /// The pass derives its processing order from the node slice's
    /// layout, so materializing the same logical graph under two
    /// index assignments makes it run in two genuinely different
    /// topological orders; T1's uniqueness proof says the values
    /// must agree, and the deterministic tie policy makes even the
    /// kept chains agree.
    #[test]
    fn confluence_across_two_topological_orders() {
        // Logical graph: root → {a, b} public, a → z, b → z public,
        // z → w public; z declares c++20 / c11, w declares c++23
        // (the join's maximum) and c99.
        fn build(mapping: [usize; 5]) -> Vec<TargetNode> {
            let [root, a, b, z, w] = mapping;
            let mut nodes = vec![node("placeholder", compiled(None, None), &[]); 5];
            nodes[root] = node(
                "root:root",
                compiled(Some(CStandard::C23), Some(CxxStandard::Cxx23)),
                &[(a, true), (b, true)],
            );
            nodes[a] = node(
                "a:a",
                compiled(Some(CStandard::C23), Some(CxxStandard::Cxx23)),
                &[(z, true)],
            );
            nodes[b] = node(
                "b:b",
                compiled(Some(CStandard::C23), Some(CxxStandard::Cxx23)),
                &[(z, true)],
            );
            nodes[z] = {
                let mut z_node = node(
                    "z:z",
                    compiled(Some(CStandard::C11), Some(CxxStandard::Cxx20)),
                    &[(w, true)],
                );
                z_node.attributes.decl_c = Some(interface_min(CStandard::C11));
                z_node.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx20));
                z_node
            };
            nodes[w] = {
                let mut w_node = node(
                    "w:w",
                    compiled(Some(CStandard::C99), Some(CxxStandard::Cxx23)),
                    &[],
                );
                w_node.attributes.decl_c = Some(interface_min(CStandard::C99));
                w_node.attributes.decl_cxx = Some(interface_min(CxxStandard::Cxx23));
                w_node
            };
            nodes
        }

        // Identity layout processes root-first (DFS discovers w
        // deepest); the reversed layout starts its scan at w, so the
        // pass folds targets in a different topological order.
        let forward: [usize; 5] = [0, 1, 2, 3, 4];
        let backward: [usize; 5] = [4, 3, 2, 1, 0];
        let forward_results = effective_requirements(&build(forward));
        let backward_results = effective_requirements(&build(backward));

        for logical in 0..5 {
            let fwd = &forward_results[forward[logical]];
            let bwd = &backward_results[backward[logical]];
            assert_eq!(
                fwd.requirements(),
                bwd.requirements(),
                "T1 confluence at logical node {logical}"
            );
        }

        // The kept chains also agree once mapped back to logical
        // indices, because ties break by declaration order in both
        // layouts.
        let map_back = |path: &[usize], mapping: [usize; 5]| -> Vec<usize> {
            path.iter()
                .map(|&index| mapping.iter().position(|&m| m == index).unwrap())
                .collect()
        };
        let fwd_chain = map_back(
            &single(provenance_cxx(&forward_results, forward[0])).path,
            forward,
        );
        let bwd_chain = map_back(
            &single(provenance_cxx(&backward_results, backward[0])).path,
            backward,
        );
        assert_eq!(fwd_chain, bwd_chain);
        assert_eq!(fwd_chain, [0, 1, 3, 4]);
    }
}
