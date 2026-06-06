//! Lenient `SemVer` version-requirement parsing.
//!
//! `semver::VersionReq` only accepts comma-separated comparator
//! lists. Cabin manifests and index entries follow the
//! npm-flavored form where space and comma are both accepted, so
//! the two crates that read `SemVer` requirements from disk
//! (`cabin-manifest` and `cabin-index`) used to carry an
//! identical normalization routine. They now both consume this
//! shared helper.

/// Parse `raw` as a `SemVer` requirement, accepting either comma-
/// or space-separated comparator lists. Bare operators (`>= 1.2`)
/// are rejoined with their version. Returns the original parse
/// error when the input cannot be coerced into either form so
/// callers' diagnostics keep pointing at the user's text.
///
/// # Errors
/// Returns the [`semver::Error`] from parsing `raw` when it parses neither
/// directly nor after normalization into a comma-separated comparator list.
pub fn parse_lenient(raw: &str) -> Result<semver::VersionReq, semver::Error> {
    if let Ok(req) = semver::VersionReq::parse(raw) {
        return Ok(req);
    }
    let normalized = normalize(raw);
    if normalized != raw
        && let Ok(req) = semver::VersionReq::parse(&normalized)
    {
        return Ok(req);
    }
    semver::VersionReq::parse(raw)
}

/// Convert a space-separated list of `SemVer` comparators into the
/// comma-separated form `semver::VersionReq::parse` accepts.
/// Operators detached from their version (`>= 1.2.3`) are
/// re-attached. Exposed alongside [`parse_lenient`] so callers
/// that want to display the canonical comma-separated form can
/// reuse the same normalization.
pub(crate) fn normalize(input: &str) -> String {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    let mut comparators: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i].trim_end_matches(',');
        if tok.is_empty() {
            i += 1;
            continue;
        }
        let bare_op = matches!(tok, ">=" | ">" | "<=" | "<" | "=" | "^" | "~");
        if bare_op && i + 1 < tokens.len() {
            let next = tokens[i + 1].trim_end_matches(',');
            comparators.push(format!("{tok}{next}"));
            i += 2;
            continue;
        }
        comparators.push(tok.to_owned());
        i += 1;
    }
    comparators.join(", ")
}

/// The exclusive upper bound of a caret (`^`) requirement, given a
/// fully specified `(major, minor, patch)`: bump the leftmost
/// non-zero segment and zero out everything to its right, per the
/// Cargo/npm caret rule.
///
/// This is the single source of truth shared by the two crates that
/// turn caret requirements into a concrete bound in different output
/// forms — the resolver (`PubGrub` `Ranges`) and `cabin-system-deps`
/// (pkg-config `<` strings) — so the subtle zero-major / zero-minor
/// cases cannot drift apart. Callers that allow *partial* comparators
/// (an absent minor or patch, e.g. `^0` or `^0.0`) must apply their
/// own widening policy before calling this, because those forms are
/// not expressible as a leftmost-non-zero bump of a single triple.
///
/// Returns `None` when the bump has no representable result. The
/// major is the leftmost segment, so a major already at the `u64`
/// ceiling cannot be bumped (`^MAX.J.K`) and yields `None`; callers
/// map that onto an unbounded upper (drop the `<` / leave the range
/// open above). A minor or patch at the ceiling instead carries into
/// the next-higher segment (`^0.MAX.K` ⇒ `Some((1, 0, 0))`,
/// `^0.0.MAX` ⇒ `Some((0, 1, 0))`), so the saturated major is the
/// only `None` case.
#[must_use]
pub fn caret_upper_bound(major: u64, minor: u64, patch: u64) -> Option<(u64, u64, u64)> {
    if major > 0 {
        // Bump the major; nothing sits above it, so a major already at
        // the `u64` ceiling has no representable upper bound.
        major.checked_add(1).map(|m| (m, 0, 0))
    } else if minor > 0 {
        // Bump the minor; a minor at the ceiling carries into the major.
        Some(match minor.checked_add(1) {
            Some(m) => (0, m, 0),
            None => (1, 0, 0),
        })
    } else {
        // Bump the patch; a patch at the ceiling carries into the minor.
        Some(match patch.checked_add(1) {
            Some(p) => (0, 0, p),
            None => (0, 1, 0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_comma_separated_form_unchanged() {
        let req = parse_lenient(">=1.2, <2").unwrap();
        assert!(req.matches(&semver::Version::new(1, 5, 0)));
        assert!(!req.matches(&semver::Version::new(2, 0, 0)));
    }

    #[test]
    fn parse_normalizes_space_separated_form() {
        let req = parse_lenient(">=1.2 <2").unwrap();
        assert!(req.matches(&semver::Version::new(1, 5, 0)));
        assert!(!req.matches(&semver::Version::new(2, 0, 0)));
    }

    #[test]
    fn parse_rejoins_bare_operator_and_version() {
        let req = parse_lenient(">= 1.2.3").unwrap();
        assert!(req.matches(&semver::Version::new(1, 2, 3)));
    }

    #[test]
    fn parse_propagates_original_error_for_garbage() {
        // Unparsable input must keep its original error so
        // wrapper diagnostics quote the user's text faithfully.
        let err = parse_lenient("not-a-version").unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn normalize_collapses_repeated_whitespace() {
        assert_eq!(normalize(">=1.2   <2"), ">=1.2, <2");
    }

    #[test]
    fn normalize_drops_trailing_comma_tokens() {
        assert_eq!(normalize(">=1.2, <2"), ">=1.2, <2");
    }

    #[test]
    fn caret_upper_bound_bumps_leftmost_nonzero_segment() {
        // major nonzero ⇒ bump major
        assert_eq!(caret_upper_bound(1, 2, 3), Some((2, 0, 0)));
        // major zero, minor nonzero ⇒ bump minor
        assert_eq!(caret_upper_bound(0, 2, 3), Some((0, 3, 0)));
        // major and minor zero ⇒ bump patch
        assert_eq!(caret_upper_bound(0, 0, 3), Some((0, 0, 4)));
        assert_eq!(caret_upper_bound(0, 0, 0), Some((0, 0, 1)));
    }

    #[test]
    fn caret_upper_bound_carries_or_drops_at_u64_ceiling() {
        let max = u64::MAX;
        // A patch at the ceiling carries into the next minor.
        assert_eq!(caret_upper_bound(0, 0, max), Some((0, 1, 0)));
        // A minor at the ceiling carries into the next major.
        assert_eq!(caret_upper_bound(0, max, 7), Some((1, 0, 0)));
        // A major at the ceiling has no representable upper bound.
        assert_eq!(caret_upper_bound(max, 0, 0), None);
        assert_eq!(caret_upper_bound(max, 5, 7), None);
    }
}
