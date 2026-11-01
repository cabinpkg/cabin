//! The unauthenticated surface: `/healthz`, the private-alpha read
//! plane (only `config.json` answers), the uniform 401 and its
//! byte-identical challenge on both roles, the session plane's
//! challenge-less 401, the closed stats subtree, the OAuth plane's
//! host-only cookies, and the launch guard end to end.
//!
//! Nothing in this module touches [`Smoke::auth`]: a leg that presents
//! a credential passes it as a request header of its own.
//!
//! The early exit with no token (one more `step`, `smoke OK`, exit 0)
//! is deliberately *not* here: it is the caller's sequencing
//! decision.

use std::path::Path;

use anyhow::{Context as _, Result, bail};

use crate::context::{Base, Smoke};
use crate::step;
use crate::text::{capture, first_line, grep_lines, status_line_is, strip_name, text};

/// The exact envelope every website-origin refusal answers with,
/// whatever the path, method or credential.  Compared as text against
/// this constant and never parsed: a `serde_json` round trip would
/// weaken the comparison to "same fields".
const EXPECTED_401: &str = r#"{"errors":[{"detail":"authentication required"}]}"#;

/// The registry host's counterpart: the private-alpha envelope, on every
/// 401 there and on every 403 for a valid credential the allowlist does
/// not admit (either role).
pub const PRIVATE_ALPHA: &str = r#"{"errors":[{"detail":"the hosted Cabin registry is in private alpha and restricted to its maintainer","code":"registry_private_alpha"}]}"#;

/// The trusted-publishing token endpoint, exchanged into here and
/// revoked from [`crate::legs::revisions`].
pub const TRUSTPUB_TOKENS_PATH: &str = "/api/v1/trusted_publishing/tokens";

/// The login-session token endpoint, minted from and revoked by
/// [`login_session_surface`].
pub const SESSION_TOKENS_PATH: &str = "/api/v1/sessions/tokens";

/// The whole anonymous surface.
///
/// # Errors
///
/// The first failed check.
pub fn run(smoke: &mut Smoke, registry_dir: &Path, github_port: u16) -> Result<()> {
    healthz(smoke)?;
    read_plane(smoke)?;
    off_the_read_plane(smoke)?;
    mutation_surface(smoke)?;
    verdict_oidc_surface(smoke)?;
    session_endpoints(smoke)?;
    closed_stats(smoke)?;
    oauth_plane(smoke, github_port)?;
    parameterless_callback(smoke)?;
    wipe_guard(smoke, registry_dir)
}

fn healthz(smoke: &mut Smoke) -> Result<()> {
    step("healthz is unauthenticated and empty");
    smoke.check("/healthz", &[200])?;
    if !smoke.body.is_empty() {
        bail!("/healthz returned a body: {}", capture(&smoke.body));
    }
    Ok(())
}

fn read_plane(smoke: &mut Smoke) -> Result<()> {
    step("only config.json is public on the read plane; it declares auth-required");
    smoke.check("/config.json", &[200])?;
    if !capture(&smoke.body).contains(r#""auth-required":true"#) {
        bail!(
            "config.json must declare auth-required true: {}",
            capture(&smoke.body)
        );
    }
    // Every other read is the private-alpha 401 before any lookup: an
    // existing package, an unknown one, and an artifact path answer the
    // same bytes.
    for path in [
        "/packages/smoke/withdep.json",
        "/packages/smoke/never-published.json",
        "/artifacts/smoke/withdep/smoke-withdep-0.1.0-0123456789abcdef.zip",
    ] {
        uniform_401(smoke, Base::Registry, path)?;
    }
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
    // Only an anonymous GET of config.json skips admission: the read
    // plane's 405 answers admitted callers alone (the publish leg's
    // `closed_to_outsiders`).
    uniform_401_with(smoke, Base::Registry, "PUT", "/config.json", &[], None)?;
    uniform_401_with(
        smoke,
        Base::Registry,
        "PUT",
        "/config.json",
        &rotated(),
        None,
    )?;
    Ok(())
}

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
    // Write methods too: the mutation surface does not exist here.
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

fn mutation_surface(smoke: &mut Smoke) -> Result<()> {
    step("the mutation surface keeps the byte-identical uniform 401");
    // Unauthenticated publish, yank, and admin requests on the website
    // origin answer the exact envelope and one byte-identical challenge.
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
    // all - is an absent credential and must answer the same bytes.
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
    // The login-session mint is auth-exempt the same way (the GitHub
    // access token in the body is the credential), and an unusable
    // token value, no JSON at all, or a bare revoke answer the same
    // bytes.
    uniform_401_with(
        smoke,
        Base::Web,
        "PUT",
        SESSION_TOKENS_PATH,
        &[],
        Some(br#"{"github_token":""}"#),
    )?;
    uniform_401_with(
        smoke,
        Base::Web,
        "PUT",
        SESSION_TOKENS_PATH,
        &[],
        Some(b"not json"),
    )?;
    uniform_401_with(smoke, Base::Web, "DELETE", SESSION_TOKENS_PATH, &[], None)?;
    uniform_401(smoke, Base::Web, "/api/v1/admin/versions?status=pending")
}

/// The verdict route's authentication surface: a GitHub Actions OIDC
/// JWT is the only credential, every refusal is the byte-identical
/// uniform 401 (no signature/audience/pin/replay oracle), and a jti
/// never authenticates twice.  Runs before anything is published on
/// purpose: the refusals must be independent of route state.
fn verdict_oidc_surface(smoke: &mut Smoke) -> Result<()> {
    const VERDICT_PATH: &str = "/api/v1/admin/versions/smoke/withdep/0.1.0";
    step("the verdict route answers every OIDC authn failure with the uniform 401");
    let bearer = |token: &str| vec![("Authorization".to_owned(), format!("Bearer {token}"))];
    // No credential, a registry-shaped bearer, and a non-JWT bearer.
    uniform_401_with(smoke, Base::Web, "PATCH", VERDICT_PATH, &[], Some(b"{}"))?;
    uniform_401_with(
        smoke,
        Base::Web,
        "PATCH",
        VERDICT_PATH,
        &rotated(),
        Some(b"{}"),
    )?;
    let garbage = bearer("not-a-jwt");
    uniform_401_with(
        smoke,
        Base::Web,
        "PATCH",
        VERDICT_PATH,
        &garbage,
        Some(b"{}"),
    )?;
    // A properly signed JWT that misses one pin per case - the exchange
    // audience included: a token minted for one endpoint is dead on the
    // other.
    for overrides in [
        r#"{"aud":"cabinpkg.com"}"#,
        r#"{"repository_owner_id":"1"}"#,
        r#"{"repository_id":"1"}"#,
        r#"{"workflow_ref":"cabinpkg/cabin/.github/workflows/other.yml@refs/heads/main"}"#,
        r#"{"ref":"refs/heads/other"}"#,
    ] {
        let jwt = smoke.mint_verifier_jwt(overrides)?;
        let mismatched = bearer(&jwt);
        uniform_401_with(
            smoke,
            Base::Web,
            "PATCH",
            VERDICT_PATH,
            &mismatched,
            Some(b"{}"),
        )?;
    }
    // A fully valid JWT authenticates - the 400 is the post-authn body
    // check - and burns its jti doing it: presenting the same bytes
    // again is the uniform 401.
    let jwt = smoke.mint_verifier_jwt("{}")?;
    let fresh = bearer(&jwt);
    let url = smoke.url(Base::Web, VERDICT_PATH);
    let status = smoke.http("PATCH", &url, &fresh, Some(b"{}"))?;
    if status != 400 {
        bail!("a fresh verifier JWT answered {status}, expected the post-authn 400");
    }
    uniform_401_with(smoke, Base::Web, "PATCH", VERDICT_PATH, &fresh, Some(b"{}"))
}

/// The exchange's verifier arm, end to end: the same pinned identity
/// the verdict route trusts, presented against the exchange's own
/// audience, mints a 30-minute verify-scoped token - the verification
/// pipeline's bootstrap credential - with no `Authorization` header at
/// all.  Anonymous in spirit (nothing here touches [`Smoke::auth`])
/// but called from the tokened phase: the arm's backing lookup needs
/// the seeded operator identity, which a tokenless run never writes.
///
/// # Errors
///
/// The first failed check, worded like the module's other legs.
pub fn verifier_exchange_surface(smoke: &mut Smoke) -> Result<()> {
    step("the exchange's verifier arm mints, confines, and revokes the verify token");
    let bearer = |token: &str| vec![("Authorization".to_owned(), format!("Bearer {token}"))];
    // The pins hold at the exchange audience too: one mismatched claim
    // falls through to the config arm, matches nothing there, and
    // answers the uniform 401.
    let jwt = smoke.mint_verifier_jwt(r#"{"aud":"cabinpkg.com","repository_id":"1"}"#)?;
    let mismatched = format!(r#"{{"jwt":"{jwt}"}}"#);
    uniform_401_with(
        smoke,
        Base::Web,
        "PUT",
        TRUSTPUB_TOKENS_PATH,
        &[],
        Some(mismatched.as_bytes()),
    )?;
    // The pinned claims mint.
    let jwt = smoke.mint_verifier_jwt(r#"{"aud":"cabinpkg.com"}"#)?;
    let body = format!(r#"{{"jwt":"{jwt}"}}"#);
    let url = smoke.url(Base::Web, TRUSTPUB_TOKENS_PATH);
    let status = smoke.http("PUT", &url, &[], Some(body.as_bytes()))?;
    if status != 200 {
        bail!(
            "the verifier exchange answered {status}, expected 200 (body: {})",
            text(&smoke.body)
        );
    }
    let minted: serde_json::Value =
        serde_json::from_slice(&smoke.body).context("parse the exchange answer")?;
    let token = minted
        .get("token")
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("no token in the exchange answer: {minted}"))?
        .to_owned();
    if minted
        .get("expires_at")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        bail!("no expires_at in the exchange answer: {minted}");
    }
    // The jti burned on the mint: the same JWT never exchanges twice.
    uniform_401_with(
        smoke,
        Base::Web,
        "PUT",
        TRUSTPUB_TOKENS_PATH,
        &[],
        Some(body.as_bytes()),
    )?;
    // The minted token is the verify plane and nothing more: it reads
    // the pending listing, and holds no publish grant.
    let auth = bearer(&token);
    let listing = smoke.url(Base::Web, "/api/v1/admin/versions?status=pending");
    let status = smoke.http("GET", &listing, &auth, None)?;
    if status != 200 {
        bail!("the minted verify token could not list pending versions: {status}");
    }
    let publish = smoke.url(Base::Web, "/api/v1/packages/smoke/withdep/0.1.0");
    let status = smoke.http("PUT", &publish, &auth, Some(b"x"))?;
    if status != 403 {
        bail!("a publish with the minted verify token answered {status}, expected the scope 403");
    }
    smoke.expect_body("publish scope")?;
    // Revocation by the token itself, and the revoked token is dead.
    let status = smoke.http("DELETE", &url, &auth, None)?;
    if status != 204 {
        bail!("revoking the minted verify token answered {status}, expected 204");
    }
    uniform_401_with(
        smoke,
        Base::Web,
        "GET",
        "/api/v1/admin/versions?status=pending",
        &auth,
        None,
    )
}

/// The login-session mint, end to end: the GitHub access token in the
/// body - proven against the mock's check-token route - mints the 12-hour
/// session token, the token exercises its scope set and revokes
/// itself, and a live token of another kind cannot revoke through the
/// route.  Anonymous in spirit (nothing here touches [`Smoke::auth`])
/// but called from the tokened phase: the mint's identity lookup needs
/// the seeded github account, which a tokenless run never writes.
///
/// # Errors
///
/// The first failed check, worded like the module's other legs.
pub fn login_session_surface(smoke: &mut Smoke, non_session_token: &str) -> Result<()> {
    step("the login-session mint answers a token that authenticates and revokes itself");
    let bearer = |token: &str| vec![("Authorization".to_owned(), format!("Bearer {token}"))];
    let url = smoke.url(Base::Web, SESSION_TOKENS_PATH);
    // A token GitHub refuses is the outbound-failure path: the uniform
    // 401, and the attempt leaves no session row behind.
    uniform_401_with(
        smoke,
        Base::Web,
        "PUT",
        SESSION_TOKENS_PATH,
        &[],
        Some(br#"{"github_token":"gho_wrong"}"#),
    )?;
    // The seeded publisher and outsider credentials are session rows
    // themselves, so the leftover probe excludes them by id.
    let rows = crate::servers::d1_rows(
        "SELECT ifnull(group_concat(id, ','), '') AS ids FROM tokens \
         WHERE kind = 'session' AND id NOT IN ('smoke', 'smoke-outsider')",
    )?;
    let leftovers = rows
        .first()
        .and_then(|row| row.get("ids"))
        .and_then(serde_json::Value::as_str)
        .context("list the session rows")?;
    if !leftovers.is_empty() {
        bail!("a refused mint left session rows behind: {leftovers}");
    }
    // An identity GitHub proves but the allowlist does not admit gets
    // the private-alpha 403 - the one mint refusal that says why - and
    // no session row.
    let status = smoke.http(
        "PUT",
        &url,
        &[],
        Some(br#"{"github_token":"gho_outsider"}"#),
    )?;
    if status != 403 || capture(&smoke.body) != PRIVATE_ALPHA {
        bail!(
            "an unadmitted identity's mint answered {status}: {}",
            text(&smoke.body)
        );
    }
    let status = smoke.http("PUT", &url, &[], Some(br#"{"github_token":"gho_smoke"}"#))?;
    if status != 200 {
        bail!(
            "the session mint answered {status}, expected 200 (body: {})",
            text(&smoke.body)
        );
    }
    let minted: serde_json::Value =
        serde_json::from_slice(&smoke.body).context("parse the mint answer")?;
    let token = minted
        .get("token")
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("no token in the mint answer: {minted}"))?
        .to_owned();
    if !token.starts_with("cabin_ses_") {
        bail!("the minted session token does not carry the cabin_ses_ prefix");
    }
    if minted
        .get("expires_at")
        .and_then(serde_json::Value::as_str)
        .is_none()
    {
        bail!("no expires_at in the mint answer: {minted}");
    }
    // The mint is user 2's, not the operator's: publish and yank, no
    // verify - the admin plane answers the session token the scope 403.
    let auth = bearer(&token);
    for path in [
        "/api/v1/admin/versions?status=pending",
        "/api/v1/admin/governor",
    ] {
        let admin = smoke.url(Base::Web, path);
        let status = smoke.http("GET", &admin, &auth, None)?;
        if status != 403 {
            bail!("the minted session token reached the admin plane at {path}: {status}");
        }
    }
    // A live token of another kind - the seeded trustpub verify
    // credential - changes no rows through this route and answers the
    // uniform 401: the revoke is not a kind oracle.
    uniform_401_with(
        smoke,
        Base::Web,
        "DELETE",
        SESSION_TOKENS_PATH,
        &bearer(non_session_token),
        None,
    )?;
    // Revocation by the token itself, and the revoked token is dead.
    let status = smoke.http("DELETE", &url, &auth, None)?;
    if status != 204 {
        bail!("revoking the minted session token answered {status}, expected 204");
    }
    uniform_401_with(smoke, Base::Web, "DELETE", SESSION_TOKENS_PATH, &auth, None)
}

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
        // A GET like the rest: the assertion is only that a request
        // without a session is refused.
        "/api/v1/user/logout",
    ] {
        smoke.wcheck(path, &[401])?;
    }
    Ok(())
}

fn closed_stats(smoke: &mut Smoke) -> Result<()> {
    step("the stats subtree answers no anonymous caller");
    // Registry-wide totals are registry data: the bearer plane's
    // uniform 401 for every path and method under the subtree, and on
    // the registry host the surface does not exist.
    uniform_401(smoke, Base::Web, "/api/v1/stats")?;
    uniform_401(smoke, Base::Web, "/api/v1/stats/anything")?;
    uniform_401_with(smoke, Base::Web, "POST", "/api/v1/stats", &[], None)?;
    uniform_401(smoke, Base::Registry, "/api/v1/stats")
}

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

fn parameterless_callback(smoke: &mut Smoke) -> Result<()> {
    step("a parameterless callback redirects to the denied page");
    headers_only(smoke, Base::Web, "/callback")?;
    if grep_lines(&text(&smoke.headers), "location: /login/denied/").is_empty() {
        bail!(
            "/callback refusal is not /login/denied/: {}",
            capture(&smoke.headers)
        );
    }
    // /login is absent from the registry host like everything
    // non-read-plane.
    uniform_401(smoke, Base::Registry, "/login")
}

/// The launch guard end to end (`registry/docs/runbook.md`, "Data
/// policy") - flip the local flag, expect the wipe to refuse with the
/// guard's message and to leave the state untouched, then flip it back.
fn wipe_guard(smoke: &mut Smoke, registry_dir: &Path) -> Result<()> {
    step("the wipe refuses while meta.launched is 'true'");
    set_launched("true")?;
    // Bytes written before the refusal must be the same bytes after
    // it - a directory that merely exists could have been removed and
    // recreated.
    let sentinel = registry_dir.join(".wrangler/state/v3/d1/smoke-wipe-sentinel");
    std::fs::write(&sentinel, b"survives the refusal").context("write the wipe-guard sentinel")?;
    // The refusal is the expected outcome, so it is read rather than
    // propagated.
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
    // would have deleted is still there.
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

fn set_launched(value: &str) -> Result<()> {
    let command = format!("UPDATE meta SET value = '{value}' WHERE key = 'launched'");
    crate::servers::d1_quiet(&command)
}

/// The header the read-plane and admin legs present: a credential that
/// cannot validate.
fn rotated() -> Vec<(String, String)> {
    vec![(
        "Authorization".to_owned(),
        "Bearer cabin_definitelyNotAToken".to_owned(),
    )]
}

/// A plain GET with no credential.
///
/// # Errors
///
/// As [`uniform_401_with`].
pub fn uniform_401(smoke: &mut Smoke, at: Base, path: &str) -> Result<()> {
    uniform_401_with(smoke, at, "GET", path, &[], None)
}

/// The uniform 401 on either role, which differ in the envelope
/// ([`PRIVATE_ALPHA`] on the registry host, the plain
/// `authentication required` one on the website origin) and the origin
/// the challenge names: the response must be the exact envelope plus the
/// byte-identical `WWW-Authenticate` challenge, whatever the path, method
/// or credential.  The header value is compared byte for byte -
/// a duplicated header or a suffixed value must fail - so the
/// comparison is against the exact expected string, never a substring.
///
/// The credential is only ever the one passed in `headers`, never
/// [`Smoke::auth`]: a later leg presenting the publisher's own token
/// passes it there too.
///
/// # Errors
///
/// If the status, the body or the challenge is not the uniform one.
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
    let expected_body = match at {
        Base::Registry => PRIVATE_ALPHA,
        Base::Web => EXPECTED_401,
    };
    if body != expected_body {
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
    format!("Cabin login_url=\"{origin}/docs/remote-registry\"")
}

/// A GET whose header block is the whole subject: [`Smoke::body`]
/// keeps whatever the previous request left in it.
fn headers_only(smoke: &mut Smoke, at: Base, path: &str) -> Result<()> {
    let url = smoke.url(at, path);
    let body = std::mem::take(&mut smoke.body);
    smoke.http("GET", &url, &[], None)?;
    smoke.body = body;
    Ok(())
}

/// The `www-authenticate` value with CRs dropped: every matching line,
/// so a duplicated header yields two lines and can never equal the
/// single-line challenge.
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

    const CHALLENGE: &str = "Cabin login_url=\"https://cabinpkg.com/docs/remote-registry\"";

    const BLOCK: &str = concat!(
        "HTTP/1.1 401 Unauthorized\r\n",
        "content-type: application/json\r\n",
        "WWW-Authenticate: Cabin login_url=\"https://cabinpkg.com/docs/remote-registry\"\r\n",
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
            "www-authenticate: Cabin login_url=\"https://cabinpkg.com/docs/remote-registry\"\r\n",
        );
        let got = challenge_value(&doubled);
        assert_eq!(got, format!("{CHALLENGE}\n{CHALLENGE}"));
        assert_ne!(got, CHALLENGE);
    }

    #[test]
    fn a_suffixed_challenge_can_never_equal_the_expected_one() {
        let suffixed = BLOCK.replace(
            "/docs/remote-registry\"\r\n",
            "/docs/remote-registry\", Basic\r\n",
        );
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
