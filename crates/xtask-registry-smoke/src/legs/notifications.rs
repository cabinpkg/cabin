//! The account notifications end to end
//! (`api/src/notifications.rs`): the sign-in that created an account
//! recorded the mail it owes, and the cron sends it; a served revision
//! owes its publisher one, which the cron sends once the publish pass
//! has completed the change (`legs/blobs.rs` calls
//! [`expect_publications`] around that pass). A claim, a change to a
//! scope's members, and a change of a yank flag owe theirs at once, and
//! the two of them that grant - an addition and an un-yank - stop at a
//! scope's daily limit ([`at_daily_limit`]). A row reads as
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

/// The account the claims leg adds to `smoke` and removes again, which
/// never signs in and so has no notification address.
const FRIEND: &str =
    "(SELECT user_id FROM identities WHERE provider = 'github' AND provider_account_id = '2')";

/// The mails one pass sends at most (`notifications::PER_PASS`).
const PER_PASS: u32 = 10;

/// The mails an account's un-yanks in a scope, or the scope's
/// additions, may owe in a UTC day
/// (`quota::SCOPE_GRANT_MAILS_PER_DAY`).
const DAILY_LIMIT: u32 = 30;

/// The kind of the mails the legs before this one can owe.
const ACCOUNT: &str = "account_created";

/// The kind of the mail a served revision owes its publisher.
const PUBLISHED: &str = "revision_published";

/// Whether a row of `notifications` is for a change of version `v`'s
/// yank flag.
const OF_VERSION: &str = "notifications.detail ->> '$.scope' = v.scope \
                          AND notifications.detail ->> '$.name' = v.name \
                          AND notifications.detail ->> '$.version' = v.version";

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
        "SELECT detail, recipient = {CREATED} AS created FROM notifications \
         WHERE kind = '{ACCOUNT}'"
    ))?;
    let [row] = owed.as_slice() else {
        bail!("the sign-ins so far owe {} mails, expected one", owed.len());
    };
    if row["created"] != 1 || row["detail"] != r#"{"login":"veteran"}"# {
        bail!("the mail owed is not the created account's: {row:?}");
    }

    claims_leg_owes_its_mails()?;

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
    claims_leg_mails_went_out()?;

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
        "DELETE FROM notifications \
         WHERE kind = '{UNREADABLE}' OR (kind = '{ACCOUNT}' AND recipient = {SEEDED})"
    ))
}

/// The claims leg's changes: the seeded account claimed `smoke` and
/// `smokeorg`, and added its friend to `smoke` and removed it again.
/// Each change of a member goes to the member and to the owner.
fn claims_leg_owes_its_mails() -> Result<()> {
    step("a claim and a change to a scope's members owe their accounts a mail");
    let owed = d1_rows(&format!(
        "SELECT kind, COUNT(*) AS owed, SUM(recipient = {SEEDED}) AS owner, \
                SUM(recipient = {FRIEND}) AS friend \
         FROM notifications WHERE kind <> '{ACCOUNT}' GROUP BY kind ORDER BY kind"
    ))?;
    let expected = serde_json::json!([
        { "kind": "member_added", "owed": 2, "owner": 1, "friend": 1 },
        { "kind": "member_removed", "owed": 2, "owner": 1, "friend": 1 },
        { "kind": "scope_claimed", "owed": 2, "owner": 2, "friend": 0 },
    ]);
    if Value::from(owed.iter().cloned().map(Value::Object).collect::<Vec<_>>()) != expected {
        bail!("the claims leg does not owe its six mails: {owed:?}");
    }
    Ok(())
}

/// After a pass: the owner's four of the claims leg are written, each
/// from its own detail, and sent; the friend's two wait for an address.
fn claims_leg_mails_went_out() -> Result<()> {
    let rows = d1_rows(&format!(
        "SELECT SUM(recipient = {SEEDED} AND attempts = 1 AND sent_at LIKE '20%Z' \
                    AND message_id <> '') AS sent, \
                SUM(recipient = {FRIEND} AND attempts = 0) AS waiting \
         FROM notifications WHERE kind <> '{ACCOUNT}'"
    ))?;
    let row = rows.first().context("the count answered no row")?;
    if row["sent"] != 4 || row["waiting"] != 2 {
        bail!(
            "the claims leg's mails are not four sent and two waiting: {}",
            render(row)
        );
    }
    Ok(())
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

/// The publications of `scope` so far owe one mail per revision the
/// read plane serves, and the mails together satisfy `holds`, an
/// aggregate over their rows. The seed removed the scope's revisions
/// and every mail of earlier runs (`servers.rs`), so both are this
/// run's.
///
/// # Errors
///
/// If a served revision owes no mail or more than one, or `holds` is
/// false.
pub fn expect_publications(scope: &str, holds: &str, what: &str) -> Result<()> {
    let rows = d1_rows(&format!(
        "SELECT COUNT(*) AS owed, COALESCE({holds}, 0) AS holds, \
                COALESCE(SUM(attempts), 0) AS attempts, COUNT(sent_at) AS sent, \
                (SELECT COUNT(*) FROM revisions \
                 WHERE scope = '{scope}' AND archived_at IS NOT NULL) AS served \
         FROM notifications WHERE kind = '{PUBLISHED}' AND event LIKE '{scope}/%'"
    ))?;
    let row = rows.first().context("the count answered no row")?;
    if row["served"] == 0 || row["owed"] != row["served"] || row["holds"] != 1 {
        bail!(
            "the publications do not owe one mail per served revision, {what}: {}",
            render(row)
        );
    }
    Ok(())
}

/// Runs `refused` while `scope` holds `mails` rows of `kind` for the
/// UTC day, owed to `recipient`: fixture rows, removed after it.
fn with_mails_owed(
    kind: &str,
    scope: &str,
    mails: u32,
    recipient: &str,
    refused: impl FnOnce() -> Result<()>,
) -> Result<()> {
    d1(&format!(
        "WITH RECURSIVE fixture(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM fixture WHERE i < {mails})
         INSERT INTO notifications (kind, event, recipient, detail, created_at)
           SELECT '{kind}', '{scope}@' || strftime('%Y-%m-%dT%H:%M:%fZ', 'now') || '/!' || i,
                  {recipient}, '{{}}', '2026-01-01T00:00:00.000Z' FROM fixture"
    ))?;
    refused()?;
    d1("DELETE FROM notifications WHERE event LIKE '%/!%'")
}

/// Runs `refused` while the seeded account's mails of `kind` in `scope`
/// for the UTC day stand at the daily limit.
///
/// # Errors
///
/// `refused`'s own, or a fixture statement's.
pub fn at_daily_limit(kind: &str, scope: &str, refused: impl FnOnce() -> Result<()>) -> Result<()> {
    with_mails_owed(kind, scope, DAILY_LIMIT, SEEDED, refused)
}

/// Runs `refused` while another account's un-yanks in `scope` for the
/// UTC day stand at the limit for all the scope's accounts
/// (`quota::SCOPE_UNYANK_MAILS_PER_DAY`).
///
/// # Errors
///
/// `refused`'s own, or a fixture statement's.
pub fn at_scope_unyank_limit(scope: &str, refused: impl FnOnce() -> Result<()>) -> Result<()> {
    with_mails_owed("version_unyanked", scope, 3 * DAILY_LIMIT, FRIEND, refused)
}

/// The newest mail of `kind` for a change of `version`'s yank flag is
/// owed to the seeded account, which made it, and is sent.
///
/// # Errors
///
/// If there is no such mail, or it is another account's or not sent.
pub fn expect_yank_sent(kind: &str, scope: &str, name: &str, version: &str) -> Result<()> {
    let rows = d1_rows(&format!(
        "SELECT recipient = {SEEDED} AND attempts = 1 AND sent_at LIKE '20%Z' \
                AND message_id <> '' AS holds, attempts, failure, sent_at \
         FROM notifications \
         WHERE kind = '{kind}' AND detail ->> '$.scope' = '{scope}' \
         AND detail ->> '$.name' = '{name}' AND detail ->> '$.version' = '{version}' \
         ORDER BY id DESC LIMIT 1"
    ))?;
    let row = rows
        .first()
        .context("the change of the flag owes no mail")?;
    if row["holds"] != 1 {
        bail!(
            "the {kind} mail is not the seeded account's and sent: {}",
            render(row)
        );
    }
    Ok(())
}

/// The run's mails are exactly the ones its changes owe: the created
/// account's; one for each revision the read plane serves under
/// `scope`, owed to the account that published it; the claims leg's
/// two claims and four for its member; and the seeded account's four
/// for the yank and un-yank of the revisions leg and of the blobs
/// leg, so that a version's yanks less its un-yanks are its flag.
/// Nothing else the run did - session mints,
/// verdicts, token exchanges, refused or repeated requests - recorded
/// one.
///
/// # Errors
///
/// If any other row exists, or one of those is missing.
pub fn nothing_else_is_owed(scope: &str) -> Result<()> {
    step("the run's mails are the ones its changes owe, and no others");
    let rows = d1_rows(&format!(
        "SELECT COUNT(*) AS total, SUM(recipient = {CREATED} AND kind = '{ACCOUNT}') AS created, \
                SUM(kind = '{PUBLISHED}' AND EXISTS ( \
                  SELECT 1 FROM revisions r \
                  WHERE r.scope || '/' || r.name || '@' || r.version || '#' || r.revision \
                        = notifications.event \
                  AND r.published_by = notifications.recipient \
                  AND r.archived_at IS NOT NULL)) AS published, \
                (SELECT COUNT(*) FROM revisions \
                 WHERE scope = '{scope}' AND archived_at IS NOT NULL) AS served, \
                SUM(kind = 'scope_claimed') AS claims, \
                SUM(kind IN ('member_added', 'member_removed')) AS members, \
                SUM(kind IN ('version_yanked', 'version_unyanked') \
                    AND recipient = {SEEDED} AND EXISTS ( \
                      SELECT 1 FROM versions v WHERE {OF_VERSION})) AS yanks, \
                (SELECT COUNT(*) FROM versions v WHERE v.scope = '{scope}' AND v.yanked <> ( \
                   SELECT COUNT(*) FILTER (WHERE kind = 'version_yanked') \
                        - COUNT(*) FILTER (WHERE kind = 'version_unyanked') \
                   FROM notifications WHERE {OF_VERSION})) AS unbalanced \
         FROM notifications"
    ))?;
    let row = rows.first().context("the count answered no row")?;
    let count = |column: &str| row[column].as_u64().unwrap_or(u64::MAX);
    if row["created"] != 1
        || row["served"] == 0
        || row["published"] != row["served"]
        || row["claims"] != 2
        || row["members"] != 4
        || row["yanks"] != 4
        || row["unbalanced"] != 0
        || count("total") != 1 + count("published") + 2 + 4 + 4
    {
        bail!(
            "the run's notifications are not the ones its changes owe: {}",
            render(row)
        );
    }
    Ok(())
}
