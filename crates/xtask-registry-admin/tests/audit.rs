//! The audit's public surface: the account id it reads out of
//! `wrangler.jsonc` before it can list anything, and the CLI shim.
//! What a listing and a D1 answer are then *read* to mean is not this
//! crate's API, and is tested beside the code in `src/audit.rs`.

use assert_cmd::Command;
use xtask_registry_admin::declared_account_id;

/// The account id is read out of `wrangler.jsonc` because no wrangler
/// command exposes it, and the R2 REST listing needs it in the path.
/// It is matched, not parsed: the first `"CF_ACCOUNT_ID":` followed
/// by 32 lower-case hex digits in quotes, wherever it sits.
#[test]
fn the_account_id_is_matched_where_the_shell_matched_it() {
    let id = "7bd7dbea3c4c76cd396153fb0e92178f";
    for text in [
        format!(r#"{{"vars":{{"CF_ACCOUNT_ID": "{id}"}}}}"#),
        format!(r#"{{"CF_ACCOUNT_ID":"{id}"}}"#),
        format!("{{\"CF_ACCOUNT_ID\":\n  \"{id}\"}}"),
        // The first *matching* occurrence wins, not the first mention.
        format!(r#"{{"CF_ACCOUNT_ID": ""}} {{"CF_ACCOUNT_ID": "{id}"}}"#),
    ] {
        assert_eq!(declared_account_id(&text).as_deref(), Some(id), "in {text}");
    }
    for text in [
        r#"{"CF_ACCOUNT_ID": "7BD7DBEA3C4C76CD396153FB0E92178F"}"#,
        r#"{"CF_ACCOUNT_ID": "7bd7dbea3c4c76cd396153fb0e92178"}"#,
        r#"{"CF_ACCOUNT_ID": "7bd7dbea3c4c76cd396153fb0e92178ff"}"#,
        r#"{"CF_ACCOUNT_IDX": "7bd7dbea3c4c76cd396153fb0e92178f"}"#,
        "{}",
    ] {
        assert_eq!(declared_account_id(text), None, "accepted {text}");
    }
    // The deployed config still declares one, which is what the
    // command reads at runtime.
    assert!(xtask_registry_admin::account_id().is_ok());
}

/// `--keys` is the audit's only flag, as it was the shell's, and the
/// shim refuses anything else rather than auditing with an argument
/// the operator expected to change something.
#[test]
fn the_audit_takes_only_the_keys_flag() {
    let admin = || Command::cargo_bin("xtask-registry-admin").unwrap();
    admin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("cargo registry-backup-audit"));
    for refused in [
        vec!["backup-audit", "--all"],
        vec!["backup-audit", "--keys", "--keys"],
        vec!["diagnose", "--keys"],
    ] {
        admin()
            .args(&refused)
            .env_remove("CLOUDFLARE_API_TOKEN")
            .assert()
            .failure()
            .stderr(predicates::str::contains("unexpected argument"));
    }
    // With the flag accepted, the run reaches its first requirement.
    admin()
        .args(["backup-audit", "--keys"])
        .env_remove("CLOUDFLARE_API_TOKEN")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "CLOUDFLARE_API_TOKEN is required",
        ));
}
