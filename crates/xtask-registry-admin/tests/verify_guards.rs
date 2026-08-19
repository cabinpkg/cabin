//! The verifier's environment guards, extracted from the retired
//! shell-vs-port differential: they run before anything is fetched, so
//! no server is needed, and each refusal is one exact stderr line with
//! exit 1.

use assert_cmd::Command;

fn verify(token: &str, registry_origin: &str, api_origin: &str) -> assert_cmd::assert::Assert {
    Command::cargo_bin("xtask-registry-admin")
        .expect("the binary")
        .arg("verify")
        .env("REGISTRY_VERIFY_TOKEN", token)
        .env("REGISTRY_ORIGIN", registry_origin)
        .env("EXPECTED_API_ORIGIN", api_origin)
        // Removed rather than left to the parent: the guards run in
        // order, and a CI job with an id-token grant must not turn
        // the mint-guard expectations below into flakes.
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .assert()
        // Exactly 1 - the tool's own abort - rather than any failure,
        // so a panic or a usage error cannot stand in for the guard.
        .code(1)
}

#[test]
fn the_guards_refuse_before_anything_is_fetched() {
    verify("", "https://x", "https://x").stderr("REGISTRY_VERIFY_TOKEN is not configured\n");

    // The empty tail is the reachable shape: an unset repository
    // variable arrives as an empty string, and the message renders it.
    for origin in [
        "",
        "http://x",
        "HTTPS://x",
        "https:/x",
        "ftp://x",
        " https://x",
    ] {
        verify("t", origin, "https://x")
            .stderr(format!("REGISTRY_ORIGIN must be https, got: {origin}\n"));
        verify("t", "https://x", origin).stderr(format!(
            "EXPECTED_API_ORIGIN must be https, got: {origin}\n"
        ));
    }

    // With the registry guards satisfied, the OIDC mint pair is next:
    // an unset URL reads as empty, which the https guard covers.
    verify("t", "https://x", "https://x").stderr(
        "ACTIONS_ID_TOKEN_REQUEST_URL must be https; \
         does the workflow grant id-token: write?\n",
    );
    Command::cargo_bin("xtask-registry-admin")
        .expect("the binary")
        .arg("verify")
        .env("REGISTRY_VERIFY_TOKEN", "t")
        .env("REGISTRY_ORIGIN", "https://x")
        .env("EXPECTED_API_ORIGIN", "https://x")
        .env("ACTIONS_ID_TOKEN_REQUEST_URL", "https://mint.invalid/x?v=1")
        .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN")
        .assert()
        .code(1)
        .stderr("ACTIONS_ID_TOKEN_REQUEST_TOKEN is not populated\n");
}

/// The dispatch-input guards, with every earlier guard satisfied: an
/// inconsistently filled resolution form must refuse before any
/// request rather than walk the listing as if nothing were asked.
fn resolve(target: &str, action: &str, reason: &str) -> assert_cmd::assert::Assert {
    Command::cargo_bin("xtask-registry-admin")
        .expect("the binary")
        .arg("verify")
        .env("REGISTRY_VERIFY_TOKEN", "t")
        .env("REGISTRY_ORIGIN", "https://x")
        .env("EXPECTED_API_ORIGIN", "https://x")
        .env("ACTIONS_ID_TOKEN_REQUEST_URL", "https://mint.invalid/x?v=1")
        .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "rt")
        .env("VERIFY_RESOLVE", target)
        .env("VERIFY_RESOLVE_ACTION", action)
        .env("VERIFY_RESOLVE_REASON", reason)
        .assert()
        .code(1)
}

#[test]
fn the_resolution_guards_refuse_an_inconsistent_dispatch() {
    // A reason or an explicit reject with no target is an inconsistent
    // form; a stray `verify` alone is not - the dispatch form always
    // submits its action default.
    resolve("", "verify", "profanity")
        .stderr("VERIFY_RESOLVE_REASON is set without VERIFY_RESOLVE\n");
    resolve("", "reject", "").stderr("the reject action needs VERIFY_RESOLVE\n");
    for target in [
        "scope-pkg@1.0.0",
        "scope/pkg",
        "@1.0.0",
        "scope/pkg@",
        "scope/pkg@1.0.0#",
        "scope/pkg@#r1",
    ] {
        resolve(target, "verify", "")
            .stderr("VERIFY_RESOLVE must be <scope>/<name>@<version>[#<revision>]\n");
    }
    resolve("scope/pkg@1.0.0", "verify", "profanity")
        .stderr("VERIFY_RESOLVE_REASON is only for the reject action\n");
    resolve("scope/pkg@1.0.0", "reject", "")
        .stderr("the reject action needs VERIFY_RESOLVE_REASON\n");
    resolve("scope/pkg@1.0.0", "", "").stderr("unknown VERIFY_RESOLVE_ACTION ''\n");
    resolve("scope/pkg@1.0.0", "abstain", "").stderr("unknown VERIFY_RESOLVE_ACTION 'abstain'\n");
}
