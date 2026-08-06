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
}
