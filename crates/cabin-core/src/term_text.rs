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

/// Escape terminal control characters in a whole rendered report, keeping
/// its line feeds and tabs.
///
/// For a final report Cabin writes to the terminal, whose line breaks are
/// layout. A newline inside quoted third-party text therefore survives, so
/// text that must stay on one line still needs [`escape_control_chars`] at
/// the boundary that owns it.
#[must_use]
pub fn escape_control_chars_keeping_lines(value: &str) -> String {
    value
        .split_inclusive(['\n', '\t'])
        .map(|piece| {
            let text = piece.strip_suffix(['\n', '\t']).unwrap_or(piece);
            escape_control_chars(text) + &piece[text.len()..]
        })
        .collect()
}

/// Make serialized JSON safe to print in a terminal without changing
/// what it decodes to.
///
/// `serde_json` escapes C0 controls but writes DEL, C1 controls (such as
/// the single-byte CSI, U+009B) and bidi controls raw. Each of those can
/// occur only inside a JSON string - JSON's structure and whitespace are
/// ASCII below DEL - so rewriting them as `\uXXXX` escapes leaves every
/// decoded value intact. `json` must be a serialized JSON document.
#[must_use]
pub fn escape_json_for_terminal(json: &str) -> String {
    use std::fmt::Write as _;
    let mut escaped = String::with_capacity(json.len());
    for ch in json.chars() {
        if (ch.is_control() && ch > '\u{1f}') || is_bidi_control(ch) {
            let _ = write!(escaped, "\\u{:04x}", u32::from(ch));
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
    use super::{
        escape_control_chars, escape_control_chars_keeping_lines, escape_json_for_terminal,
    };

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
    fn report_escaping_keeps_line_feeds_and_tabs() {
        assert_eq!(
            escape_control_chars_keeping_lines("a\tb\n\u{1b}[2K\r\n"),
            "a\tb\n\\u{1b}[2K\\r\n"
        );
    }

    #[test]
    fn json_escaping_keeps_every_decoded_value() {
        let hostile = [
            "\u{1b}[2J",
            "\u{1b}]8;;https://example.com\u{7}link\u{1b}]8;;\u{7}",
            "\u{9b}2J\u{85}\u{7f}",
            "\u{202e}gpj.exe",
            "a\rb\nc\td",
            "パッケージ café",
        ];
        let value =
            serde_json::json!({ "flags": hostile, "nested": { "\u{202e}key": hostile[2] } });
        for json in [
            serde_json::to_string(&value).unwrap(),
            serde_json::to_string_pretty(&value).unwrap(),
        ] {
            let escaped = escape_json_for_terminal(&json);
            assert!(
                !escaped.chars().any(|ch| ch == '\u{1b}'
                    || ch == '\u{7f}'
                    || ('\u{80}'..='\u{9f}').contains(&ch)
                    || ch == '\u{202e}'),
                "{escaped}"
            );
            assert!(escaped.contains("パッケージ café"));
            let decoded: serde_json::Value = serde_json::from_str(&escaped).unwrap();
            assert_eq!(decoded, value);
        }
    }

    #[test]
    fn json_without_controls_is_unchanged() {
        let json = serde_json::to_string_pretty(
            &serde_json::json!({ "name": "acme/demo", "dir": "/tmp/パッケージ" }),
        )
        .unwrap();
        assert_eq!(escape_json_for_terminal(&json), json);
    }

    #[test]
    fn escaping_is_idempotent() {
        let once = escape_control_chars("\u{1b}]0;title\u{7}");
        assert_eq!(escape_control_chars(&once), once);
    }
}
