//! The session-authenticated surface, `registry/scripts/smoke.sh`
//! L806-996: the token create round-trip, the logout CSRF pair, the
//! claim flow end to end against the GitHub mock, membership
//! management, the owner gate's byte-identical 403, and the generation
//! header - plus the claim-initiation fetch-metadata gate, which
//! postdates the shell and so carries no `L` anchor.
//!
//! Every request here rides the session cookie the session leg minted,
//! so the cookie arrives as a parameter rather than being re-derived -
//! the shell had restored `$session_cookie` to the real one at L796,
//! and this span never swaps it again.
//!
//! The buffer subtleties the checks depend on are spelled out at the
//! sites the shell spelled them: the owner gate copies the first 403
//! body before the second request overwrites it, and L994 redirects the
//! *header* block into `$body`.  One shell detail is deliberately not
//! reproduced: `session_request` carried no `-D`, so it left the header
//! block alone, where the port writes both buffers.  Nothing between
//! L806 and L1169 reads a header block a `session_request` would have
//! clobbered, so the two are indistinguishable here.

use anyhow::{Context as _, Result, bail};
use serde_json::Value;
use xtask_registry_admin::display;

use crate::context::{Base, Smoke};
use crate::legs::anonymous::uniform_401_with;
use crate::legs::session::{CREATE_BODY, csrf_headers, session_request};
use crate::step;
use crate::text::{capture, contains, first_line, grep_lines, status_line_is, strip_name, text};

/// L907-908: the claim's frozen proof, read straight out of the local
/// database because no route exposes it.
const PROOF_QUERY: &str =
    "SELECT proof_provider, proof_account_id FROM scopes WHERE name = 'smoke'";

/// The whole span, in the shell's order.
///
/// # Errors
///
/// The first failed check, worded as the shell's `fail` worded it.
pub fn run(smoke: &mut Smoke, session_cookie: &str, github_port: u16) -> Result<()> {
    token_round_trip(smoke, session_cookie)?;
    logout(smoke, session_cookie)?;
    claim_initiation_intent(smoke)?;
    claim_cookie_scope(smoke)?;
    drifted_self_claim(smoke, github_port)?;
    self_claim(smoke)?;
    org_claims(smoke)?;
    reserved_scopes(smoke)?;
    permanent_claims(smoke)?;
    unmatched_state(smoke)?;
    members(smoke, session_cookie)?;
    last_owner(smoke, session_cookie)?;
    owner_gate(smoke, session_cookie)?;
    generation_header(smoke)
}

/// L806-827.
fn token_round_trip(smoke: &mut Smoke, cookie: &str) -> Result<()> {
    step("token create round-trip: plaintext once, usable, then revoked");
    let create = CREATE_BODY.as_bytes();
    session_request(
        smoke,
        cookie,
        "POST",
        "/api/v1/user/tokens",
        201,
        &csrf_headers(),
        Some(create),
    )?;
    smoke.expect_body(r#""name":"smoke-session""#)?;
    let (minted, minted_id) = minted(&smoke.body)?;

    // The minted token works on the bearer plane...
    smoke.auth = vec![("Authorization".to_owned(), format!("Bearer {minted}"))];
    smoke.check("/config.json", &[200])?;
    // ...and the listing shows metadata only - never the plaintext.
    session_request(smoke, cookie, "GET", "/api/v1/user/tokens", 200, &[], None)?;
    smoke.expect_body(r#""name":"smoke-session""#)?;
    if contains(&smoke.body, minted.as_bytes()) {
        bail!("the token listing leaked a plaintext token");
    }
    session_request(
        smoke,
        cookie,
        "POST",
        &format!("/api/v1/user/tokens/{minted_id}/revoke"),
        200,
        &csrf_headers(),
        None,
    )?;
    smoke.expect_body(r#""ok":true"#)?;
    let bearer = vec![("Authorization".to_owned(), format!("Bearer {minted}"))];
    uniform_401_with(smoke, Base::Registry, "GET", "/config.json", &bearer, None)?;
    smoke.as_publisher();
    Ok(())
}

/// L829-840.
fn logout(smoke: &mut Smoke, cookie: &str) -> Result<()> {
    step("logout requires the csrf pair and clears the session cookie");
    session_request(smoke, cookie, "POST", "/api/v1/user/logout", 403, &[], None)?;
    smoke.expect_body("X-CSRF-Protection")?;

    let url = smoke.url(Base::Web, "/api/v1/user/logout");
    let mut headers = vec![("Cookie".to_owned(), cookie.to_owned())];
    headers.extend(csrf_headers());
    smoke.http("POST", &url, &headers, None)?;
    if !contains(&smoke.body, br#""ok":true"#) {
        bail!("logout did not answer ok: {}", capture(&smoke.body));
    }
    let block = text(&smoke.headers).into_owned();
    let logout_cookie = grep_lines(&block, "set-cookie: cabin_session=").join("\n");
    if logout_cookie.is_empty() {
        bail!("logout set no clearing cookie: {}", capture(&smoke.headers));
    }
    if !logout_cookie.contains("Max-Age=0") {
        bail!("logout cookie does not clear: {logout_cookie}");
    }
    Ok(())
}

/// The fetch-metadata gate on claim initiation, which has no shell
/// ancestor: `smoke.sh` predates it.  A refusal must seal nothing - a
/// sealed cookie would leave `/callback/claim` reachable with a state
/// the victim's browser is carrying.
fn claim_initiation_intent(smoke: &mut Smoke) -> Result<()> {
    step("starting a claim needs a same-origin or user-typed navigation");
    for site in ["cross-site", "same-site"] {
        refuse_claim_start(smoke, &fetch_metadata(site, "navigate", None), site)?;
    }
    // A same-origin *subresource*: the website prefetches its own links
    // wholesale, so a claim link would seal state on hover.
    let prefetch = fetch_metadata("same-origin", "no-cors", None);
    refuse_claim_start(smoke, &prefetch, "same-origin prefetch")?;
    // A same-origin prerender: a navigation the user may never perform.
    let prerender = fetch_metadata("same-origin", "navigate", Some("prefetch;prerender"));
    refuse_claim_start(smoke, &prerender, "prerender")?;
    // Absent metadata is the fail-closed case, and the only leg left
    // proving it once the initiations below all send the headers.
    refuse_claim_start(smoke, &[], "no fetch metadata")
}

/// One refused claim start: the uniform denial, and no state.  Uses
/// 'statedrift', which is unclaimed and fully grantable in the mock, so
/// nothing but the fetch-metadata gate can be what refused it.
fn refuse_claim_start(smoke: &mut Smoke, headers: &[(String, String)], what: &str) -> Result<()> {
    let url = smoke.url(Base::Web, "/claim/statedrift");
    smoke.http("GET", &url, headers, None)?;
    let block = text(&smoke.headers).into_owned();
    let location = location_value(&block);
    if location != "/dashboard?claim=denied" {
        bail!("a {what} claim start answered '{location}', expected the uniform denial");
    }
    if !claim_state_value(&block).is_empty() {
        bail!(
            "a {what} claim start sealed a claim-state cookie: {}",
            capture(&smoke.headers)
        );
    }
    Ok(())
}

/// L883-894.
fn claim_cookie_scope(smoke: &mut Smoke) -> Result<()> {
    step("the claim-state cookie is scoped to the claim callback");
    let url = smoke.url(Base::Web, "/claim/smoke");
    smoke.http("GET", &url, &user_initiated(), None)?;
    let block = text(&smoke.headers).into_owned();
    // `$(grep ... || true)`: the capture keeps each line's trailing CR,
    // which every diagnostic below carries.
    let line = grep_lines(&block, "set-cookie: cabin_claim_state=").join("\n");
    if line.is_empty() {
        bail!(
            "/claim/smoke set no claim-state cookie: {}",
            capture(&smoke.headers)
        );
    }
    for attribute in ["Path=/callback/claim", "HttpOnly", "Secure", "SameSite=Lax"] {
        if !line.contains(attribute) {
            bail!("claim-state cookie is missing {attribute}: {line}");
        }
    }
    if line.to_ascii_lowercase().contains("domain=") {
        bail!("the claim-state cookie must be host-only: {line}");
    }
    Ok(())
}

/// L896-902.  With the drift toggle on, `/users/smoke` names account
/// 999 while the authenticated `/user` is account 0; 'smoke' is still
/// unclaimed, so the id-equality binding is the only thing refusing
/// here.
fn drifted_self_claim(smoke: &mut Smoke, github_port: u16) -> Result<()> {
    step("a self-claim is refused when /users/<scope> is another account");
    drift(smoke, github_port, "on")?;
    claim_scope(smoke, "smoke", "denied")?;
    drift(smoke, github_port, "off")
}

/// L904-915.  The mock `/user` answers login 'Smoke'; the grant
/// compares it lowercased.
fn self_claim(smoke: &mut Smoke) -> Result<()> {
    step("a self-claim grants the scope, frozen to the account's numeric id");
    claim_scope(smoke, "smoke", "granted")?;
    let rows = crate::servers::d1_rows(PROOF_QUERY)?;
    let row = rows
        .first()
        .context("the claim did not freeze the numeric proof")?;
    let field = |name: &str| display(row.get(name).unwrap_or(&Value::Null));
    let proof = format!("{}:{}", field("proof_provider"), field("proof_account_id"));
    if proof != "github:0" {
        bail!("the claim did not freeze the numeric proof: {proof}");
    }
    Ok(())
}

/// L917-924.  A plain member, a membership naming a different user than
/// the authenticated one, and a membership naming a different
/// organization than `/users/<scope>` resolves must all refuse.
fn org_claims(smoke: &mut Smoke) -> Result<()> {
    step("an org claim needs an active admin membership bound by numeric ids");
    claim_scope(smoke, "smokeorg", "granted")?;
    claim_scope(smoke, "denyorg", "denied")?;
    claim_scope(smoke, "imposterorg", "denied")?;
    claim_scope(smoke, "swaporg", "denied")
}

/// L926-934.  Both are fully grantable in the mock (like statedrift),
/// so nothing but the name-fidelity checks can be what refused them.
fn reserved_scopes(smoke: &mut Smoke) -> Result<()> {
    step("reserved and skeleton-confusable scopes refuse uniformly");
    claim_scope(smoke, "core", "denied")?;
    claim_scope(smoke, "sm0keorg", "denied")
}

/// L936-937.
fn permanent_claims(smoke: &mut Smoke) -> Result<()> {
    step("claims are permanent: a re-claim refuses even the owning account");
    claim_scope(smoke, "smoke", "denied")
}

/// L939-953.
fn unmatched_state(smoke: &mut Smoke) -> Result<()> {
    step("a claim callback without a valid matching state is refused");
    let url = smoke.url(Base::Web, "/callback/claim");
    smoke.http("GET", &url, &[], None)?;
    if !refuses(&text(&smoke.headers)) {
        bail!(
            "a bare claim callback did not refuse: {}",
            capture(&smoke.headers)
        );
    }

    // A sealed cookie with a mismatched state parameter refuses before
    // any GitHub call.  'statedrift' is unclaimed AND fully grantable in
    // the mock, so nothing but the state comparison can be what refused
    // it.
    let url = smoke.url(Base::Web, "/claim/statedrift");
    smoke.http("GET", &url, &user_initiated(), None)?;
    let cookie = claim_state_value(&text(&smoke.headers));
    if cookie.is_empty() {
        bail!("/claim/statedrift set no claim-state cookie");
    }
    let url = smoke.url(Base::Web, "/callback/claim?code=smoke&state=deadbeef");
    let headers = vec![("Cookie".to_owned(), format!("cabin_claim_state={cookie}"))];
    smoke.http("GET", &url, &headers, None)?;
    if !refuses(&text(&smoke.headers)) {
        bail!(
            "a mismatched claim state did not refuse: {}",
            capture(&smoke.headers)
        );
    }
    Ok(())
}

/// L956-979.
fn members(smoke: &mut Smoke, cookie: &str) -> Result<()> {
    step("scope owners list, add, and remove members");
    let list = "/api/v1/user/scopes/smoke/members";
    session_request(smoke, cookie, "GET", list, 200, &[], None)?;
    smoke.expect_body(r#""github_id":0"#)?;
    smoke.expect_body(r#""role":"owner""#)?;
    let add_member = br#"{"github_id":2,"role":"member"}"#;
    session_request(
        smoke,
        cookie,
        "POST",
        list,
        200,
        &csrf_headers(),
        Some(add_member),
    )?;
    smoke.expect_body(r#""changed":true"#)?;
    session_request(smoke, cookie, "GET", list, 200, &[], None)?;
    smoke.expect_body(r#""login":"friend""#)?;

    // An existing member keeps their role: no role-change endpoint.
    let add_owner = br#"{"github_id":2,"role":"owner"}"#;
    session_request(
        smoke,
        cookie,
        "POST",
        list,
        200,
        &csrf_headers(),
        Some(add_owner),
    )?;
    smoke.expect_body(r#""role":"member""#)?;
    smoke.expect_body(r#""changed":false"#)?;
    let unknown = br#"{"github_id":999,"role":"member"}"#;
    session_request(
        smoke,
        cookie,
        "POST",
        list,
        400,
        &csrf_headers(),
        Some(unknown),
    )?;
    smoke.expect_body("no registry account")?;
    let json_only = vec![("Content-Type".to_owned(), "application/json".to_owned())];
    session_request(
        smoke,
        cookie,
        "POST",
        list,
        403,
        &json_only,
        Some(add_member),
    )?;
    smoke.expect_body("X-CSRF-Protection")?;

    let remove = "/api/v1/user/scopes/smoke/members/2/remove";
    session_request(smoke, cookie, "POST", remove, 200, &csrf_headers(), None)?;
    smoke.expect_body(r#""changed":true"#)?;
    session_request(smoke, cookie, "POST", remove, 200, &csrf_headers(), None)?;
    smoke.expect_body(r#""changed":false"#)
}

/// L981-983.
fn last_owner(smoke: &mut Smoke, cookie: &str) -> Result<()> {
    step("the last owner cannot be removed");
    let path = "/api/v1/user/scopes/smoke/members/0/remove";
    session_request(smoke, cookie, "POST", path, 409, &csrf_headers(), None)?;
    smoke.expect_body("last owner")
}

/// L985-991.
fn owner_gate(smoke: &mut Smoke, cookie: &str) -> Result<()> {
    step("the owner gate is one uniform 403 for foreign and unclaimed scopes");
    let foreign = "/api/v1/user/scopes/foreign/members";
    session_request(smoke, cookie, "GET", foreign, 403, &[], None)?;
    smoke.expect_body("not an owner")?;
    // `cp "$body" "$mock_dir/owner-403.json"`: the copy is taken before
    // the next request overwrites the buffer, and the comparison is
    // `cmp -s` - byte identity, never parsed equality.
    let expected = smoke.body.clone();
    session_request(
        smoke,
        cookie,
        "GET",
        "/api/v1/user/scopes/ghost/members",
        403,
        &[],
        None,
    )?;
    if smoke.body != expected {
        bail!(
            "foreign-scope and unclaimed-scope owner 403s differ: {}",
            capture(&smoke.body)
        );
    }
    Ok(())
}

/// L993-996.
fn generation_header(smoke: &mut Smoke) -> Result<()> {
    step("authenticated responses carry the generation header");
    let url = smoke.url(Base::Registry, "/config.json");
    let auth = smoke.auth.clone();
    // `-o /dev/null -D "$body"`: the header block lands in the *body*
    // buffer, which is what the assertion reads, and the header buffer
    // keeps what the last `-D "$headers"` request left in it.
    let stale = std::mem::take(&mut smoke.headers);
    smoke.http("GET", &url, &auth, None)?;
    smoke.body = std::mem::replace(&mut smoke.headers, stale);
    if grep_lines(&text(&smoke.body), "x-cabin-registry-generation:").is_empty() {
        bail!("missing x-cabin-registry-generation header");
    }
    Ok(())
}

/// `claim_scope <scope> <granted|denied>` (L847-881): drive the claim
/// flow - initiate, capture the sealed state cookie and the authorize
/// redirect's state, then complete the callback (the mock exchanges any
/// code).  Neither hop follows the redirect: the 302 *is* the subject,
/// and the next request is made by hand from what it carried.
fn claim_scope(smoke: &mut Smoke, scope: &str, expected: &str) -> Result<()> {
    let url = smoke.url(Base::Web, &format!("/claim/{scope}"));
    smoke.http("GET", &url, &user_initiated(), None)?;
    let block = text(&smoke.headers).into_owned();
    if !status_line_is(&block, 302) {
        bail!("/claim/{scope} did not answer 302: {}", first_line(&block));
    }
    let location = location_value(&block);
    if !location.contains("/login/oauth/authorize?") {
        bail!("/claim/{scope} redirect is not the authorize page: {location}");
    }
    // The dedicated roundtrip's shape: read:org, and the subdirectory
    // callback GitHub accepts under the registered /callback URL.
    if !location.contains("scope=read%3Aorg") {
        bail!("the claim authorize request must ask for read:org: {location}");
    }
    if !location.contains("redirect_uri=https%3A%2F%2Fcabinpkg.com%2Fcallback%2Fclaim") {
        bail!("the claim redirect_uri is not /callback/claim: {location}");
    }
    let state = state_value(&location);
    if state.is_empty() {
        bail!("no state in the authorize redirect: {location}");
    }
    let cookie = claim_state_value(&block);
    if cookie.is_empty() {
        bail!(
            "/claim/{scope} set no claim-state cookie: {}",
            capture(&smoke.headers)
        );
    }

    let url = smoke.url(
        Base::Web,
        &format!("/callback/claim?code=smoke&state={state}"),
    );
    let headers = vec![("Cookie".to_owned(), format!("cabin_claim_state={cookie}"))];
    smoke.http("GET", &url, &headers, None)?;
    let block = text(&smoke.headers).into_owned();
    let location = location_value(&block);
    if location != format!("/dashboard?claim={expected}") {
        bail!("/callback/claim for {scope} answered '{location}', expected claim={expected}");
    }
    // The claim-state cookie is one-shot: cleared on every outcome.
    if !grep_lines(&block, "set-cookie: cabin_claim_state=")
        .iter()
        .any(|line| line.contains("Max-Age=0"))
    {
        bail!(
            "the claim callback did not clear the state cookie: {}",
            capture(&smoke.headers)
        );
    }
    println!("    claim {scope} -> {expected}");
    Ok(())
}

/// The GitHub mock's drift toggle (L900, L902), which is neither dev
/// role and so takes its address rather than a base.  `curl -f` had no
/// message of its own: the shell exited on curl's status.
fn drift(smoke: &mut Smoke, github_port: u16, state: &str) -> Result<()> {
    let url = format!("http://127.0.0.1:{github_port}/__drift/{state}");
    let status = smoke.http("POST", &url, &[], None)?;
    if status >= 400 {
        bail!("POST {url} returned {status}");
    }
    Ok(())
}

/// The two `node` reads of the create response (L810-816).  A document
/// that does not parse and a `token` that is not the plaintext shape
/// were the same nonzero exit, and the shell answered both with one
/// message; the id is read with no check of its own, rendered as
/// JavaScript rendered it.
fn minted(body: &[u8]) -> Result<(String, String)> {
    let document = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let token = document
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| token.starts_with("cabin_"))
        .with_context(|| {
            format!(
                "create response carries no plaintext token: {}",
                capture(body)
            )
        })?;
    let id = display(document.get("id").unwrap_or(&Value::Null));
    Ok((token.to_owned(), id))
}

/// Fetch metadata as a browser stamps it onto one request.
fn fetch_metadata(site: &str, mode: &str, purpose: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Sec-Fetch-Site".to_owned(), site.to_owned()),
        ("Sec-Fetch-Mode".to_owned(), mode.to_owned()),
    ];
    if let Some(purpose) = purpose {
        headers.push(("Sec-Purpose".to_owned(), purpose.to_owned()));
    }
    headers
}

/// What a browser sends on the navigation that starts a claim today: no
/// dashboard form exists yet, so the user types or bookmarks the URL and
/// the site reads `none` rather than `same-origin`.  The refusal cases
/// vary one field at a time off this baseline, so each names the single
/// condition that refused it.
fn user_initiated() -> Vec<(String, String)> {
    fetch_metadata("none", "navigate", None)
}

/// `grep -qi '^location: /dashboard?claim=denied'`: `?` is a literal in
/// a basic regular expression, so the whole pattern is a prefix.
fn refuses(block: &str) -> bool {
    !grep_lines(block, "location: /dashboard?claim=denied").is_empty()
}

/// The `location` value as
/// `grep -i '^location: ' | sed 's/^[^:]*: //' | tr -d '\r'` produced
/// it: every matching line, so a duplicated header can never equal the
/// single expected value.
pub(crate) fn location_value(block: &str) -> String {
    joined(grep_lines(block, "location: ").into_iter().map(strip_name))
}

/// The claim-state cookie's value as
/// `grep -i '^set-cookie: cabin_claim_state=' | sed
/// 's/^[^:]*: cabin_claim_state=\([^;]*\);.*/\1/' | tr -d '\r'`
/// produced it.
fn claim_state_value(block: &str) -> String {
    joined(
        grep_lines(block, "set-cookie: cabin_claim_state=")
            .into_iter()
            .map(strip_claim_state),
    )
}

/// `| tr -d '\r'` over a multi-line capture: the lines join with a
/// newline first, exactly as the command substitution saw them.
fn joined<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    lines.collect::<Vec<_>>().join("\n").replace('\r', "")
}

/// `sed 's/^[^:]*: cabin_claim_state=\([^;]*\);.*/\1/'`: the value up to
/// the first `;`.  A cookie carrying no attribute at all matches
/// nothing, and `sed` then passes the whole line through unchanged.
fn strip_claim_state(line: &str) -> &str {
    let Some(colon) = line.find(':') else {
        return line;
    };
    let Some(rest) = line[colon..].strip_prefix(": cabin_claim_state=") else {
        return line;
    };
    match rest.find(';') {
        Some(end) => &rest[..end],
        None => line,
    }
}

/// `sed -n 's/.*[?&]state=\([0-9a-f]*\).*/\1/p'`: the leading `.*` is
/// greedy, so the *last* `state=` parameter in a line wins, and the
/// capture is the lower-case hex run right after it.  A line with no
/// match prints nothing at all.
pub(crate) fn state_value(location: &str) -> String {
    location
        .split('\n')
        .filter_map(state_in_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn state_in_line(line: &str) -> Option<&str> {
    let at = [line.rfind("?state="), line.rfind("&state=")]
        .into_iter()
        .flatten()
        .max()?;
    let rest = &line[at + "?state=".len()..];
    let end = rest
        .find(|character: char| !matches!(character, '0'..='9' | 'a'..='f'))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = concat!(
        "HTTP/1.1 302 Found\r\n",
        "location: http://127.0.0.1:8790/login/oauth/authorize\
         ?client_id=smoke&scope=read%3Aorg&state=00ff1234\r\n",
        "set-cookie: cabin_claim_state=sealed.value; Path=/callback/claim; \
         HttpOnly; Secure; SameSite=Lax\r\n",
        "\r\n",
    );

    #[test]
    fn the_location_value_drops_the_name_and_the_cr() {
        let location = location_value(BLOCK);
        assert!(location.starts_with("http://127.0.0.1:8790/login/oauth/authorize?"));
        assert!(!location.contains('\r'));
        assert_eq!(location_value("HTTP/1.1 200 OK\r\n\r\n"), "");
    }

    /// A duplicated header must not collapse into one value: the
    /// comparison against the single expected string has to fail.
    #[test]
    fn duplicate_headers_join_rather_than_collapse() {
        let block = "HTTP/1.1 302 Found\r\nLocation: /a\r\nlocation: /a\r\n\r\n";
        assert_eq!(location_value(block), "/a\n/a");
    }

    #[test]
    fn the_claim_cookie_value_stops_at_the_first_attribute() {
        assert_eq!(claim_state_value(BLOCK), "sealed.value");
        assert_eq!(claim_state_value("HTTP/1.1 200 OK\r\n\r\n"), "");
    }

    /// `sed` leaves a line its pattern does not match alone, so a
    /// cookie with no attributes yields the whole header line.
    #[test]
    fn an_attributeless_claim_cookie_keeps_the_line() {
        let block = "set-cookie: cabin_claim_state=bare\r\n";
        assert_eq!(
            claim_state_value(block),
            "set-cookie: cabin_claim_state=bare"
        );
    }

    #[test]
    fn the_state_is_the_last_lower_hex_parameter() {
        assert_eq!(state_value(&location_value(BLOCK)), "00ff1234");
        assert_eq!(state_value("/x?state=abc&y=1"), "abc");
        assert_eq!(state_value("/x?state=abc&state=def0"), "def0");
        // Upper-case hex is outside `[0-9a-f]`, and the run stops there.
        assert_eq!(state_value("/x?state=aBc"), "a");
        assert_eq!(state_value("/x?nostate=abc"), "");
        assert_eq!(state_value("/login/oauth/authorize?client_id=1"), "");
    }

    #[test]
    fn the_refusal_prefix_is_case_insensitive_and_anchored() {
        assert!(refuses(
            "HTTP/1.1 302 Found\r\nLocation: /dashboard?claim=denied\r\n\r\n"
        ));
        assert!(!refuses(
            "HTTP/1.1 302 Found\r\nlocation: /dashboard?claim=granted\r\n\r\n"
        ));
        assert!(!refuses("x-echo: location: /dashboard?claim=denied\r\n"));
    }

    #[test]
    fn the_minted_token_must_be_the_plaintext_shape() {
        let body = br#"{"id":7,"token":"cabin_abc","name":"smoke-session"}"#;
        assert_eq!(
            minted(body).expect("minted"),
            ("cabin_abc".to_owned(), "7".to_owned())
        );
        assert_eq!(
            minted(br#"{"id":7}"#).expect_err("no token").to_string(),
            r#"create response carries no plaintext token: {"id":7}"#
        );
        assert_eq!(
            minted(br#"{"token":"nope"}"#)
                .expect_err("wrong prefix")
                .to_string(),
            r#"create response carries no plaintext token: {"token":"nope"}"#
        );
        // A document that does not parse takes the same message.
        assert!(minted(b"<html>").is_err());
    }

    /// The id is rendered as `console.log` rendered it: a string id
    /// keeps its own text rather than gaining JSON quotes.
    #[test]
    fn a_string_id_keeps_its_text() {
        let body = br#"{"id":"tok_1","token":"cabin_x"}"#;
        assert_eq!(minted(body).expect("minted").1, "tok_1");
    }
}
