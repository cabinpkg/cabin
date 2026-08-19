//! Making third-party text safe to print in a terminal.

/// Escape terminal control characters in text a third party supplied.
///
/// Registry-provided diagnostics are useful, but a third-party registry
/// must not be able to steer the terminal it is quoted into: ANSI escapes
/// can repaint or erase what Cabin already wrote, and a bare newline can
/// forge a second `error:` line the user reads as Cabin's own.
///
/// An error variant that escapes third-party text in its `Display` must not
/// also carry that text as a `#[source]`: the CLI's coded and plain paths
/// render errors with anyhow's `{:#}`, which re-appends every source
/// verbatim and so would undo the escape. Store such text as a plain field
/// instead, and do not name that field `source` - `thiserror` adopts a
/// field of that name even without the attribute.
///
/// The result contains no control or bidi characters, so escaping twice
/// changes nothing after the first pass - the escaper is safe to apply at
/// whichever boundary owns the text.
#[must_use]
pub fn escape_control_chars(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_control() || is_bidi_control(ch) {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

/// Unicode's bidirectional controls can reorder otherwise printable text
/// in terminal diagnostics. Keep ordinary international text intact while
/// making those invisible formatting characters explicit.
fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::escape_control_chars;

    #[test]
    fn control_and_bidi_characters_lose_their_effect() {
        assert_eq!(escape_control_chars("\u{1b}[2K"), "\\u{1b}[2K");
        assert_eq!(escape_control_chars("a\nb"), "a\\nb");
        assert_eq!(escape_control_chars("\u{202e}gpj.exe"), "\\u{202e}gpj.exe");
    }

    #[test]
    fn ordinary_text_survives_unchanged() {
        for value in ["acme/demo 1.2.3", "パッケージ", "café"] {
            assert_eq!(escape_control_chars(value), value);
        }
    }

    #[test]
    fn escaping_is_idempotent() {
        let once = escape_control_chars("\u{1b}]0;title\u{7}");
        assert_eq!(escape_control_chars(&once), once);
    }
}
