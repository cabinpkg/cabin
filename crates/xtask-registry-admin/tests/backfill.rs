//! The backfill's offline half.  The copy loop itself needs an
//! authenticated operator and two live buckets, which no test has.

use xtask_registry_admin::backfill::is_checksum;
use xtask_registry_admin::{Nullish, column_lines};

/// The backfill refuses any checksum the shell's `^[0-9a-f]{64}$` would
/// have refused: the value becomes an R2 key it writes to, so an
/// unexpected answer must stop the run rather than name an object.
#[test]
fn the_backfill_takes_only_lower_case_sha256() {
    assert!(is_checksum(&format!(
        "sha256:{}",
        "0123456789abcdef".repeat(4)
    )));
    for refused in [
        "",
        // The bare pre-prefix spelling no longer matches the column.
        &"0123456789abcdef".repeat(4),
        &format!("sha256:{}", "a".repeat(63)),
        &format!("sha256:{}", "a".repeat(65)),
        &format!("sha256:{}", "A".repeat(64)),
        &format!("SHA256:{}", "a".repeat(64)),
        &format!("sha256:{}", "z".repeat(64)),
        &format!("sha256:{} ", "a".repeat(63)),
        &format!("sha256:{}", "a".repeat(32).repeat(2).replace('a', "á")),
    ] {
        assert!(!is_checksum(refused), "accepted {refused:?}");
    }
}

/// `d1_column` was `console.log(row[column])` piped through `$(...)`
/// and `while IFS= read -r`, which is NOT the `${...}` coercion the
/// diagnostics bundle used.  The lines it produced - and *when* a bad
/// one stopped the run - are the behavior.
#[test]
fn the_enumeration_splits_as_the_pipeline_split_it() {
    let lines = |json: &str| {
        column_lines(
            &xtask_registry_admin::results(json).unwrap(),
            "checksum",
            Nullish::Printed,
        )
    };

    assert_eq!(
        lines(r#"[{"results":[{"checksum":"a"},{"checksum":"b"}],"success":true}]"#),
        ["a", "b"]
    );
    // An empty enumeration still fed the loop the here-string's one
    // blank line, which the loop skipped.
    assert_eq!(lines(r#"[{"results":[],"success":true}]"#), [""]);
    // `$(...)` strips trailing newlines; the rest still split.
    assert_eq!(
        lines(r#"[{"results":[{"checksum":"a\n"}],"success":true}]"#),
        ["a"]
    );
    assert_eq!(
        lines(r#"[{"results":[{"checksum":"a\nb"}],"success":true}]"#),
        ["a", "b"]
    );

    // A non-string renders to a line the checksum grammar refuses,
    // rather than stopping the run before it starts: the shell copied
    // every good row that came first, and so must this.
    let mixed = lines(&format!(
        r#"[{{"results":[{{"checksum":"sha256:{}"}},{{"checksum":["b"]}}],"success":true}}]"#,
        "a".repeat(64)
    ));
    assert_eq!(mixed.len(), 2);
    assert!(is_checksum(&mixed[0]), "the good row still comes first");
    assert!(
        !is_checksum(&mixed[1]),
        "the bad row is refused in the loop"
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
        assert!(!is_checksum(rendered));
    }

    // Bash cannot hold a NUL: command substitution dropped it, so this
    // reached the shell's grammar as 64 hex digits and was copied.
    let split = format!("sha256:{}\0{}", "a".repeat(32), "a".repeat(32));
    let answer = serde_json::json!([{"results": [{"checksum": split}], "success": true}]);
    let joined = lines(&answer.to_string());
    assert_eq!(joined, [format!("sha256:{}", "a".repeat(64))]);
    assert!(is_checksum(&joined[0]));
}
