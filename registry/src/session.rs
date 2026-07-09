//! HMAC-signed values for the browser plane: the OAuth `state` cookie, the
//! session cookie, and the CSRF token tied to a session.
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
/// Cookie carrying the sealed session after a successful sign-in.
pub const SESSION_COOKIE: &str = "cabin_session";
/// The state cookie lives just long enough to complete the OAuth dance.
pub const STATE_MAX_AGE_SECS: u64 = 600;
/// Sessions last eight hours; signing in again is cheap.
pub const SESSION_MAX_AGE_SECS: u64 = 8 * 60 * 60;

/// A verified session: who, and until when. The numeric GitHub id is the
/// identity; the login name is display-only and lives in D1.
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

/// The CSRF token for `session`: an HMAC over the session fields under its
/// own purpose, embedded as a hidden form field and recomputed server-side
/// on every POST. Deriving it from the session means it needs no storage
/// and dies with the session.
pub fn csrf_token(secret: &[u8], session: &Session) -> String {
    mac_hex(
        secret,
        "csrf",
        &format!("{}:{}", session.github_id, session.expires_at),
    )
}

/// Whether a presented CSRF form field matches `session`, in constant
/// time.
pub fn csrf_matches(secret: &[u8], session: &Session, presented: &str) -> bool {
    mac_eq(&csrf_token(secret, session), presented)
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
    }

    #[test]
    fn purposes_are_not_interchangeable() {
        // A state cookie must never open as a session, however its
        // payload happens to parse, and vice versa.
        let state = seal_state(SECRET, "42", 2_000);
        assert_eq!(open_session(SECRET, &state, 0), None);
        let session = seal_session(SECRET, 42, 2_000);
        assert_eq!(open_state(SECRET, &session, 0), None);
    }

    #[test]
    fn csrf_round_trips_and_rejects_tampering() {
        let session = Session {
            github_id: 583_231,
            expires_at: 2_000,
        };
        let token = csrf_token(SECRET, &session);
        assert!(csrf_matches(SECRET, &session, &token));
        assert!(!csrf_matches(SECRET, &session, &token.to_uppercase()));
        assert!(!csrf_matches(SECRET, &session, &token[..token.len() - 1]));
        assert!(!csrf_matches(SECRET, &session, ""));
        let other = Session {
            github_id: 583_232,
            expires_at: 2_000,
        };
        assert!(!csrf_matches(SECRET, &other, &token));
    }

    #[test]
    fn csrf_is_not_the_session_mac() {
        // The session cookie's own MAC (which the browser holds) must not
        // pass as the CSRF field: purposes separate the two.
        let sealed = seal_session(SECRET, 583_231, 2_000);
        let session = open_session(SECRET, &sealed, 0).unwrap();
        let (_, session_mac) = sealed.rsplit_once('.').unwrap();
        assert!(!csrf_matches(SECRET, &session, session_mac));
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
