//! Executed-semantics tests for `src/sql/notifications.rs`: which mail
//! an event owes, and which row a cron pass claims next, claim after
//! claim.

use cabin_api_worker::notifications::{ATTEMPTS, BACKOFF_MINUTES, account_created, mail};
use cabin_api_worker::quota::{SCOPE_GRANT_MAILS_PER_DAY, SCOPE_UNYANK_MAILS_PER_DAY};
use cabin_api_worker::sql;
use rusqlite::Connection;
use rusqlite::types::Value;

use crate::common::{
    batch, count, migrated_connection, resolve, rows, seed_scope_collision, sign_in, verify_beta,
};

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

fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

/// The batch `glue::publication::place_archive` records the archive of
/// the seed's `beta/pkg@1.0.0#bb` with, at `minutes`; each statement's
/// changed rows.
fn place_beta(conn: &Connection, minutes: u32) -> Vec<usize> {
    let package = || vec![text("beta"), text("pkg")];
    let revision = || [package(), vec![text("1.0.0"), text("bb")]].concat();
    batch(
        conn,
        vec![
            (
                sql::MARK_REVISION_ARCHIVED,
                [vec![text(&at(minutes))], revision()].concat(),
            ),
            (sql::ADVANCE_CATALOG_GENERATION, vec![]),
            (sql::STAMP_PACKAGE_GENERATION, package()),
            (
                sql::RECORD_PUBLICATION_JOB,
                [package(), vec![text(&at(minutes))]].concat(),
            ),
            (sql::NOTIFY_REVISION_PUBLISHED, revision()),
        ],
    )
    .into_iter()
    .map(|(changed, _)| changed)
    .collect()
}

/// A user id as D1 binds it.
fn real(user: i64) -> Value {
    Value::Real(f64::from(i32::try_from(user).expect("a small id")))
}

/// The day's limit as D1 binds it.
fn limit() -> Value {
    Value::Real(f64::from(SCOPE_GRANT_MAILS_PER_DAY))
}

/// The batch `user`'s yank (`yanked`) or un-yank of `scope/pkg@1.0.0`
/// runs at `minutes`; each statement's changed rows.
fn set_yanked(conn: &Connection, scope: &str, yanked: bool, user: i64, minutes: u32) -> Vec<usize> {
    let package = || vec![text(scope), text("pkg")];
    let flag = || Value::Real(f64::from(u8::from(yanked)));
    batch(
        conn,
        vec![
            (
                sql::SET_VERSION_YANKED,
                [
                    vec![flag()],
                    package(),
                    vec![
                        text("1.0.0"),
                        text(&at(minutes)),
                        limit(),
                        real(user),
                        Value::Real(f64::from(SCOPE_UNYANK_MAILS_PER_DAY)),
                    ],
                ]
                .concat(),
            ),
            (sql::ADVANCE_CATALOG_GENERATION, vec![]),
            (sql::STAMP_PACKAGE_GENERATION, package()),
            (
                sql::RECORD_PUBLICATION_JOB,
                [package(), vec![text(&at(minutes))]].concat(),
            ),
            (
                sql::NOTIFY_VERSION_YANK,
                [
                    package(),
                    vec![text("1.0.0"), flag(), real(user), text(&at(minutes))],
                ]
                .concat(),
            ),
        ],
    )
    .into_iter()
    .map(|(changed, _)| changed)
    .collect()
}

/// The batch that adds `member` to `fmtlib` with `role` or, with none,
/// removes it, by `by` at `minutes`; each statement's changed rows.
fn change_member(
    conn: &Connection,
    member: i64,
    role: Option<&str>,
    by: i64,
    minutes: u32,
) -> Vec<usize> {
    let change = match role {
        Some(role) => (
            sql::ADD_SCOPE_MEMBER,
            vec![
                text("fmtlib"),
                real(member),
                text(role),
                text(&at(minutes)),
                limit(),
            ],
        ),
        None => (sql::REMOVE_SCOPE_MEMBER, vec![text("fmtlib"), real(member)]),
    };
    let mail = (
        sql::NOTIFY_MEMBERSHIP,
        vec![
            text("fmtlib"),
            real(member),
            real(by),
            text("github"),
            text(&at(minutes)),
            role.map_or(Value::Null, text),
        ],
    );
    batch(conn, vec![change, mail])
        .into_iter()
        .map(|(changed, _)| changed)
        .collect()
}

/// The scope `fmtlib`, owned by `owners`.
fn fmtlib(conn: &Connection, owners: &[i64]) {
    conn.execute(
        "INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at) \
         VALUES ('fmtlib', 'github', '7280970', ?1)",
        [at(0)],
    )
    .expect("the scope");
    for owner in owners {
        conn.execute(
            "INSERT INTO scope_members (scope_name, user_id, role) VALUES ('fmtlib', ?1, 'owner')",
            [owner],
        )
        .expect("an owner");
    }
}

/// The mail `mail` writes from the one row `filter` selects.
fn mail_of(conn: &Connection, filter: &str) -> String {
    let (kind, detail): (String, String) = conn
        .query_row(
            &format!("SELECT kind, detail FROM notifications WHERE {filter}"),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("the row");
    mail(&kind, &detail, "https://cabinpkg.com")
        .expect("a mail")
        .text
}

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

/// A revision owes the account that published it one mail when its
/// archive is recorded, not before; a second record of the archive
/// lands without owing another, and a batch that records none owes
/// none.
#[test]
fn a_placed_revision_owes_its_publisher_one_mail() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    // The seed's user created the package and owns nothing of beta's
    // revision once another account published it.
    let mona = create_account(&conn, "424242", "mona");
    conn.execute(
        "UPDATE revisions SET published_by = ?1 WHERE scope = 'beta'",
        [mona],
    )
    .expect("mona published beta's revision");
    let published =
        "SELECT event, recipient, created_at FROM notifications WHERE kind = 'revision_published'";

    assert_eq!(place_beta(&conn, 15), [0; 5], "pending");
    assert!(rows(&conn, published).is_empty());

    verify_beta(&conn);
    assert_eq!(place_beta(&conn, 15), [1; 5]);
    let owed = [format!(
        r#"Text("beta/pkg@1.0.0#bb") | Integer({mona}) | Text("{}")"#,
        at(15)
    )];
    assert_eq!(rows(&conn, published), owed);
    let detail: String = conn
        .query_row(
            "SELECT detail FROM notifications WHERE kind = 'revision_published'",
            [],
            |row| row.get(0),
        )
        .expect("the row");
    let mail = mail("revision_published", &detail, "https://cabinpkg.com").expect("a mail");
    for line in [
        "beta/pkg 1.0.0 was published",
        "Revision: bb\n",
        &format!("Published: {}\n", at(15)),
    ] {
        assert!(mail.text.contains(line), "{line:?} in {}", mail.text);
    }

    assert_eq!(place_beta(&conn, 30), [0; 5], "recorded already");
    // No statement of the Worker's clears `archived_at`. Were an
    // archive recorded twice all the same, the record must land.
    conn.execute(
        "UPDATE revisions SET archived_at = NULL WHERE scope = 'beta'",
        [],
    )
    .expect("un-record the archive");
    assert_eq!(place_beta(&conn, 45), [1, 1, 1, 1, 0]);
    assert_eq!(rows(&conn, published), owed);

    // A revision served before the registry sent these mails has no
    // row, and a later batch that records nothing gives it none.
    conn.execute("DELETE FROM notifications", [])
        .expect("forget the mail");
    assert_eq!(place_beta(&conn, 60), [0; 5]);
    assert!(rows(&conn, published).is_empty());
}

/// A publication's mail is not claimed while the job of its change is
/// open, and does not stop the rows after it. Once the publish pass
/// completes that job the mail is owed, whatever later change is open.
#[test]
fn a_publication_mail_waits_for_its_change_to_be_published() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    set_address(&conn, 2, Some("owner@example.com"));
    verify_beta(&conn);
    assert_eq!(place_beta(&conn, 0), [1; 5]);
    let mona = addressed_account(&conn, "424242", "mona");

    assert_eq!(claim(&conn, 0), [format!("{mona} mona@example.com")]);
    assert_eq!(claim(&conn, 15), NONE);
    assert_eq!(
        rows(
            &conn,
            "SELECT attempts FROM notifications WHERE kind = 'revision_published'"
        ),
        ["Integer(0)"]
    );

    // The placement took generation 1; the yanks take 2 and 3. Their
    // own mails go to an account with no address, and so to no claim.
    let hubot = create_account(&conn, "480938", "hubot");
    assert_eq!(set_yanked(&conn, "alpha", true, hubot, 0), [1; 5]);
    assert_eq!(set_yanked(&conn, "beta", true, hubot, 0), [1; 5]);
    let completed = conn
        .execute(
            sql::COMPLETE_PUBLICATION_JOBS,
            (at(15), "beta", "pkg", 1.0, at(0)),
        )
        .expect("COMPLETE_PUBLICATION_JOBS");
    assert_eq!(completed, 1);
    assert_eq!(
        rows(
            &conn,
            "SELECT generation FROM publication_jobs WHERE completed_at IS NULL ORDER BY 1"
        ),
        ["Integer(2)", "Integer(3)"]
    );
    assert_eq!(claim(&conn, 15), ["2 owner@example.com"]);
}

/// A yank that changes the flag owes the account that made it one
/// mail, and the un-yank after it another; a request that changes
/// nothing owes none. The mail is owed at once: the job of its change
/// is open, and the row is claimed all the same.
#[test]
fn a_changed_yank_owes_its_account_a_mail_at_once() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    set_address(&conn, 2, Some("owner@example.com"));
    // The event is the scope, the time, and the id before the row's.
    let yanks = "SELECT kind, event = 'alpha@' || created_at || '/' || (id - 1), recipient, \
                        created_at \
                 FROM notifications WHERE kind LIKE 'version_%' ORDER BY id";

    assert_eq!(
        set_yanked(&conn, "alpha", false, 2, 0),
        [0; 5],
        "not yanked"
    );
    assert!(rows(&conn, yanks).is_empty());

    assert_eq!(set_yanked(&conn, "alpha", true, 2, 15), [1; 5]);
    assert_eq!(
        set_yanked(&conn, "alpha", true, 2, 16),
        [0; 5],
        "yanked already"
    );
    let yanked = format!(
        r#"Text("version_yanked") | Integer(1) | Integer(2) | Text("{}")"#,
        at(15)
    );
    assert_eq!(rows(&conn, yanks), std::slice::from_ref(&yanked));
    assert!(
        mail_of(&conn, "kind = 'version_yanked'")
            .starts_with("alpha/pkg 1.0.0 was yanked under your account.\n")
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT generation FROM publication_jobs WHERE completed_at IS NULL"
        ),
        ["Integer(1)"]
    );
    assert_eq!(claim(&conn, 15), ["2 owner@example.com"]);

    // Each change of the flag is an event of its own.
    assert_eq!(set_yanked(&conn, "alpha", false, 2, 30), [1; 5]);
    assert_eq!(set_yanked(&conn, "alpha", true, 2, 45), [1; 5]);
    assert_eq!(
        rows(&conn, yanks),
        [
            yanked,
            format!(
                r#"Text("version_unyanked") | Integer(1) | Integer(2) | Text("{}")"#,
                at(30)
            ),
            format!(
                r#"Text("version_yanked") | Integer(1) | Integer(2) | Text("{}")"#,
                at(45)
            ),
        ]
    );
    assert!(
        mail_of(&conn, "kind = 'version_unyanked'")
            .starts_with("alpha/pkg 1.0.0 was un-yanked under your account.\n")
    );

    // So are changes recorded in one millisecond.
    for yanked in [false, true, false] {
        assert_eq!(set_yanked(&conn, "alpha", yanked, 2, 60), [1; 5]);
    }
    assert_eq!(rows(&conn, yanks).len(), 6);
}

/// A member's addition or removal owes a mail to the member and to
/// each owner the scope has after it, one each, and a request that
/// changes nothing owes none.
#[test]
fn a_membership_change_owes_the_member_and_each_owner_a_mail() {
    let conn = migrated_connection();
    let [mona, octocat, hubot, defunkt] = [
        ("424242", "mona"),
        ("583231", "octocat"),
        ("480938", "hubot"),
        ("2", "defunkt"),
    ]
    .map(|(account, login)| create_account(&conn, account, login));
    fmtlib(&conn, &[mona, hubot]);
    // A member who owns nothing is told of no change but its own.
    conn.execute(
        "INSERT INTO scope_members (scope_name, user_id, role) VALUES ('fmtlib', ?1, 'member')",
        [defunkt],
    )
    .expect("a plain member");
    // The recipients of the change recorded at `minutes`, in id order,
    // under the one event of the change.
    let told = |kind: &str, minutes: u32| {
        let of_change = format!("kind = '{kind}' AND created_at = '{}'", at(minutes));
        assert_eq!(
            rows(
                &conn,
                &format!("SELECT COUNT(DISTINCT event) FROM notifications WHERE {of_change}")
            ),
            ["Integer(1)"]
        );
        rows(
            &conn,
            &format!("SELECT recipient FROM notifications WHERE {of_change} ORDER BY 1"),
        )
    };
    let all = |users: &[i64]| -> Vec<String> {
        let mut users = users.to_vec();
        users.sort_unstable();
        users
            .iter()
            .map(|user| format!("Integer({user})"))
            .collect()
    };
    let changes = || count(&conn, "notifications") - 4;

    assert_eq!(
        change_member(&conn, octocat, Some("member"), mona, 15),
        [1, 3]
    );
    assert_eq!(told("member_added", 15), all(&[mona, hubot, octocat]));
    assert!(
        mail_of(
            &conn,
            &format!("kind = 'member_added' AND recipient = {octocat}")
        )
        .starts_with(
            "The GitHub account octocat was added to the Cabin scope fmtlib as member by mona.\n"
        )
    );
    assert_eq!(
        change_member(&conn, octocat, Some("owner"), mona, 16),
        [0, 0],
        "a member already"
    );

    assert_eq!(change_member(&conn, octocat, None, hubot, 30), [1, 3]);
    assert_eq!(told("member_removed", 30), all(&[mona, hubot, octocat]));
    assert!(
        mail_of(
            &conn,
            &format!("kind = 'member_removed' AND recipient = {mona}")
        )
        .starts_with(
            "The GitHub account octocat was removed from the Cabin scope fmtlib by hubot.\n"
        )
    );
    assert_eq!(
        change_member(&conn, octocat, None, hubot, 31),
        [0, 0],
        "no member"
    );

    // An owner the change added or removed is told once, as the member.
    assert_eq!(
        change_member(&conn, octocat, Some("owner"), mona, 45),
        [1, 3]
    );
    assert_eq!(told("member_added", 45), all(&[mona, hubot, octocat]));
    assert_eq!(change_member(&conn, hubot, None, hubot, 50), [1, 3]);
    assert_eq!(told("member_removed", 50), all(&[mona, hubot, octocat]));
    assert_eq!(change_member(&conn, octocat, None, mona, 55), [1, 2]);
    assert_eq!(told("member_removed", 55), all(&[mona, octocat]));
    assert_eq!(changes(), 14);

    // The last owner stays, and a removal that removed nothing owes
    // nothing.
    assert_eq!(change_member(&conn, mona, None, mona, 60), [0, 0]);
    assert_eq!(changes(), 14);

    // Changes recorded in one millisecond are events of their own: the
    // owner is told of the addition that came after the removal.
    for role in [Some("member"), None, Some("owner")] {
        assert_eq!(change_member(&conn, octocat, role, mona, 75), [1, 2]);
    }
    assert_eq!(changes(), 20);
}

/// A granted claim owes its account one mail, in the claim's batch.
#[test]
fn a_granted_claim_owes_its_account_one_mail() {
    let conn = migrated_connection();
    let mona = create_account(&conn, "424242", "mona");
    let octocat = create_account(&conn, "583231", "octocat");
    // The claim's batch as the callback runs it, for a limit of one
    // scope an account.
    let claim_scope = |scope: &str, user: i64, minutes: u32| -> Vec<usize> {
        let skeleton = || text(&cabin_api_worker::names::skeleton(scope));
        let (scope, now) = (|| text(scope), || text(&at(minutes)));
        let limit = || Value::Real(1.0);
        batch(
            &conn,
            vec![
                (
                    sql::CLAIM_SCOPE,
                    vec![
                        scope(),
                        text("github"),
                        text("7280970"),
                        now(),
                        real(user),
                        limit(),
                        skeleton(),
                    ],
                ),
                (
                    sql::SEED_CLAIM_OWNER,
                    vec![scope(), real(user), limit(), skeleton()],
                ),
                (
                    sql::RECORD_SCOPE_CLAIM,
                    vec![scope(), real(user), now(), limit(), skeleton()],
                ),
                (sql::NOTIFY_SCOPE_CLAIMED, vec![scope(), real(user), now()]),
            ],
        )
        .into_iter()
        .map(|(changed, _)| changed)
        .collect()
    };
    let claims =
        "SELECT event, recipient, created_at FROM notifications WHERE kind = 'scope_claimed'";

    assert_eq!(claim_scope("fmtlib", mona, 15), [1; 4]);
    let owed = [format!(
        r#"Text("fmtlib@{0}") | Integer({mona}) | Text("{0}")"#,
        at(15)
    )];
    assert_eq!(rows(&conn, claims), owed);
    assert!(
        mail_of(&conn, "kind = 'scope_claimed'")
            .starts_with("The Cabin scope fmtlib was claimed by your account.\n")
    );

    // A claim over the account's limit, and one of a name too like a
    // claimed one, record nothing and owe nothing.
    assert_eq!(claim_scope("boost", mona, 30), [0; 4]);
    assert_eq!(claim_scope("fmt-lib", octocat, 30), [0; 4]);
    assert_eq!(rows(&conn, claims), owed);
}

/// An account's un-yanks in a scope stop at the day's limit, which is
/// the account's own, the scope's own and the day's own, and all
/// accounts' stop at the scope's; yanks never stop.
#[test]
fn a_scopes_unyanks_of_a_day_are_bounded_and_its_yanks_are_not() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    // A day's limit of un-yanks under each name next to `alpha`, and
    // alpha's own of the day before: none of them is alpha's today.
    for prefix in [
        "alph@2026-07-16",
        "alpha-b@2026-07-16",
        "alphaa@2026-07-16",
        "alpha@2026-07-15",
    ] {
        for row in 0..SCOPE_GRANT_MAILS_PER_DAY {
            conn.execute(
                "INSERT INTO notifications (kind, event, recipient, detail, created_at) \
                 VALUES ('version_unyanked', ?1, 2, '{}', ?2)",
                (format!("{prefix}T23:{row:02}:00.000Z/{row}"), at(0)),
            )
            .expect("a neighbor's un-yank");
        }
    }

    for change in 0..SCOPE_GRANT_MAILS_PER_DAY {
        assert_eq!(set_yanked(&conn, "alpha", true, 2, change), [1; 5]);
        assert_eq!(set_yanked(&conn, "alpha", false, 2, change), [1; 5]);
    }
    assert_eq!(set_yanked(&conn, "alpha", true, 2, 1439), [1; 5]);
    assert_eq!(
        set_yanked(&conn, "alpha", false, 2, 1439),
        [0; 5],
        "the day's limit"
    );
    // Two more accounts spend theirs, and that is the scope's limit.
    let [hubot, octocat, defunkt] = [
        ("480938", "hubot"),
        ("583231", "octocat"),
        ("9919", "defunkt"),
    ]
    .map(|(account, login)| create_account(&conn, account, login));
    for account in [hubot, octocat] {
        for _ in 0..SCOPE_GRANT_MAILS_PER_DAY {
            assert_eq!(set_yanked(&conn, "alpha", false, account, 1439), [1; 5]);
            assert_eq!(set_yanked(&conn, "alpha", true, account, 1439), [1; 5]);
        }
    }
    assert_eq!(
        set_yanked(&conn, "alpha", false, defunkt, 1439),
        [0; 5],
        "the scope's limit"
    );
    assert_eq!(set_yanked(&conn, "beta", true, 2, 1439), [1; 5]);
    assert_eq!(set_yanked(&conn, "beta", false, 2, 1439), [1; 5]);
    assert_eq!(set_yanked(&conn, "alpha", false, 2, 1440), [1; 5]);
}

/// A scope's additions stop once the day's owe the limit of mails; a
/// removal is never refused.
#[test]
fn a_scopes_additions_of_a_day_are_bounded_and_its_removals_are_not() {
    let conn = migrated_connection();
    let [mona, octocat, hubot] = [
        ("424242", "mona"),
        ("583231", "octocat"),
        ("480938", "hubot"),
    ]
    .map(|(account, login)| create_account(&conn, account, login));
    fmtlib(&conn, &[mona]);

    // Each addition owes two mails here: the member's and the owner's.
    assert_eq!(change_member(&conn, hubot, Some("member"), mona, 0), [1, 2]);
    let additions = SCOPE_GRANT_MAILS_PER_DAY.div_ceil(2);
    for addition in 1..additions {
        assert_eq!(
            change_member(&conn, octocat, Some("member"), mona, addition),
            [1, 2]
        );
        assert_eq!(change_member(&conn, octocat, None, mona, addition), [1, 2]);
    }
    assert_eq!(
        change_member(&conn, octocat, Some("member"), mona, additions),
        [0, 0],
        "the day's limit"
    );
    assert_eq!(change_member(&conn, hubot, None, mona, 1439), [1, 2]);
    assert_eq!(
        change_member(&conn, hubot, Some("member"), mona, 1439),
        [0, 0]
    );
    assert_eq!(
        change_member(&conn, hubot, Some("member"), mona, 1440),
        [1, 2]
    );
}
