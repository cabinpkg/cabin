//! Resolver-level standard-compatibility core.
//!
//! One-to-one implementation of the normative specification in
//! `docs/design/standard-compatibility/spec.md`; every public item
//! cites the definition (D1, D2, ...) or lemma (L1, ...) it
//! implements, and the tests cite the lemma they verify.  Pure data
//! and logic only: no I/O, no logging, no globals.
//!
//! The requirement domain is the interval domain of spec D3/D4:
//! joins intersect accepted ranges, an empty intersection is
//! `forbidden`, and the strictness order is a partial order - no
//! code here or downstream may assume two requirements are
//! comparable, or that one source alone determines a composed
//! value (a range's bounds may come from different targets).
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

use crate::language_standard::{
    CStandard, CxxStandard, InterfaceRequirement, LanguageStandardSettings,
    ResolvedLanguageStandards, effective_c, effective_cxx,
};
use crate::{SourceLanguage, Target, classify_source};

/// Spec D3: the per-language requirement domain `Req_L` - the
/// interval domain over the level chain.  Each value denotes the
/// set of consumer levels it accepts: everything, an up-set
/// `[min, ∞)`, an inclusive interval `[min, max]`, or nothing.
/// The strictness order `⊑` is reverse inclusion of the denoted
/// sets; it is a **partial** order (two disjoint or overlapping
/// intervals are incomparable), so there is no `Ord` here and no
/// code may assume one requirement of a pair is the stricter.
/// `Min(m)` and a `Bounded` range `[m, top]` denote the same
/// set but stay distinct shapes: a minimum-only requirement keeps
/// accepting levels a future Cabin adds above today's chain, and
/// diagnostics report exactly the declared bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Requirement<S> {
    /// Spec D3: imposes nothing on consumers.
    Unconstrained,
    /// Spec D3: `[m, ∞)` - requires a consumer level of at least
    /// the carried minimum, unbounded above.
    Min(S),
    /// Spec D3: `[min, max]` - requires a consumer level inside the
    /// inclusive range.  The payload is a validated
    /// [`BoundedRange`], so an inverted range is unrepresentable
    /// even through this public variant; the join collapses an
    /// empty intersection to [`Self::Forbidden`] instead of
    /// building one.
    Bounded(BoundedRange<S>),
    /// Spec D3: unsatisfiable.
    Forbidden,
}

/// A validated inclusive range `[min, max]`: `min <= max` holds by
/// construction (the fields are private), so every
/// [`Requirement::Bounded`] denotes a non-empty interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BoundedRange<S> {
    min: S,
    max: S,
}

impl<S: Copy + Ord> BoundedRange<S> {
    /// `None` when the range would be empty (`max < min`).
    #[must_use]
    pub fn new(min: S, max: S) -> Option<Self> {
        (min <= max).then_some(Self { min, max })
    }

    /// The inclusive lower endpoint.
    #[must_use]
    pub fn min(self) -> S {
        self.min
    }

    /// The inclusive upper endpoint.
    #[must_use]
    pub fn max(self) -> S {
        self.max
    }
}

impl<S: Copy + Ord> Requirement<S> {
    /// The declared-requirement embedding (spec D9 row 2): a
    /// minimum-only declaration is `[m, ∞)`, a bounded one the
    /// inclusive interval.
    #[must_use]
    pub fn from_declared(declared: crate::language_standard::StandardRequirement<S>) -> Self {
        match declared.max() {
            None => Self::Min(declared.min()),
            // Non-empty by `StandardRequirement`'s own invariant.
            Some(max) => Self::Bounded(BoundedRange {
                min: declared.min(),
                max,
            }),
        }
    }

    /// A validated bounded requirement; `None` when the range would
    /// be empty.
    #[must_use]
    pub fn bounded(min: S, max: S) -> Option<Self> {
        BoundedRange::new(min, max).map(Self::Bounded)
    }

    /// The lower endpoint of the denoted set, when one exists.
    #[must_use]
    pub fn lower_bound(self) -> Option<S> {
        match self {
            Self::Unconstrained | Self::Forbidden => None,
            Self::Min(min) => Some(min),
            Self::Bounded(range) => Some(range.min),
        }
    }

    /// The upper endpoint of the denoted set, when one exists.
    #[must_use]
    pub fn upper_bound(self) -> Option<S> {
        match self {
            Self::Unconstrained | Self::Min(_) | Self::Forbidden => None,
            Self::Bounded(range) => Some(range.max),
        }
    }

    /// Spec D4: the join `r1 ⊔ r2` is the **intersection** of the
    /// denoted sets - the least upper bound in the strictness order
    /// (spec L2).  An empty intersection is [`Self::Forbidden`]:
    /// composing two requirements no consumer level satisfies
    /// simultaneously forbids the edge outright.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        let (min1, max1) = match self {
            Self::Forbidden => return Self::Forbidden,
            Self::Unconstrained => (None, None),
            Self::Min(min) => (Some(min), None),
            Self::Bounded(range) => (Some(range.min), Some(range.max)),
        };
        let (min2, max2) = match other {
            Self::Forbidden => return Self::Forbidden,
            Self::Unconstrained => (None, None),
            Self::Min(min) => (Some(min), None),
            Self::Bounded(range) => (Some(range.min), Some(range.max)),
        };
        let min = match (min1, min2) {
            (None, bound) | (bound, None) => bound,
            (Some(a), Some(b)) => Some(a.max(b)),
        };
        let max = match (max1, max2) {
            (None, bound) | (bound, None) => bound,
            (Some(a), Some(b)) => Some(a.min(b)),
        };
        match (min, max) {
            (None, None) => Self::Unconstrained,
            (Some(min), None) => Self::Min(min),
            (Some(min), Some(max)) if min <= max => Self::Bounded(BoundedRange { min, max }),
            (Some(_), Some(_)) => Self::Forbidden,
            // Every shape carrying an upper bound also carries a
            // lower bound, and bounds only come from the inputs.
            (None, Some(_)) => {
                unreachable!("an upper bound only exists on Bounded, which carries a lower bound")
            }
        }
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
    /// compile level in `L` is `level` - membership in the denoted
    /// set.
    #[must_use]
    pub fn satisfied_by(self, level: S) -> bool {
        match self {
            Self::Unconstrained => true,
            Self::Min(min) => level >= min,
            Self::Bounded(range) => range.min <= level && level <= range.max,
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

impl<S: crate::language_standard::StandardLevel> std::fmt::Display for Requirement<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconstrained => f.write_str("unconstrained"),
            Self::Min(min) => write!(f, "{} or newer", min.level_str()),
            Self::Bounded(range) => {
                write!(f, "{}..{}", range.min.level_str(), range.max.level_str())
            }
            Self::Forbidden => f.write_str("none"),
        }
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
        // and over both cross-language defaults.  A bounded
        // declaration carries its inclusive `[min, max]` range into
        // the requirement.
        (Some(InterfaceRequirement::Requirement(requirement)), _) => (
            Requirement::from_declared(requirement),
            ReqOfSource::Declared,
        ),
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

/// Spec D6 attribute mapping for one target, from resolved manifest
/// attributes (target-over-package precedence and workspace
/// inheritance already applied).
///
/// Population contract (D6): `impl_L` is `Some` exactly when the
/// target itself implements `L` - a compiled target implements `L`
/// when it has sources of `L` (level via target-over-package
/// precedence), a header-only target only via a target-level
/// implementation declaration.  `decl_L` is the explicit interface
/// declaration only (target over package tier, workspace-inherited
/// counting as declared) - never the build-time implementation-
/// standard fallback.  This is the single source of truth for the
/// mapping, shared by the resolver-graph pass (`cabin-build`) and the
/// published-index derivation (`crate::index_standards`), so the two
/// cannot drift.
#[must_use]
pub fn dependency_attributes(
    target: &Target,
    package_standards: &ResolvedLanguageStandards,
    package_settings: &LanguageStandardSettings,
) -> DependencyAttributes {
    let header_only = target.kind.is_header_only();
    let kind = if header_only {
        DependencyKind::HeaderOnly
    } else {
        DependencyKind::Compiled
    };

    let has_sources_of = |language: SourceLanguage| {
        target
            .sources
            .iter()
            .any(|source| classify_source(source) == Some(language))
    };

    let impl_c = if header_only {
        target.language.c_standard_value()
    } else if has_sources_of(SourceLanguage::C) {
        effective_c(package_standards, target).map(|resolved| resolved.standard)
    } else {
        None
    };
    let impl_cxx = if header_only {
        target.language.cxx_standard_value()
    } else if has_sources_of(SourceLanguage::Cxx) {
        effective_cxx(package_standards, target).map(|resolved| resolved.standard)
    } else {
        None
    };

    // Package-level interface fields default a library's public
    // interface (`docs/language-standards.md`); they never apply to
    // executable-like targets.  Target-level interface fields only
    // exist on library-like kinds (the manifest parser rejects them
    // elsewhere).
    let library_like = target.kind.is_library_like();
    let decl_c = target.language.interface_c_standard_value().or_else(|| {
        library_like
            .then(|| package_settings.interface_c_standard_value())
            .flatten()
    });
    let decl_cxx = target.language.interface_cxx_standard_value().or_else(|| {
        library_like
            .then(|| package_settings.interface_cxx_standard_value())
            .flatten()
    });

    DependencyAttributes {
        kind,
        impl_c,
        impl_cxx,
        decl_c,
        decl_cxx,
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

/// The `[resolver] incompatible-standards` preference knob.
///
/// The value vocabulary is deliberately identical to Cargo's
/// `resolver.incompatible-rust-versions` (`allow` / `fallback`), and
/// the semantics mirror it: standards are a *version-selection
/// preference*, never a hard constraint on solvability.  See
/// `docs/design/standard-compatibility/preference-mode.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum IncompatibleStandards {
    /// Standards have no influence on version selection - selection is
    /// a pure function of semver constraints, and lockfiles never move
    /// when standards change.  Incompatibilities surface only through
    /// the post-resolution validation.  The strict/deterministic mode.
    Allow,
    /// Prefer standard-compatible versions, falling back to
    /// newest-first when no compatible candidate exists (never a
    /// resolution failure `Allow` would not also produce).  The
    /// default.
    #[default]
    Fallback,
}

impl IncompatibleStandards {
    /// Both values, for enumeration in tests and diagnostics.
    pub const ALL: [Self; 2] = [Self::Allow, Self::Fallback];

    /// The canonical spelling used in config, env vars, and messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Fallback => "fallback",
        }
    }

    /// Parse a config / env-var value.  The accepted spellings are
    /// Cargo's verbatim.
    ///
    /// # Errors
    /// Returns [`UnknownIncompatibleStandards`] for any other value.
    pub fn parse(value: &str) -> Result<Self, UnknownIncompatibleStandards> {
        match value {
            "allow" => Ok(Self::Allow),
            "fallback" => Ok(Self::Fallback),
            other => Err(UnknownIncompatibleStandards {
                value: other.to_owned(),
            }),
        }
    }
}

impl std::fmt::Display for IncompatibleStandards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An `incompatible-standards` value that is neither `allow` nor
/// `fallback`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownIncompatibleStandards {
    /// The rejected value, for the diagnostic.
    pub value: String,
}

impl std::fmt::Display for UnknownIncompatibleStandards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid incompatible-standards value {:?}; expected one of: allow, fallback",
            self.value
        )
    }
}

impl std::error::Error for UnknownIncompatibleStandards {}

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
    use crate::language_standard::{StandardLevel, StandardRequirement};

    const KINDS: [DependencyKind; 2] = [DependencyKind::Compiled, DependencyKind::HeaderOnly];

    /// Every element of `Req_L` (spec D3) for the given `Level_L`:
    /// unconstrained, every minimum, every valid inclusive range,
    /// and forbidden.
    fn all_requirements<S: Copy + Ord>(levels: &[S]) -> Vec<Requirement<S>> {
        let mut requirements = vec![Requirement::Unconstrained];
        requirements.extend(levels.iter().copied().map(Requirement::Min));
        for (index, &min) in levels.iter().enumerate() {
            for &max in &levels[index..] {
                requirements.push(Requirement::bounded(min, max).unwrap());
            }
        }
        requirements.push(Requirement::Forbidden);
        requirements
    }

    /// The semantic oracle: `Sat` as a set of levels.
    fn sat_set<S: Copy + Ord>(requirement: Requirement<S>, levels: &[S]) -> Vec<S> {
        requirement.sat(levels)
    }

    /// Spec D3's strictness order: reverse inclusion of the denoted
    /// sets.  `spec_le(r, s)` means `r ⊑ s` - `s` is at least as
    /// strict, i.e. `Sat(s) ⊆ Sat(r)`.
    fn spec_le<S: Copy + Ord>(r: Requirement<S>, s: Requirement<S>, levels: &[S]) -> bool {
        let sat_r = sat_set(r, levels);
        sat_set(s, levels).iter().all(|level| sat_r.contains(level))
    }

    fn interface_min<S>(min: S) -> InterfaceRequirement<S> {
        InterfaceRequirement::Requirement(StandardRequirement::at_least(min))
    }

    fn interface_range<S: StandardLevel>(min: S, max: S) -> InterfaceRequirement<S> {
        InterfaceRequirement::Requirement(StandardRequirement::bounded(min, Some(max)).unwrap())
    }

    /// `⊥` plus every level: the domain of `impl_L(t)` (spec D6).
    fn optional_levels<S: Copy>(levels: &[S]) -> Vec<Option<S>> {
        let mut options = vec![None];
        options.extend(levels.iter().copied().map(Some));
        options
    }

    fn intersect<S: Copy + PartialEq>(a: &[S], b: &[S]) -> Vec<S> {
        a.iter()
            .copied()
            .filter(|level| b.contains(level))
            .collect()
    }

    /// L4 (now the definition of D4): `Sat(r1 ⊔ r2) = Sat(r1) ∩
    /// Sat(r2)` - exhaustively over every requirement pair and
    /// triple of both languages, with the empty join denoting all
    /// of `Level_L`.
    #[test]
    fn join_is_intersection_of_satisfaction_sets() {
        fn check<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
            let requirements = all_requirements(levels);
            for &r1 in &requirements {
                for &r2 in &requirements {
                    let expected = intersect(&sat_set(r1, levels), &sat_set(r2, levels));
                    assert_eq!(
                        sat_set(r1.join(r2), levels),
                        expected,
                        "D4 at {r1:?}, {r2:?}"
                    );
                    // An empty intersection is forbidden, and only
                    // an empty intersection is.
                    assert_eq!(
                        r1.join(r2) == Requirement::Forbidden,
                        expected.is_empty(),
                        "empty-intersection collapse at {r1:?}, {r2:?}"
                    );
                }
            }
            assert_eq!(
                Requirement::join_all(std::iter::empty::<Requirement<S>>()).sat(levels),
                levels
            );
        }
        check(&CStandard::ALL);
        check(&CxxStandard::ALL);
    }

    /// Triple joins agree with iterated binary intersection, so the
    /// fold in `join_all` is order-independent (checked over every
    /// triple of the C chain, which already exercises all shape
    /// combinations).
    #[test]
    fn triple_joins_intersect_and_commute() {
        let levels = &CStandard::ALL;
        let requirements = all_requirements(levels);
        for &r1 in &requirements {
            for &r2 in &requirements {
                for &r3 in &requirements {
                    let expected = intersect(
                        &intersect(&sat_set(r1, levels), &sat_set(r2, levels)),
                        &sat_set(r3, levels),
                    );
                    let joined = Requirement::join_all([r1, r2, r3]);
                    assert_eq!(sat_set(joined, levels), expected);
                    assert_eq!(Requirement::join_all([r3, r1, r2]), joined);
                }
            }
        }
    }

    /// L2: `(Req_L, ⊑, ⊔)` is a bounded join-semilattice on the
    /// quotient by `Sat`-equality: `⊔` is associative, commutative,
    /// idempotent, `unconstrained` is the identity and `forbidden`
    /// absorbing - all literal shape equalities here, because the
    /// structural join is deterministic.
    #[test]
    fn join_is_a_bounded_semilattice() {
        fn check<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
            let requirements = all_requirements(levels);
            for &r in &requirements {
                assert_eq!(r.join(r), r);
                assert_eq!(Requirement::Unconstrained.join(r), r);
                assert_eq!(r.join(Requirement::Unconstrained), r);
                assert_eq!(Requirement::Forbidden.join(r), Requirement::Forbidden);
                assert_eq!(r.join(Requirement::Forbidden), Requirement::Forbidden);
                for &q in &requirements {
                    assert_eq!(r.join(q), q.join(r), "commutativity at {r:?}, {q:?}");
                    for &t in &requirements {
                        assert_eq!(
                            r.join(q).join(t),
                            r.join(q.join(t)),
                            "associativity at {r:?}, {q:?}, {t:?}"
                        );
                    }
                }
            }
        }
        check(&CStandard::ALL);
        // Pairwise laws also hold on the longer C++ chain; the
        // associativity triple is O(n^6) there, so the C chain
        // carries the exhaustive triple.
        let requirements = all_requirements(&CxxStandard::ALL);
        for &r in &requirements {
            assert_eq!(r.join(r), r);
            for &q in &requirements {
                assert_eq!(r.join(q), q.join(r));
            }
        }
    }

    /// The join is the least upper bound in the strictness order
    /// (up to `Sat`-equality): an upper bound of both operands, and
    /// below every other upper bound.
    #[test]
    fn join_is_the_least_upper_bound() {
        let levels = &CStandard::ALL;
        let requirements = all_requirements(levels);
        for &r1 in &requirements {
            for &r2 in &requirements {
                let join = r1.join(r2);
                assert!(spec_le(r1, join, levels), "upper bound at {r1:?}, {r2:?}");
                assert!(spec_le(r2, join, levels), "upper bound at {r1:?}, {r2:?}");
                for &upper in &requirements {
                    if spec_le(r1, upper, levels) && spec_le(r2, upper, levels) {
                        assert!(
                            spec_le(join, upper, levels),
                            "leastness at {r1:?}, {r2:?}, {upper:?}"
                        );
                    }
                }
            }
        }
    }

    /// The strictness order is genuinely partial now: disjoint (and
    /// merely overlapping) ranges are incomparable, so no code may
    /// pick "the stricter" of two requirements.
    #[test]
    fn strictness_is_a_partial_order_with_incomparable_ranges() {
        let levels = &CxxStandard::ALL;
        let low = Requirement::bounded(CxxStandard::Cxx11, CxxStandard::Cxx14).unwrap();
        let high = Requirement::bounded(CxxStandard::Cxx20, CxxStandard::Cxx23).unwrap();
        assert!(!spec_le(low, high, levels));
        assert!(!spec_le(high, low, levels));
        // Their join is the empty intersection.
        assert_eq!(low.join(high), Requirement::Forbidden);

        let overlapping = Requirement::bounded(CxxStandard::Cxx14, CxxStandard::Cxx20).unwrap();
        let shifted = Requirement::bounded(CxxStandard::Cxx17, CxxStandard::Cxx26).unwrap();
        assert!(!spec_le(overlapping, shifted, levels));
        assert!(!spec_le(shifted, overlapping, levels));
        assert_eq!(
            overlapping.join(shifted),
            Requirement::bounded(CxxStandard::Cxx17, CxxStandard::Cxx20).unwrap()
        );
    }

    /// L5 (antitonicity, restated on `Sat`): if `Sat(r2) ⊆ Sat(r1)`
    /// then satisfying `r2` satisfies `r1` - exhaustive.
    #[test]
    fn satisfying_a_stricter_requirement_satisfies_a_laxer_one() {
        fn check<S: Copy + Ord + std::fmt::Debug>(levels: &[S]) {
            let requirements = all_requirements(levels);
            for &r1 in &requirements {
                for &r2 in &requirements {
                    if !spec_le(r1, r2, levels) {
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
        check(&CStandard::ALL);
        check(&CxxStandard::ALL);
    }

    /// Satisfaction sets are convex, and upward closure fails
    /// exactly for bounded shapes capped below the top of the chain
    /// (spec L6).  This is the deliberate reversal of the old
    /// minimum-only lemma - remedies must not unconditionally say
    /// "raise your standard".
    #[test]
    fn satisfaction_is_convex_and_upward_closure_fails_only_below_the_top() {
        fn convex<S: Copy + Ord>(requirement: Requirement<S>, levels: &[S]) -> bool {
            levels.iter().all(|&lo| {
                levels.iter().all(|&hi| {
                    !requirement.satisfied_by(lo)
                        || !requirement.satisfied_by(hi)
                        || levels
                            .iter()
                            .filter(|&&x| lo <= x && x <= hi)
                            .all(|&x| requirement.satisfied_by(x))
                })
            })
        }

        fn upward_closed<S: Copy + Ord>(requirement: Requirement<S>, levels: &[S]) -> bool {
            levels.iter().all(|&level| {
                !requirement.satisfied_by(level)
                    || levels
                        .iter()
                        .filter(|&&higher| higher >= level)
                        .all(|&higher| requirement.satisfied_by(higher))
            })
        }

        let capped = Requirement::bounded(CxxStandard::Cxx11, CxxStandard::Cxx14).unwrap();
        assert!(capped.satisfied_by(CxxStandard::Cxx14));
        assert!(!capped.satisfied_by(CxxStandard::Cxx17));
        assert!(!capped.satisfied_by(CxxStandard::Cxx98));
        assert_eq!(
            capped.sat(&CxxStandard::ALL),
            [CxxStandard::Cxx11, CxxStandard::Cxx14]
        );
        assert!(!upward_closed(capped, &CxxStandard::ALL));

        // Every non-bounded shape stays upward closed - including
        // forbidden (vacuously) - and so does a range reaching the
        // top of today's chain.
        assert!(upward_closed(
            Requirement::<CxxStandard>::Unconstrained,
            &CxxStandard::ALL
        ));
        assert!(upward_closed(
            Requirement::Min(CxxStandard::Cxx23),
            &CxxStandard::ALL
        ));
        assert!(upward_closed(
            Requirement::<CxxStandard>::Forbidden,
            &CxxStandard::ALL
        ));
        assert!(upward_closed(
            Requirement::bounded(CxxStandard::Cxx17, CxxStandard::Cxx26).unwrap(),
            &CxxStandard::ALL
        ));

        // Exhaustive convexity, both languages: anything between two
        // accepted levels is accepted.
        for requirement in all_requirements(&CStandard::ALL) {
            assert!(convex(requirement, &CStandard::ALL), "{requirement:?}");
        }
        for requirement in all_requirements(&CxxStandard::ALL) {
            assert!(convex(requirement, &CxxStandard::ALL), "{requirement:?}");
        }
    }

    /// The declared-requirement embedding (D9 row 2): minimum-only
    /// declarations become `Min`, bounded ones `Bounded`, and the
    /// bounds surface through the accessors.
    #[test]
    fn from_declared_embeds_both_shapes() {
        let min_only = Requirement::from_declared(StandardRequirement::at_least(CStandard::C11));
        assert_eq!(min_only, Requirement::Min(CStandard::C11));
        assert_eq!(min_only.lower_bound(), Some(CStandard::C11));
        assert_eq!(min_only.upper_bound(), None);

        let bounded = Requirement::from_declared(
            StandardRequirement::bounded(CStandard::C99, Some(CStandard::C17)).unwrap(),
        );
        assert_eq!(
            bounded,
            Requirement::bounded(CStandard::C99, CStandard::C17).unwrap()
        );
        assert_eq!(bounded.lower_bound(), Some(CStandard::C99));
        assert_eq!(bounded.upper_bound(), Some(CStandard::C17));
        assert_eq!(Requirement::<CStandard>::Unconstrained.lower_bound(), None);
        assert_eq!(Requirement::<CStandard>::Forbidden.upper_bound(), None);
    }

    /// The human rendering the diagnostics embed.
    #[test]
    fn requirement_display_names_the_accepted_range() {
        assert_eq!(
            Requirement::<CxxStandard>::Unconstrained.to_string(),
            "unconstrained"
        );
        assert_eq!(
            Requirement::Min(CxxStandard::Cxx17).to_string(),
            "c++17 or newer"
        );
        assert_eq!(
            Requirement::bounded(CxxStandard::Cxx11, CxxStandard::Cxx20)
                .unwrap()
                .to_string(),
            "c++11..c++20"
        );
        assert_eq!(Requirement::<CStandard>::Forbidden.to_string(), "none");
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

    /// D9 row 2: a bounded declaration carries its inclusive range
    /// into the requirement, for both kinds and languages.
    #[test]
    fn d9_row_2_bounded_declaration_becomes_a_bounded_requirement() {
        for kind in KINDS {
            assert_eq!(
                req_of_c_with_source(&c_attrs(
                    kind,
                    Some(interface_range(CStandard::C99, CStandard::C17)),
                    Some(CStandard::C11),
                )),
                (
                    Requirement::bounded(CStandard::C99, CStandard::C17).unwrap(),
                    ReqOfSource::Declared
                )
            );
            assert_eq!(
                req_of_cxx_with_source(&cxx_attrs(
                    kind,
                    Some(interface_range(CxxStandard::Cxx11, CxxStandard::Cxx20)),
                    None,
                )),
                (
                    Requirement::bounded(CxxStandard::Cxx11, CxxStandard::Cxx20).unwrap(),
                    ReqOfSource::Declared
                )
            );
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

    /// `IncompatibleStandards` round-trips Cargo's verbatim vocabulary
    /// and rejects anything else; `fallback` is the default.
    #[test]
    fn incompatible_standards_parses_cargo_vocabulary() {
        assert_eq!(
            IncompatibleStandards::default(),
            IncompatibleStandards::Fallback
        );
        for value in IncompatibleStandards::ALL {
            assert_eq!(IncompatibleStandards::parse(value.as_str()), Ok(value));
        }
        let err = IncompatibleStandards::parse("warn").unwrap_err();
        assert_eq!(err.value, "warn");
        assert!(err.to_string().contains("allow, fallback"));
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
        // The relaxation widened the accepted set (the first
        // deliberate exception in the remark after C3).
        assert!(spec_le(
            req_of_cxx(&h_declared),
            req_of_cxx(&h),
            &CxxStandard::ALL
        ));
    }
}
