//! The account's notification address: the verified primary e-mail of
//! its GitHub account, read at each interactive sign-in - the
//! website's, or `cabin login`'s session mint - and kept in
//! `users.notification_email`. It is the address for the account's
//! notices and nothing more: the account stays the numeric GitHub id,
//! and no address is ever matched against another account's, served to
//! anyone but its owner, or logged.

use std::time::Duration;

use serde::Deserialize;

/// The OAuth scope the read needs.
pub const SCOPE: &str = "user:email";

/// Entries asked for per page of `GET /user/emails`, GitHub's maximum.
pub const PAGE_SIZE: usize = 100;

/// Pages read before the search for the primary entry gives up.
pub const MAX_PAGES: usize = 10;

/// How long a sign-in or a session mint waits for the whole read
/// before it goes on without one: a GitHub that stalls must not stall
/// its caller, and `cabin login` gives its mint 30 seconds in all.
pub const DEADLINE: Duration = Duration::from_secs(5);

/// What a sign-in learned about the address.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// GitHub's verified primary address.
    Address(String),
    /// The primary address is not verified.
    Unverified,
    /// GitHub lists no primary address the registry can use.
    NoPrimary,
    /// The sign-in's token was not granted [`SCOPE`].
    NoScope,
    /// GitHub did not answer the read.
    Failed,
}

impl Lookup {
    /// Why no address was read, for the log.
    pub fn unavailable(&self) -> Option<&'static str> {
        match self {
            Lookup::Address(_) => None,
            Lookup::Unverified => Some("the primary address is not verified"),
            Lookup::NoPrimary => Some("no usable primary address"),
            Lookup::NoScope => Some("the user:email scope was not granted"),
            Lookup::Failed => Some("the e-mail read failed"),
        }
    }
}

/// Whether scopes GitHub granted a token cover the read: one scope, or
/// several comma-separated as its token response lists them. `user`
/// includes `user:email`.
pub fn scope_granted(granted: &str) -> bool {
    granted
        .split(',')
        .any(|scope| matches!(scope.trim(), SCOPE | "user"))
}

/// What one page of the listing settles.
#[derive(Debug, PartialEq, Eq)]
pub enum Page {
    Settled(Lookup),
    /// A full page without the primary entry: it is on a later one.
    Next,
}

#[derive(Deserialize)]
struct Entry {
    email: String,
    primary: bool,
    verified: bool,
}

/// Reads one page of `GET /user/emails`. The primary entry settles the
/// lookup wherever it sits on the page; a short page without one is the
/// end of the listing. A verified primary entry that is not one plain
/// address ([`crate::alerts::is_address`]) is no address to mail.
pub fn read_page(body: &[u8]) -> Page {
    let Ok(entries) = serde_json::from_slice::<Vec<Entry>>(body) else {
        return Page::Settled(Lookup::Failed);
    };
    match entries.iter().find(|entry| entry.primary) {
        Some(primary) if !primary.verified => Page::Settled(Lookup::Unverified),
        Some(primary) if crate::alerts::is_address(&primary.email) => {
            Page::Settled(Lookup::Address(primary.email.clone()))
        }
        Some(_) => Page::Settled(Lookup::NoPrimary),
        None if entries.len() < PAGE_SIZE => Page::Settled(Lookup::NoPrimary),
        None => Page::Next,
    }
}

/// The value to store after a sign-in, or `None` while the stored one
/// stands. An answer GitHub did not give ([`Lookup::NoScope`],
/// [`Lookup::Failed`]) changes nothing, so an outage never erases an
/// address; an authoritative answer without a usable address clears
/// it; and an unchanged address is not written again.
pub fn change<'a>(stored: Option<&str>, lookup: &'a Lookup) -> Option<Option<&'a str>> {
    let found = match lookup {
        Lookup::Address(address) => Some(address.as_str()),
        Lookup::Unverified | Lookup::NoPrimary => None,
        Lookup::NoScope | Lookup::Failed => return None,
    };
    (found != stored).then_some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(email: &str, primary: bool, verified: bool) -> serde_json::Value {
        serde_json::json!({
            "email": email,
            "primary": primary,
            "verified": verified,
            "visibility": null,
        })
    }

    fn page(entries: &[serde_json::Value]) -> Page {
        read_page(serde_json::Value::from(entries).to_string().as_bytes())
    }

    fn address(email: &str) -> Page {
        Page::Settled(Lookup::Address(email.to_owned()))
    }

    #[test]
    fn the_scope_is_read_from_what_github_granted() {
        assert!(scope_granted("user:email"));
        assert!(scope_granted("read:org,user:email"));
        assert!(scope_granted("read:org, user:email"));
        assert!(scope_granted("user"));
        assert!(!scope_granted(""));
        assert!(!scope_granted("read:org"));
        assert!(!scope_granted("read:user"));
    }

    #[test]
    fn only_the_verified_primary_entry_is_the_address() {
        // The primary entry is not the first one, and a verified
        // secondary address is never taken in its place.
        assert_eq!(
            page(&[
                entry("old@example.com", false, true),
                entry("mona@example.com", true, true),
            ]),
            address("mona@example.com")
        );
        assert_eq!(
            page(&[
                entry("mona@example.com", true, false),
                entry("old@example.com", false, true),
            ]),
            Page::Settled(Lookup::Unverified)
        );
        assert_eq!(
            page(&[entry("old@example.com", false, true)]),
            Page::Settled(Lookup::NoPrimary)
        );
        assert_eq!(page(&[]), Page::Settled(Lookup::NoPrimary));
    }

    #[test]
    fn a_full_page_without_the_primary_entry_asks_for_the_next() {
        let mut entries = vec![entry("old@example.com", false, true); PAGE_SIZE];
        assert_eq!(page(&entries), Page::Next);
        entries[PAGE_SIZE - 1] = entry("mona@example.com", true, true);
        assert_eq!(page(&entries), address("mona@example.com"));
    }

    #[test]
    fn a_body_that_is_not_the_listing_settles_nothing_about_the_account() {
        for body in [&b"{\"message\":\"Not Found\"}"[..], b"", b"[{\"email\":1}]"] {
            assert_eq!(read_page(body), Page::Settled(Lookup::Failed));
        }
    }

    #[test]
    fn a_primary_entry_that_is_not_one_plain_address_is_not_used() {
        assert_eq!(
            page(&[entry(
                "mona@example.com\r\nBcc: eve@example.com",
                true,
                true
            )]),
            Page::Settled(Lookup::NoPrimary)
        );
    }

    #[test]
    fn a_sign_in_writes_only_what_github_settled_and_changed() {
        let found = Lookup::Address("mona@example.com".to_owned());
        // First read, and a changed address: written.
        assert_eq!(change(None, &found), Some(Some("mona@example.com")));
        assert_eq!(
            change(Some("old@example.com"), &found),
            Some(Some("mona@example.com"))
        );
        // The same address again: no write.
        assert_eq!(change(Some("mona@example.com"), &found), None);
        // GitHub says there is no usable address: the stored one goes,
        // and nothing is written when none is stored.
        for gone in [Lookup::Unverified, Lookup::NoPrimary] {
            assert_eq!(change(Some("mona@example.com"), &gone), Some(None));
            assert_eq!(change(None, &gone), None);
        }
        // GitHub did not say: the stored address stands.
        for unknown in [Lookup::NoScope, Lookup::Failed] {
            assert_eq!(change(Some("mona@example.com"), &unknown), None);
            assert_eq!(change(None, &unknown), None);
        }
    }
}
