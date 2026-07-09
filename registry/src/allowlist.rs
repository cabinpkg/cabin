//! Parsing of the `ALLOWED_GITHUB_IDS` sign-in allowlist.

/// Parses the comma-separated numeric GitHub user ids allowed to sign in
/// (`wrangler.jsonc`, `ALLOWED_GITHUB_IDS`). Entries are trimmed, and
/// empty entries - including the whole variable being empty, meaning
/// nobody - are skipped.
///
/// # Panics
///
/// On any non-numeric entry: that is a deployment mistake (most likely a
/// login name where the numeric id belongs), and refusing to start beats
/// silently locking everyone out - or worse, guessing.
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

#[cfg(test)]
mod tests {
    use super::*;

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
