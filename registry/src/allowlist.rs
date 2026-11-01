//! Parsing of the `ALLOWED_GITHUB_IDS` sign-in allowlist.

/// Parses the comma-separated numeric GitHub user ids allowed to sign in
/// (`wrangler.jsonc`, `ALLOWED_GITHUB_IDS`). Entries are trimmed, and
/// empty entries - including the whole variable being empty, meaning
/// nobody - are skipped.
///
/// # Panics
///
/// On a non-numeric entry or one that overflows an `i64`: a deployment
/// mistake (most likely a login name where the numeric id belongs), and
/// refusing to start beats silently locking everyone out.
pub fn parse_allowed_ids(raw: &str) -> Vec<i64> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            assert!(
                entry.bytes().all(|b| b.is_ascii_digit()),
                "ALLOWED_GITHUB_IDS entry {entry:?} is not a numeric GitHub user id"
            );
            entry
                .parse()
                .unwrap_or_else(|_| panic!("ALLOWED_GITHUB_IDS entry {entry:?} overflows an i64"))
        })
        .collect()
}

/// The private-alpha admission rule for an authenticated caller: the
/// registry account behind the credential must hold a GitHub identity
/// (`identities.provider_account_id`, stored as decimal text) on the
/// allowlist. Checked on every use, not only at sign-in or mint, so a
/// credential issued to an account the allowlist no longer names stops
/// working at once. No identity, or one that does not parse, is refused.
#[must_use]
pub fn admits(allowed: &[i64], github_id: Option<&str>) -> bool {
    github_id
        .and_then(|id| id.parse::<i64>().ok())
        .is_some_and(|id| allowed.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_only_an_allowlisted_github_identity() {
        let allowed = parse_allowed_ids("26405363");
        assert!(admits(&allowed, Some("26405363")));
        // A valid account that is not on the list, an account with no
        // GitHub identity, and an unparsable stored id are all refused.
        assert!(!admits(&allowed, Some("583231")));
        assert!(!admits(&allowed, None));
        assert!(!admits(&allowed, Some("26405363.0")));
        assert!(!admits(&allowed, Some("")));
        // An empty allowlist admits nobody.
        assert!(!admits(&[], Some("26405363")));
    }

    #[test]
    fn parses_ids_and_tolerates_whitespace_and_empty_entries() {
        assert_eq!(parse_allowed_ids("583231"), vec![583_231]);
        assert_eq!(parse_allowed_ids(" 583231 , 42 "), vec![583_231, 42]);
        assert_eq!(parse_allowed_ids("583231,,42,"), vec![583_231, 42]);
    }

    #[test]
    fn an_empty_variable_allows_nobody() {
        assert_eq!(parse_allowed_ids(""), Vec::<i64>::new());
        assert_eq!(parse_allowed_ids("  "), Vec::<i64>::new());
    }

    #[test]
    #[should_panic(expected = "\"octocat\" is not a numeric GitHub user id")]
    fn a_login_name_panics() {
        parse_allowed_ids("583231,octocat");
    }

    #[test]
    #[should_panic(expected = "\"-1\" is not a numeric GitHub user id")]
    fn a_signed_entry_panics() {
        parse_allowed_ids("-1");
    }

    #[test]
    #[should_panic(expected = "\"99999999999999999999\" overflows an i64")]
    fn an_overflowing_entry_panics() {
        parse_allowed_ids("99999999999999999999");
    }
}
