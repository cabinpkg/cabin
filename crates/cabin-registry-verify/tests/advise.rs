//! The binary's advisory mode: the proceed and abstain output
//! shapes, and the operational-failure exit for malformed inputs.
//! The mode's argument list has no archive path at all - the
//! advisories need no bytes, which is what lets the workflow run
//! them before any download.

use assert_cmd::Command;
use assert_fs::TempDir;
use assert_fs::prelude::*;

fn entry_json(name: &str) -> String {
    serde_json::json!({
        "name": name,
        "version": "1.0.0",
        "checksum": "aa",
        "published_at": "2026-07-18T00:00:00.000Z",
        "metadata": {},
    })
    .to_string()
}

fn advise(entry: &str, corpus: &str) -> assert_cmd::assert::Assert {
    let dir = TempDir::new().expect("temp dir");
    dir.child("entry.json").write_str(entry).expect("entry");
    dir.child("corpus.json").write_str(corpus).expect("corpus");
    Command::cargo_bin("cabin-registry-verify")
        .expect("binary")
        .arg("--name-advisories")
        .arg(dir.child("entry.json").path())
        .arg(dir.child("corpus.json").path())
        .assert()
}

#[test]
fn a_clean_name_proceeds() {
    advise(
        &entry_json("acme/widgets"),
        r#"{"packages":[{"scope":"fmtlib","name":"fmt","vetted":true}]}"#,
    )
    .success()
    .stdout("{\"advice\":\"proceed\"}\n");
}

#[test]
fn a_confusable_name_abstains_with_named_findings() {
    advise(
        &entry_json("fmtl1b/fmt"),
        r#"{"packages":[{"scope":"fmtlib","name":"fmt","vetted":true}]}"#,
    )
    .success()
    .stdout(
        "{\"advice\":\"abstain\",\"findings\":\
         [\"confusable_package (fmtlib/fmt)\",\"confusable_scope (fmtlib)\"]}\n",
    );
}

#[test]
fn malformed_inputs_are_operational_failures() {
    // A bare listing name and an unparsable corpus both exit 2 with
    // no advice: the version stays pending, exactly like any other
    // operational failure.
    advise(&entry_json("bare-name"), r#"{"packages":[]}"#)
        .failure()
        .code(2)
        .stdout("");
    advise(&entry_json("acme/widgets"), "not json")
        .failure()
        .code(2)
        .stdout("");
}
