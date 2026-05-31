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
#[must_use]
pub fn caret_upper_bound(major: u64, minor: u64, patch: u64) -> (u64, u64, u64) {
    if major > 0 {
        (major.saturating_add(1), 0, 0)
    } else if minor > 0 {
        (0, minor.saturating_add(1), 0)
    } else {
        (0, 0, patch.saturating_add(1))
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
        assert_eq!(caret_upper_bound(1, 2, 3), (2, 0, 0));
        // major zero, minor nonzero ⇒ bump minor
        assert_eq!(caret_upper_bound(0, 2, 3), (0, 3, 0));
        // major and minor zero ⇒ bump patch
        assert_eq!(caret_upper_bound(0, 0, 3), (0, 0, 4));
        assert_eq!(caret_upper_bound(0, 0, 0), (0, 0, 1));
    }
}
