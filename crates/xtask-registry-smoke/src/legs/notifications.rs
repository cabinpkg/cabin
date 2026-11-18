//! The account notifications end to end
//! (`api/src/notifications.rs`): the sign-in that created an account
//! recorded the mail it owes, and the cron sends it. A row reads as
//! sent once the local `NOTIFY_EMAIL` binding has accepted its mail and
//! the pass has recorded the message. What the local binding shows of
//! a mail is the dev tool's own log, which is no contract to assert on,
//! so what the mail says is `notifications::mail`'s own test.

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};

use crate::context::Smoke;
use crate::legs::finale::BREAKER_CRON;
use crate::servers::{d1, d1_rows};
use crate::step;

/// The account the sign-in leg's `fresh-old` roundtrip created.
const CREATED: &str =
    "(SELECT user_id FROM identities WHERE provider = 'github' AND provider_account_id = '4')";

/// The seeded account, which only ever signed in to an existing user.
const SEEDED: &str =
    "(SELECT user_id FROM identities WHERE provider = 'github' AND provider_account_id = '0')";

/// The mails one pass sends at most (`notifications::PER_PASS`).
const PER_PASS: u32 = 10;

/// The kind of the mails the legs before this one can owe.
const ACCOUNT: &str = "account_created";

/// A kind the Worker has no mail for.
const UNREADABLE: &str = "smoke";

/// Runs after the sign-in leg, which leaves both accounts with a
/// notification address. What a prior run left was removed before this
/// run's first request (`servers.rs`), so the account and its row are
/// this run's.
///
/// # Errors
///
/// The first failed check.
pub fn run(smoke: &mut Smoke) -> Result<()> {
    step("the sign-in that created an account owes it one mail");
    let owed = d1_rows(&format!(
        "SELECT kind, detail, recipient = {CREATED} AS created FROM notifications"
    ))?;
    let [row] = owed.as_slice() else {
        bail!("the sign-ins so far owe {} mails, expected one", owed.len());
    };
    if row["kind"] != ACCOUNT || row["created"] != 1 || row["detail"] != r#"{"login":"veteran"}"# {
        bail!("the mail owed is not the created account's: {row:?}");
    }

    step("a mail waits for its account's notification address");
    d1(&format!(
        "UPDATE users SET notification_email = NULL WHERE id = {CREATED}"
    ))?;
    smoke.check(BREAKER_CRON, &[200])?;
    expect(
        ACCOUNT,
        CREATED,
        "attempts = 0 AND sent_at IS NULL",
        "waiting",
    )?;

    step("a mail whose record fails stays owed, and the pass goes on to its limit");
    // The created account's row is the oldest, so it takes the first of
    // the pass's places and the seeded account's fixtures the others:
    // there is one fixture more than those, and after it a row of a
    // kind the Worker cannot write a mail for.
    d1(&format!(
        "UPDATE users SET notification_email = 'smoke@example.com' WHERE id = {CREATED};
         WITH RECURSIVE fixture(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM fixture WHERE i < {PER_PASS})
         INSERT INTO notifications (kind, event, recipient, detail, created_at)
           SELECT '{ACCOUNT}', 'smoke-' || i, {SEEDED}, '{{\"login\":\"smoke\"}}',
                  '2026-01-01T00:00:00.000Z' FROM fixture;
         INSERT INTO notifications (kind, event, recipient, detail, created_at)
           SELECT '{UNREADABLE}', '', {SEEDED}, '{{}}', '2026-01-01T00:00:00.000Z'"
    ))?;
    d1(&format!(
        "CREATE TRIGGER smoke_refuse_sent BEFORE UPDATE OF sent_at ON notifications \
         WHEN OLD.recipient = {CREATED} BEGIN SELECT RAISE(ABORT, 'smoke'); END"
    ))?;
    let passed = smoke.check(BREAKER_CRON, &[200]);
    d1("DROP TRIGGER smoke_refuse_sent")?;
    passed?;
    expect(
        ACCOUNT,
        CREATED,
        "attempts = 1 AND sent_at IS NULL AND failure IS NULL AND failed_at IS NULL",
        "owed, with no outcome recorded",
    )?;
    expect_fixtures_sent(PER_PASS - 1)?;
    expect(UNREADABLE, SEEDED, "attempts = 0", "past the pass's limit")?;

    step("the cron waits before it sends that mail again, and records a failed one");
    smoke.check(BREAKER_CRON, &[200])?;
    expect(
        ACCOUNT,
        CREATED,
        "attempts = 1 AND sent_at IS NULL",
        "still waiting",
    )?;
    expect_fixtures_sent(PER_PASS)?;
    expect(
        UNREADABLE,
        SEEDED,
        "attempts = 1 AND failure = 'unreadable' AND failed_at IS NULL AND sent_at IS NULL",
        "failed, to be tried again",
    )?;
    d1(&format!(
        "UPDATE notifications SET attempted_at = '2026-01-01T00:00:00.000Z' \
         WHERE recipient = {CREATED}"
    ))?;
    smoke.check(BREAKER_CRON, &[200])?;
    expect(
        ACCOUNT,
        CREATED,
        "attempts = 2 AND sent_at LIKE '20%Z' AND message_id <> ''",
        "sent on its second try",
    )?;
    d1(&format!(
        "DELETE FROM notifications WHERE recipient = {SEEDED}"
    ))
}

/// The one row of `kind` owed to `recipient` must satisfy `state`.
fn expect(kind: &str, recipient: &str, state: &str, what: &str) -> Result<()> {
    let rows = d1_rows(&format!(
        "SELECT ({state}) AS holds, attempts, attempted_at, failure, failed_at, sent_at \
         FROM notifications WHERE kind = '{kind}' AND recipient = {recipient}"
    ))?;
    let [row] = rows.as_slice() else {
        bail!("{} {kind} rows for the recipient, expected one", rows.len());
    };
    if row["holds"] != 1 {
        bail!("the mail is not {what}: {}", render(row));
    }
    Ok(())
}

/// `sent` of the seeded account's fixtures must be sent, each at its
/// first try, and the others not tried.
fn expect_fixtures_sent(sent: u32) -> Result<()> {
    let rows = d1_rows(&format!(
        "SELECT SUM(attempts = 1 AND sent_at LIKE '20%Z' AND message_id <> '') AS sent, \
                SUM(attempts = 0 AND sent_at IS NULL) AS untried \
         FROM notifications WHERE kind = '{ACCOUNT}' AND recipient = {SEEDED}"
    ))?;
    let row = rows.first().context("the count answered no row")?;
    if row["sent"] != sent || row["untried"] != PER_PASS - sent {
        bail!(
            "the fixtures are not {sent} sent and the rest untried: {}",
            render(row)
        );
    }
    Ok(())
}

fn render(row: &Map<String, Value>) -> String {
    serde_json::to_string(row).unwrap_or_default()
}

/// Everything the run did after the sign-in leg - session mints,
/// publishes, verdicts, yanks, claims, membership changes, token
/// exchanges - recorded no mail: the created account's is still the
/// only row.
///
/// # Errors
///
/// If any other row exists.
pub fn nothing_else_is_owed() -> Result<()> {
    step("no other request of the run recorded a mail");
    let rows = d1_rows(&format!(
        "SELECT COUNT(*) AS total, SUM(recipient = {CREATED} AND kind = '{ACCOUNT}') \
         AS created FROM notifications"
    ))?;
    let row = rows.first().context("the count answered no row")?;
    if row["total"] != 1 || row["created"] != 1 {
        bail!(
            "the run's notifications are not the one row: {}",
            render(row)
        );
    }
    Ok(())
}
