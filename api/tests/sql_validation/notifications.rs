//! Executed-semantics tests for `src/sql/notifications.rs`: which mail
//! an event owes, and which row a cron pass claims next, claim after
//! claim.

use cabin_api_worker::notifications::{ATTEMPTS, BACKOFF_MINUTES, account_created, mail};
use cabin_api_worker::sql;
use rusqlite::Connection;

use crate::common::{migrated_connection, resolve, rows, sign_in};

/// The time `minutes` after the first pass, as the cron's clock writes it.
fn at(minutes: u32) -> String {
    format!(
        "2026-07-{:02}T{:02}:{:02}:00.000Z",
        16 + minutes / 1440,
        minutes % 1440 / 60,
        minutes % 60
    )
}

/// Signs a new account in and records the mail of its creation, as the
/// sign-in's batch does; the account's user id.
fn create_account(conn: &Connection, account_id: &str, login: &str) -> i64 {
    sign_in(conn, "github", account_id, login, &at(0));
    notify_account_created(conn, account_id, login);
    resolve(conn, "github", account_id).expect("the identity").0
}

fn notify_account_created(conn: &Connection, account_id: &str, login: &str) -> usize {
    conn.execute(
        sql::NOTIFY_ACCOUNT_CREATED,
        ("github", account_id, account_created(login), at(0)),
    )
    .expect("NOTIFY_ACCOUNT_CREATED")
}

fn set_address(conn: &Connection, user: i64, address: Option<&str>) {
    conn.execute(
        "UPDATE users SET notification_email = ?2 WHERE id = ?1",
        (user, address),
    )
    .expect("set the address");
}

/// A new account that has a notification address; its user id.
fn addressed_account(conn: &Connection, account_id: &str, login: &str) -> i64 {
    let user = create_account(conn, account_id, login);
    set_address(conn, user, Some(&format!("{login}@example.com")));
    user
}

/// The claim at `minutes`: the row it took, as `recipient address`, or
/// none. D1 binds every number as a REAL.
fn claim(conn: &Connection, minutes: u32) -> Vec<String> {
    let mut claim = conn.prepare(sql::CLAIM_NOTIFICATION).expect("prepare");
    let claimed: Vec<(i64, String)> = claim
        .query_map(
            (at(minutes), f64::from(ATTEMPTS), f64::from(BACKOFF_MINUTES)),
            |row| Ok((row.get(0)?, row.get(3)?)),
        )
        .expect("CLAIM_NOTIFICATION")
        .collect::<Result<_, _>>()
        .expect("claimed row");
    claimed
        .into_iter()
        .map(|(id, address)| {
            let recipient: i64 = conn
                .query_row(
                    "SELECT recipient FROM notifications WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .expect("the claimed row");
            format!("{recipient} {address}")
        })
        .collect()
}

/// Records the failed send of `user`'s row, as the pass at `minutes`
/// does.
fn fail(conn: &Connection, user: i64, minutes: u32, failure: &str, permanent: bool) {
    conn.execute(
        sql::MARK_NOTIFICATION_FAILED,
        (
            row_of(conn, user),
            failure,
            f64::from(u8::from(permanent)),
            at(minutes),
        ),
    )
    .expect("MARK_NOTIFICATION_FAILED");
}

fn row_of(conn: &Connection, user: i64) -> f64 {
    conn.query_row(
        "SELECT id FROM notifications WHERE recipient = ?1",
        [user],
        |row| row.get(0),
    )
    .expect("the user's row")
}

const NONE: [&str; 0] = [];

/// An account's creation owes that account one mail, however often it
/// is recorded, and the row is one the mail can be written from.
#[test]
fn an_account_creation_owes_its_account_one_mail() {
    let conn = migrated_connection();
    let mona = create_account(&conn, "424242", "mona");
    assert_eq!(notify_account_created(&conn, "424242", "mona"), 0);
    assert_eq!(notify_account_created(&conn, "no-such-account", "ghost"), 0);
    let hubot = create_account(&conn, "515151", "hubot");

    assert_eq!(
        rows(
            &conn,
            "SELECT kind, event, recipient, created_at, attempts, sent_at FROM notifications \
             ORDER BY id"
        ),
        [
            format!(
                r#"Text("account_created") | Text("") | Integer({mona}) | Text("{}") | Integer(0) | Null"#,
                at(0)
            ),
            format!(
                r#"Text("account_created") | Text("") | Integer({hubot}) | Text("{}") | Integer(0) | Null"#,
                at(0)
            ),
        ]
    );
    let (kind, detail): (String, String) = conn
        .query_row(
            "SELECT kind, detail FROM notifications WHERE recipient = ?1",
            [mona],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("mona's row");
    let mail = mail(&kind, &detail, "https://cabinpkg.com").expect("a mail");
    assert!(mail.text.contains("GitHub account mona."), "{}", mail.text);
}

/// A row whose recipient has no notification address is not claimed,
/// and no attempt is spent on it; it is claimed, with the address then
/// stored, once there is one.
#[test]
fn a_row_waits_unclaimed_for_its_recipients_address() {
    let conn = migrated_connection();
    let mona = create_account(&conn, "424242", "mona");
    assert_eq!(claim(&conn, 0), NONE);
    assert_eq!(claim(&conn, 15), NONE);
    assert_eq!(
        rows(&conn, "SELECT attempts, attempted_at FROM notifications"),
        ["Integer(0) | Null"]
    );

    set_address(&conn, mona, Some("mona@example.com"));
    assert_eq!(claim(&conn, 30), [format!("{mona} mona@example.com")]);
}

/// A sent row records the pass and the message, and is never claimed
/// again.
#[test]
fn a_sent_mail_is_recorded_and_not_claimed_again() {
    let conn = migrated_connection();
    let mona = addressed_account(&conn, "424242", "mona");
    assert_eq!(claim(&conn, 0), [format!("{mona} mona@example.com")]);
    conn.execute(
        sql::MARK_NOTIFICATION_SENT,
        (row_of(&conn, mona), at(0), "<message@example.com>"),
    )
    .expect("MARK_NOTIFICATION_SENT");

    for minutes in [15, 30, 60, 10_000] {
        assert_eq!(claim(&conn, minutes), NONE, "at {minutes}");
    }
    assert_eq!(
        rows(
            &conn,
            "SELECT attempts, sent_at, message_id, failure, failed_at FROM notifications"
        ),
        [format!(
            r#"Integer(1) | Text("{}") | Text("<message@example.com>") | Null | Null"#,
            at(0)
        )]
    );
}

/// The waits between a row's tries double from 30 minutes, and its
/// last try gives it up.
#[test]
fn a_failed_mail_is_tried_at_doubling_waits_and_then_given_up() {
    let conn = migrated_connection();
    let mona = addressed_account(&conn, "424242", "mona");
    let mine = [format!("{mona} mona@example.com")];

    let mut now = 0;
    for attempt in 1..=u32::from(ATTEMPTS) {
        assert_eq!(claim(&conn, now), mine, "attempt {attempt}");
        // The claim clears the failure of the try before, and the last
        // claim gives the row up before its send.
        let given_up = if attempt == u32::from(ATTEMPTS) {
            format!(r#"Text("{}")"#, at(now))
        } else {
            "Null".to_owned()
        };
        assert_eq!(
            rows(
                &conn,
                "SELECT attempts, failure, failed_at FROM notifications"
            ),
            [format!("Integer({attempt}) | Null | {given_up}")]
        );
        fail(&conn, mona, now, "E_RATE_LIMIT_EXCEEDED", false);
        let wait = u32::from(BACKOFF_MINUTES) << attempt;
        if attempt < u32::from(ATTEMPTS) {
            assert_eq!(claim(&conn, now + wait - 1), NONE, "attempt {attempt}");
        }
        now += wait;
    }
    assert_eq!(now, 15 * (2 + 4 + 8 + 16 + 32 + 64 + 128 + 256));

    for later in [now, now + 10_000] {
        assert_eq!(claim(&conn, later), NONE, "given up");
    }
    assert_eq!(
        rows(
            &conn,
            "SELECT attempts, failure, failed_at IS NOT NULL, sent_at FROM notifications"
        ),
        [r#"Integer(8) | Text("E_RATE_LIMIT_EXCEEDED") | Integer(1) | Null"#]
    );
}

/// A permanent failure gives the row up at its first try.
#[test]
fn a_suppressed_recipient_is_given_up_at_once() {
    let conn = migrated_connection();
    let mona = addressed_account(&conn, "424242", "mona");
    assert_eq!(claim(&conn, 0), [format!("{mona} mona@example.com")]);
    fail(&conn, mona, 0, "E_RECIPIENT_SUPPRESSED", true);

    for minutes in [30, 60, 10_000] {
        assert_eq!(claim(&conn, minutes), NONE, "at {minutes}");
    }
    assert_eq!(
        rows(
            &conn,
            "SELECT attempts, failure, failed_at FROM notifications"
        ),
        [format!(
            r#"Integer(1) | Text("E_RECIPIENT_SUPPRESSED") | Text("{}")"#,
            at(0)
        )]
    );
}

/// A pass that claimed a row and recorded nothing - it died, or its
/// record failed after the send - leaves the row owed: the next claim
/// sends the mail again. The claims are counted all the same, and the
/// last one gives the row up, so a row no pass ever settles is not
/// claimed forever and ends where a given-up row does. A send recorded
/// after that claim still settles the row as sent.
#[test]
fn a_claim_with_no_recorded_outcome_is_tried_again_and_still_finite() {
    let conn = migrated_connection();
    let mona = addressed_account(&conn, "424242", "mona");
    let mine = [format!("{mona} mona@example.com")];

    assert_eq!(claim(&conn, 0), mine);
    assert_eq!(claim(&conn, 15), NONE, "the first pass may still hold it");
    assert_eq!(claim(&conn, 30), mine);

    let mut now = 30;
    for attempt in 2..u32::from(ATTEMPTS) {
        now += u32::from(BACKOFF_MINUTES) << attempt;
        assert_eq!(claim(&conn, now), mine, "attempt {}", attempt + 1);
    }
    assert_eq!(claim(&conn, now + 10_000), NONE);
    assert_eq!(
        rows(
            &conn,
            "SELECT attempts, failure, failed_at, sent_at FROM notifications"
        ),
        [format!(r#"Integer(8) | Null | Text("{}") | Null"#, at(now))]
    );

    conn.execute(
        sql::MARK_NOTIFICATION_SENT,
        (row_of(&conn, mona), at(now), "<message@example.com>"),
    )
    .expect("MARK_NOTIFICATION_SENT");
    assert_eq!(
        rows(
            &conn,
            "SELECT failed_at, sent_at IS NOT NULL FROM notifications"
        ),
        ["Null | Integer(1)"]
    );
}

/// A claim takes one row, the oldest that is owed a mail; a row that
/// is held, waiting or given up is passed over.
#[test]
fn a_claim_takes_the_oldest_row_owed() {
    let conn = migrated_connection();
    let mona = addressed_account(&conn, "424242", "mona");
    let hubot = addressed_account(&conn, "515151", "hubot");
    let octo = addressed_account(&conn, "616161", "octo");

    assert_eq!(claim(&conn, 0), [format!("{mona} mona@example.com")]);
    assert_eq!(claim(&conn, 0), [format!("{hubot} hubot@example.com")]);
    fail(&conn, mona, 0, "E_RECIPIENT_SUPPRESSED", true);
    fail(&conn, hubot, 0, "E_INTERNAL_SERVER_ERROR", false);
    assert_eq!(claim(&conn, 15), [format!("{octo} octo@example.com")]);
    assert_eq!(claim(&conn, 15), NONE);
    assert_eq!(claim(&conn, 30), [format!("{hubot} hubot@example.com")]);
}

/// The address a claim returns is the one stored when it runs, not the
/// one stored when the event happened.
#[test]
fn a_claim_reads_the_address_stored_when_it_runs() {
    let conn = migrated_connection();
    let mona = addressed_account(&conn, "424242", "mona");
    set_address(&conn, mona, Some("moved@example.com"));
    assert_eq!(claim(&conn, 0), [format!("{mona} moved@example.com")]);

    // An address removed between two tries holds the row where it is.
    fail(&conn, mona, 0, "E_INTERNAL_SERVER_ERROR", false);
    set_address(&conn, mona, None);
    assert_eq!(claim(&conn, 30), NONE);
    set_address(&conn, mona, Some("back@example.com"));
    assert_eq!(claim(&conn, 30), [format!("{mona} back@example.com")]);
}
