//! Operator alerts (`docs/runbook.md`, "Alerts"): the kinds the cron
//! observes on every pass, their levels, and the one mail a pass sends
//! for the rows `sql::CLAIM_ALERTS` hands it. Which rows are owed a
//! mail is decided by the statements in `src/sql/alerts.rs`, not here.

use std::fmt::Write as _;

use serde::Deserialize;

use crate::breaker::Mode;

/// The alert kinds: the `alerts.kind` values and the names a mail
/// uses. A fixed set, so no caller's text becomes a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    ServiceMode,
    Verification,
    Publication,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::ServiceMode => "service_mode",
            Kind::Verification => "verification",
            Kind::Publication => "publication",
        }
    }
}

/// The levels above 0, which is healthy. A kind is mailed when its
/// level rises and, if its firing was mailed, once when it returns to
/// 0.
pub const WARNING: u8 = 1;
/// At this level and above a kind that stays is mailed again every
/// [`RENOTIFY`].
pub const CRITICAL: u8 = 2;

/// The service mode's level. `reads_blocked` is one above
/// `writes_blocked`, so the step between them is mailed as a rise.
pub fn service_mode_level(mode: Mode) -> u8 {
    match mode {
        Mode::Normal => 0,
        Mode::Warn => WARNING,
        Mode::WritesBlocked => CRITICAL,
        Mode::ReadsBlocked => CRITICAL + 1,
    }
}

/// How long after its mail a kind still at [`CRITICAL`] is mailed
/// again, and how long a row waits for its next try once its mail has
/// failed [`ATTEMPTS`] times running (an `SQLite` time modifier).
pub const RENOTIFY: &str = "-24 hours";

/// The passes in a row that try a mail before the row waits
/// [`RENOTIFY`]: an hour of the 15-minute cron.
pub const ATTEMPTS: u8 = 4;

/// The one sender: the `NOTIFY_EMAIL` binding in `wrangler.jsonc`
/// allows no other. The recipient is the `ALERT_RECIPIENT_EMAIL` secret.
pub const SENDER: &str = "noreply@cabinpkg.com";
pub const SENDER_NAME: &str = "Cabin";

/// Whether `value` is one plain address, `local@domain.tld`: the form
/// `ALERT_RECIPIENT_EMAIL` must have. A display name, a list, or stray
/// whitespace is not one.
pub fn is_address(value: &str) -> bool {
    let plain = !value
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || ",;<>".contains(c));
    plain
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && domain.split('.').all(|label| !label.is_empty())
        })
}

/// One row a pass claimed for its mail (`sql::CLAIM_ALERTS`).
#[derive(Debug, Deserialize)]
pub struct Claimed {
    pub kind: String,
    pub level: i64,
    pub notified_level: i64,
    pub since: String,
    pub summary: String,
}

pub struct Mail {
    pub subject: String,
    pub text: String,
}

fn label(level: i64) -> &'static str {
    match level {
        0 => "RECOVERED",
        1 => "WARNING",
        _ => "CRITICAL",
    }
}

/// The mail for one pass's claimed rows, the most severe first. The
/// subject carries the highest level and every kind, and only those:
/// both are fixed values. `since` is when the kind left healthy, not
/// when it reached the level the line names; for a recovered row it is
/// when the kind returned.
pub fn digest(claimed: &[Claimed]) -> Mail {
    let mut claimed: Vec<&Claimed> = claimed.iter().collect();
    claimed.sort_by(|a, b| b.level.cmp(&a.level).then_with(|| a.kind.cmp(&b.kind)));
    let top = claimed.first().map_or(0, |row| row.level);
    let kinds: Vec<&str> = claimed.iter().map(|row| row.kind.as_str()).collect();
    let subject = format!("[Cabin][{}] {}", label(top), kinds.join(", "));

    let mut text = String::new();
    for row in claimed {
        let Claimed {
            kind,
            level,
            notified_level,
            since,
            summary,
        } = row;
        let label = label(*level);
        // Infallible: the sink is a String.
        let _ = if *level == 0 {
            write!(text, "{label} {kind}, at {since}\n  it was: {summary}\n\n")
        } else if level == notified_level {
            write!(
                text,
                "{label} {kind}, still firing since {since}\n  {summary}\n\n"
            )
        } else {
            write!(
                text,
                "{label} {kind}, firing since {since}\n  {summary}\n\n"
            )
        };
    }
    text.push_str(
        "A kind is mailed when its level rises and once when it recovers; \
         a CRITICAL one is repeated every 24 hours.\n\
         api/docs/runbook.md, \"Alerts\"\n",
    );
    Mail { subject, text }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: Kind, level: u8, notified_level: u8, summary: &str) -> Claimed {
        Claimed {
            kind: kind.as_str().to_owned(),
            level: level.into(),
            notified_level: notified_level.into(),
            since: "2026-11-13T01:00:00.000Z".to_owned(),
            summary: summary.to_owned(),
        }
    }

    #[test]
    fn the_mail_leads_with_the_most_severe_row_and_says_what_each_row_is() {
        let mail = digest(&[
            row(Kind::Verification, 0, 1, "3 revision(s) pending"),
            row(Kind::ServiceMode, 3, 3, "reads_blocked: class b"),
            row(Kind::Publication, 2, 0, "a change has been open"),
        ]);
        assert_eq!(
            mail.subject,
            "[Cabin][CRITICAL] service_mode, publication, verification"
        );
        assert_eq!(
            mail.text,
            r#"CRITICAL service_mode, still firing since 2026-11-13T01:00:00.000Z
  reads_blocked: class b

CRITICAL publication, firing since 2026-11-13T01:00:00.000Z
  a change has been open

RECOVERED verification, at 2026-11-13T01:00:00.000Z
  it was: 3 revision(s) pending

A kind is mailed when its level rises and once when it recovers; a CRITICAL one is repeated every 24 hours.
api/docs/runbook.md, "Alerts"
"#
        );
    }

    #[test]
    fn the_subject_names_the_highest_level_claimed() {
        let warning = digest(&[row(Kind::Verification, 1, 0, "pending")]);
        assert_eq!(warning.subject, "[Cabin][WARNING] verification");
        let recovered = digest(&[row(Kind::Publication, 0, 2, "open")]);
        assert_eq!(recovered.subject, "[Cabin][RECOVERED] publication");
    }

    #[test]
    fn the_recipient_is_one_plain_address() {
        assert!(is_address("alerts@example.com"));
        assert!(is_address("first.last+tag@mail.example.com"));
        for not_one in [
            "",
            "example.com",
            "@example.com",
            "alerts@",
            "alerts@localhost",
            "alerts@example.",
            "alerts@.com",
            "alerts@@example.com",
            "alerts@example.com\n",
            " alerts@example.com",
            "alerts@example.com\0",
            "a,b@example.com",
            "a;b@example.com",
            "<alerts@example.com",
            "alerts@example.com>",
            "a@example.com,b@example.com",
            "Alerts <alerts@example.com>",
        ] {
            assert!(!is_address(not_one), "{not_one:?}");
        }
    }

    /// Each step up the service modes is a rise, so each is mailed, and
    /// both blocked modes are reminded of daily.
    #[test]
    fn every_service_mode_is_a_level_above_the_one_before() {
        let levels = [
            Mode::Normal,
            Mode::Warn,
            Mode::WritesBlocked,
            Mode::ReadsBlocked,
        ]
        .map(service_mode_level);
        assert_eq!(levels, [0, WARNING, CRITICAL, CRITICAL + 1]);
    }
}
