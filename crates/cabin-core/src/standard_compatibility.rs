//! Resolver-level standard-compatibility core.
//!
//! One-to-one implementation of the normative specification in
//! `docs/design/standard-compatibility/spec.md`; every public item
//! cites the definition (D1, D2, ...) or lemma (L1, ...) it
//! implements, and the tests cite the lemma they verify.  Pure data
//! and logic only: no I/O, no logging, no globals.
//!
//! The effective-requirement recursion `R_L` (spec D10/T1 -
//! transitive composition over the dependency graph) is deliberately
//! not implemented here: it is a graph algorithm, and graph
//! algorithms live in `cabin-workspace` (`cabin_workspace::standards`
//! is the implementation).  This module operates on already-composed
//! requirements: callers fold [`req_of_c`] / [`req_of_cxx`] values
//! with [`Requirement::join`] along public edges and hand the result
//! to [`edge_compatible`] as [`EffectiveRequirements`].
//!
//! Invariant I1 (spec D8): `gnu-extensions` never participates in
//! compatibility.  Nothing in this module takes it as an input, and
//! future changes must keep it that way.
//!
//! The languages themselves (spec D1) and their level chains (spec
//! D2) are the existing [`crate::SourceLanguage`], [`CStandard`],
//! and [`CxxStandard`] types; the two languages appear here as the
//! paired `c` / `cxx` fields of the structs below.

use crate::language_standard::{CStandard, CxxStandard, InterfaceRequirement};

/// Spec D3: the per-language requirement domain `Req_L`, a finite
/// chain under the strictness order `⊑` (spec L1).  The derived
/// `Ord` follows declaration order, which is exactly that chain:
/// `unconstrained ⊑ [⊥_L] ⊑ ... ⊑ [max Level_L] ⊑ forbidden` (the
/// tests verify the derived order against D3's case-by-case
/// definition by exhaustive enumeration).  Populating the reserved
/// interface `max` later is a domain swap to the interval domain of
/// D4's remark, not a signature change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Requirement<S> {
    /// Spec D3: imposes nothing on consumers.
    Unconstrained,
    /// Spec D3: `[m]` - requires a consumer level of at least the
    /// carried minimum.
    Min(S),
    /// Spec D3: unsatisfiable.
    Forbidden,
}

impl<S: Copy + Ord> Requirement<S> {
    /// Spec D4: the join `r1 ⊔ r2` is the `⊑`-maximum of the two
    /// requirements (well-defined because the chain is total, spec
    /// L1; a bounded join-semilattice by spec L2).
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        self.max(other)
    }

    /// Spec D4: the set join `⨆ S` over a finite (multi)set, with
    /// `⨆ ∅ = unconstrained`.
    #[must_use]
    pub fn join_all(requirements: impl IntoIterator<Item = Self>) -> Self {
        requirements
            .into_iter()
            .fold(Self::Unconstrained, Self::join)
    }

    /// Spec D11: `satisfies(c, L, r)` for a consumer whose effective
    /// compile level in `L` is `level`.
    #[must_use]
    pub fn satisfied_by(self, level: S) -> bool {
        match self {
            Self::Unconstrained => true,
            Self::Min(min) => level >= min,
            Self::Forbidden => false,
        }
    }

    /// Spec D12: the satisfaction set `Sat_L(r)`, as the sub-slice
    /// of `levels` a consumer may compile at.  `levels` must be the
    /// full chain `Level_L` of spec D2 ([`CStandard::ALL`] /
    /// [`CxxStandard::ALL`]).
    #[must_use]
    pub fn sat(self, levels: &[S]) -> Vec<S> {
        levels
            .iter()
            .copied()
            .filter(|&level| self.satisfied_by(level))
            .collect()
    }
}

/// Spec D6: `kind(t)` - whether a dependency target has translation
/// units of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    /// The target compiles translation units.
    Compiled,
    /// The target has no translation units; its headers are the
    /// implementation.
    HeaderOnly,
}

/// Spec D6: the resolved attributes of a dependency target, as
/// produced by the manifest layer (precedence and inheritance
/// already applied).  Per D6's population contract, `impl_*` is
/// `Some` exactly when the target itself implements the language - a
/// package-level implementation default alone never populates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DependencyAttributes {
    /// Spec D6: `kind(t)`.
    pub kind: DependencyKind,
    /// Spec D6: `impl_C(t)`; `None` is `⊥` (the target does not
    /// implement C).
    pub impl_c: Option<CStandard>,
    /// Spec D6: `impl_C++(t)`; `None` is `⊥`.
    pub impl_cxx: Option<CxxStandard>,
    /// Spec D6: `decl_C(t)`; `None` is `⊥` (no explicit interface
    /// declaration), `Some(InterfaceRequirement::None)` the declared
    /// `"none"`, and `Some(InterfaceRequirement::Requirement(..))` a
    /// declared minimum.
    pub decl_c: Option<InterfaceRequirement<CStandard>>,
    /// Spec D6: `decl_C++(t)`.
    pub decl_cxx: Option<InterfaceRequirement<CxxStandard>>,
}

/// Spec D9: which row of the declaration-to-requirement table
/// produced a `ReqOf` value.  Provenance-tracking callers (the
/// effective-requirement composition of D10) record this alongside
/// the requirement so a diagnostic can say *why* a target imposes
/// what it imposes; the rows are the routing of D9, so this enum is
/// derived here and nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReqOfSource {
    /// Row 1: the explicit interface declaration `"none"`.
    DeclaredNone,
    /// Row 2: an explicit declared interface minimum.
    Declared,
    /// Row 3: header-only inference from the implementation
    /// standard.
    HeaderOnlyInference,
    /// Row 4: a compiled target without an interface declaration
    /// imposes no constraint.
    CompiledNoDeclaration,
    /// Rows 5-6: the cross-language default - permissive
    /// (`unconstrained`) toward C++, strict (`forbidden`) toward C.
    CrossLanguageDefault,
}

/// Spec D9: `ReqOf(d, C)`.  Shares rows 1-4 with the C++ side and
/// lands on row 6 - the strict C++-to-C default `forbidden` - when
/// the target neither declares a C interface nor implements C.
#[must_use]
pub fn req_of_c(dependency: &DependencyAttributes) -> Requirement<CStandard> {
    req_of_c_with_source(dependency).0
}

/// Spec D9: `ReqOf(d, C)` together with the row that produced it.
#[must_use]
pub fn req_of_c_with_source(
    dependency: &DependencyAttributes,
) -> (Requirement<CStandard>, ReqOfSource) {
    req_of(
        dependency.kind,
        dependency.decl_c,
        dependency.impl_c,
        Requirement::Forbidden,
    )
}

/// Spec D9: `ReqOf(d, C++)`.  Shares rows 1-4 with the C side and
/// lands on row 5 - the permissive C-to-C++ default `unconstrained` -
/// when the target neither declares a C++ interface nor implements
/// C++.
#[must_use]
pub fn req_of_cxx(dependency: &DependencyAttributes) -> Requirement<CxxStandard> {
    req_of_cxx_with_source(dependency).0
}

/// Spec D9: `ReqOf(d, C++)` together with the row that produced it.
#[must_use]
pub fn req_of_cxx_with_source(
    dependency: &DependencyAttributes,
) -> (Requirement<CxxStandard>, ReqOfSource) {
    req_of(
        dependency.kind,
        dependency.decl_cxx,
        dependency.impl_cxx,
        Requirement::Unconstrained,
    )
}

/// Spec D9 rows 1-4, shared by both languages; `absent_default` is
/// the language-specific row 5/6 outcome for `decl = ⊥`, `impl = ⊥`.
fn req_of<S: Copy + Ord>(
    kind: DependencyKind,
    decl: Option<InterfaceRequirement<S>>,
    implementation: Option<S>,
    absent_default: Requirement<S>,
) -> (Requirement<S>, ReqOfSource) {
    match (decl, implementation) {
        // Row 1: the declared `"none"` is forbidden.
        (Some(InterfaceRequirement::None), _) => {
            (Requirement::Forbidden, ReqOfSource::DeclaredNone)
        }
        // Row 2: an explicit declaration always wins, over inference
        // and over both cross-language defaults.  `max` is reserved
        // and always absent in v1 (spec D4 remark).
        (Some(InterfaceRequirement::Requirement(requirement)), _) => {
            (Requirement::Min(requirement.min), ReqOfSource::Declared)
        }
        // Row 3: header-only inference from the implementation
        // standard.  Row 4: a compiled target without an interface
        // declaration imposes no constraint.
        (None, Some(min)) => match kind {
            DependencyKind::HeaderOnly => (Requirement::Min(min), ReqOfSource::HeaderOnlyInference),
            DependencyKind::Compiled => (
                Requirement::Unconstrained,
                ReqOfSource::CompiledNoDeclaration,
            ),
        },
        // Rows 5-6: the cross-language defaults.
        (None, None) => (absent_default, ReqOfSource::CrossLanguageDefault),
    }
}

/// Spec D7: a consumer's compiled languages and effective compile
/// levels.  `langs(c)` is the set of `Some` fields and `lvl(c, L)`
/// the carried level, so `lvl` is total on `langs(c)` by
/// construction.  A header-only consumer compiles no language: both
/// fields `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsumerStandards {
    /// Spec D7: `lvl(c, C)` when `C ∈ langs(c)`.
    pub c: Option<CStandard>,
    /// Spec D7: `lvl(c, C++)` when `C++ ∈ langs(c)`.
    pub cxx: Option<CxxStandard>,
}

/// Spec D10: a dependency target's effective requirement `R_L(d)`
/// for each language, already composed by the caller (the `R_L`
/// recursion itself is outside this module; see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveRequirements {
    /// Spec D10: `R_C(d)`.
    pub c: Requirement<CStandard>,
    /// Spec D10: `R_C++(d)`.
    pub cxx: Requirement<CxxStandard>,
}

/// Spec D13: edge compatibility - the conjunction, over every
/// language the consumer compiles, of `satisfies(c, L, R_L(d))`.
/// Languages the consumer does not compile impose nothing, so a
/// header-only consumer (empty `langs(c)`) is compatible vacuously;
/// the edge's own public/private classification does not appear in
/// the condition.
#[must_use]
pub fn edge_compatible(consumer: ConsumerStandards, dependency: EffectiveRequirements) -> bool {
    consumer
        .c
        .is_none_or(|level| dependency.c.satisfied_by(level))
        && consumer
            .cxx
            .is_none_or(|level| dependency.cxx.satisfied_by(level))
}

/// Spec D14: package-version viability - the conjunction of edge
/// compatibility (spec D13) over every dependency edge resolving to
/// the candidate version.  One incompatible edge excludes the
/// version.
#[must_use]
pub fn version_viable(
    edges: impl IntoIterator<Item = (ConsumerStandards, EffectiveRequirements)>,
) -> bool {
    edges
        .into_iter()
        .all(|(consumer, dependency)| edge_compatible(consumer, dependency))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_standard::StandardRequirement;

    const KINDS: [DependencyKind; 2] = [DependencyKind::Compiled, DependencyKind::HeaderOnly];

    /// The full `Req_L` chain (spec D3) for the given `Level_L`.
    fn all_requirements<S: Copy>(levels: &[S]) -> Vec<Requirement<S>> {
        let mut requirements = vec![Requirement::Unconstrained];
        requirements.extend(levels.iter().copied().map(Requirement::Min));
        requirements.push(Requirement::Forbidden);
        requirements
    }

    /// Spec D3's order definition transcribed case by case: the
    /// oracle every lemma test uses, and the definition the derived
    /// `Ord` must match (verified exhaustively in the L1 test).
    fn spec_le<S: Copy + Ord>(r: Requirement<S>, s: Requirement<S>) -> bool {
        match (r, s) {
            (Requirement::Unconstrained, _) | (_, Requirement::Forbidden) => true,
            (Requirement::Min(a), Requirement::Min(b)) => a <= b,
            // No other pairs are related; `⊑` is reflexive.
            _ => r == s,
        }
    }

    fn interface_min<S>(min: S) -> InterfaceRequirement<S> {
        InterfaceRequirement::Requirement(StandardRequirement { min, max: None })
    }

    /// `⊥` plus every level: the domain of `impl_L(t)` (spec D6).
    fn optional_levels<S: Copy>(levels: &[S]) -> Vec<Option<S>> {
        let mut options = vec![None];
        options.extend(levels.iter().copied().map(Some));
        options
    }

    fn check_l1_finite_chain<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
        let requirements = all_requirements(levels);
        for &r in &requirements {
            // Reflexivity, and the chain's bounds: unconstrained is
            // least, forbidden greatest.
            assert!(spec_le(r, r));
            assert!(spec_le(Requirement::Unconstrained, r));
            assert!(spec_le(r, Requirement::Forbidden));
            for &s in &requirements {
                // The derived order is exactly D3's definition.
                assert_eq!(r <= s, spec_le(r, s), "derived Ord vs D3 at {r:?} ⊑ {s:?}");
                // Totality and antisymmetry.
                assert!(spec_le(r, s) || spec_le(s, r), "totality at {r:?}, {s:?}");
                if spec_le(r, s) && spec_le(s, r) {
                    assert_eq!(r, s, "antisymmetry at {r:?}, {s:?}");
                }
                // Transitivity.
                for &t in &requirements {
                    if spec_le(r, s) && spec_le(s, t) {
                        assert!(spec_le(r, t), "transitivity at {r:?}, {s:?}, {t:?}");
                    }
                }
            }
        }
    }

    /// L1: `(Req_L, ⊑)` is a finite total order with least element
    /// `unconstrained` and greatest element `forbidden`, and the
    /// derived `Ord` coincides with D3's case-by-case definition.
    #[test]
    fn l1_requirement_domain_is_a_finite_chain() {
        check_l1_finite_chain(&CStandard::ALL);
        check_l1_finite_chain(&CxxStandard::ALL);
    }

    fn check_l2_bounded_semilattice<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
        let requirements = all_requirements(levels);
        for &r in &requirements {
            // Idempotence, identity, absorption.
            assert_eq!(r.join(r), r);
            assert_eq!(Requirement::Unconstrained.join(r), r);
            assert_eq!(Requirement::Forbidden.join(r), Requirement::Forbidden);
            for &s in &requirements {
                let join = r.join(s);
                // Commutativity.
                assert_eq!(join, s.join(r));
                // `⊔` is the least upper bound.
                assert!(
                    spec_le(r, join) && spec_le(s, join),
                    "upper bound at {r:?}, {s:?}"
                );
                for &upper in &requirements {
                    if spec_le(r, upper) && spec_le(s, upper) {
                        assert!(spec_le(join, upper), "leastness at {r:?}, {s:?}, {upper:?}");
                    }
                }
                // `⨆` is order- and multiplicity-independent.
                assert_eq!(Requirement::join_all([r, s]), join);
                assert_eq!(Requirement::join_all([s, r]), join);
                assert_eq!(Requirement::join_all([r, r, s]), join);
                // Associativity.
                for &t in &requirements {
                    assert_eq!(r.join(s).join(t), r.join(s.join(t)));
                    assert_eq!(Requirement::join_all([r, s, t]), r.join(s).join(t));
                }
            }
        }
        // The empty-join convention and the flattening law
        // `⨆(S ∪ S') = ⨆S ⊔ ⨆S'`, over every pair of subsets.
        assert_eq!(
            Requirement::join_all(std::iter::empty::<Requirement<S>>()),
            Requirement::Unconstrained
        );
        for union_of in 0u32..(1 << requirements.len()) {
            for other in 0u32..(1 << requirements.len()) {
                assert_eq!(
                    mask_join(&requirements, union_of | other),
                    mask_join(&requirements, union_of).join(mask_join(&requirements, other))
                );
            }
        }
    }

    /// The join of the sub-multiset of `requirements` selected by
    /// `mask`.
    fn mask_join<S: Copy + Ord>(requirements: &[Requirement<S>], mask: u32) -> Requirement<S> {
        Requirement::join_all(
            requirements
                .iter()
                .enumerate()
                .filter(|&(index, _)| mask & (1 << index) != 0)
                .map(|(_, &requirement)| requirement),
        )
    }

    /// L2: `(Req_L, ⊑, ⊔)` is a bounded join-semilattice - `⊔` is the
    /// least upper bound; associative, commutative, idempotent, with
    /// `unconstrained` as identity and `forbidden` absorbing - and
    /// `⨆` is well-defined on finite multisets with `⨆∅ =
    /// unconstrained` and `⨆(S ∪ S') = ⨆S ⊔ ⨆S'`.
    #[test]
    fn l2_join_is_a_bounded_semilattice() {
        check_l2_bounded_semilattice(&CStandard::ALL);
        check_l2_bounded_semilattice(&CxxStandard::ALL);
    }

    fn check_l3_sat_characterization<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
        let bottom = levels[0];
        let degenerate = (Requirement::Min(bottom), Requirement::Unconstrained);
        for &r1 in &all_requirements(levels) {
            for &r2 in &all_requirements(levels) {
                let sat1 = r1.sat(levels);
                let sat2 = r2.sat(levels);
                let included = sat2.iter().all(|level| sat1.contains(level));
                // (1) Soundness.
                if spec_le(r1, r2) {
                    assert!(included, "L3(1) at {r1:?}, {r2:?}");
                }
                // (2) Completeness, except the one degenerate pair.
                if included && (r1, r2) != degenerate {
                    assert!(spec_le(r1, r2), "L3(2) at {r1:?}, {r2:?}");
                }
                // (3) Induced equivalence.  `sat` filters the same
                // ordered slice, so `Vec` equality is set equality.
                let equivalent = r1 == r2 || (r1, r2) == degenerate || (r2, r1) == degenerate;
                assert_eq!(sat1 == sat2, equivalent, "L3(3) at {r1:?}, {r2:?}");
            }
        }
        // The exception is genuine: `Sat` agrees on the pair while
        // the order does not relate it.
        assert_eq!(
            Requirement::Min(bottom).sat(levels),
            Requirement::<S>::Unconstrained.sat(levels)
        );
        assert!(!spec_le(
            Requirement::Min(bottom),
            Requirement::Unconstrained
        ));
    }

    /// L3: `⊑` coincides with reverse `Sat`-inclusion - soundness,
    /// completeness up to the single degenerate pair
    /// `([⊥_L], unconstrained)`, and the induced equivalence.
    #[test]
    fn l3_sat_inclusion_characterizes_strictness() {
        check_l3_sat_characterization(&CStandard::ALL);
        check_l3_sat_characterization(&CxxStandard::ALL);
    }

    fn check_l4_join_is_intersection<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
        let requirements = all_requirements(levels);
        let intersect = |a: &[S], b: &[S]| -> Vec<S> {
            a.iter()
                .copied()
                .filter(|level| b.contains(level))
                .collect()
        };
        for &r1 in &requirements {
            for &r2 in &requirements {
                let expected = intersect(&r1.sat(levels), &r2.sat(levels));
                assert_eq!(r1.join(r2).sat(levels), expected, "L4 at {r1:?}, {r2:?}");
                // The finite generalization, on triples.
                for &r3 in &requirements {
                    assert_eq!(
                        Requirement::join_all([r1, r2, r3]).sat(levels),
                        intersect(&expected, &r3.sat(levels))
                    );
                }
            }
        }
        // The empty intersection convention is `Level_L` itself.
        assert_eq!(
            Requirement::join_all(std::iter::empty::<Requirement<S>>()).sat(levels),
            levels
        );
    }

    /// L4: `Sat(r1 ⊔ r2) = Sat(r1) ∩ Sat(r2)`, and the finite
    /// generalization with the empty join denoting all of `Level_L`.
    #[test]
    fn l4_sat_of_join_is_intersection() {
        check_l4_join_is_intersection(&CStandard::ALL);
        check_l4_join_is_intersection(&CxxStandard::ALL);
    }

    fn check_l5_antitonicity<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
        for &r1 in &all_requirements(levels) {
            for &r2 in &all_requirements(levels) {
                if !spec_le(r1, r2) {
                    continue;
                }
                for &level in levels {
                    if r2.satisfied_by(level) {
                        assert!(r1.satisfied_by(level), "L5 at {r1:?} ⊑ {r2:?}, {level:?}");
                    }
                }
            }
        }
    }

    /// L5: `satisfies` is antitone in the requirement - satisfying a
    /// stricter requirement satisfies every laxer one.
    #[test]
    fn l5_satisfies_is_antitone() {
        check_l5_antitonicity(&CStandard::ALL);
        check_l5_antitonicity(&CxxStandard::ALL);
    }

    fn check_l6_upward_closure<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
        for &requirement in &all_requirements(levels) {
            for &level in levels {
                if !requirement.satisfied_by(level) {
                    continue;
                }
                for &higher in levels {
                    if higher >= level {
                        assert!(
                            requirement.satisfied_by(higher),
                            "L6 at {requirement:?}, {level:?} ≤ {higher:?}"
                        );
                    }
                }
            }
        }
    }

    /// L6: every satisfaction set is upward closed - raising a
    /// consumer's effective level never breaks satisfaction.
    #[test]
    fn l6_satisfaction_sets_are_upward_closed() {
        check_l6_upward_closure(&CStandard::ALL);
        check_l6_upward_closure(&CxxStandard::ALL);
    }

    fn check_l7_monotone_joins<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
        let requirements = all_requirements(levels);
        // Subset claim: `S ⊆ S' ⟹ ⨆S ⊑ ⨆S'`, over every subset pair.
        for superset in 0u32..(1 << requirements.len()) {
            let mut subset = superset;
            loop {
                assert!(spec_le(
                    mask_join(&requirements, subset),
                    mask_join(&requirements, superset)
                ));
                if subset == 0 {
                    break;
                }
                subset = (subset - 1) & superset;
            }
        }
        // Pointwise claim, exhaustively for k = 2.
        for &r1 in &requirements {
            for &s1 in &requirements {
                if !spec_le(r1, s1) {
                    continue;
                }
                for &r2 in &requirements {
                    for &s2 in &requirements {
                        if spec_le(r2, s2) {
                            assert!(spec_le(r1.join(r2), s1.join(s2)));
                        }
                    }
                }
            }
        }
    }

    /// L7: set joins are monotone, both in the subset sense and
    /// pointwise.
    #[test]
    fn l7_set_joins_are_monotone() {
        check_l7_monotone_joins(&CStandard::ALL);
        check_l7_monotone_joins(&CxxStandard::ALL);
    }

    fn c_attrs(
        kind: DependencyKind,
        decl: Option<InterfaceRequirement<CStandard>>,
        implementation: Option<CStandard>,
    ) -> DependencyAttributes {
        DependencyAttributes {
            kind,
            impl_c: implementation,
            impl_cxx: None,
            decl_c: decl,
            decl_cxx: None,
        }
    }

    fn cxx_attrs(
        kind: DependencyKind,
        decl: Option<InterfaceRequirement<CxxStandard>>,
        implementation: Option<CxxStandard>,
    ) -> DependencyAttributes {
        DependencyAttributes {
            kind,
            impl_c: None,
            impl_cxx: implementation,
            decl_c: None,
            decl_cxx: decl,
        }
    }

    /// D9 row 1: the declared `"none"` is forbidden, whatever the
    /// kind and implementation say.
    #[test]
    fn d9_row_1_declared_none_is_forbidden() {
        for kind in KINDS {
            for implementation in optional_levels(&CStandard::ALL) {
                assert_eq!(
                    req_of_c_with_source(&c_attrs(
                        kind,
                        Some(InterfaceRequirement::None),
                        implementation
                    )),
                    (Requirement::Forbidden, ReqOfSource::DeclaredNone)
                );
            }
            for implementation in optional_levels(&CxxStandard::ALL) {
                assert_eq!(
                    req_of_cxx_with_source(&cxx_attrs(
                        kind,
                        Some(InterfaceRequirement::None),
                        implementation
                    )),
                    (Requirement::Forbidden, ReqOfSource::DeclaredNone)
                );
            }
        }
    }

    /// D9 row 2: an explicit declared minimum always wins - over
    /// header-only inference and over both cross-language defaults.
    #[test]
    fn d9_row_2_explicit_declaration_wins() {
        for kind in KINDS {
            for min in CStandard::ALL {
                for implementation in optional_levels(&CStandard::ALL) {
                    assert_eq!(
                        req_of_c_with_source(&c_attrs(
                            kind,
                            Some(interface_min(min)),
                            implementation
                        )),
                        (Requirement::Min(min), ReqOfSource::Declared)
                    );
                }
            }
            for min in CxxStandard::ALL {
                for implementation in optional_levels(&CxxStandard::ALL) {
                    assert_eq!(
                        req_of_cxx_with_source(&cxx_attrs(
                            kind,
                            Some(interface_min(min)),
                            implementation
                        )),
                        (Requirement::Min(min), ReqOfSource::Declared)
                    );
                }
            }
        }
    }

    /// D9 row 3: a header-only target without a declaration infers
    /// its interface minimum from its implementation standard.
    #[test]
    fn d9_row_3_header_only_inference() {
        for min in CStandard::ALL {
            assert_eq!(
                req_of_c_with_source(&c_attrs(DependencyKind::HeaderOnly, None, Some(min))),
                (Requirement::Min(min), ReqOfSource::HeaderOnlyInference)
            );
        }
        for min in CxxStandard::ALL {
            assert_eq!(
                req_of_cxx_with_source(&cxx_attrs(DependencyKind::HeaderOnly, None, Some(min))),
                (Requirement::Min(min), ReqOfSource::HeaderOnlyInference)
            );
        }
    }

    /// D9 row 4: a compiled target without a declaration imposes no
    /// constraint.
    #[test]
    fn d9_row_4_compiled_absence_is_unconstrained() {
        for implementation in CStandard::ALL {
            assert_eq!(
                req_of_c_with_source(&c_attrs(
                    DependencyKind::Compiled,
                    None,
                    Some(implementation)
                )),
                (
                    Requirement::Unconstrained,
                    ReqOfSource::CompiledNoDeclaration
                )
            );
        }
        for implementation in CxxStandard::ALL {
            assert_eq!(
                req_of_cxx_with_source(&cxx_attrs(
                    DependencyKind::Compiled,
                    None,
                    Some(implementation)
                )),
                (
                    Requirement::Unconstrained,
                    ReqOfSource::CompiledNoDeclaration
                )
            );
        }
    }

    /// D9 row 5: the permissive C-to-C++ default - no C++
    /// implementation and no declaration is consumable from any C++
    /// level.
    #[test]
    fn d9_row_5_permissive_c_to_cxx_default() {
        for kind in KINDS {
            assert_eq!(
                req_of_cxx_with_source(&cxx_attrs(kind, None, None)),
                (
                    Requirement::Unconstrained,
                    ReqOfSource::CrossLanguageDefault
                )
            );
        }
    }

    /// D9 row 6: the strict C++-to-C default - no C implementation
    /// and no declaration is not consumable from C.
    #[test]
    fn d9_row_6_strict_cxx_to_c_default() {
        for kind in KINDS {
            assert_eq!(
                req_of_c_with_source(&c_attrs(kind, None, None)),
                (Requirement::Forbidden, ReqOfSource::CrossLanguageDefault)
            );
        }
    }

    /// D13: a header-only consumer compiles no language, so every
    /// edge out of it is compatible vacuously - even against
    /// `forbidden` on both sides.
    #[test]
    fn d13_header_only_consumer_is_vacuously_compatible() {
        let header_only = ConsumerStandards { c: None, cxx: None };
        for &c in &all_requirements(&CStandard::ALL) {
            for &cxx in &all_requirements(&CxxStandard::ALL) {
                assert!(edge_compatible(
                    header_only,
                    EffectiveRequirements { c, cxx }
                ));
            }
        }
    }

    /// D14: viability is a conjunction over the version's incoming
    /// edges - vacuously true with no edges, and one incompatible
    /// edge excludes the version.
    #[test]
    fn d14_viability_is_a_conjunction_over_edges() {
        assert!(version_viable(std::iter::empty()));
        let compatible = (
            ConsumerStandards {
                c: None,
                cxx: Some(CxxStandard::Cxx20),
            },
            EffectiveRequirements {
                c: Requirement::Forbidden,
                cxx: Requirement::Min(CxxStandard::Cxx17),
            },
        );
        let incompatible = (
            ConsumerStandards {
                c: None,
                cxx: Some(CxxStandard::Cxx11),
            },
            compatible.1,
        );
        assert!(version_viable([compatible]));
        assert!(!version_viable([compatible, incompatible]));
    }

    /// Appendix reference table: `satisfies` over all of `CxxLevel`
    /// for the four requirements the worked examples use.
    #[test]
    fn appendix_reference_table_satisfies_over_cxx_levels() {
        let table: [(Requirement<CxxStandard>, [bool; 7]); 4] = [
            (Requirement::Unconstrained, [true; 7]),
            (
                Requirement::Min(CxxStandard::Cxx17),
                [false, false, false, true, true, true, true],
            ),
            (
                Requirement::Min(CxxStandard::Cxx20),
                [false, false, false, false, true, true, true],
            ),
            (Requirement::Forbidden, [false; 7]),
        ];
        for (requirement, cells) in table {
            for (level, expected) in CxxStandard::ALL.into_iter().zip(cells) {
                assert_eq!(
                    requirement.satisfied_by(level),
                    expected,
                    "{requirement:?} at {level}"
                );
            }
        }
    }

    /// Appendix example 1: a compiled C++23 implementation with a
    /// declared `c++17` interface, consumed from `c++17`.
    #[test]
    fn appendix_example_1_declared_interface_on_compiled_target() {
        let z = DependencyAttributes {
            kind: DependencyKind::Compiled,
            impl_c: None,
            impl_cxx: Some(CxxStandard::Cxx23),
            decl_c: None,
            decl_cxx: Some(interface_min(CxxStandard::Cxx17)),
        };
        // D9 row 2: the explicit declaration wins; the
        // implementation standard never enters.
        assert_eq!(req_of_cxx(&z), Requirement::Min(CxxStandard::Cxx17));
        // Contrast: absent declaration on this compiled target
        // would give unconstrained by row 4.
        let z_undeclared = DependencyAttributes {
            decl_cxx: None,
            ..z
        };
        assert_eq!(req_of_cxx(&z_undeclared), Requirement::Unconstrained);
        // R(Z) = ReqOf(Z) ⊔ ⨆∅ = [c++17] (D10's arithmetic on a
        // dependency with no public deps, composed here by hand).
        let r_z = req_of_cxx(&z).join(Requirement::join_all([]));
        assert_eq!(r_z, Requirement::Min(CxxStandard::Cxx17));
        // Edge (X, Z) at c++17 is compatible; Z's C-side forbidden
        // (row 6) is inert for a language X does not compile.
        let x = ConsumerStandards {
            c: None,
            cxx: Some(CxxStandard::Cxx17),
        };
        let z_requirements = EffectiveRequirements {
            c: req_of_c(&z),
            cxx: r_z,
        };
        assert_eq!(z_requirements.c, Requirement::Forbidden);
        assert!(edge_compatible(x, z_requirements));
        // The only edge resolving to Z's version: viable (D14).
        assert!(version_viable([(x, z_requirements)]));
    }

    /// Appendix example 2: the diamond - consumers at c++17 and
    /// c++23 sharing one dependency version; the incompatible edge
    /// poisons the version for the whole graph.
    #[test]
    fn appendix_example_2_diamond_shared_version() {
        let z = cxx_attrs(
            DependencyKind::Compiled,
            Some(interface_min(CxxStandard::Cxx20)),
            None,
        );
        let z_requirements = EffectiveRequirements {
            c: req_of_c(&z),
            cxx: req_of_cxx(&z).join(Requirement::join_all([])),
        };
        assert_eq!(z_requirements.cxx, Requirement::Min(CxxStandard::Cxx20));
        let y = ConsumerStandards {
            c: None,
            cxx: Some(CxxStandard::Cxx23),
        };
        let x = ConsumerStandards {
            c: None,
            cxx: Some(CxxStandard::Cxx17),
        };
        assert!(edge_compatible(y, z_requirements));
        assert!(!edge_compatible(x, z_requirements));
        // The (Y, Z) edge cannot rescue the version.
        assert!(!version_viable([(y, z_requirements), (x, z_requirements)]));
    }

    /// Appendix example 3: `"none"` on a transitive public
    /// dependency poisons the root; a private edge would not
    /// propagate.
    #[test]
    fn appendix_example_3_none_poisons_the_public_chain() {
        let b = cxx_attrs(
            DependencyKind::Compiled,
            Some(InterfaceRequirement::None),
            Some(CxxStandard::Cxx17),
        );
        assert_eq!(req_of_cxx(&b), Requirement::Forbidden);
        let a = cxx_attrs(DependencyKind::Compiled, None, Some(CxxStandard::Cxx17));
        assert_eq!(req_of_cxx(&a), Requirement::Unconstrained);
        // Public edge A → B: R(A) = ReqOf(A) ⊔ R(B) = forbidden -
        // L2's absorbing element; nothing recovers.
        let r_a = req_of_cxx(&a).join(req_of_cxx(&b));
        assert_eq!(r_a, Requirement::Forbidden);
        // Incompatible at every consumer level, even c++26.
        let root = ConsumerStandards {
            c: None,
            cxx: Some(CxxStandard::Cxx26),
        };
        let a_requirements = EffectiveRequirements {
            c: req_of_c(&a).join(req_of_c(&b)),
            cxx: r_a,
        };
        assert!(!edge_compatible(root, a_requirements));
        assert!(!version_viable([(root, a_requirements)]));
        // Had A → B been private, D10 would not fold R(B) in and
        // the root would be unaffected.
        let a_private = EffectiveRequirements {
            c: req_of_c(&a),
            cxx: req_of_cxx(&a),
        };
        assert!(edge_compatible(root, a_private));
    }

    /// Appendix example 4: a mixed-language consumer must satisfy
    /// every language it compiles; one failed conjunct suffices.
    #[test]
    fn appendix_example_4_mixed_language_consumer() {
        let w = DependencyAttributes {
            kind: DependencyKind::Compiled,
            impl_c: Some(CStandard::C17),
            impl_cxx: None,
            decl_c: Some(interface_min(CStandard::C17)),
            decl_cxx: None,
        };
        let w_requirements = EffectiveRequirements {
            c: req_of_c(&w),
            cxx: req_of_cxx(&w),
        };
        // D9 row 2 on the C side, row 5 (permissive default) on the
        // C++ side.
        assert_eq!(w_requirements.c, Requirement::Min(CStandard::C17));
        assert_eq!(w_requirements.cxx, Requirement::Unconstrained);
        // c11 < c17 (no equivalence special case): the C conjunct
        // fails even though the C++ conjunct passes.
        let m = ConsumerStandards {
            c: Some(CStandard::C11),
            cxx: Some(CxxStandard::Cxx20),
        };
        assert!(!edge_compatible(m, w_requirements));
        // Raising the C level to c17 or c23 fixes it (L6).
        for c in [CStandard::C17, CStandard::C23] {
            assert!(edge_compatible(
                ConsumerStandards { c: Some(c), ..m },
                w_requirements
            ));
        }
        // A C++-only consumer takes only the first conjunct.
        let cxx_only = ConsumerStandards {
            c: None,
            cxx: Some(CxxStandard::Cxx20),
        };
        assert!(edge_compatible(cxx_only, w_requirements));
        // The strict opposite direction: a compiled C++ library
        // without `interface-c-standard` fails at every C level.
        let v = cxx_attrs(DependencyKind::Compiled, None, Some(CxxStandard::Cxx20));
        let v_requirements = EffectiveRequirements {
            c: req_of_c(&v),
            cxx: req_of_cxx(&v),
        };
        assert_eq!(v_requirements.c, Requirement::Forbidden);
        for c in CStandard::ALL {
            let consumer = ConsumerStandards {
                c: Some(c),
                cxx: Some(CxxStandard::Cxx20),
            };
            assert!(!edge_compatible(consumer, v_requirements));
        }
    }

    /// Appendix example 5: header-only inference, then the explicit
    /// declaration relaxing the requirement down the chain.
    #[test]
    fn appendix_example_5_header_only_inference_then_relaxation() {
        let h = cxx_attrs(DependencyKind::HeaderOnly, None, Some(CxxStandard::Cxx20));
        // D9 row 3: the headers are the implementation.
        assert_eq!(req_of_cxx(&h), Requirement::Min(CxxStandard::Cxx20));
        let x = ConsumerStandards {
            c: None,
            cxx: Some(CxxStandard::Cxx17),
        };
        let h_requirements = EffectiveRequirements {
            c: req_of_c(&h),
            cxx: req_of_cxx(&h),
        };
        assert!(!edge_compatible(x, h_requirements));
        // The author audits the headers and declares
        // `interface-cxx-standard = "c++17"`: row 2 preempts row 3.
        let h_declared = DependencyAttributes {
            decl_cxx: Some(interface_min(CxxStandard::Cxx17)),
            ..h
        };
        assert_eq!(
            req_of_cxx(&h_declared),
            Requirement::Min(CxxStandard::Cxx17)
        );
        let declared_requirements = EffectiveRequirements {
            c: req_of_c(&h_declared),
            cxx: req_of_cxx(&h_declared),
        };
        assert!(edge_compatible(x, declared_requirements));
        // The relaxation moved down the chain (the first deliberate
        // exception in the remark after C3).
        assert!(req_of_cxx(&h_declared) <= req_of_cxx(&h));
    }
}
