//! The shell's text idioms over the shared response buffers - `grep`,
//! `sed`, `head` and `"$(cat …)"` - plus the file read and write the
//! legs that touch disk spell the same way.  One copy each: a
//! leg-local copy that drifts changes what the assertion built on it
//! means.

use std::borrow::Cow;
use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};

/// `"$(cat <file>)"`: the bytes as text with trailing newlines dropped.
pub(crate) fn capture(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches('\n')
        .to_owned()
}

/// `grep -qF`: a fixed byte substring, never a pattern.
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub(crate) fn text(bytes: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(bytes)
}

/// `grep -i '^<prefix>'` over the header block: every line whose start
/// matches case-insensitively, keeping duplicates and each line's
/// trailing CR - the shell's captures carry it into the diagnostics.
pub(crate) fn grep_lines<'a>(block: &'a str, prefix: &str) -> Vec<&'a str> {
    let prefix = prefix.to_ascii_lowercase();
    block
        .split('\n')
        .filter(|line| line.to_ascii_lowercase().starts_with(&prefix))
        .collect()
}

/// `sed 's/^[^:]*: //'`: the name and the single space after its colon,
/// and only when that space is there.
pub(crate) fn strip_name(line: &str) -> &str {
    match line.find(':') {
        Some(colon) if line[colon..].starts_with(": ") => &line[colon + 2..],
        _ => line,
    }
}

/// `grep -q '^HTTP/[^ ]* <code>'`: case-sensitive, with any version
/// token between the scheme and the code.
pub(crate) fn status_line_is(block: &str, code: u16) -> bool {
    block.split('\n').any(|line| {
        line.strip_prefix("HTTP/").is_some_and(|rest| {
            let version = rest.split(' ').next().unwrap_or_default();
            rest[version.len()..].starts_with(&format!(" {code}"))
        })
    })
}

/// `head -1`, which keeps the status line's trailing CR.
pub(crate) fn first_line(block: &str) -> &str {
    block.split('\n').next().unwrap_or_default()
}

pub(crate) fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("read {}", path.display()))
}

pub(crate) fn write(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = concat!(
        "HTTP/1.1 401 Unauthorized\r\n",
        "content-type: application/json\r\n",
        "WWW-Authenticate: Cabin login_url=\"https://cabinpkg.com/docs/remote-registry\"\r\n",
        "\r\n",
    );

    #[test]
    fn the_name_is_stripped_only_together_with_its_space() {
        assert_eq!(strip_name("www-authenticate: Cabin x"), "Cabin x");
        assert_eq!(
            strip_name("www-authenticate:Cabin"),
            "www-authenticate:Cabin"
        );
    }

    #[test]
    fn grep_keeps_every_duplicate_and_the_carriage_return() {
        let cookies = concat!(
            "HTTP/1.1 302 Found\r\n",
            "Set-Cookie: cabin_oauth_state=a; Path=/callback; HttpOnly\r\n",
            "set-cookie: cabin_oauth_state=b; Path=/; Domain=cabinpkg.com\r\n",
            "\r\n",
        );
        let lines = grep_lines(cookies, "set-cookie: cabin_oauth_state=");
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.ends_with('\r')), "{lines:?}");
        // A second cookie is what makes the host-only check fail.
        assert!(lines.join("\n").to_ascii_lowercase().contains("domain="));
    }

    #[test]
    fn the_status_line_match_spans_any_version() {
        assert!(status_line_is(BLOCK, 401));
        assert!(!status_line_is(BLOCK, 302));
        assert!(status_line_is("HTTP/2 302 Found\r\n\r\n", 302));
        assert!(!status_line_is("x-note: HTTP/1.1 302\r\n\r\n", 302));
        // The code must follow the version, not appear anywhere.
        assert!(!status_line_is("HTTP/1.1 200 302 Found\r\n", 302));
    }

    #[test]
    fn the_first_line_and_the_capture_keep_the_cr() {
        assert_eq!(first_line(BLOCK), "HTTP/1.1 401 Unauthorized\r");
        assert_eq!(capture(BLOCK.as_bytes()), BLOCK.trim_end_matches('\n'));
        assert_eq!(capture(br#"{"a":1}"#), r#"{"a":1}"#);
        assert_eq!(capture(b"body\n"), "body");
    }

    #[test]
    fn a_capture_drops_only_trailing_newlines() {
        assert_eq!(capture(b"one\ntwo\n\n"), "one\ntwo");
        assert_eq!(capture(b"head\r\n\r\n"), "head\r\n\r");
        assert_eq!(capture(b"{\"a\":1}\n\n"), r#"{"a":1}"#);
    }

    #[test]
    fn the_fixed_substring_search_is_over_bytes() {
        assert!(contains(b"a cabin_secret b", b"cabin_secret"));
        assert!(!contains(b"short", b"a much longer needle"));
        assert!(contains(
            br#"{"detail":"not a member of ghost"}"#,
            b"not a member"
        ));
        assert!(!contains(b"{}", b"not a member"));
    }
}
