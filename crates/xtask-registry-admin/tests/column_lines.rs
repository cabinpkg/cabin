//! How a D1 answer's column becomes the lines a command walks.

use xtask_registry_admin::column_lines;

/// The enumeration is NOT the `display` coercion the diagnostics
/// bundle uses.  The lines it produces are the behavior.
#[test]
fn the_enumeration_splits_the_rendered_rows_into_lines() {
    let lines =
        |json: &str| column_lines(&xtask_registry_admin::results(json).unwrap(), "checksum");

    assert_eq!(
        lines(r#"[{"results":[{"checksum":"a"},{"checksum":"b"}],"success":true}]"#),
        ["a", "b"]
    );
    // An empty enumeration still yields one blank line.
    assert_eq!(lines(r#"[{"results":[],"success":true}]"#), [""]);
    // Trailing newlines are stripped; the rest still split.
    assert_eq!(
        lines(r#"[{"results":[{"checksum":"a\n"}],"success":true}]"#),
        ["a"]
    );
    assert_eq!(
        lines(r#"[{"results":[{"checksum":"a\nb"}],"success":true}]"#),
        ["a", "b"]
    );

    // A non-string renders to a line of its own rather than stopping
    // the enumeration, so every good row that comes first is still
    // listed.
    assert_eq!(
        lines(r#"[{"results":[{"checksum":"a"},{"checksum":["b"]}],"success":true}]"#),
        ["a", r#"["b"]"#]
    );

    for (answer, rendered) in [
        (r#"[{"results":[{"checksum":7}],"success":true}]"#, "7"),
        (
            r#"[{"results":[{"checksum":null}],"success":true}]"#,
            "null",
        ),
        (
            r#"[{"results":[{"other":"a"}],"success":true}]"#,
            "undefined",
        ),
    ] {
        assert_eq!(lines(answer), [rendered]);
    }

    // NULs are dropped.
    let answer = serde_json::json!([{"results": [{"checksum": "a\0b"}], "success": true}]);
    assert_eq!(lines(&answer.to_string()), ["ab"]);
}
