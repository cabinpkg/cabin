//! The diagnostics bundle's offline half: the wrangler-answer parsing
//! and the CLI surface.  The live D1 reads need an authenticated
//! operator, which no test has.

use assert_cmd::Command;
use xtask_registry_admin::{display, key_value, results};

/// A D1 answer carries JSON types, and the shell printed them through
/// `${row.value}`: a string keeps its own text, an array joins with
/// `,` (D1 hands back a BLOB that way), everything else takes its JSON
/// form.  Printing `"7"` where D1 answered `7`, or `[1,2]` where the
/// shell printed `1,2`, would make a diagnostics bundle disagree with
/// the database it describes.
#[test]
fn values_print_as_the_shell_printed_them() {
    let answer = r#"[{"results":[
        {"key":"service_mode","value":"normal"},
        {"key":"total_stored_bytes","value":4096},
        {"key":"launched","value":false},
        {"key":"a_blob","value":[1,2,255]},
        {"key":"a_sparse_blob","value":[1,null,3]},
        {"key":"last_backup_at","value":null}
    ],"success":true}]"#;
    let rows = results(answer).unwrap();
    let printed: Vec<String> = rows
        .iter()
        .map(|row| format!("{}: {}", display(&row["key"]), display(&row["value"])))
        .collect();
    assert_eq!(
        printed,
        [
            "service_mode: normal",
            "total_stored_bytes: 4096",
            "launched: false",
            "a_blob: 1,2,255",
            "a_sparse_blob: 1,,3",
            "last_backup_at: null",
        ]
    );
}

/// The counts section prints one line per column, and the operator
/// reads them against the SQL that produced them - so they must arrive
/// in the SELECT's order, not a hash order.
#[test]
fn count_columns_keep_their_select_order() {
    let answer =
        r#"[{"results":[{"users":3,"scopes":2,"packages":9,"versions":11}],"success":true}]"#;
    let rows = results(answer).unwrap();
    let order: Vec<&str> = rows[0].keys().map(String::as_str).collect();
    assert_eq!(order, ["users", "scopes", "packages", "versions"]);
}

/// Fail loudly, never as an empty bundle: a malformed answer that read
/// as zero rows would print a section with no rows and a final
/// `diagnose OK`, which is the one thing a diagnostics tool must not
/// do.
#[test]
fn a_malformed_answer_is_an_error_not_an_empty_result() {
    for answer in [
        "not json at all",
        "{}",
        "[]",
        r#"[{"success":true}]"#,
        r#"[{"results":"nope"}]"#,
        r#"[{"results":["not an object"]}]"#,
    ] {
        assert!(results(answer).is_err(), "accepted {answer}");
    }
}

/// A service-state row is `SELECT key, value FROM meta`, so a row
/// carrying neither is not the answer this asked for.  The shell
/// printed `undefined: undefined` and still finished `diagnose OK`;
/// skipping such a row would hide it even better.  The bundle reports
/// it instead.
#[test]
fn a_row_that_is_not_a_key_value_pair_is_an_error() {
    let rows = results(r#"[{"results":[{"name":"service_mode"}],"success":true}]"#).unwrap();
    assert!(key_value(&rows[0]).is_err());

    let rows =
        results(r#"[{"results":[{"key":"launched","value":null}],"success":true}]"#).unwrap();
    assert_eq!(
        key_value(&rows[0]).unwrap(),
        ("launched".to_owned(), "null".to_owned()),
        "a null value is an answer, not a missing pair"
    );
}

#[test]
fn the_command_is_required() {
    Command::cargo_bin("xtask-registry-admin")
        .unwrap()
        .assert()
        .failure();

    Command::cargo_bin("xtask-registry-admin")
        .unwrap()
        .arg("nuke")
        .assert()
        .failure()
        .stderr(predicates::str::contains("nuke"));

    Command::cargo_bin("xtask-registry-admin")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("cargo registry-diagnose"));
}

/// The two D1 reads differ in what an empty answer means, as the
/// shell's two `node` snippets did: the service-state loop over an
/// empty result printed nothing and passed, while the counts snippet
/// hit `Object.entries(undefined)` and failed. `results` therefore
/// hands back the empty list rather than deciding for either caller.
#[test]
fn an_empty_result_reaches_the_caller_to_judge() {
    let empty = results(r#"[{"results":[],"success":true}]"#).unwrap();
    assert!(empty.is_empty(), "the counts section refuses this");
}

/// The drill takes no arguments: the pre-cutover form took an
/// environment argument, and silently acting on the sole remaining
/// deployment is the one thing it must not do.
#[test]
fn the_restore_drill_takes_no_arguments() {
    let admin = || Command::cargo_bin("xtask-registry-admin").unwrap();
    admin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("cargo registry-restore-drill"));
    for refused in [
        vec!["restore-drill", "production"],
        vec!["restore-drill", "--keys"],
    ] {
        admin()
            .args(&refused)
            .assert()
            .failure()
            .stderr(predicates::str::contains("unexpected argument"));
    }
}
