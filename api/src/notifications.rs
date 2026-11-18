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

/// `value` as a mail shows it: its ASCII letters and digits and its
/// characters of `also`, so the value cannot start a line of its own
/// in the mail or put an address there. Without a `.` it cannot put a
/// link there either; a version needs one, so a pre-release label
/// that reads as a host name is shown as it is.
fn plain(value: &str, also: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || also.contains(*c))
        .collect()
}

/// The mail for a row of `kind` with `detail`, linking to the website
/// at `web_origin`; `None` when this build knows no such kind or cannot
/// read the detail. The subject is fixed per kind, so nothing an
/// account chose reaches a header.
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
            let version = plain(&version, "-.");
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
            r"alphaxy1z/pkgBccotherexamplecom 1.0.0httpsexample.coma was published to the Cabin registry under your account.

Revision: abcdexamplecom
Published: 2026-07-16T00:15:00.000ZTo:otherexample.com

https://cabinpkg.com/dashboard/package?name=alphaxy1z%2FpkgBccotherexamplecom
"
        );
    }

    #[test]
    fn a_row_this_build_cannot_write_has_no_mail() {
        assert!(mail("no_such_kind", "{}", "https://cabinpkg.com").is_none());
        assert!(mail("account_created", "{}", "https://cabinpkg.com").is_none());
        assert!(mail("account_created", "not json", "https://cabinpkg.com").is_none());
        assert!(mail("revision_published", "{}", "https://cabinpkg.com").is_none());
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
