//! Conversion between `semver::VersionReq` and `PubGrub`'s
//! [`Ranges<semver::Version>`].
//!
//! `PubGrub` reasons about version sets via the
//! [`VersionSet`](pubgrub::VersionSet) trait, which on
//! [`Ranges`] expects mathematical intervals.  Cabin's manifest
//! and index syntax is `semver::VersionReq`, a conjunction of
//! `Comparator`s.  This module translates each comparator into
//! the equivalent interval on the totally-ordered
//! `semver::Version` space and intersects them, so a single
//! `VersionReq` round-trips through `PubGrub` without losing
//! constraint information.
//!
//! ## Pre-release boundary
//!
//! `semver::VersionReq::matches` excludes pre-release versions
//! unless one of the comparators carries the same
//! `major.minor.patch` with a non-empty `pre` tag - a filter
//! that does not map cleanly onto the [`Ranges`] interval
//! algebra.  The resolver applies the equivalent exclusion at
//! candidate-selection time (see `provider::DependencyProvider`),
//! so the ranges produced here describe the *numeric* interval
//! and the candidate filter handles the pre-release rule.
//!
//! ## Build-metadata boundary
//!
//! `matches` ignores build metadata, but `semver::Version`'s total
//! order does not: `1.2.3+meta` sorts strictly above `1.2.3`.  A
//! comparator boundary that falls exactly on a full version must
//! therefore extend past that version's build-metadata variants -
//! otherwise `=1.2.3` would exclude `1.2.3+meta` and `>1.2.3` would
//! include it, disagreeing with `matches`.  Boundary comparators use
//! the [`just_above_all_builds`] successor as their bound; the
//! successor carries a marker build tag so the pre-release candidate
//! filter can tell synthetic bounds from comparator-derived ones.
//!
//! One documented approximation remains: a compound requirement that
//! itself places an `I.J.(K+1)-0` pre-release bound exactly at a
//! full-version boundary's successor can disagree with `matches` on
//! that single bare `-0` version, in either direction - `>I.J.K`
//! excludes it (the marker bound sorts just above it) and
//! `<=I.J.K` / `=I.J.K` admit it (their exclusive upper bound sorts
//! just above it).  The trade is deliberate: an unmarked bare
//! successor would instead admit pre-releases for every plain
//! full-version `>` requirement, and would surface pre-release-
//! carrying bounds in the complements `PubGrub` derives during
//! conflict resolution - both far likelier surfaces than an
//! explicit `-0` bound (see the
//! `synthetic_bounds_do_not_admit_prereleases` /
//! `explicit_successor_prerelease_bounds_stay_approximate` tests).

use pubgrub::Ranges;
use semver::{BuildMetadata, Comparator, Op, Prerelease, Version, VersionReq};

/// Build a [`Version`] from its parts using the empty build-metadata tag.
fn version(major: u64, minor: u64, patch: u64, pre: Prerelease) -> Version {
    Version {
        major,
        minor,
        patch,
        pre,
        build: BuildMetadata::EMPTY,
    }
}

/// Build metadata carried by synthetic boundary versions produced by
/// [`just_above_all_builds`].  Comparator-derived bounds always have
/// empty build metadata (a [`Comparator`] has no build field), so a
/// non-empty tag unambiguously marks a bound as synthetic - the
/// pre-release candidate filter must ignore such bounds (see
/// `provider::range_admits_prerelease_of`).
pub(crate) const SYNTHETIC_BOUND_BUILD: &str = "0";

/// The smallest version that sorts above `v` *and every build-metadata
/// variant of `v`* in `semver::Version`'s total order.
///
/// `semver::VersionReq::matches` ignores build metadata, so a
/// requirement boundary that falls exactly on a full version `V` must
/// treat `V+anything` like `V`.  The interval algebra cannot express
/// "V and its builds" with `V` itself as a bound (builds sort strictly
/// above the bare version), so boundary comparators use this
/// successor instead:
///
/// - for a pre-release `I.J.K-pre`, the successor is `I.J.K-pre.0` -
///   the smallest extension of the same tag.  It carries no marker:
///   it is itself a comparator-adjacent pre-release boundary, and the
///   pre-release candidate filter must keep honoring it;
/// - for a release `I.J.K`, the successor is `I.J.(K+1)-0` plus the
///   [`SYNTHETIC_BOUND_BUILD`] marker (nothing sorts between the last
///   build of `I.J.K` and the first pre-release of `I.J.(K+1)`),
///   carrying components at the `u64` ceiling.  The marker keeps the
///   pre-release filter from reading the `-0` tag as a comparator's
///   pre-release opt-in for the `I.J.(K+1)` triple.
///
/// Returns `None` when no representable successor exists
/// (`MAX.MAX.MAX`): nothing but its own builds sorts above it.
fn just_above_all_builds(v: &Version) -> Option<Version> {
    if !v.pre.is_empty() {
        let pre = Prerelease::new(&format!("{}.0", v.pre))
            .expect("appending `.0` to a valid pre-release tag stays valid");
        return Some(Version {
            major: v.major,
            minor: v.minor,
            patch: v.patch,
            pre,
            build: BuildMetadata::EMPTY,
        });
    }
    let build = BuildMetadata::new(SYNTHETIC_BOUND_BUILD)
        .expect("the synthetic bound tag is valid build metadata");
    let zero = Prerelease::new("0").expect("`0` is a valid pre-release tag");
    let (major, minor, patch) = match v.patch.checked_add(1) {
        Some(patch) => (v.major, v.minor, patch),
        None => match v.minor.checked_add(1) {
            Some(minor) => (v.major, minor, 0),
            None => (v.major.checked_add(1)?, 0, 0),
        },
    };
    Some(Version {
        major,
        minor,
        patch,
        pre: zero,
        build,
    })
}

/// Failure produced when a [`VersionReq`] uses a comparator
/// operator this build of `cabin-resolver` does not know how to
/// translate into a [`Ranges<Version>`].
///
/// Kept crate-private; call sites map it onto
/// [`crate::error::ResolveError::UnsupportedVersionRequirement`]
/// with the package context they have.
#[derive(Debug)]
pub(crate) struct RangeConversionError {
    /// Textual form of the requirement that failed to convert,
    /// captured eagerly so the error message stays readable even
    /// after the source [`VersionReq`] is dropped.
    pub(crate) requirement: String,
}

/// Convert a [`VersionReq`] into the [`Ranges<Version>`] that
/// represents the same numeric interval (pre-release rule
/// excluded - see module docs).
///
/// An empty requirement (`VersionReq::parse("")` is rejected by
/// semver, but `VersionReq::default()` == `*`) maps to
/// [`Ranges::full`].  Returns [`RangeConversionError`] when any
/// comparator uses an operator this resolver build cannot
/// translate (see [`comparator_to_range`]).
pub(crate) fn req_to_range(req: &VersionReq) -> Result<Ranges<Version>, RangeConversionError> {
    if req.comparators.is_empty() {
        return Ok(Ranges::full());
    }
    let mut range = Ranges::full();
    for cmp in &req.comparators {
        let cmp_range = comparator_to_range(cmp).ok_or_else(|| RangeConversionError {
            requirement: req.to_string(),
        })?;
        range = range.intersection(&cmp_range);
    }
    Ok(range)
}

/// Convert one [`Comparator`] into its [`Ranges<Version>`] form.
///
/// The translations follow the [`semver::Op`] documentation -
/// the same source of truth as `semver::VersionReq::matches`.
/// Partial versions (e.g. `=I.J`) widen to the closed-open
/// interval `[I.J.0, I.(J+1).0)`, matching semver's documented
/// equivalences.
///
/// Returns `None` for operators this build of `cabin-resolver`
/// does not recognize. `semver::Op` is `#[non_exhaustive]`, so a
/// future semver release may add variants this match does not
/// cover; falling back to [`Ranges::full`] would silently widen
/// the constraint into an unconstrained dependency.  The caller
/// surfaces the `None` as
/// [`crate::error::ResolveError::UnsupportedVersionRequirement`]
/// instead.
fn comparator_to_range(cmp: &Comparator) -> Option<Ranges<Version>> {
    Some(match cmp.op {
        Op::Exact | Op::Wildcard => exact_range(cmp),
        Op::Greater => greater_range(cmp),
        Op::GreaterEq => greater_eq_range(cmp),
        Op::Less => less_range(cmp),
        Op::LessEq => less_eq_range(cmp),
        Op::Tilde => tilde_range(cmp),
        Op::Caret => caret_range(cmp),
        _ => return None,
    })
}

fn exact_range(cmp: &Comparator) -> Ranges<Version> {
    match (cmp.minor, cmp.patch) {
        // `=I.J.K` covers the named version *and its build-metadata
        // variants* (`matches` ignores build metadata), so the range
        // is the closed-open interval up to the synthetic successor
        // rather than a singleton that would exclude `I.J.K+meta`.
        (Some(minor), Some(patch)) => {
            let exact = version(cmp.major, minor, patch, cmp.pre.clone());
            let upper = just_above_all_builds(&exact);
            between_or_unbounded(exact, upper)
        }
        // `=I.J` ≡ `[I.J.0, I.(J+1).0)`; the next-series bound carries
        // past a `u64`-ceiling component instead of saturating.
        (Some(minor), None) => between_or_unbounded(
            version(cmp.major, minor, 0, Prerelease::EMPTY),
            next_minor_series(cmp.major, minor),
        ),
        // `=I` ≡ `[I.0.0, (I+1).0.0)`.
        (None, _) => between_or_unbounded(
            version(cmp.major, 0, 0, Prerelease::EMPTY),
            next_major_series(cmp.major),
        ),
    }
}

fn greater_range(cmp: &Comparator) -> Ranges<Version> {
    match (cmp.minor, cmp.patch) {
        // `>I.J.K` excludes the named version's build-metadata
        // variants too (they compare equal under `matches`), so the
        // bound is the synthetic successor, not the bare version
        // (which sorts below its own builds).
        (Some(minor), Some(patch)) => {
            let exact = version(cmp.major, minor, patch, cmp.pre.clone());
            higher_than_or_empty(just_above_all_builds(&exact))
        }
        // `>I.J` excludes the whole `I.J` series, i.e. `>= I.(J+1).0`.
        (Some(minor), None) => higher_than_or_empty(next_minor_series(cmp.major, minor)),
        // `>I` excludes the whole `I` major series, i.e. `>= (I+1).0.0`.
        (None, _) => higher_than_or_empty(next_major_series(cmp.major)),
    }
}

fn greater_eq_range(cmp: &Comparator) -> Ranges<Version> {
    match (cmp.minor, cmp.patch) {
        (Some(minor), Some(patch)) => {
            Ranges::higher_than(version(cmp.major, minor, patch, cmp.pre.clone()))
        }
        (Some(minor), None) => Ranges::higher_than(version(cmp.major, minor, 0, Prerelease::EMPTY)),
        (None, _) => Ranges::higher_than(version(cmp.major, 0, 0, Prerelease::EMPTY)),
    }
}

fn less_range(cmp: &Comparator) -> Ranges<Version> {
    match (cmp.minor, cmp.patch) {
        (Some(minor), Some(patch)) => {
            Ranges::strictly_lower_than(version(cmp.major, minor, patch, cmp.pre.clone()))
        }
        (Some(minor), None) => {
            Ranges::strictly_lower_than(version(cmp.major, minor, 0, Prerelease::EMPTY))
        }
        (None, _) => Ranges::strictly_lower_than(version(cmp.major, 0, 0, Prerelease::EMPTY)),
    }
}

fn less_eq_range(cmp: &Comparator) -> Ranges<Version> {
    match (cmp.minor, cmp.patch) {
        // `<=I.J.K` includes the named version's build-metadata
        // variants (equal under `matches`), so the range runs up to
        // the synthetic successor exclusively.
        (Some(minor), Some(patch)) => {
            let exact = version(cmp.major, minor, patch, cmp.pre.clone());
            strictly_lower_than_or_full(just_above_all_builds(&exact))
        }
        // `<=I.J` includes the whole `I.J` series, i.e. `< I.(J+1).0`.
        (Some(minor), None) => strictly_lower_than_or_full(next_minor_series(cmp.major, minor)),
        // `<=I` includes the whole `I` major series, i.e. `< (I+1).0.0`.
        (None, _) => strictly_lower_than_or_full(next_major_series(cmp.major)),
    }
}

fn tilde_range(cmp: &Comparator) -> Ranges<Version> {
    // `~I.J.K` = `>=I.J.K, <I.(J+1).0`
    // `~I.J` = `=I.J`
    // `~I` = `=I`
    match (cmp.minor, cmp.patch) {
        (Some(minor), Some(patch)) => between_or_unbounded(
            version(cmp.major, minor, patch, cmp.pre.clone()),
            next_minor_series(cmp.major, minor),
        ),
        _ => exact_range(cmp),
    }
}

fn caret_range(cmp: &Comparator) -> Ranges<Version> {
    // `^I.J.K` (I>0) = `>=I.J.K, <(I+1).0.0`
    // `^0.J.K` (J>0) = `>=0.J.K, <0.(J+1).0`
    // `^0.0.K` = `=0.0.K`
    // `^I.J` (I>0 or J>0) = `^I.J.0`
    // `^0.0` = `=0.0`
    // `^I` = `=I`
    let major = cmp.major;
    // `^I` == `=I`.
    let Some(minor) = cmp.minor else {
        return exact_range(cmp);
    };
    let Some(patch) = cmp.patch else {
        // `^I.J` with at least one nonzero component widens to
        // the next caret bound; `^0.0` collapses to the exact
        // form.
        return if major > 0 || minor > 0 {
            between_or_unbounded(
                version(major, minor, 0, Prerelease::EMPTY),
                upper_caret(major, minor, 0),
            )
        } else {
            exact_range(cmp)
        };
    };
    if major == 0 && minor == 0 && patch == 0 && cmp.pre.is_empty() {
        // `^0.0.0` covers `0.0.0` and its build-metadata variants,
        // like the exact form.
        let exact = version(0, 0, 0, Prerelease::EMPTY);
        let upper = just_above_all_builds(&exact);
        return between_or_unbounded(exact, upper);
    }
    between_or_unbounded(
        version(major, minor, patch, cmp.pre.clone()),
        upper_caret(major, minor, patch),
    )
}

/// Compute the (exclusive) upper bound of a caret requirement
/// per semver's "first nonzero component" rule, as a [`Version`].
/// The bump itself is the shared
/// [`cabin_core::version_req::caret_upper_bound`] kernel so the
/// resolver and `cabin-system-deps` agree on the zero-major /
/// zero-minor cases.  `None` when the major is at the `u64` ceiling
/// and no representable upper bound exists - see
/// [`between_or_unbounded`].
fn upper_caret(major: u64, minor: u64, patch: u64) -> Option<Version> {
    cabin_core::version_req::caret_upper_bound(major, minor, patch)
        .map(|(major, minor, patch)| version(major, minor, patch, Prerelease::EMPTY))
}

/// The exclusive start of the next minor series - `major.(minor+1).0`
/// - carrying a minor at the `u64` ceiling into the next major
///   (`I.MAX` ⇒ `(I+1).0.0`).  `None` when the major is also saturated,
///   so no representable version sits above the `major.minor` series.
fn next_minor_series(major: u64, minor: u64) -> Option<Version> {
    match minor.checked_add(1) {
        Some(m) => Some(version(major, m, 0, Prerelease::EMPTY)),
        None => next_major_series(major),
    }
}

/// The exclusive start of the next major series - `(major+1).0.0`.
/// `None` when the major is at the `u64` ceiling.
fn next_major_series(major: u64) -> Option<Version> {
    major
        .checked_add(1)
        .map(|m| version(m, 0, 0, Prerelease::EMPTY))
}

/// A closed-open interval `[lower, upper)`, or the open-above range
/// `[lower, ∞)` when `upper` is not representable - a series bound
/// past the `u64` ceiling (`=MAX`, `~MAX.MAX.K`, `^MAX.J.K`).  Nothing
/// sorts above `MAX.MAX.MAX`, so dropping the unrepresentable upper is
/// exact, whereas the empty interval saturation would produce is wrong.
fn between_or_unbounded(lower: Version, upper: Option<Version>) -> Ranges<Version> {
    match upper {
        Some(upper) => Ranges::between(lower, upper),
        None => Ranges::higher_than(lower),
    }
}

/// The open-above range `[lower, ∞)`, or the empty range when `lower`
/// is not representable.  A strict lower bound past the `u64` ceiling
/// (`>MAX`, `>MAX.MAX`) has no version above it, so the requirement is
/// unsatisfiable.
fn higher_than_or_empty(lower: Option<Version>) -> Ranges<Version> {
    match lower {
        Some(lower) => Ranges::higher_than(lower),
        None => Ranges::empty(),
    }
}

/// The open-below range `(-∞, upper)`, or the full range when `upper`
/// is not representable.  An inclusive upper bound past the `u64`
/// ceiling (`<=MAX`, `<=MAX.MAX`) admits every version.
fn strictly_lower_than_or_full(upper: Option<Version>) -> Ranges<Version> {
    match upper {
        Some(upper) => Ranges::strictly_lower_than(upper),
        None => Ranges::full(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(s: &str) -> VersionReq {
        VersionReq::parse(s).unwrap()
    }

    fn ver(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    /// The translated range and `semver::VersionReq::matches`
    /// must agree on every version listed here, modulo the
    /// pre-release rule (which is enforced by the candidate
    /// filter, not the range).
    fn assert_matches(req_str: &str, samples: &[(&str, bool)]) {
        let parsed = req(req_str);
        let range = req_to_range(&parsed).expect("supported requirement converts");
        for (sample, expected) in samples {
            let v = ver(sample);
            assert_eq!(
                range.contains(&v),
                *expected,
                "range mismatch for `{req_str}` against `{sample}`: \
                 range={range}",
            );
        }
    }

    #[test]
    fn caret_full_version() {
        assert_matches(
            "^1.2.3",
            &[
                ("1.2.3", true),
                ("1.2.4", true),
                ("1.9.0", true),
                ("2.0.0", false),
                ("1.2.2", false),
                ("0.9.9", false),
            ],
        );
    }

    #[test]
    fn caret_zero_major() {
        assert_matches(
            "^0.1.3",
            &[
                ("0.1.3", true),
                ("0.1.9", true),
                ("0.2.0", false),
                ("0.1.2", false),
            ],
        );
    }

    #[test]
    fn caret_zero_major_and_minor() {
        assert_matches(
            "^0.0.3",
            &[("0.0.3", true), ("0.0.4", false), ("0.0.2", false)],
        );
    }

    #[test]
    fn caret_minor_at_u64_ceiling_carries_to_next_major() {
        // `^0.MAX.0` ≡ `>=0.MAX.0, <1.0.0`: the saturated minor
        // carries into the next major instead of collapsing the
        // interval to the empty range a bare bump would produce.
        let max = u64::MAX;
        assert_matches(
            &format!("^0.{max}.0"),
            &[
                (&format!("0.{max}.0"), true),
                (&format!("0.{max}.99"), true),
                ("1.0.0", false),
            ],
        );
    }

    #[test]
    fn caret_patch_at_u64_ceiling_carries_to_next_minor() {
        // `^0.0.MAX` ≡ `>=0.0.MAX, <0.1.0`.
        let max = u64::MAX;
        assert_matches(
            &format!("^0.0.{max}"),
            &[(&format!("0.0.{max}"), true), ("0.1.0", false)],
        );
    }

    #[test]
    fn caret_major_at_u64_ceiling_is_unbounded_above() {
        // `^MAX.J.K` has no representable upper bound, so the range
        // stays open above (every version `>= MAX.J.K`) rather than
        // collapsing to the empty interval saturation would give.
        let max = u64::MAX;
        assert_matches(
            &format!("^{max}.5.7"),
            &[
                (&format!("{max}.5.7"), true),
                (&format!("{max}.9.9"), true),
                (&format!("{max}.5.6"), false),
            ],
        );
    }

    #[test]
    fn exact_partial_at_u64_ceiling_is_unbounded_above() {
        // `=MAX` / `=MAX.MAX` have no representable upper bound, so
        // they stay open above rather than collapsing to empty.
        let max = u64::MAX;
        assert_matches(
            &format!("={max}"),
            &[
                (&format!("{max}.0.0"), true),
                (&format!("{max}.9.9"), true),
                ("1.0.0", false),
            ],
        );
        assert_matches(
            &format!("=1.{max}"),
            &[
                (&format!("1.{max}.0"), true),
                (&format!("1.{max}.{max}"), true),
                ("2.0.0", false),
            ],
        );
    }

    #[test]
    fn wildcard_at_u64_ceiling_is_unbounded_above() {
        // `MAX.*` matches the whole MAX major series, open above.
        let max = u64::MAX;
        assert_matches(
            &format!("{max}.*"),
            &[(&format!("{max}.0.0"), true), (&format!("{max}.9.9"), true)],
        );
    }

    #[test]
    fn greater_partial_at_u64_ceiling_carries_or_empties() {
        // `>1.MAX` carries to `>= 2.0.0`; `>MAX` matches nothing.
        let max = u64::MAX;
        assert_matches(
            &format!(">1.{max}"),
            &[("2.0.0", true), (&format!("1.{max}.{max}"), false)],
        );
        assert_matches(
            &format!(">{max}"),
            &[(&format!("{max}.{max}.{max}"), false), ("1.0.0", false)],
        );
    }

    #[test]
    fn less_eq_partial_at_u64_ceiling_carries_or_fulls() {
        // `<=1.MAX` carries to `< 2.0.0`; `<=MAX` admits everything.
        let max = u64::MAX;
        assert_matches(
            &format!("<=1.{max}"),
            &[(&format!("1.{max}.{max}"), true), ("2.0.0", false)],
        );
        assert_matches(
            &format!("<={max}"),
            &[("0.0.0", true), (&format!("{max}.{max}.{max}"), true)],
        );
    }

    #[test]
    fn tilde_minor_at_u64_ceiling_carries_to_next_major() {
        // `~1.MAX.3` carries to `< 2.0.0` instead of an empty range.
        let max = u64::MAX;
        assert_matches(
            &format!("~1.{max}.3"),
            &[
                (&format!("1.{max}.3"), true),
                (&format!("1.{max}.{max}"), true),
                ("2.0.0", false),
            ],
        );
    }

    #[test]
    fn caret_major_only_and_full_agree_at_u64_ceiling() {
        // `^MAX` ≡ `^MAX.0.0` per semver; both stay open above now
        // that neither saturates into an empty range. `^MAX` routes
        // through `exact_range`, `^MAX.0.0` through the caret kernel -
        // this pins that they no longer diverge.
        let max = u64::MAX;
        let major_only = req_to_range(&req(&format!("^{max}"))).unwrap();
        let full = req_to_range(&req(&format!("^{max}.0.0"))).unwrap();
        assert_eq!(major_only, full);
    }

    /// The strongest guarantee this module owes: the translated range
    /// agrees with `semver::VersionReq::matches` on every non-pre
    /// version.  Crossing every operator in normal *and* `u64`-ceiling
    /// forms against boundary versions both validates the ceiling
    /// carry logic and proves the fix is ceiling-only - for any
    /// non-`MAX` input, `saturating_add == checked_add`, so behavior
    /// is byte-identical to before.
    #[test]
    fn ranges_agree_with_semver_matches_including_u64_ceiling() {
        let max = u64::MAX;
        let reqs: Vec<String> = vec![
            // Normal forms: no component at the ceiling, so the carry
            // path never fires and the result is identical to the old
            // saturating code.
            "=1.2.3".into(),
            "=1.2".into(),
            "=1".into(),
            "1.2.*".into(),
            "1.*".into(),
            "*".into(),
            ">1.2.3".into(),
            ">1.2".into(),
            ">1".into(),
            ">=1.2.3".into(),
            ">=1.2".into(),
            "<1.2.3".into(),
            "<1.2".into(),
            "<=1.2.3".into(),
            "<=1.2".into(),
            "<=1".into(),
            "~1.2.3".into(),
            "~1.2".into(),
            "~1".into(),
            "^1.2.3".into(),
            "^0.2.3".into(),
            "^0.0.3".into(),
            "^1.2".into(),
            "^0.0".into(),
            "^1".into(),
            // `u64`-ceiling forms across every operator that computes a
            // next-series bound.
            format!("=1.{max}"),
            format!("={max}.{max}"),
            format!("={max}"),
            format!("{max}.*"),
            format!("1.{max}.*"),
            format!(">1.{max}"),
            format!(">{max}.{max}"),
            format!(">{max}"),
            format!("<=1.{max}"),
            format!("<={max}.{max}"),
            format!("<={max}"),
            format!("~1.{max}.3"),
            format!("~{max}.{max}.5"),
            format!("^0.{max}.5"),
            format!("^0.0.{max}"),
            format!("^{max}.5.7"),
            format!("^{max}"),
            format!("^{max}.0.0"),
        ];
        let samples: Vec<String> = vec![
            "0.0.0".into(),
            "0.0.1".into(),
            "0.1.0".into(),
            "0.2.0".into(),
            "1.0.0".into(),
            "1.2.2".into(),
            "1.2.3".into(),
            "1.2.4".into(),
            "1.3.0".into(),
            "1.9.9".into(),
            "2.0.0".into(),
            "3.0.0".into(),
            format!("0.{max}.0"),
            format!("0.{max}.{max}"),
            format!("1.{max}.0"),
            format!("1.{max}.{max}"),
            format!("{max}.0.0"),
            format!("{max}.5.6"),
            format!("{max}.5.7"),
            format!("{max}.{max}.0"),
            format!("{max}.{max}.{max}"),
        ];
        for req_str in &reqs {
            let parsed = req(req_str);
            let range = req_to_range(&parsed).expect("supported requirement converts");
            for s in &samples {
                let v = ver(s);
                assert_eq!(
                    range.contains(&v),
                    parsed.matches(&v),
                    "range/semver disagree for `{req_str}` at `{s}`: range={range}",
                );
            }
        }
    }

    /// Build metadata is invisible to `matches`, so every boundary
    /// comparator must treat `V+meta` exactly like `V`.  These are
    /// the cases the old singleton / bare-version bounds got wrong.
    #[test]
    fn build_metadata_is_ignored_at_full_version_boundaries() {
        assert_matches(
            "=1.2.3",
            &[
                ("1.2.3", true),
                ("1.2.3+cabin.1", true),
                ("1.2.3+zzz", true),
                ("1.2.4", false),
                ("1.2.2+cabin.9", false),
            ],
        );
        assert_matches(
            "<=1.2.3",
            &[
                ("1.2.3+cabin.1", true),
                ("1.2.4", false),
                ("1.2.4+a", false),
            ],
        );
        assert_matches(
            ">1.2.3",
            &[("1.2.3+cabin.1", false), ("1.2.4", true), ("1.2.4+a", true)],
        );
        assert_matches("<1.2.3", &[("1.2.3+cabin.1", false), ("1.2.2+a", true)]);
        assert_matches(">=1.2.3", &[("1.2.3+cabin.1", true), ("1.2.2+zzz", false)]);
        assert_matches(
            "^1.2.3",
            &[
                ("1.2.3+cabin.1", true),
                ("1.9.9+b", true),
                ("2.0.0+b", false),
            ],
        );
        assert_matches("~1.2.3", &[("1.2.99+cabin.2", true), ("1.3.0+b", false)]);
        assert_matches("^0.0.0", &[("0.0.0+cabin.1", true), ("0.0.1", false)]);
        // The exact pre-release form keeps admitting its own builds
        // and nothing else.
        assert_matches(
            "=1.0.0-alpha",
            &[
                ("1.0.0-alpha", true),
                ("1.0.0-alpha+b", true),
                ("1.0.0-alpha.0", false),
                ("1.0.0-beta", false),
                ("1.0.0", false),
            ],
        );
    }

    /// The synthetic successor bound must not read as a pre-release
    /// opt-in: a plain full-version boundary comparator admits no
    /// pre-release of the successor triple, while a user-authored
    /// pre-release comparator at that triple keeps admitting.
    #[test]
    fn synthetic_bounds_do_not_admit_prereleases() {
        use crate::provider::candidate_admits_prerelease;

        let plain = req_to_range(&req(">1.2.3")).unwrap();
        assert!(!candidate_admits_prerelease(&plain, &ver("1.2.4-0")));
        assert!(!candidate_admits_prerelease(&plain, &ver("1.2.4-alpha")));

        let opted_in = req_to_range(&req(">=1.2.4-0")).unwrap();
        assert!(candidate_admits_prerelease(&opted_in, &ver("1.2.4-0")));
        assert!(candidate_admits_prerelease(&opted_in, &ver("1.2.4-alpha")));
    }

    /// Documented approximation: a compound requirement that places
    /// an explicit `I.J.(K+1)-0` pre-release bound right at a
    /// full-version boundary comparator's synthetic successor can
    /// disagree with `matches` (the marker bound sorts just above
    /// the bare `-0`).  Accepted deliberately: dropping the marker
    /// would instead wrongly admit pre-releases for every plain
    /// full-version `>` requirement (see
    /// [`synthetic_bounds_do_not_admit_prereleases`]), a far
    /// likelier requirement shape than an explicit `-0` bound.
    #[test]
    fn explicit_successor_prerelease_bounds_stay_approximate() {
        let range = req_to_range(&req(">1.2.3, >=1.2.4-0")).unwrap();
        let boundary = ver("1.2.4-0");
        assert!(req(">1.2.3, >=1.2.4-0").matches(&boundary));
        assert!(!range.contains(&boundary));
        // One identifier deeper, the approximation ends.
        assert!(range.contains(&ver("1.2.4-0.0")));
        assert!(range.contains(&ver("1.2.4")));

        // The mirror direction: `<=` (and `=`) place their exclusive
        // upper bound just above the bare successor, so the same
        // pathological intersection retains it where `matches`
        // rejects it.  Same single-version approximation, accepted
        // for the same reason.
        for req_str in ["<=1.2.3, >=1.2.4-0", "=1.2.3, >=1.2.4-0"] {
            let range = req_to_range(&req(req_str)).unwrap();
            assert!(!req(req_str).matches(&boundary), "{req_str}");
            assert!(range.contains(&boundary), "{req_str}: {range}");
            // Nothing but that bare boundary version diverges.
            assert!(!range.contains(&ver("1.2.4-0.0")), "{req_str}");
            assert!(!range.contains(&ver("1.2.4")), "{req_str}");
        }
    }

    /// The agreement guarantee extended to build-carrying samples:
    /// the translated range and `matches` agree everywhere `matches`
    /// ignores the build tag.
    #[test]
    fn ranges_agree_with_semver_matches_on_build_metadata() {
        let reqs = [
            "=1.2.3", "=1.2", "=1", ">1.2.3", ">=1.2.3", "<1.2.3", "<=1.2.3", "~1.2.3", "^1.2.3",
            "^0.0.3", "^0.0.0", "1.2.*", "*",
        ];
        let samples = [
            "1.2.2+m",
            "1.2.3+m",
            "1.2.3+cabin.10",
            "1.2.4+m",
            "1.3.0+m",
            "2.0.0+m",
            "0.0.0+m",
            "0.0.3+m",
        ];
        for req_str in reqs {
            let parsed = req(req_str);
            let range = req_to_range(&parsed).expect("supported requirement converts");
            for s in samples {
                let v = ver(s);
                assert_eq!(
                    range.contains(&v),
                    parsed.matches(&v),
                    "range/semver disagree for `{req_str}` at `{s}`: range={range}",
                );
            }
        }
    }

    #[test]
    fn caret_major_only() {
        // `^1` == `=1` == `>=1.0.0, <2.0.0`.
        assert_matches(
            "^1",
            &[
                ("1.0.0", true),
                ("1.99.99", true),
                ("2.0.0", false),
                ("0.99.99", false),
            ],
        );
    }

    #[test]
    fn wildcard_minor() {
        assert_matches(
            "1.*",
            &[
                ("1.0.0", true),
                ("1.99.0", true),
                ("2.0.0", false),
                ("0.99.0", false),
            ],
        );
    }

    #[test]
    fn wildcard_patch() {
        assert_matches(
            "1.2.*",
            &[
                ("1.2.0", true),
                ("1.2.99", true),
                ("1.3.0", false),
                ("1.1.99", false),
            ],
        );
    }

    #[test]
    fn comparison_inclusive_exclusive() {
        assert_matches(
            ">=1.2.3, <1.5.0",
            &[
                ("1.2.3", true),
                ("1.4.99", true),
                ("1.5.0", false),
                ("1.2.2", false),
            ],
        );
    }

    #[test]
    fn comparison_strict() {
        assert_matches(
            ">1.0.0, <=2.0.0",
            &[
                ("1.0.0", false),
                ("1.0.1", true),
                ("2.0.0", true),
                ("2.0.1", false),
            ],
        );
    }

    #[test]
    fn comma_intersection() {
        // The comma form `>=10, <11` is what the existing
        // resolver test corpus uses.
        assert_matches(
            ">=10, <11",
            &[
                ("10.0.0", true),
                ("10.2.1", true),
                ("11.0.0", false),
                ("9.9.9", false),
            ],
        );
    }

    #[test]
    fn exact_full_version() {
        assert_matches(
            "=1.2.3",
            &[("1.2.3", true), ("1.2.4", false), ("1.2.2", false)],
        );
    }

    #[test]
    fn exact_partial_version() {
        assert_matches(
            "=1.2",
            &[("1.2.0", true), ("1.2.99", true), ("1.3.0", false)],
        );
    }

    #[test]
    fn tilde_full_version() {
        assert_matches(
            "~1.2.3",
            &[
                ("1.2.3", true),
                ("1.2.99", true),
                ("1.3.0", false),
                ("1.2.2", false),
            ],
        );
    }

    #[test]
    fn wildcard_star_only() {
        assert_matches("*", &[("0.0.1", true), ("1.0.0", true), ("99.99.99", true)]);
    }

    /// Verify that the algebra round-trips: intersecting two
    /// ranges agrees with the conjunction of two requirements.
    #[test]
    fn intersection_matches_compound_requirement() {
        let r1 = req_to_range(&req(">=1.0.0")).unwrap();
        let r2 = req_to_range(&req("<2.0.0")).unwrap();
        let inter = r1.intersection(&r2);
        let compound = req_to_range(&req(">=1.0.0, <2.0.0")).unwrap();
        assert_eq!(inter, compound);
    }

    /// Pre-release versions sit numerically inside their
    /// surrounding range; the resolver excludes them at
    /// candidate-selection time.  The range itself reports them
    /// as contained because the underlying interval algebra is
    /// purely numeric.
    #[test]
    fn range_includes_prerelease_numerically() {
        // `>=1.0.0, <2.0.0` includes `1.5.0-alpha` numerically.
        let range = req_to_range(&req(">=1.0.0, <2.0.0")).unwrap();
        assert!(range.contains(&ver("1.5.0-alpha")));
        // semver's `matches` excludes it.
        assert!(!req(">=1.0.0, <2.0.0").matches(&ver("1.5.0-alpha")));
    }

    /// An exact requirement carrying a pre-release tag yields
    /// a singleton range containing exactly that version.
    #[test]
    fn exact_prerelease_singleton_is_exact() {
        let range = req_to_range(&req("=1.0.0-alpha")).unwrap();
        assert!(range.contains(&ver("1.0.0-alpha")));
        assert!(!range.contains(&ver("1.0.0-beta")));
        assert!(!range.contains(&ver("1.0.0")));
    }

    /// Every currently-known [`semver::Op`] variant must take a
    /// translated path in [`comparator_to_range`]; none may fall
    /// through to the fail-closed `None` arm.  This pins the
    /// boundary so a stale `Op` value cannot be silently widened
    /// into [`Ranges::full`].
    ///
    /// The fail-closed arm itself cannot be exercised at runtime
    /// from this build because `semver::Op` is
    /// `#[non_exhaustive]` and its unknown variant cannot be
    /// constructed by downstream code.  When semver publishes a
    /// new variant, the right response is to add it to
    /// [`comparator_to_range`] (and to this list), or to ship a
    /// resolver that explicitly fails closed for it.
    #[test]
    fn every_known_op_variant_converts() {
        let supported = [
            Op::Exact,
            Op::Wildcard,
            Op::Greater,
            Op::GreaterEq,
            Op::Less,
            Op::LessEq,
            Op::Tilde,
            Op::Caret,
        ];
        for op in supported {
            let cmp = Comparator {
                op,
                major: 1,
                minor: Some(0),
                patch: Some(0),
                pre: Prerelease::EMPTY,
            };
            assert!(
                comparator_to_range(&cmp).is_some(),
                "expected known op {op:?} to convert to a range",
            );
        }
    }
}
