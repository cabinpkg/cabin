//! Conversion between `semver::VersionReq` and `PubGrub`'s
//! [`Ranges<semver::Version>`].
//!
//! `PubGrub` reasons about version sets via the
//! [`VersionSet`](pubgrub::VersionSet) trait, which on
//! [`Ranges`] expects mathematical intervals. Cabin's manifest
//! and index syntax is `semver::VersionReq`, a conjunction of
//! `Comparator`s. This module translates each comparator into
//! the equivalent interval on the totally-ordered
//! `semver::Version` space and intersects them, so a single
//! `VersionReq` round-trips through `PubGrub` without losing
//! constraint information.
//!
//! ## Pre-release boundary
//!
//! `semver::VersionReq::matches` excludes pre-release versions
//! unless one of the comparators carries the same
//! `major.minor.patch` with a non-empty `pre` tag — a filter
//! that does not map cleanly onto the [`Ranges`] interval
//! algebra. The resolver applies the equivalent exclusion at
//! candidate-selection time (see `provider::DependencyProvider`),
//! so the ranges produced here describe the *numeric* interval
//! and the candidate filter handles the pre-release rule.

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

/// Convert a [`VersionReq`] into the [`Ranges<Version>`] that
/// represents the same numeric interval (pre-release rule
/// excluded — see module docs).
///
/// An empty requirement (`VersionReq::parse("")` is rejected by
/// semver, but `VersionReq::default()` == `*`) maps to
/// [`Ranges::full`].
pub(crate) fn req_to_range(req: &VersionReq) -> Ranges<Version> {
    if req.comparators.is_empty() {
        return Ranges::full();
    }
    let mut range = Ranges::full();
    for cmp in &req.comparators {
        range = range.intersection(&comparator_to_range(cmp));
    }
    range
}

/// Convert one [`Comparator`] into its [`Ranges<Version>`] form.
///
/// The translations follow the
/// [`semver::Op`](semver::Op) documentation — the same source
/// of truth as `semver::VersionReq::matches`. Partial versions
/// (e.g. `=I.J`) widen to the closed-open interval
/// `[I.J.0, I.(J+1).0)`, matching semver's documented
/// equivalences.
fn comparator_to_range(cmp: &Comparator) -> Ranges<Version> {
    match cmp.op {
        Op::Exact | Op::Wildcard => exact_range(cmp),
        Op::Greater => greater_range(cmp),
        Op::GreaterEq => greater_eq_range(cmp),
        Op::Less => less_range(cmp),
        Op::LessEq => less_eq_range(cmp),
        Op::Tilde => tilde_range(cmp),
        Op::Caret => caret_range(cmp),
        // `semver::Op` is `#[non_exhaustive]`. Any new variant
        // would be silently misinterpreted here, so collapse to
        // the universal range and let the candidate filter run
        // the original `matches` check.
        _ => Ranges::full(),
    }
}

fn exact_range(cmp: &Comparator) -> Ranges<Version> {
    match (cmp.minor, cmp.patch) {
        (Some(minor), Some(patch)) => {
            Ranges::singleton(version(cmp.major, minor, patch, cmp.pre.clone()))
        }
        (Some(minor), None) => Ranges::between(
            version(cmp.major, minor, 0, Prerelease::EMPTY),
            version(cmp.major, minor.saturating_add(1), 0, Prerelease::EMPTY),
        ),
        (None, _) => Ranges::between(
            version(cmp.major, 0, 0, Prerelease::EMPTY),
            version(cmp.major.saturating_add(1), 0, 0, Prerelease::EMPTY),
        ),
    }
}

fn greater_range(cmp: &Comparator) -> Ranges<Version> {
    match (cmp.minor, cmp.patch) {
        (Some(minor), Some(patch)) => {
            Ranges::strictly_higher_than(version(cmp.major, minor, patch, cmp.pre.clone()))
        }
        (Some(minor), None) => Ranges::higher_than(version(
            cmp.major,
            minor.saturating_add(1),
            0,
            Prerelease::EMPTY,
        )),
        (None, _) => Ranges::higher_than(version(
            cmp.major.saturating_add(1),
            0,
            0,
            Prerelease::EMPTY,
        )),
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
        (Some(minor), Some(patch)) => {
            Ranges::lower_than(version(cmp.major, minor, patch, cmp.pre.clone()))
        }
        (Some(minor), None) => Ranges::strictly_lower_than(version(
            cmp.major,
            minor.saturating_add(1),
            0,
            Prerelease::EMPTY,
        )),
        (None, _) => Ranges::strictly_lower_than(version(
            cmp.major.saturating_add(1),
            0,
            0,
            Prerelease::EMPTY,
        )),
    }
}

fn tilde_range(cmp: &Comparator) -> Ranges<Version> {
    // `~I.J.K` = `>=I.J.K, <I.(J+1).0`
    // `~I.J`   = `=I.J`
    // `~I`     = `=I`
    match (cmp.minor, cmp.patch) {
        (Some(minor), Some(patch)) => Ranges::between(
            version(cmp.major, minor, patch, cmp.pre.clone()),
            version(cmp.major, minor.saturating_add(1), 0, Prerelease::EMPTY),
        ),
        _ => exact_range(cmp),
    }
}

fn caret_range(cmp: &Comparator) -> Ranges<Version> {
    // `^I.J.K` (I>0)        = `>=I.J.K, <(I+1).0.0`
    // `^0.J.K` (J>0)        = `>=0.J.K, <0.(J+1).0`
    // `^0.0.K`              = `=0.0.K`
    // `^I.J`   (I>0 or J>0) = `^I.J.0`
    // `^0.0`                = `=0.0`
    // `^I`                  = `=I`
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
            Ranges::between(
                version(major, minor, 0, Prerelease::EMPTY),
                upper_caret(major, minor, 0),
            )
        } else {
            exact_range(cmp)
        };
    };
    if major == 0 && minor == 0 && patch == 0 && cmp.pre.is_empty() {
        return Ranges::singleton(version(0, 0, 0, Prerelease::EMPTY));
    }
    Ranges::between(
        version(major, minor, patch, cmp.pre.clone()),
        upper_caret(major, minor, patch),
    )
}

/// Compute the (exclusive) upper bound of a caret requirement
/// per semver's "first nonzero component" rule.
fn upper_caret(major: u64, minor: u64, patch: u64) -> Version {
    if major > 0 {
        version(major.saturating_add(1), 0, 0, Prerelease::EMPTY)
    } else if minor > 0 {
        version(0, minor.saturating_add(1), 0, Prerelease::EMPTY)
    } else {
        version(0, 0, patch.saturating_add(1), Prerelease::EMPTY)
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
        let range = req_to_range(&parsed);
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
        let r1 = req_to_range(&req(">=1.0.0"));
        let r2 = req_to_range(&req("<2.0.0"));
        let inter = r1.intersection(&r2);
        let compound = req_to_range(&req(">=1.0.0, <2.0.0"));
        assert_eq!(inter, compound);
    }

    /// Pre-release versions sit numerically inside their
    /// surrounding range; the resolver excludes them at
    /// candidate-selection time. The range itself reports them
    /// as contained because the underlying interval algebra is
    /// purely numeric.
    #[test]
    fn range_includes_prerelease_numerically() {
        // `>=1.0.0, <2.0.0` includes `1.5.0-alpha` numerically.
        let range = req_to_range(&req(">=1.0.0, <2.0.0"));
        assert!(range.contains(&ver("1.5.0-alpha")));
        // semver's `matches` excludes it.
        assert!(!req(">=1.0.0, <2.0.0").matches(&ver("1.5.0-alpha")));
    }

    /// An exact requirement carrying a pre-release tag yields
    /// a singleton range containing exactly that version.
    #[test]
    fn exact_prerelease_singleton_is_exact() {
        let range = req_to_range(&req("=1.0.0-alpha"));
        assert!(range.contains(&ver("1.0.0-alpha")));
        assert!(!range.contains(&ver("1.0.0-beta")));
        assert!(!range.contains(&ver("1.0.0")));
    }
}
