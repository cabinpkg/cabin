//! The sign-in callback end to end against the GitHub mock: an
//! existing user signs in whatever their account age, a fresh account
//! under the sign-up gate's 30 days is refused with the first eligible
//! date, and a fresh account past them is created and signed in
//! (`registry/src/signup.rs`).  Post-migration coverage with no
//! `smoke.sh` ancestor - the shell only ever drove `/callback` to its
//! parameterless refusal.

use anyhow::{Result, bail};

use crate::context::{Base, Smoke};
use crate::legs::claims::{location_value, state_value};
use crate::step;
use crate::text::{capture, first_line, grep_lines, status_line_is, text};

/// The three roundtrips, in an order that keeps reruns green: only the
/// `fresh-old` sign-in writes an identity row (github id 4), and a
/// rerun then takes the existing-user path to the same `/dashboard`.
/// The mock's `/user` is restored on the way out.
///
/// # Errors
///
/// The first failed check.
pub fn run(smoke: &mut Smoke, github_port: u16) -> Result<()> {
    step("the sign-up gate refuses young accounts and leaves the rest alone");
    // The seeded user (github id 0) is an existing account: the gate
    // must not consult its age, so a freshly-created profile must
    // still land on the dashboard with a session.
    signin_mode(smoke, github_port, "existing-young")?;
    let block = callback_roundtrip(smoke)?;
    expect_dashboard(smoke, &block, "an existing user's young-account sign-in")?;

    // A fresh allowlisted account five days old: refused, no session,
    // and the one redirect that names its reason and the first
    // eligible UTC date.
    signin_mode(smoke, github_port, "fresh-young")?;
    let block = callback_roundtrip(smoke)?;
    let location = location_value(&block);
    let Some(date) = location.strip_prefix("/login/denied?reason=account-age&eligible=") else {
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

    // The same fresh path past the age line: account creation works.
    signin_mode(smoke, github_port, "fresh-old")?;
    let block = callback_roundtrip(smoke)?;
    expect_dashboard(smoke, &block, "an old-enough fresh account's sign-in")?;

    signin_mode(smoke, github_port, "off")
}

/// Drives `/login` -> `/callback` the way the claim leg drives its own
/// callback: capture the sealed state cookie and the authorize
/// redirect's `state`, replay both, and hand back the callback's
/// header block.  Neither hop follows the redirect: the 302 *is* the
/// subject.
fn callback_roundtrip(smoke: &mut Smoke) -> Result<String> {
    let url = smoke.url(Base::Web, "/login");
    smoke.http("GET", &url, &[], None)?;
    let block = text(&smoke.headers).into_owned();
    if !status_line_is(&block, 302) {
        bail!("/login did not answer 302: {}", first_line(&block));
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
        bail!("/login set no state cookie: {}", capture(&smoke.headers));
    }

    let url = smoke.url(Base::Web, &format!("/callback?code=smoke&state={state}"));
    let headers = vec![("Cookie".to_owned(), format!("cabin_oauth_state={cookie}"))];
    smoke.http("GET", &url, &headers, None)?;
    let block = text(&smoke.headers).into_owned();
    if !status_line_is(&block, 302) {
        bail!("/callback did not answer 302: {}", first_line(&block));
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
    if location != "/dashboard" {
        bail!("{what} answered '{location}', expected /dashboard");
    }
    if session_cookies(block).is_empty() {
        bail!("{what} set no session cookie: {}", capture(&smoke.headers));
    }
    Ok(())
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

/// The GitHub mock's `/user` sign-in modes; like the drift toggle,
/// neither dev role, so addressed by port rather than a base.
fn signin_mode(smoke: &mut Smoke, github_port: u16, mode: &str) -> Result<()> {
    let url = format!("http://127.0.0.1:{github_port}/__signin/{mode}");
    let status = smoke.http("POST", &url, &[], None)?;
    if status >= 400 {
        bail!("POST {url} returned {status}");
    }
    Ok(())
}
