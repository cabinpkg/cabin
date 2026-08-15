//! The unauthenticated surface, `registry/scripts/smoke.sh` L487-659:
//! `/healthz`, the public machine read plane, the uniform 401 and its
//! byte-identical challenge on both roles, the session plane's
//! challenge-less 401, the public stats subtree, the OAuth plane's
//! host-only cookies, and the launch guard end to end.
//!
//! Everything here runs anonymously - the shell's `curl_args=()` at
//! L486 - so nothing in this module touches [`Smoke::auth`], and the
//! two `uniform_401` legs that present a credential pass it as a
//! request header of their own, exactly as the shell passed it as an
//! extra `curl` argument rather than through `curl_args`.
//!
//! The early exit at L655-659 (no token: one more `step`, `smoke OK`,
//! exit 0) is deliberately *not* here: it is the caller's sequencing
//! decision, and this module ends where the shell's anonymous surface
//! does.

use std::path::Path;

use anyhow::{Context as _, Result, bail};

use crate::context::{Base, Smoke};
use crate::step;
use crate::text::{capture, first_line, grep_lines, status_line_is, strip_name, text};

/// The exact envelope every refusal answers with, on either role and
/// whatever the path, method or credential.  Compared as text against
/// this constant and never parsed: a `serde_json` round trip would
/// weaken six assertions to "same fields".
const EXPECTED_401: &str = r#"{"errors":[{"detail":"authentication required"}]}"#;

/// The trusted-publishing token endpoint, exchanged into here and
/// revoked from [`crate::legs::revisions`].
pub const TRUSTPUB_TOKENS_PATH: &str = "/api/v1/trusted_publishing/tokens";

/// The whole anonymous surface, in the shell's order.
///
/// # Errors
///
/// The first failed check, worded as the shell's `fail` worded it.
pub fn run(smoke: &mut Smoke, registry_dir: &Path, github_port: u16) -> Result<()> {
    healthz(smoke)?;
    read_plane(smoke)?;
    off_the_read_plane(smoke)?;
    mutation_surface(smoke)?;
    session_endpoints(smoke)?;
    public_stats(smoke)?;
    oauth_plane(smoke, github_port)?;
    parameterless_callback(smoke)?;
    wipe_guard(smoke, registry_dir)
}

/// L487-489.
fn healthz(smoke: &mut Smoke) -> Result<()> {
    step("healthz is unauthenticated and empty");
    smoke.check("/healthz", &[200])?;
    if !smoke.body.is_empty() {
        bail!("/healthz returned a body: {}", capture(&smoke.body));
    }
    Ok(())
}

/// L512-530.
fn read_plane(smoke: &mut Smoke) -> Result<()> {
    step("the machine read plane is public; unknown packages are plain 404s");
    smoke.check("/config.json", &[200])?;
    if !capture(&smoke.body).contains(r#""auth-required":false"#) {
        bail!(
            "config.json must declare auth-required false: {}",
            capture(&smoke.body)
        );
    }
    smoke.check("/packages/smoke/withdep.json", &[200, 404])?;
    // A read-plane path with a credential that fails to validate is the
    // uniform 401 - a presented credential is a claim, never silently
    // downgraded to anonymous (the verifier must fail loudly on a
    // rotated token).
    uniform_401_with(
        smoke,
        Base::Registry,
        "GET",
        "/config.json",
        &rotated(),
        None,
    )?;
    // Method discipline: the public read plane is GET-only, and the
    // refusal never depends on the credential - an invalid token must
    // not turn the 405 into a token-validity oracle.
    for credential in [None, Some(ROTATED_HEADER)] {
        let headers = if credential.is_some() {
            rotated()
        } else {
            Vec::new()
        };
        let url = smoke.url(Base::Registry, "/config.json");
        let status = smoke.http("PUT", &url, &headers, None)?;
        if status != 405 {
            bail!(
                "PUT /config.json (credential: {}) returned {status}, expected 405",
                credential.unwrap_or("none")
            );
        }
    }
    Ok(())
}

/// L532-546.
fn off_the_read_plane(smoke: &mut Smoke) -> Result<()> {
    step("the registry host is a uniform 401 with the challenge off the read plane");
    // Non-read-plane paths - the whole API and session surface included
    // - are indistinguishable from unknown paths, whatever credential
    // comes along.
    for path in [
        "/api/v1/packages/smoke/withdep/0.1.0",
        "/api/v1/user",
        "/api/v1/user/search?q=withdep",
        "/api/v1/user/package/smoke/withdep",
        "/unknown/path",
        "/me",
    ] {
        uniform_401(smoke, Base::Registry, path)?;
    }
    uniform_401_with(
        smoke,
        Base::Registry,
        "GET",
        "/api/v1/admin/versions?status=pending",
        &rotated(),
        None,
    )?;
    // Write methods too: the mutation surface simply does not exist here.
    uniform_401_with(
        smoke,
        Base::Registry,
        "PUT",
        "/api/v1/packages/smoke/withdep/0.1.0",
        &[],
        Some(b"x"),
    )?;
    uniform_401_with(
        smoke,
        Base::Registry,
        "PATCH",
        "/api/v1/packages/smoke/withdep/0.1.0/yank",
        &[],
        Some(b"{}"),
    )
}

/// L548-570.
fn mutation_surface(smoke: &mut Smoke) -> Result<()> {
    step("the mutation surface keeps the byte-identical uniform 401");
    // Public reads narrowed the uniform-401 discipline to the mutation
    // surface; unauthenticated publish, yank, and admin requests on the
    // website origin must still answer the exact envelope and one
    // byte-identical challenge.
    uniform_401_with(
        smoke,
        Base::Web,
        "PUT",
        "/api/v1/packages/smoke/withdep/0.1.0",
        &[],
        Some(b"x"),
    )?;
    uniform_401_with(
        smoke,
        Base::Web,
        "PATCH",
        "/api/v1/packages/smoke/withdep/0.1.0/yank",
        &[],
        Some(b"{}"),
    )?;
    // The trusted-publishing exchange is auth-exempt (the JWT in the
    // body is the credential), but an unverifiable JWT - or no JSON at
    // all - is an absent credential and must answer the same bytes:
    // post-migration coverage with no shell ancestor.
    uniform_401_with(
        smoke,
        Base::Web,
        "PUT",
        TRUSTPUB_TOKENS_PATH,
        &[],
        Some(br#"{"jwt":"not-a-jwt"}"#),
    )?;
    uniform_401_with(
        smoke,
        Base::Web,
        "PUT",
        TRUSTPUB_TOKENS_PATH,
        &[],
        Some(b"not json"),
    )?;
    uniform_401_with(smoke, Base::Web, "DELETE", TRUSTPUB_TOKENS_PATH, &[], None)?;
    uniform_401(smoke, Base::Web, "/api/v1/admin/versions?status=pending")
}

/// L572-584.
fn session_endpoints(smoke: &mut Smoke) -> Result<()> {
    step("session endpoints answer 401 json without a session (no challenge)");
    smoke.wcheck("/api/v1/user", &[401])?;
    if capture(&smoke.body) != EXPECTED_401 {
        bail!("session 401 body mismatch: {}", capture(&smoke.body));
    }
    headers_only(smoke, Base::Web, "/api/v1/user")?;
    if has_challenge(&text(&smoke.headers)) {
        bail!("the session-plane 401 must not carry the bearer challenge");
    }
    for path in [
        "/api/v1/user/usage",
        "/api/v1/user/packages",
        "/api/v1/user/search?q=withdep",
        "/api/v1/user/package/smoke/withdep",
        "/api/v1/user/package/smoke/withdep/reverse-dependencies",
        "/api/v1/user/tokens",
        // L584 spells `-X POST` after the expected status, where
        // `check_at` reads everything past the path as another expected
        // status: the request the shell makes is this GET, and the
        // extra words only ever reached its `fail` wording.
        "/api/v1/user/logout",
    ] {
        smoke.wcheck(path, &[401])?;
    }
    Ok(())
}

/// L586-597.
fn public_stats(smoke: &mut Smoke) -> Result<()> {
    step("the public stats endpoint is unauthenticated json on the website origin");
    smoke.wcheck("/api/v1/stats", &[200])?;
    smoke.expect_body(r#""packages":"#)?;
    smoke.expect_body(r#""versions":"#)?;
    smoke.expect_body(r#""downloads":"#)?;
    // The subtree is its own plane: unknown paths under it are public
    // 404s, non-GET is 405, and on the registry host the surface does
    // not exist.
    smoke.wcheck("/api/v1/stats/anything", &[404])?;
    let url = smoke.url(Base::Web, "/api/v1/stats");
    let status = smoke.http("POST", &url, &[], None)?;
    if status != 405 {
        bail!("POST /api/v1/stats returned {status}, expected 405");
    }
    uniform_401(smoke, Base::Registry, "/api/v1/stats")
}

/// L599-622.
fn oauth_plane(smoke: &mut Smoke, github_port: u16) -> Result<()> {
    step("the oauth plane lives on the website origin with host-only cookies");
    headers_only(smoke, Base::Web, "/login")?;
    let block = text(&smoke.headers).into_owned();
    if !status_line_is(&block, 302) {
        bail!("/login did not answer 302: {}", first_line(&block));
    }
    // The authorize base is the GitHub mock (GITHUB_OAUTH_BASE); deployed
    // environments use the real https://github.com default.
    let authorize = format!("location: http://127.0.0.1:{github_port}/login/oauth/authorize");
    if grep_lines(&block, &authorize).is_empty() {
        bail!(
            "/login redirect is not the authorize page: {}",
            capture(&smoke.headers)
        );
    }
    // Ordinary sign-in requests no OAuth scopes; only the claim flow does.
    if grep_lines(&block, "location: ")
        .iter()
        .any(|line| line.contains("scope="))
    {
        bail!(
            "/login must not request an oauth scope: {}",
            capture(&smoke.headers)
        );
    }
    // Every matching line, so a second state cookie is visible to the
    // attribute checks below rather than hidden behind the first.
    let state_cookie = grep_lines(&block, "set-cookie: cabin_oauth_state=").join("\n");
    if state_cookie.is_empty() {
        bail!("/login set no state cookie: {}", capture(&smoke.headers));
    }
    if !state_cookie.contains("Path=/callback") {
        bail!("state cookie is not scoped to /callback: {state_cookie}");
    }
    for attribute in ["HttpOnly", "Secure", "SameSite=Lax"] {
        if !state_cookie.contains(attribute) {
            bail!("state cookie is missing {attribute}: {state_cookie}");
        }
    }
    if state_cookie.to_ascii_lowercase().contains("domain=") {
        bail!("the state cookie must be host-only: {state_cookie}");
    }
    Ok(())
}

/// L624-629.
fn parameterless_callback(smoke: &mut Smoke) -> Result<()> {
    step("a parameterless callback redirects to the denied page");
    headers_only(smoke, Base::Web, "/callback")?;
    if grep_lines(&text(&smoke.headers), "location: /login/denied").is_empty() {
        bail!(
            "/callback refusal is not /login/denied: {}",
            capture(&smoke.headers)
        );
    }
    // /login is absent from the registry host like everything
    // non-read-plane.
    uniform_401(smoke, Base::Registry, "/login")
}

/// L631-653: the launch guard end to end
/// (`registry/docs/runbook.md`, "Data policy") - flip the local flag,
/// expect the wipe to refuse with the guard's message and to leave the
/// state untouched, then flip it back.
fn wipe_guard(smoke: &mut Smoke, registry_dir: &Path) -> Result<()> {
    step("the wipe refuses while meta.launched is 'true'");
    set_launched("true")?;
    // The deleted test's sentinel, kept literally: bytes written before
    // the refusal must be the same bytes after it - a directory that
    // merely exists could have been removed and recreated.
    let sentinel = registry_dir.join(".wrangler/state/v3/d1/smoke-wipe-sentinel");
    std::fs::write(&sentinel, b"survives the refusal").context("write the wipe-guard sentinel")?;
    // The refusal is the expected outcome, so it is read rather than
    // propagated. The shell ran `scripts/wipe.sh --local 2>&1` and
    // grepped the interleaved streams for the guard's line; the wipe is
    // `cargo registry-wipe` now and this crate can call it, so the
    // refusal IS the error value.
    let Err(refusal) =
        xtask_registry_admin::wipe::run(xtask_registry_admin::launch_guard::Mode::Local)
    else {
        bail!("the wipe ran --local against a launched registry");
    };
    let refusal = format!("{refusal:#}");
    if !refusal.contains("meta.launched = 'true'") {
        bail!("the wipe's refusal is missing the guard's message: {refusal}");
    }
    // A refusal stops before the first mutation, so the state the wipe
    // would have deleted is still there. `registry/tests/launch_guard.rs`
    // held this with a sentinel file while the wipe was a shell script
    // whose ordering only an end-to-end run could check; the script and
    // that test go together, and the property stays here.
    let survived = std::fs::read(&sentinel).context("read the wipe-guard sentinel back")?;
    if survived != b"survives the refusal" {
        bail!("the refused wipe disturbed the local D1 state");
    }
    std::fs::remove_file(&sentinel).context("remove the wipe-guard sentinel")?;
    // Sentinel: the database survived the refusal (and the servers with
    // it).
    let rows = crate::servers::d1_rows("SELECT value FROM meta WHERE key = 'registry_generation'")?;
    if rows.len() != 1 {
        bail!("the refused wipe still touched the database");
    }
    smoke.check("/healthz", &[200])?;
    set_launched("false")
}

/// `UPDATE meta SET value = '<value>' WHERE key = 'launched'` against
/// the local state, stdout swallowed as the shell swallowed it.
fn set_launched(value: &str) -> Result<()> {
    let command = format!("UPDATE meta SET value = '{value}' WHERE key = 'launched'");
    crate::servers::d1_quiet(&command)
}

/// The `-H` argument the read-plane and admin legs present: a
/// credential that cannot validate.
const ROTATED_HEADER: &str = "Authorization: Bearer cabin_definitelyNotAToken";

fn rotated() -> Vec<(String, String)> {
    vec![(
        "Authorization".to_owned(),
        "Bearer cabin_definitelyNotAToken".to_owned(),
    )]
}

/// `uniform_401 <base> <path>`: a plain GET with no credential.
///
/// # Errors
///
/// As [`uniform_401_with`].
pub fn uniform_401(smoke: &mut Smoke, at: Base, path: &str) -> Result<()> {
    uniform_401_with(smoke, at, "GET", path, &[], None)
}

/// `uniform_401` and `uniform_401_web`, which differ only in the role
/// they ask and therefore in the origin the challenge names: the
/// response must be the exact envelope plus the byte-identical
/// `WWW-Authenticate` challenge, whatever the path, method or
/// credential.  The header value is compared byte for byte - a
/// duplicated header or a suffixed value must fail - so the comparison
/// is against the exact expected string, never a substring.
///
/// Neither shell helper carried `curl_args`, so the credential is only
/// ever the one passed in `headers`: a later leg presenting the
/// publisher's own token passes it there too (L737-738).
///
/// # Errors
///
/// If the status, the body or the challenge is not the uniform one,
/// worded as the shell worded it.
pub fn uniform_401_with(
    smoke: &mut Smoke,
    at: Base,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    data: Option<&[u8]>,
) -> Result<()> {
    let url = smoke.url(at, path);
    let status = smoke.http(method, &url, headers, data)?;
    if status != 401 {
        bail!("{path} returned {status}, expected the uniform 401");
    }
    let body = capture(&smoke.body);
    if body != EXPECTED_401 {
        bail!("401 body mismatch on {path}: {body}");
    }
    let expected = challenge(smoke, at);
    let got = challenge_value(&text(&smoke.headers));
    if got != expected {
        bail!("401 on {path} challenge mismatch: got '{got}', expected '{expected}'");
    }
    println!("    {path} -> uniform 401 with the challenge");
    Ok(())
}

/// The challenge the role answering the request must name: `WEB_ORIGIN`
/// on the registry host, and the local address on the website role -
/// `wrangler dev` rewrites the emulated website origin to the address
/// it is bound to in response headers, so the expected challenge is
/// compared in its rewritten form there.
fn challenge(smoke: &Smoke, at: Base) -> String {
    let origin = match at {
        Base::Registry => smoke.web_origin().to_owned(),
        Base::Web => smoke.url(Base::Web, ""),
    };
    format!("Cabin login_url=\"{origin}/settings/tokens\"")
}

/// `curl -o /dev/null -D "$headers"`: the header block is the whole
/// subject, and `$body` keeps whatever the previous request left in it
/// - the shell wrote only one of the two buffers here.
fn headers_only(smoke: &mut Smoke, at: Base, path: &str) -> Result<()> {
    let url = smoke.url(at, path);
    let body = std::mem::take(&mut smoke.body);
    smoke.http("GET", &url, &[], None)?;
    smoke.body = body;
    Ok(())
}

/// The `www-authenticate` value as
/// `grep -i '^www-authenticate:' | sed 's/^[^:]*: //' | tr -d '\r'`
/// extracted it: every matching line, so a duplicated header yields two
/// lines and can never equal the single-line challenge.
fn challenge_value(block: &str) -> String {
    grep_lines(block, "www-authenticate:")
        .into_iter()
        .map(strip_name)
        .collect::<Vec<_>>()
        .join("\n")
        .replace('\r', "")
}

/// Whether the block carries the bearer challenge at all.  The session
/// plane's 401 must not, which is an assertion about absence.
fn has_challenge(block: &str) -> bool {
    !grep_lines(block, "www-authenticate:").is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHALLENGE: &str = "Cabin login_url=\"https://cabinpkg.com/settings/tokens\"";

    const BLOCK: &str = concat!(
        "HTTP/1.1 401 Unauthorized\r\n",
        "content-type: application/json\r\n",
        "WWW-Authenticate: Cabin login_url=\"https://cabinpkg.com/settings/tokens\"\r\n",
        "\r\n",
    );

    #[test]
    fn the_challenge_is_read_case_insensitively_without_its_cr() {
        assert_eq!(challenge_value(BLOCK), CHALLENGE);
    }

    #[test]
    fn a_duplicated_challenge_can_never_equal_the_expected_one() {
        let doubled = BLOCK.replace(
            "content-type: application/json\r\n",
            "www-authenticate: Cabin login_url=\"https://cabinpkg.com/settings/tokens\"\r\n",
        );
        let got = challenge_value(&doubled);
        assert_eq!(got, format!("{CHALLENGE}\n{CHALLENGE}"));
        assert_ne!(got, CHALLENGE);
    }

    #[test]
    fn a_suffixed_challenge_can_never_equal_the_expected_one() {
        let suffixed = BLOCK.replace("/settings/tokens\"\r\n", "/settings/tokens\", Basic\r\n");
        assert_ne!(challenge_value(&suffixed), CHALLENGE);
    }

    #[test]
    fn the_session_plane_block_carries_no_challenge() {
        assert!(!has_challenge(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\n\r\n"
        ));
        assert!(has_challenge(BLOCK));
        // The absence is line-anchored: a header whose value mentions
        // the name is not the challenge.
        assert!(!has_challenge(
            "HTTP/1.1 401 Unauthorized\r\nvary: www-authenticate:\r\n\r\n"
        ));
    }
}
