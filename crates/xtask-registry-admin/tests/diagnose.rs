//! The diagnostics bundle's offline half: the wrangler-answer parsing
//! and the CLI surface.  The live D1 reads need an authenticated
//! operator, which no test has.

use assert_cmd::Command;
use xtask_registry_admin::{key_value, results};

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

/// A malformed answer that read as zero rows would print a section with
/// no rows and a final `diagnose OK`.
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
/// carrying neither is not the answer this asked for.  Skipping such a
/// row would hide it, so the bundle reports it instead.
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

/// The two D1 reads differ in what an empty answer means: the
/// service-state loop over an empty result prints nothing and passes,
/// while the counts section fails. `results` therefore hands back the
/// empty list rather than deciding for either caller.
#[test]
fn an_empty_result_reaches_the_caller_to_judge() {
    let empty = results(r#"[{"results":[],"success":true}]"#).unwrap();
    assert!(empty.is_empty(), "the counts section refuses this");
}

/// The drill takes no arguments, so an environment argument refuses
/// rather than being ignored while the drill acts on the sole
/// deployment.
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
