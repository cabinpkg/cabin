//! Account notifications (`docs/runbook.md`, "Notifications"): the
//! mails the registry owes an account for what happened to it. The
//! batch that makes an event true records the mail in the D1 table
//! `notifications`; the cron sends each one to its recipient's
//! notification address ([`crate::notification_email`]) and records
//! what the mail service answered. Which rows a pass may send is
//! decided by the statements in `src/sql/notifications.rs`, not here.

use serde::Deserialize;

use crate::alerts::Mail;
use crate::routes::{PACKAGE_PAGE_PREFIX, POST_LOGIN_REDIRECT};

/// The passes that try a row's mail before the cron gives the row up.
pub const ATTEMPTS: u8 = 8;

/// A row's wait after its `n`th try is this many minutes doubled `n`
/// times: 30 minutes after the first, 32 hours after the seventh.
pub const BACKOFF_MINUTES: u8 = 15;

/// The mails one pass sends at most. The mail service publishes no
/// rate it accepts, so a burst of events is spread over the passes
/// instead of sent at once.
pub const PER_PASS: u8 = 10;

/// The `meta` key under which a pass records, with each mail it
/// records as sent, when that was. A check from outside the cron reads
/// its age (`sql::NOTIFICATION_DELIVERY_STATE`).
pub const SENT_META_KEY: &str = "notifications_sent_at";

/// The `failure` of a send whose error carries no code.
pub const UNKNOWN: &str = "unknown";

/// The `failure` of a row this build cannot write a mail for.
pub const UNREADABLE: &str = "unreadable";

/// The row a pass claimed (`sql::CLAIM_NOTIFICATION`), with its
/// recipient's notification address. Not `Debug`: the address is not
/// for a log.
#[derive(Deserialize)]
pub struct Claimed {
    pub id: i64,
    pub kind: String,
    pub detail: String,
    pub address: String,
}

#[derive(Deserialize)]
struct AccountCreated {
    login: String,
}

/// The `detail` of an `account_created` row
/// (`sql::NOTIFY_ACCOUNT_CREATED`): the GitHub login the account was
/// created for, as of then.
pub fn account_created(login: &str) -> String {
    serde_json::json!({ "login": login }).to_string()
}

/// The `detail` of a `revision_published` row
/// (`sql::NOTIFY_REVISION_PUBLISHED`): the revision, and when the
/// registry began to serve it.
#[derive(Deserialize)]
struct RevisionPublished {
    scope: String,
    name: String,
    version: String,
    revision: String,
    at: String,
}

/// The `detail` of a `version_yanked` or `version_unyanked` row
/// (`sql::NOTIFY_VERSION_YANK`): the version, and when its flag
/// changed.
#[derive(Deserialize)]
struct VersionYank {
    scope: String,
    name: String,
    version: String,
    at: String,
}

/// The `detail` of a `member_added` or `member_removed` row
/// (`sql::NOTIFY_MEMBERSHIP`): the scope, the GitHub logins of the
/// member and of the owner who made the change, the role an added
/// member got, and when.
#[derive(Deserialize)]
struct Membership {
    scope: String,
    member: String,
    role: Option<String>,
    by: String,
    at: String,
}

/// The `detail` of a `scope_claimed` row (`sql::NOTIFY_SCOPE_CLAIMED`).
#[derive(Deserialize)]
struct ScopeClaimed {
    scope: String,
    at: String,
}

/// `value` as a mail shows it: its ASCII letters and digits and its
/// characters of `also`, so the value cannot start a line of its own
/// in the mail or put an address there. Without a `.` it cannot put a
/// link there either.
fn plain(value: &str, also: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || also.contains(*c))
        .collect()
}

/// `version` as a mail shows it: [`plain`] with its `.` and `-`, and
/// each `.` before a letter written `[.]`. A host name's last label
/// begins with a letter, so no pre-release label reads as a host name
/// a mail client would link.
fn version_text(version: &str) -> String {
    let version = plain(version, "-.");
    let mut text = String::with_capacity(version.len());
    let mut chars = version.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '.' && chars.peek().is_some_and(char::is_ascii_alphabetic) {
            text.push_str("[.]");
        } else {
            text.push(c);
        }
    }
    text
}

/// The mail for a row of `kind` with `detail`, linking to the website
/// at `web_origin`; `None` when this build knows no such kind or cannot
/// read the detail. A kind's subject is fixed, so nothing an account
/// chose reaches a header.
#[allow(clippy::too_many_lines)] // one arm per kind
pub fn mail(kind: &str, detail: &str, web_origin: &str) -> Option<Mail> {
    match kind {
        "account_created" => {
            let AccountCreated { login } = serde_json::from_str(detail).ok()?;
            let login = plain(&login, "-_");
            Some(Mail {
                subject: "Your Cabin account was created".to_owned(),
                text: format!(
                    r"A Cabin account was created for the GitHub account {login}.

Cabin sends notices about the account to this address, the verified primary email address of that GitHub account.

{web_origin}{POST_LOGIN_REDIRECT}
"
                ),
            })
        }
        "revision_published" => {
            let RevisionPublished {
                scope,
                name,
                version,
                revision,
                at,
            } = serde_json::from_str(detail).ok()?;
            let [scope, name, revision] = [scope, name, revision].map(|value| plain(&value, "-_"));
            let version = version_text(&version);
            let at = plain(&at, "-.:");
            Some(Mail {
                subject: "Your Cabin package was published".to_owned(),
                text: format!(
                    r"{scope}/{name} {version} was published to the Cabin registry under your account.

Revision: {revision}
Published: {at}

{web_origin}{PACKAGE_PAGE_PREFIX}{scope}%2F{name}
"
                ),
            })
        }
        "version_yanked" | "version_unyanked" => {
            let VersionYank {
                scope,
                name,
                version,
                at,
            } = serde_json::from_str(detail).ok()?;
            let [scope, name] = [scope, name].map(|value| plain(&value, "-_"));
            let version = version_text(&version);
            let at = plain(&at, "-.:");
            let done = if kind == "version_yanked" {
                "yanked"
            } else {
                "un-yanked"
            };
            Some(Mail {
                subject: format!("A version of your Cabin package was {done}"),
                text: format!(
                    r"{scope}/{name} {version} was {done} under your account.

Changed: {at}

{web_origin}{PACKAGE_PAGE_PREFIX}{scope}%2F{name}
"
                ),
            })
        }
        "member_added" | "member_removed" => {
            let Membership {
                scope,
                member,
                role,
                by,
                at,
            } = serde_json::from_str(detail).ok()?;
            let [scope, member, by] = [scope, member, by].map(|value| plain(&value, "-_"));
            let at = plain(&at, "-.:");
            let change = match (kind, role) {
                ("member_added", Some(role)) => {
                    format!("added to the Cabin scope {scope} as {}", plain(&role, ""))
                }
                ("member_removed", None) => format!("removed from the Cabin scope {scope}"),
                _ => return None,
            };
            Some(Mail {
                subject: "The members of a Cabin scope changed".to_owned(),
                text: format!(
                    r"The GitHub account {member} was {change} by {by}.

Changed: {at}

{web_origin}{POST_LOGIN_REDIRECT}
"
                ),
            })
        }
        "scope_claimed" => {
            let ScopeClaimed { scope, at } = serde_json::from_str(detail).ok()?;
            let scope = plain(&scope, "-_");
            let at = plain(&at, "-.:");
            Some(Mail {
                subject: "Your Cabin scope was claimed".to_owned(),
                text: format!(
                    r"The Cabin scope {scope} was claimed by your account.

Claimed: {at}

{web_origin}{POST_LOGIN_REDIRECT}
"
                ),
            })
        }
        _ => None,
    }
}

/// A send error's code as `failure` records it: the mail service's
/// code, or [`UNKNOWN`] when the error has none that reads as one. An
/// error's message can name the recipient, so it is never recorded.
pub fn failure_code(code: Option<&str>) -> &str {
    code.filter(|code| {
        !code.is_empty()
            && code.len() <= 64
            && code
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    })
    .unwrap_or(UNKNOWN)
}

/// Whether a failure is the mail service refusing a suppressed
/// recipient, which no later try changes. Every other failure is tried
/// again: a limit passes, and a refusal of the sender or of the
/// binding's configuration is the operator's to repair while the row
/// waits.
pub fn is_permanent(failure: &str) -> bool {
    failure == "E_RECIPIENT_SUPPRESSED"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_mail_names_the_login_and_links_to_the_dashboard() {
        let mail = mail(
            "account_created",
            &account_created("octocat"),
            "https://cabinpkg.com",
        )
        .expect("a mail");
        assert_eq!(mail.subject, "Your Cabin account was created");
        assert_eq!(
            mail.text,
            r"A Cabin account was created for the GitHub account octocat.

Cabin sends notices about the account to this address, the verified primary email address of that GitHub account.

https://cabinpkg.com/dashboard/
"
        );
    }

    /// A login cannot add a line, a link or an address to the mail, and
    /// never reaches the subject.
    #[test]
    fn a_login_is_written_from_its_own_characters() {
        let detail =
            account_created("octo_cat-1\r\nBcc: other@example.com\n\u{2028}https://example.com");
        let mail = mail("account_created", &detail, "https://cabinpkg.com").expect("a mail");
        assert_eq!(mail.subject, "Your Cabin account was created");
        assert_eq!(
            mail.text.lines().next(),
            Some(
                "A Cabin account was created for the GitHub account \
                 octo_cat-1Bccotherexamplecomhttpsexamplecom."
            )
        );
        assert_eq!(mail.text.lines().count(), 5);
    }

    /// The detail is `sql::NOTIFY_REVISION_PUBLISHED`'s, whose shape
    /// `tests/sql_validation/notifications.rs` reads through `mail`.
    #[test]
    fn the_publication_mail_names_the_revision_and_links_to_its_package() {
        let detail = r#"{"scope":"cabin-ports","name":"z_lib","version":"1.3.1-rc.1","revision":"0123456789abcdef","at":"2026-07-16T00:15:00.000Z","generation":7}"#;
        let mail = mail("revision_published", detail, "https://cabinpkg.com").expect("a mail");
        assert_eq!(mail.subject, "Your Cabin package was published");
        assert_eq!(
            mail.text,
            r"cabin-ports/z_lib 1.3.1-rc.1 was published to the Cabin registry under your account.

Revision: 0123456789abcdef
Published: 2026-07-16T00:15:00.000Z

https://cabinpkg.com/dashboard/package?name=cabin-ports%2Fz_lib
"
        );
    }

    /// No value of a publication can add a line, a header or an address
    /// to the mail, or change where its link goes.
    #[test]
    fn a_publication_is_written_from_its_own_characters() {
        let detail = serde_json::json!({
            "scope": "al\npha/../x?y=1&z#",
            "name": "pkg\r\nBcc: other@example.com",
            "version": "1.0.0\n\nhttps://example.com/ <a>",
            "revision": "ab\u{2028}cd@example.com",
            "at": "2026-07-16T00:15:00.000Z\nTo: other@example.com",
            "generation": 7,
        })
        .to_string();
        let mail = mail("revision_published", &detail, "https://cabinpkg.com").expect("a mail");
        assert_eq!(mail.subject, "Your Cabin package was published");
        assert_eq!(
            mail.text,
            r"alphaxy1z/pkgBccotherexamplecom 1.0.0httpsexample[.]coma was published to the Cabin registry under your account.

Revision: abcdexamplecom
Published: 2026-07-16T00:15:00.000ZTo:otherexample.com

https://cabinpkg.com/dashboard/package?name=alphaxy1z%2FpkgBccotherexamplecom
"
        );
    }

    /// The details below are `sql::NOTIFY_VERSION_YANK`'s,
    /// `sql::NOTIFY_MEMBERSHIP`'s and `sql::NOTIFY_SCOPE_CLAIMED`'s,
    /// whose shapes `tests/sql_validation/notifications.rs` reads
    /// through `mail`.
    #[test]
    fn a_yank_mail_names_the_version_and_links_to_its_package() {
        let detail = r#"{"scope":"cabin-ports","name":"z_lib","version":"1.3.1-rc.1","at":"2026-07-16T00:15:00.000Z"}"#;
        let yanked = mail("version_yanked", detail, "https://cabinpkg.com").expect("a mail");
        assert_eq!(yanked.subject, "A version of your Cabin package was yanked");
        assert_eq!(
            yanked.text,
            r"cabin-ports/z_lib 1.3.1-rc.1 was yanked under your account.

Changed: 2026-07-16T00:15:00.000Z

https://cabinpkg.com/dashboard/package?name=cabin-ports%2Fz_lib
"
        );
        let unyanked = mail("version_unyanked", detail, "https://cabinpkg.com").expect("a mail");
        assert_eq!(
            unyanked.subject,
            "A version of your Cabin package was un-yanked"
        );
        assert_eq!(
            unyanked.text.lines().next(),
            Some("cabin-ports/z_lib 1.3.1-rc.1 was un-yanked under your account.")
        );
    }

    /// A version keeps the dots its numbers need; one before a letter
    /// could end a host name, and is written so that it does not.
    #[test]
    fn a_version_shaped_like_a_host_name_is_no_link() {
        assert_eq!(version_text("1.3.1-rc.1"), "1.3.1-rc.1");
        assert_eq!(
            version_text("1.0.0-rc.example.com"),
            "1.0.0-rc[.]example[.]com"
        );
        assert_eq!(
            version_text("1.0.0-0.3m.xn--p1ai.7"),
            "1.0.0-0.3m[.]xn--p1ai.7"
        );
    }

    #[test]
    fn a_membership_mail_names_the_member_the_scope_and_who_changed_it() {
        let added = r#"{"scope":"fmtlib","member":"octocat","role":"member","by":"mona","at":"2026-07-16T00:15:00.000Z"}"#;
        let removed = r#"{"scope":"fmtlib","member":"octocat","role":null,"by":"mona","at":"2026-07-16T00:15:00.000Z"}"#;
        let mail_of = |kind, detail| mail(kind, detail, "https://cabinpkg.com");
        let addition = mail_of("member_added", added).expect("a mail");
        assert_eq!(addition.subject, "The members of a Cabin scope changed");
        assert_eq!(
            addition.text,
            r"The GitHub account octocat was added to the Cabin scope fmtlib as member by mona.

Changed: 2026-07-16T00:15:00.000Z

https://cabinpkg.com/dashboard/
"
        );
        let removal = mail_of("member_removed", removed).expect("a mail");
        assert_eq!(removal.subject, addition.subject);
        assert_eq!(
            removal.text.lines().next(),
            Some("The GitHub account octocat was removed from the Cabin scope fmtlib by mona.")
        );
        // A kind and a role that disagree are no mail.
        assert!(mail_of("member_added", removed).is_none());
        assert!(mail_of("member_removed", added).is_none());

        // A login, like a role, cannot add a line or an address.
        let hostile = serde_json::json!({
            "scope": "fmtlib",
            "member": "octo\ncat\r\nBcc: other@example.com",
            "role": "owner\nhttps://example.com",
            "by": "mo\u{2028}na",
            "at": "2026-07-16T00:15:00.000Z",
        })
        .to_string();
        let mail = mail_of("member_added", &hostile).expect("a mail");
        assert_eq!(
            mail.text.lines().next(),
            Some(
                "The GitHub account octocatBccotherexamplecom was added to the Cabin scope \
                 fmtlib as ownerhttpsexamplecom by mona."
            )
        );
        assert_eq!(mail.text.lines().count(), 5);
    }

    #[test]
    fn a_claim_mail_names_the_scope() {
        let detail = r#"{"scope":"fmtlib","at":"2026-07-16T00:15:00.000Z"}"#;
        let mail = mail("scope_claimed", detail, "https://cabinpkg.com").expect("a mail");
        assert_eq!(mail.subject, "Your Cabin scope was claimed");
        assert_eq!(
            mail.text,
            r"The Cabin scope fmtlib was claimed by your account.

Claimed: 2026-07-16T00:15:00.000Z

https://cabinpkg.com/dashboard/
"
        );
    }

    /// No value of a yank or a claim can add a line, a header, an
    /// address or a link to the mail.
    #[test]
    fn a_yank_and_a_claim_are_written_from_their_own_characters() {
        let detail = serde_json::json!({
            "scope": "al\npha/../x?y=1&z#",
            "name": "pkg\r\nBcc: other@example.com",
            "version": "1.0.0-rc.example.com\n<a>",
            "at": "2026-07-16T00:15:00.000Z\nTo: other@example.com",
        })
        .to_string();
        let yanked = mail("version_yanked", &detail, "https://cabinpkg.com").expect("a mail");
        assert_eq!(
            yanked.text,
            r"alphaxy1z/pkgBccotherexamplecom 1.0.0-rc[.]example[.]coma was yanked under your account.

Changed: 2026-07-16T00:15:00.000ZTo:otherexample.com

https://cabinpkg.com/dashboard/package?name=alphaxy1z%2FpkgBccotherexamplecom
"
        );
        let claimed = mail("scope_claimed", &detail, "https://cabinpkg.com").expect("a mail");
        assert_eq!(
            claimed.text,
            r"The Cabin scope alphaxy1z was claimed by your account.

Claimed: 2026-07-16T00:15:00.000ZTo:otherexample.com

https://cabinpkg.com/dashboard/
"
        );
    }

    #[test]
    fn a_row_this_build_cannot_write_has_no_mail() {
        assert!(mail("no_such_kind", "{}", "https://cabinpkg.com").is_none());
        assert!(mail("account_created", "not json", "https://cabinpkg.com").is_none());
        for kind in [
            "account_created",
            "revision_published",
            "version_yanked",
            "version_unyanked",
            "member_added",
            "member_removed",
            "scope_claimed",
        ] {
            assert!(mail(kind, "{}", "https://cabinpkg.com").is_none(), "{kind}");
        }
    }

    #[test]
    fn a_failure_records_a_code_and_never_a_message() {
        assert_eq!(
            failure_code(Some("E_RATE_LIMIT_EXCEEDED")),
            "E_RATE_LIMIT_EXCEEDED"
        );
        for not_a_code in [
            None,
            Some(""),
            Some("recipient someone@example.com is suppressed"),
            Some("E_RECIPIENT someone@example.com"),
            Some("e_lowercase"),
        ] {
            assert_eq!(failure_code(not_a_code), UNKNOWN, "{not_a_code:?}");
        }
        assert_eq!(failure_code(Some(&"E".repeat(65))), UNKNOWN);
    }

    #[test]
    fn only_a_suppressed_recipient_is_permanent() {
        assert!(is_permanent("E_RECIPIENT_SUPPRESSED"));
        for retried in [
            "E_RECIPIENT_NOT_ALLOWED",
            "E_SENDER_NOT_VERIFIED",
            "E_RATE_LIMIT_EXCEEDED",
            "E_DAILY_LIMIT_EXCEEDED",
            "E_INTERNAL_SERVER_ERROR",
            "E_DELIVERY_FAILED",
            UNKNOWN,
            UNREADABLE,
        ] {
            assert!(!is_permanent(retried), "{retried}");
        }
    }
}
