//! HMAC-signed values for the browser plane - the OAuth `state` cookie and
//! the session cookie - plus the cookie shape and the JSON API's CSRF
//! discipline.
//!
//! Every sealed value is `<payload>.<mac-hex>` with the MAC (HMAC-SHA-256
//! keyed by `SESSION_SECRET`) computed over `<purpose>:<payload>`, so a
//! value sealed for one purpose can never be replayed as another. Payloads
//! are plain `:`-separated fields carrying their own expiry; MAC checks are
//! constant-time.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::auth::hex;

type HmacSha256 = Hmac<Sha256>;

/// Cookie carrying the sealed OAuth `state` between `/login` and
/// `/callback`.
pub const STATE_COOKIE: &str = "cabin_oauth_state";
/// Cookie carrying the sealed claim `state` (which also names the scope
/// being claimed) between `/claim/<scope>` and `/callback/claim`.
pub const CLAIM_STATE_COOKIE: &str = "cabin_claim_state";
/// Cookie carrying the sealed session after a successful sign-in.
pub const SESSION_COOKIE: &str = "cabin_session";
/// The state cookies live just long enough to complete the OAuth dance.
pub const STATE_MAX_AGE_SECS: u64 = 600;
/// Sessions last eight hours; signing in again is cheap.
pub const SESSION_MAX_AGE_SECS: u64 = 8 * 60 * 60;
/// Minimum configured HMAC key length. Provisioning generates 32 random
/// bytes (base64-encoded); this catches truncated or placeholder secrets.
pub const MIN_SECRET_BYTES: usize = 32;

/// Whether a configured session secret meets the minimum key length.
#[must_use]
pub fn secret_has_minimum_length(secret: &[u8]) -> bool {
    secret.len() >= MIN_SECRET_BYTES
}

/// A verified session: who, and until when. The numeric GitHub id names
/// the external identity, resolved through the `identities` table to
/// the registry-native user on every request - deliberately not the
/// `users.id`, which a pre-launch wipe would re-issue: a still-valid
/// ghost cookie sealed over a row id could then bind to whoever
/// received that id after the wipe.
#[derive(Debug, PartialEq, Eq)]
pub struct Session {
    pub github_id: i64,
    pub expires_at: u64,
}

fn mac_hex(secret: &[u8], purpose: &str, payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    mac.update(purpose.as_bytes());
    mac.update(b":");
    mac.update(payload.as_bytes());
    hex(&mac.finalize().into_bytes())
}

/// Constant-time equality of two MAC hex strings.
fn mac_eq(expected: &str, presented: &str) -> bool {
    expected.len() == presented.len()
        && expected
            .bytes()
            .zip(presented.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn seal(secret: &[u8], purpose: &str, payload: &str) -> String {
    format!("{payload}.{mac}", mac = mac_hex(secret, purpose, payload))
}

fn open<'a>(secret: &[u8], purpose: &str, sealed: &'a str) -> Option<&'a str> {
    let (payload, mac) = sealed.rsplit_once('.')?;
    mac_eq(&mac_hex(secret, purpose, payload), mac).then_some(payload)
}

/// Seals the OAuth `state` into the state-cookie value.
pub fn seal_state(secret: &[u8], state: &str, expires_at: u64) -> String {
    seal(secret, "state", &format!("{state}:{expires_at}"))
}

/// Opens a state-cookie value, returning the state while it is unexpired.
pub fn open_state(secret: &[u8], sealed: &str, now: u64) -> Option<String> {
    let payload = open(secret, "state", sealed)?;
    let (state, expires_at) = payload.rsplit_once(':')?;
    let expires_at: u64 = expires_at.parse().ok()?;
    (!state.is_empty() && now < expires_at).then(|| state.to_owned())
}

/// Seals a claim's OAuth `state` plus the scope being claimed into the
/// claim-state-cookie value. The scope rides inside the sealed payload -
/// never a query parameter the callback would have to trust - so the
/// callback grants exactly the scope the initiate route validated.
pub fn seal_claim_state(secret: &[u8], scope: &str, state: &str, expires_at: u64) -> String {
    seal(secret, "claim", &format!("{scope}:{state}:{expires_at}"))
}

/// Opens a claim-state-cookie value, returning `(scope, state)` while it
/// is unexpired.
pub fn open_claim_state(secret: &[u8], sealed: &str, now: u64) -> Option<(String, String)> {
    let payload = open(secret, "claim", sealed)?;
    let (rest, expires_at) = payload.rsplit_once(':')?;
    let (scope, state) = rest.rsplit_once(':')?;
    let expires_at: u64 = expires_at.parse().ok()?;
    (!scope.is_empty() && !state.is_empty() && now < expires_at)
        .then(|| (scope.to_owned(), state.to_owned()))
}

/// Seals a session into the session-cookie value.
pub fn seal_session(secret: &[u8], github_id: i64, expires_at: u64) -> String {
    seal(secret, "session", &format!("{github_id}:{expires_at}"))
}

/// Opens a session-cookie value while it is unexpired.
pub fn open_session(secret: &[u8], sealed: &str, now: u64) -> Option<Session> {
    let payload = open(secret, "session", sealed)?;
    let (github_id, expires_at) = payload.rsplit_once(':')?;
    let session = Session {
        github_id: github_id.parse().ok()?,
        expires_at: expires_at.parse().ok()?,
    };
    (now < session.expires_at).then_some(session)
}

/// The session-plane mutation CSRF discipline, suited to a JSON API: the
/// request must declare `Content-Type: application/json` (media type,
/// parameters ignored) **and** carry [`CSRF_HEADER`]`: 1`. Neither can
/// ride on an HTML form or a simple cross-site request, and a cross-site
/// `fetch` that adds them triggers a CORS preflight the registry never
/// answers - so with `SameSite=Lax` host-only cookies no server-side
/// token state is needed. Checked before the body is read.
pub fn csrf_headers_ok(content_type: Option<&str>, csrf_header: Option<&str>) -> bool {
    let json = content_type.is_some_and(|value| {
        value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case("application/json")
    });
    json && csrf_header.is_some_and(|value| value.trim() == "1")
}

/// The custom request header session-plane mutations must carry (value
/// exactly `1`).
pub const CSRF_HEADER: &str = "x-csrf-protection";

/// The session cookie only ever travels to the session-plane subtree,
/// and each state cookie only to its own callback: even the website
/// origin's own page loads never carry any of them, so the website
/// Worker never sees a session. (`Path=/callback` also cookie-matches
/// `/callback/claim`, which reads only the claim cookie - distinct
/// names and MAC purposes keep the two states apart regardless.)
pub const SESSION_COOKIE_PATH: &str = "/api/v1/user";
pub const STATE_COOKIE_PATH: &str = "/callback";
pub const CLAIM_STATE_COOKIE_PATH: &str = "/callback/claim";

/// `Set-Cookie` value for a browser-plane cookie. Host-only on purpose:
/// no `Domain` attribute, so the cookie never flows to registry
/// subdomains. `SameSite=Lax` keeps cross-site POSTs cookie-less (the
/// CSRF header pair is the second factor); `Max-Age=0` clears (with the
/// same `Path`, or the browser keeps the original).
pub fn set_cookie(name: &str, value: &str, max_age_secs: u64, path: &str) -> String {
    format!("{name}={value}; Max-Age={max_age_secs}; Path={path}; HttpOnly; Secure; SameSite=Lax")
}

/// Extracts the value of the cookie named `name` from a `Cookie` header.
pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').map(str::trim).find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"test-session-secret";

    #[test]
    fn state_round_trips_until_expiry() {
        let sealed = seal_state(SECRET, "a1b2c3", 1_000);
        assert_eq!(open_state(SECRET, &sealed, 999), Some("a1b2c3".to_owned()));
        assert_eq!(open_state(SECRET, &sealed, 1_000), None);
        assert_eq!(open_state(SECRET, &sealed, 2_000), None);
    }

    #[test]
    fn configured_secret_requires_at_least_32_bytes() {
        assert!(!secret_has_minimum_length(&[0; MIN_SECRET_BYTES - 1]));
        assert!(secret_has_minimum_length(&[0; MIN_SECRET_BYTES]));
    }

    #[test]
    fn claim_state_round_trips_until_expiry() {
        let sealed = seal_claim_state(SECRET, "fmtlib", "a1b2c3", 1_000);
        assert_eq!(
            open_claim_state(SECRET, &sealed, 999),
            Some(("fmtlib".to_owned(), "a1b2c3".to_owned()))
        );
        assert_eq!(open_claim_state(SECRET, &sealed, 1_000), None);
    }

    #[test]
    fn session_round_trips_until_expiry() {
        let sealed = seal_session(SECRET, 583_231, 2_000);
        assert_eq!(
            open_session(SECRET, &sealed, 1_999),
            Some(Session {
                github_id: 583_231,
                expires_at: 2_000
            })
        );
        assert_eq!(open_session(SECRET, &sealed, 2_000), None);
    }

    #[test]
    fn tampering_with_any_part_is_rejected() {
        let sealed = seal_session(SECRET, 583_231, 2_000);
        let (payload, mac) = sealed.rsplit_once('.').unwrap();
        for tampered in [
            format!("583232:2000.{mac}"),
            format!("583231:9999.{mac}"),
            format!("{payload}.{}", mac.to_uppercase()),
            format!("{payload}.{}", &mac[..mac.len() - 1]),
            payload.to_owned(),
            String::new(),
        ] {
            assert_eq!(open_session(SECRET, &tampered, 0), None, "{tampered}");
        }
        assert_eq!(open_session(b"other-secret", &sealed, 0), None);
    }

    #[test]
    fn correctly_signed_but_malformed_payloads_are_rejected() {
        // A valid MAC does not rescue a payload whose fields do not parse
        // (or an empty state): these only arise from a signer bug, and
        // must fail closed rather than open as Some.
        for payload in ["", ":1000", "abc:notanumber", "abc"] {
            let sealed = seal(SECRET, "state", payload);
            assert_eq!(open_state(SECRET, &sealed, 0), None, "{payload:?}");
        }
        for payload in ["notanumber:1000", "42:notanumber", "42", ""] {
            let sealed = seal(SECRET, "session", payload);
            assert_eq!(open_session(SECRET, &sealed, 0), None, "{payload:?}");
        }
        for payload in ["", "fmtlib:1000", ":abc:1000", "fmtlib::1000", "a:b:c"] {
            let sealed = seal(SECRET, "claim", payload);
            assert_eq!(open_claim_state(SECRET, &sealed, 0), None, "{payload:?}");
        }
    }

    #[test]
    fn purposes_are_not_interchangeable() {
        // A state cookie must never open as a session or a claim state,
        // however its payload happens to parse - and so on for every
        // other pair of purposes.
        let state = seal_state(SECRET, "42", 2_000);
        assert_eq!(open_session(SECRET, &state, 0), None);
        assert_eq!(open_claim_state(SECRET, &state, 0), None);
        let session = seal_session(SECRET, 42, 2_000);
        assert_eq!(open_state(SECRET, &session, 0), None);
        assert_eq!(open_claim_state(SECRET, &session, 0), None);
        let claim = seal_claim_state(SECRET, "fmtlib", "42", 2_000);
        assert_eq!(open_state(SECRET, &claim, 0), None);
        assert_eq!(open_session(SECRET, &claim, 0), None);
    }

    #[test]
    fn csrf_requires_the_json_content_type_and_the_custom_header() {
        assert!(csrf_headers_ok(Some("application/json"), Some("1")));
        assert!(csrf_headers_ok(
            Some("application/json; charset=utf-8"),
            Some("1")
        ));
        assert!(csrf_headers_ok(Some("Application/JSON"), Some("1")));
        // Either half missing or wrong fails; the header value is
        // exactly `1`.
        assert!(!csrf_headers_ok(None, Some("1")));
        assert!(!csrf_headers_ok(Some("application/json"), None));
        assert!(!csrf_headers_ok(Some("application/json"), Some("")));
        assert!(!csrf_headers_ok(Some("application/json"), Some("yes")));
        assert!(!csrf_headers_ok(Some("text/plain"), Some("1")));
        // The form content types a cross-site POST can send without a
        // preflight must never pass.
        assert!(!csrf_headers_ok(
            Some("application/x-www-form-urlencoded"),
            Some("1")
        ));
        assert!(!csrf_headers_ok(Some("multipart/form-data"), Some("1")));
        assert!(!csrf_headers_ok(None, None));
    }

    #[test]
    fn cookies_are_host_only_with_the_locked_attributes() {
        let cookie = set_cookie(SESSION_COOKIE, "v.mac", 3_600, SESSION_COOKIE_PATH);
        assert_eq!(
            cookie,
            "cabin_session=v.mac; Max-Age=3600; Path=/api/v1/user; HttpOnly; Secure; SameSite=Lax"
        );
        // Host-only is the point: a `Domain` attribute would leak the
        // website origin's cookies to registry subdomains.
        assert!(!cookie.to_ascii_lowercase().contains("domain"));
        // Clearing uses the same shape with Max-Age=0.
        assert!(set_cookie(STATE_COOKIE, "", 0, STATE_COOKIE_PATH).contains("Max-Age=0"));
    }

    #[test]
    fn cookie_value_finds_the_named_cookie() {
        let header = "a=1; cabin_session=x.y; b=2";
        assert_eq!(cookie_value(header, "cabin_session"), Some("x.y"));
        assert_eq!(cookie_value(header, "a"), Some("1"));
        assert_eq!(cookie_value(header, "cabin"), None);
        assert_eq!(cookie_value("", "cabin_session"), None);
    }
}
