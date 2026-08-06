//! The audit's public surface: the account id it reads out of
//! `wrangler.jsonc` before it can list anything, and the CLI shim.
//! What a listing and a D1 answer are then *read* to mean is not this
//! crate's API, and is tested beside the code in `src/audit.rs`.

use assert_cmd::Command;
use xtask_registry_admin::{declared_account_id, declared_database_id};

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
    for refused in [vec!["backup-audit", "--all"], vec!["diagnose", "--keys"]] {
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

/// The two config-id matchers separate whitespace exactly as
/// JavaScript's `\s` did, not as `str::trim_start` does.  The two
/// disagree in both directions - Rust trims U+0085 where the regex does
/// not, and the regex consumes U+FEFF where Rust does not - and landing
/// on a different candidate than the shell is how the launch guard
/// could cross-check a database the config does not bind.
#[test]
fn a_declared_id_separates_whitespace_as_javascript_did() {
    let id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    for (name, space, matches) in [
        ("space", " ", true),
        ("newline", "\n", true),
        ("tab", "\t", true),
        ("none", "", true),
        ("no-break space", "\u{a0}", true),
        ("byte-order mark", "\u{feff}", true),
        ("ideographic space", "\u{3000}", true),
        // Rust's `char::is_whitespace` trims this one; `\s` never did.
        ("next line", "\u{85}", false),
    ] {
        let text = format!("{{\"database_id\":{space}\"{id}\"}}");
        assert_eq!(
            declared_database_id(&text).is_some(),
            matches,
            "database_id after {name}"
        );
        let account = "0123456789abcdef0123456789abcdef";
        let text = format!("{{\"CF_ACCOUNT_ID\":{space}\"{account}\"}}");
        assert_eq!(
            declared_account_id(&text).is_some(),
            matches,
            "CF_ACCOUNT_ID after {name}"
        );
    }
}

/// The guard's matcher takes the first candidate that fits the shape,
/// so a commented-out binding above the live one wins - exactly as the
/// unanchored regex did.  It checks the alphabet and the width, never
/// the 8-4-4-4-12 layout.
#[test]
fn the_database_id_is_matched_where_the_shell_matched_it() {
    let id = "61c6b514-e91b-47a5-8898-00a3cd981c70";
    assert_eq!(
        declared_database_id(&format!(r#"{{"database_id": "{id}"}}"#)).as_deref(),
        Some(id)
    );
    // 36 characters from the alphabet, whatever their layout.
    assert_eq!(
        declared_database_id(r#"{"database_id": "------------------------------------"}"#)
            .as_deref(),
        Some("------------------------------------")
    );
    for refused in [
        r#"{"database_id": "61C6B514-E91B-47A5-8898-00A3CD981C70"}"#,
        r#"{"database_id": "61c6b514-e91b-47a5-8898-00a3cd981c7"}"#,
        r#"{"my_database_id": "61c6b514-e91b-47a5-8898-00a3cd981c70"}"#,
        "{}",
    ] {
        assert_eq!(declared_database_id(refused), None, "accepted {refused}");
    }
    // The deployed config still binds one, which is what the guard
    // cross-checks the account listing against at runtime.
    let config = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../registry/wrangler.jsonc"),
    )
    .expect("wrangler.jsonc");
    assert!(declared_database_id(&config).is_some());
}
