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

#[cfg(test)]
mod tests {
    use super::*;
    use cabin_core::standard_compatibility::DependencyKind;
    use cabin_core::{InterfaceRequirement, StandardRequirement};

    fn interface_min<S>(min: S) -> InterfaceRequirement<S> {
        InterfaceRequirement::Requirement(StandardRequirement { min, max: None })
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

        let cxx = provenance_cxx(&results, 0);
        assert_eq!(cxx.path, [0, 1, 2]);
        assert_eq!(cxx.origin.source, ReqOfSource::Declared);
        assert_eq!(cxx.origin.site, site("b/cabin.toml", 200));

        let c = provenance_c(&results, 0);
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
        assert_eq!(provenance_cxx(&results, 0).path, [0, 1, 3]);
        assert_eq!(provenance_c(&results, 0).path, [0, 1, 3]);

        // a's own c++17 declaration is exceeded by z's c++20.
        assert_eq!(
            results[1].cxx.requirement,
            Requirement::Min(CxxStandard::Cxx20)
        );
        assert_eq!(provenance_cxx(&results, 1).path, [1, 3]);
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

        let cxx = provenance_cxx(&results, 0);
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
        assert_eq!(provenance_cxx(&results, 1).path, [1]);
        assert_eq!(
            provenance_cxx(&results, 0).origin.source,
            ReqOfSource::CompiledNoDeclaration
        );
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

        let cxx = provenance_cxx(&results, 0);
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
        let c = provenance_c(&results, 0);
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

        let cxx = provenance_cxx(&results, 0);
        assert_eq!(cxx.path, [0, 1, 2]);
        assert_eq!(cxx.origin.source, ReqOfSource::HeaderOnlyInference);
        assert_eq!(cxx.origin.site, site("h/cabin.toml", 20));
        assert_eq!(
            provenance_c(&results, 0).origin.site,
            site("h/cabin.toml", 10)
        );
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
        let fwd_chain = map_back(&provenance_cxx(&forward_results, forward[0]).path, forward);
        let bwd_chain = map_back(
            &provenance_cxx(&backward_results, backward[0]).path,
            backward,
        );
        assert_eq!(fwd_chain, bwd_chain);
        assert_eq!(fwd_chain, [0, 1, 3, 4]);
    }
}
