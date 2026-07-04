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
//! not implemented here.  This module operates on already-composed
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

/// Spec D9: `ReqOf(d, C)`.  Shares rows 1-4 with the C++ side and
/// lands on row 6 - the strict C++-to-C default `forbidden` - when
/// the target neither declares a C interface nor implements C.
#[must_use]
pub fn req_of_c(dependency: &DependencyAttributes) -> Requirement<CStandard> {
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
) -> Requirement<S> {
    match (decl, implementation) {
        // Row 1: the declared `"none"` is forbidden.
        (Some(InterfaceRequirement::None), _) => Requirement::Forbidden,
        // Row 2: an explicit declaration always wins, over inference
        // and over both cross-language defaults.  `max` is reserved
        // and always absent in v1 (spec D4 remark).
        (Some(InterfaceRequirement::Requirement(requirement)), _) => {
            Requirement::Min(requirement.min)
        }
        // Row 3: header-only inference from the implementation
        // standard.  Row 4: a compiled target without an interface
        // declaration imposes no constraint.
        (None, Some(min)) => match kind {
            DependencyKind::HeaderOnly => Requirement::Min(min),
            DependencyKind::Compiled => Requirement::Unconstrained,
        },
        // Rows 5-6: the cross-language defaults.
        (None, None) => absent_default,
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
