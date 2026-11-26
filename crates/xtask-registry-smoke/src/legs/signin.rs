//! The sign-in callback end to end against the GitHub mock: an
//! existing user signs in whatever their account age, a fresh account
//! under the sign-up gate's 90 days is refused with the first eligible
//! date, a fresh account with no creation time or off the allowlist is
//! refused with no account, and a fresh account past them is created
//! and signed in (`api/src/signup.rs`). Each granted sign-in also reads
//! the account's notification address
//! (`api/src/notification_email.rs`), and so does a login-session mint.

use anyhow::{Context as _, Result, bail};

use crate::context::{Base, Smoke};
use crate::legs::claims::{location_value, state_value};
use crate::legs::session::{csrf_headers, session_request};
use crate::servers::{d1, d1_rows};
use crate::step;
use crate::text::{capture, first_line, grep_lines, status_line_is, text};

/// The callback's admission roundtrips. Only the `fresh-old` sign-in
/// writes an identity row (github id 4), and the seed removes that
/// account before a rerun (`servers.rs`), so every run creates it. The
/// mock's `/user` is restored before the notification-address checks.
///
/// # Errors
///
/// The first failed check.
pub fn run(smoke: &mut Smoke, github_port: u16) -> Result<()> {
    step("a first sign-in needs an old enough, allowlisted account; an existing user's does not");
    // The seeded user (github id 0) is an existing account: the gate
    // must not consult its age, so a freshly-created profile must
    // still land on the dashboard with a session.
    signin_mode(smoke, github_port, "existing-young")?;
    let block = callback_roundtrip(smoke)?;
    expect_dashboard(smoke, &block, "an existing user's young-account sign-in")?;
    minted_session_lifecycle(smoke, &block)?;

    // A fresh allowlisted account five days old: refused, no session,
    // and the one redirect that names its reason and the first
    // eligible UTC date.
    signin_mode(smoke, github_port, "fresh-young")?;
    let block = callback_roundtrip(smoke)?;
    let location = location_value(&block);
    let Some(date) = location.strip_prefix("/login/denied/?reason=account-age&eligible=") else {
        bail!("a young account's sign-in answered '{location}', expected the account-age refusal");
    };
    let dated = date.len() == 10
        && date.bytes().enumerate().all(|(at, byte)| match at {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        });
    if !dated {
        bail!("the account-age refusal names no yyyy-mm-dd date: {location}");
    }
    if !session_cookies(&block).is_empty() {
        bail!(
            "a refused sign-in set a session cookie: {}",
            capture(&smoke.headers)
        );
    }

    // The two other refusals of a first sign-in: a profile with no
    // creation time (id 3 is allowlisted and has no account), and an
    // old account that the allowlist does not admit (id 5).  Neither
    // gets a session or an account.
    for (mode, id, check) in [
        ("fresh-undated", "3", "the creation-time gate"),
        ("fresh-unlisted", "5", "the allowlist"),
    ] {
        let users = d1_rows("SELECT COUNT(*) AS n FROM users")?;
        signin_mode(smoke, github_port, mode)?;
        let block = callback_roundtrip(smoke)?;
        let location = location_value(&block);
        if location != "/login/denied/" || !session_cookies(&block).is_empty() {
            bail!(
                "{check} did not refuse the sign-in: '{location}' {}",
                capture(&smoke.headers)
            );
        }
        let created = d1_rows(&format!(
            "SELECT 1 FROM identities WHERE provider = 'github' AND provider_account_id = '{id}'"
        ))?;
        if !created.is_empty() || d1_rows("SELECT COUNT(*) AS n FROM users")? != users {
            bail!("{check} refused the sign-in but still created an account");
        }
    }

    // The same fresh path past the age line: account creation works.
    signin_mode(smoke, github_port, "fresh-old")?;
    let block = callback_roundtrip(smoke)?;
    expect_dashboard(smoke, &block, "an old-enough fresh account's sign-in")?;
    // The sign-in that creates an account stores its address with it.
    let created = stored_address("4")?;
    if created.as_deref() != Some("smoke@example.com") {
        bail!("the created account does not hold the verified primary address: {created:?}");
    }

    signin_mode(smoke, github_port, "off")?;
    notification_address(smoke, github_port)
}

/// What a sign-in and a login-session mint do to the seeded account's
/// notification address, under each answer the mock's `/user/emails`
/// gives. The steps run in this order because the later ones need a
/// stored address to keep.
fn notification_address(smoke: &mut Smoke, github_port: u16) -> Result<()> {
    const SMOKE: &str = r#""notification_email":"smoke@example.com""#;
    const MOVED: &str = r#""notification_email":"moved@example.com""#;
    step("a sign-in stores the verified primary address and keeps it through an outage");
    // The primary entry is on the listing's second page.
    signed_in_address(smoke, github_port, "listed", SMOKE)?;
    // The same address again is no write.
    with_address_writes_refused(|| signed_in_address(smoke, github_port, "listed", SMOKE))?;
    signed_in_address(smoke, github_port, "moved", MOVED)?;
    // GitHub not answering, and a grant without the scope, say nothing
    // about the address: the stored one stands after a sign-in and
    // after a login-session mint, and neither refuses the mint - not
    // even a listing that never answers. The unscoped listing is there
    // to read.
    signed_in_address(smoke, github_port, "down", MOVED)?;
    signed_in_address(smoke, github_port, "unscoped", MOVED)?;
    minted_address(smoke, github_port, "stalled", Some("moved@example.com"))?;
    minted_address(smoke, github_port, "unscoped", Some("moved@example.com"))?;
    // GitHub answering that the primary address is unverified clears it.
    signed_in_address(
        smoke,
        github_port,
        "unverified",
        r#""notification_email":null"#,
    )?;

    step("a login-session mint stores the verified primary address");
    // These mints leave the mock on its default listing.
    minted_address(smoke, github_port, "listed", Some("smoke@example.com"))?;
    with_address_writes_refused(|| {
        minted_address(smoke, github_port, "listed", Some("smoke@example.com"))
    })
}

/// Runs `check` with every write of the address column refused, so a
/// batch that writes an unchanged address again fails instead of
/// committing.
fn with_address_writes_refused(check: impl FnOnce() -> Result<()>) -> Result<()> {
    d1("CREATE TRIGGER smoke_refuse_notification_email \
        BEFORE UPDATE OF notification_email ON users \
        BEGIN SELECT RAISE(ABORT, 'smoke'); END")?;
    let outcome = check();
    d1("DROP TRIGGER smoke_refuse_notification_email")?;
    outcome
}

/// Mints a login session for the seeded account under the mock's
/// `emails` mode, revokes it, and expects the account to hold the
/// `expected` notification address afterwards.
fn minted_address(
    smoke: &mut Smoke,
    github_port: u16,
    emails: &str,
    expected: Option<&str>,
) -> Result<()> {
    mock_post(smoke, github_port, &format!("/__emails/{emails}"))?;
    let url = smoke.url(Base::Web, "/api/v1/sessions/tokens");
    let status = smoke.http("PUT", &url, &[], Some(br#"{"github_token":"gho_smoke"}"#))?;
    if status != 200 {
        bail!("a session mint under '{emails}' e-mails answered {status}, expected 200");
    }
    let minted: serde_json::Value =
        serde_json::from_slice(&smoke.body).context("parse the mint answer")?;
    let token = minted["token"]
        .as_str()
        .context("no token in the mint answer")?;
    let auth = vec![("Authorization".to_owned(), format!("Bearer {token}"))];
    let status = smoke.http("DELETE", &url, &auth, None)?;
    if status != 204 {
        bail!("revoking the minted session answered {status}, expected 204");
    }
    let stored = stored_address("0")?;
    if stored.as_deref() != expected {
        bail!("a session mint under '{emails}' e-mails left {stored:?}, expected {expected:?}");
    }
    Ok(())
}

/// The notification address stored for a GitHub account id.
fn stored_address(github_id: &str) -> Result<Option<String>> {
    let rows = d1_rows(&format!(
        "SELECT u.notification_email AS address FROM users u \
         JOIN identities i ON i.user_id = u.id WHERE i.provider_account_id = '{github_id}'"
    ))?;
    Ok(rows
        .first()
        .and_then(|row| row["address"].as_str())
        .map(str::to_owned))
}

/// Signs the seeded account in under the mock's `emails` mode and
/// expects `/api/v1/user`, read with the session that sign-in minted,
/// to carry `expected`.
fn signed_in_address(
    smoke: &mut Smoke,
    github_port: u16,
    emails: &str,
    expected: &str,
) -> Result<()> {
    mock_post(smoke, github_port, &format!("/__emails/{emails}"))?;
    let block = callback_roundtrip(smoke)?;
    expect_dashboard(
        smoke,
        &block,
        &format!("a sign-in under '{emails}' e-mails"),
    )?;
    let cookie = minted_cookie(&block)?;
    session_request(smoke, &cookie, "GET", "/api/v1/user", 200, &[], None)?;
    smoke.expect_body(expected)
}

/// Drives `/api/auth/login` -> `/api/auth/callback` the way the claim
/// leg drives its own roundtrip: capture the sealed state cookie and the authorize
/// redirect's `state`, replay both, and hand back the callback's
/// header block.  Neither hop follows the redirect: the 302 *is* the
/// subject.
fn callback_roundtrip(smoke: &mut Smoke) -> Result<String> {
    let url = smoke.url(Base::Web, "/api/auth/login");
    smoke.http("GET", &url, &[], None)?;
    let block = text(&smoke.headers).into_owned();
    if !status_line_is(&block, 302) {
        bail!("/api/auth/login did not answer 302: {}", first_line(&block));
    }
    let state = state_value(&location_value(&block));
    if state.is_empty() {
        bail!(
            "no state in the authorize redirect: {}",
            capture(&smoke.headers)
        );
    }
    let cookie = oauth_state_value(&block);
    if cookie.is_empty() {
        bail!(
            "/api/auth/login set no state cookie: {}",
            capture(&smoke.headers)
        );
    }

    let url = smoke.url(
        Base::Web,
        &format!("/api/auth/callback?code=smoke&state={state}"),
    );
    let headers = vec![("Cookie".to_owned(), format!("cabin_oauth_state={cookie}"))];
    smoke.http("GET", &url, &headers, None)?;
    let block = text(&smoke.headers).into_owned();
    if !status_line_is(&block, 302) {
        bail!(
            "/api/auth/callback did not answer 302: {}",
            first_line(&block)
        );
    }
    // The state cookie is one-shot: cleared on every outcome.
    if !grep_lines(&block, "set-cookie: cabin_oauth_state=")
        .iter()
        .any(|line| line.contains("Max-Age=0"))
    {
        bail!(
            "the callback did not clear the state cookie: {}",
            capture(&smoke.headers)
        );
    }
    Ok(block)
}

/// A granted sign-in: the dashboard redirect carrying a session cookie.
fn expect_dashboard(smoke: &Smoke, block: &str, what: &str) -> Result<()> {
    let location = location_value(block);
    if location != "/dashboard/" {
        bail!("{what} answered '{location}', expected /dashboard/");
    }
    if session_cookies(block).is_empty() {
        bail!("{what} set no session cookie: {}", capture(&smoke.headers));
    }
    Ok(())
}

/// The cookie `/api/auth/callback` itself minted - not one this harness sealed -
/// authenticates, logs out, and is dead on replay: the session row the
/// callback inserted must be the one the cookie names.
fn minted_session_lifecycle(smoke: &mut Smoke, block: &str) -> Result<()> {
    step("a callback-minted session authenticates until it logs out");
    let cookie = minted_cookie(block)?;
    session_request(smoke, &cookie, "GET", "/api/v1/user", 200, &[], None)?;
    smoke.expect_body(r#""github_id":0"#)?;
    session_request(
        smoke,
        &cookie,
        "POST",
        "/api/v1/user/logout",
        200,
        &csrf_headers(),
        None,
    )?;
    session_request(smoke, &cookie, "GET", "/api/v1/user", 401, &[], None)
}

/// The `Cookie` header value for the session a callback minted.
fn minted_cookie(block: &str) -> Result<String> {
    let Some(value) = session_cookies(block).first().and_then(|line| {
        let (_, rest) = line.split_once(": cabin_session=")?;
        rest.split(';').next()
    }) else {
        bail!("the sign-in set no session cookie value");
    };
    Ok(format!("cabin_session={}", value.trim()))
}

/// Every `Set-Cookie` that stores a session (`Max-Age=0` clears one).
fn session_cookies(block: &str) -> Vec<&str> {
    grep_lines(block, "set-cookie: cabin_session=")
        .into_iter()
        .filter(|line| !line.contains("Max-Age=0"))
        .collect()
}

/// The oauth-state cookie's value, up to the first `;` - the shape
/// [`crate::legs::claims`] reads for its own state cookie.
fn oauth_state_value(block: &str) -> String {
    grep_lines(block, "set-cookie: cabin_oauth_state=")
        .into_iter()
        .filter_map(|line| {
            let (_, rest) = line.split_once(": cabin_oauth_state=")?;
            rest.split(';').next()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace('\r', "")
}

/// The GitHub mock's `/user` sign-in modes.
fn signin_mode(smoke: &mut Smoke, github_port: u16, mode: &str) -> Result<()> {
    mock_post(smoke, github_port, &format!("/__signin/{mode}"))
}

/// One of the GitHub mock's toggles; like the drift toggle, neither
/// dev role, so addressed by port rather than a base.
fn mock_post(smoke: &mut Smoke, github_port: u16, path: &str) -> Result<()> {
    let url = format!("http://127.0.0.1:{github_port}{path}");
    let status = smoke.http("POST", &url, &[], None)?;
    if status >= 400 {
        bail!("POST {url} returned {status}");
    }
    Ok(())
}
