//! Executed-semantics tests for `src/sql/scopes.rs`: the claim flow
//! and membership management.

use cabin_registry_worker::{names, sql};

use crate::common::{count, migrated_connection, seed_scope_collision};

/// One claim's write, modeled on how the claim callback runs it: the
/// three statements inside one transaction (a D1 batch), aborting at
/// the first failure the way D1 aborts and rolls back a batch. The
/// error comes back to the caller because the scope insert's failure
/// is load-bearing: it is what makes the loser of a claim race roll
/// back seedless. `Ok(applied)` mirrors the glue's zero-changed-rows
/// read on the scope insert - an over-limit claim suppresses every
/// statement and refuses in-band. The skeleton bind mirrors the
/// glue's: the scope's fold, `NULL` when the operator exempted it.
fn claim(
    conn: &rusqlite::Connection,
    scope: &str,
    account_id: &str,
    user_id: i64,
    now: &str,
    limit: i64,
) -> rusqlite::Result<bool> {
    claim_with_skeleton(
        conn,
        scope,
        Some(&names::skeleton(scope)),
        account_id,
        user_id,
        now,
        limit,
    )
}

fn claim_with_skeleton(
    conn: &rusqlite::Connection,
    scope: &str,
    skeleton: Option<&str>,
    account_id: &str,
    user_id: i64,
    now: &str,
    limit: i64,
) -> rusqlite::Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let applied = tx.execute(
        sql::CLAIM_SCOPE,
        rusqlite::params![scope, "github", account_id, now, user_id, limit, skeleton],
    )?;
    tx.execute(
        sql::SEED_CLAIM_OWNER,
        rusqlite::params![scope, user_id, limit, skeleton],
    )?;
    tx.execute(
        sql::RECORD_SCOPE_CLAIM,
        rusqlite::params![scope, user_id, now, limit, skeleton],
    )?;
    tx.commit()?;
    Ok(applied > 0)
}

fn member_role(conn: &rusqlite::Connection, scope: &str, user_id: i64) -> Option<String> {
    conn.query_row(
        sql::SCOPE_MEMBER_ROLE,
        rusqlite::params![scope, user_id],
        |row| row.get(0),
    )
    .map(Some)
    .or_else(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
    .expect("member role")
}

#[test]
fn a_claim_seeds_its_owner_and_a_lost_race_fails_seedless() {
    let conn = migrated_connection();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (2, '2026-07-15T00:00:00.000Z'),
                                                   (3, '2026-07-15T00:00:00.000Z');",
    )
    .expect("seed users");

    let applied =
        claim(&conn, "fmtlib", "7280970", 2, "2026-07-15T00:00:00.000Z", 3).expect("winning claim");
    assert!(applied);
    assert_eq!(member_role(&conn, "fmtlib", 2), Some("owner".to_owned()));

    // The claim callback pre-checks SCOPE_EXISTS, but the write must
    // stay correct without it: a claim that lost the race between the
    // pre-check and the batch fails the primary-key insert - even with
    // byte-identical proof and timestamp, the collision two same-instant
    // admins of one org produce - which aborts and rolls back its
    // batch, so the loser never becomes an owner and the winner's row
    // is untouched.
    let lost = claim(&conn, "fmtlib", "7280970", 3, "2026-07-15T00:00:00.000Z", 3);
    assert!(lost.is_err(), "a second claim must fail the insert");
    assert_eq!(member_role(&conn, "fmtlib", 3), None);
    // The rollback covers the claim-history insert too: a failed claim
    // never consumes claim capacity.
    assert_eq!(
        count(&conn, "scope_claims"),
        1,
        "only the winner's claim is on record"
    );
    let (proof, claimed_at): (String, String) = conn
        .query_row(
            "SELECT proof_account_id, claimed_at FROM scopes WHERE name = 'fmtlib'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("scope row");
    assert_eq!(proof, "7280970");
    assert_eq!(claimed_at, "2026-07-15T00:00:00.000Z");
    assert_eq!(
        count(&conn, "scope_members"),
        1,
        "the winner stays the sole owner"
    );

    // SCOPE_EXISTS is the pre-check the callback's refusal rests on.
    for (scope, expected) in [("fmtlib", 1), ("ghost", 0)] {
        let n: i64 = conn
            .query_row(sql::SCOPE_EXISTS, [scope], |row| row.get(0))
            .expect("scope exists");
        assert_eq!(n, expected, "scope: {scope}");
    }
}

#[test]
fn the_lifetime_claim_limit_counts_history_not_ownership() {
    let conn = migrated_connection();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (2, '2026-07-15T00:00:00.000Z'),
                                                   (3, '2026-07-15T00:00:00.000Z');",
    )
    .expect("seed users");
    let now = "2026-07-15T00:00:00.000Z";

    // The default class's lifetime capacity: three grants land...
    for (scope, account_id) in [("one", "10"), ("two", "20"), ("three", "30")] {
        assert!(
            claim(&conn, scope, account_id, 2, now, 3).expect("claim under the limit"),
            "scope: {scope}"
        );
    }
    // ...and the fourth refuses in-band: every statement of the batch
    // repeats the guard, so nothing is inserted anywhere.
    assert!(!claim(&conn, "four", "40", 2, now, 3).expect("over-limit claim still executes"));
    for (table, expected) in [("scopes", 3), ("scope_members", 3), ("scope_claims", 3)] {
        assert_eq!(count(&conn, table), expected, "table: {table}");
    }
    assert_eq!(member_role(&conn, "four", 2), None);

    // Releasing scopes (operator surgery; there is no release or
    // transfer endpoint) never restores capacity: the append-only
    // history outlives the `scopes` rows.
    conn.execute_batch(
        "DELETE FROM scope_members WHERE scope_name IN ('one', 'two');
         DELETE FROM scopes WHERE name IN ('one', 'two');",
    )
    .expect("release two scopes");
    assert!(!claim(&conn, "five", "50", 2, now, 3).expect("claim after release"));

    // The limit is per user: another account's capacity is untouched,
    // and a re-claim of a released name spends the new claimant's.
    assert!(claim(&conn, "one", "10", 3, now, 3).expect("another user's claim"));

    // The usage read reports the history count the guard enforces.
    for (user, expected) in [(2, 3), (3, 1)] {
        let n: i64 = conn
            .query_row(sql::USER_SCOPE_CLAIM_COUNT, [user], |row| row.get(0))
            .expect("claim count");
        assert_eq!(n, expected, "user: {user}");
    }
}

/// The confusability guard runs inside the batch, so a claim whose
/// skeleton matches an already-claimed scope refuses in-band however
/// it got past the preflight read - the two-OAuth-roundtrip race -
/// and writes nothing anywhere. The SQL fold is pinned to the Rust
/// one through the same examples `names::skeleton` is tested with.
#[test]
fn a_confusable_claim_refuses_inside_the_batch() {
    let conn = migrated_connection();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (2, '2026-07-15T00:00:00.000Z'),
                                                   (3, '2026-07-15T00:00:00.000Z');",
    )
    .expect("seed users");
    let now = "2026-07-15T00:00:00.000Z";
    assert!(claim(&conn, "fmtlib", "10", 2, now, 10).expect("first claim"));
    assert!(claim(&conn, "zolog", "20", 2, now, 10).expect("second claim"));

    for confusable in ["fmtl1b", "f-mt-lib", "fmtl-ib", "z0log"] {
        assert!(
            !claim(&conn, confusable, "30", 3, now, 10).expect("confusable claim executes"),
            "scope: {confusable}"
        );
    }
    for (table, expected) in [("scopes", 2), ("scope_members", 2), ("scope_claims", 2)] {
        assert_eq!(count(&conn, table), expected, "table: {table}");
    }
    // A different skeleton is not confusable.
    assert!(claim(&conn, "fmts", "40", 3, now, 10).expect("distinct claim"));
    // The operator exemption binds no skeleton, which skips the guard.
    assert!(claim_with_skeleton(&conn, "fmtl1b", None, "30", 3, now, 10).expect("exempt claim"));
    assert_eq!(member_role(&conn, "fmtl1b", 3), Some("owner".to_owned()));
}

/// The reverse order pins the SQL fold itself: here the *stored* name
/// carries the digit, hyphen or underscore, so only the `REPLACE`
/// chain - not the Rust fold of the candidate - can connect the two.
/// Each case starts from a fresh database, so an earlier refusal
/// cannot mask a missing fold, and a refusal leaves every table as the
/// stored claim left it.
#[test]
fn the_sql_fold_matches_stored_digits_hyphens_and_underscores() {
    for (stored, candidate) in [
        ("fmtl1b", "fmtlib"),
        ("fmt-lib", "fmtlib"),
        ("fmt_lib", "fmtlib"),
        ("z0log", "zolog"),
    ] {
        let conn = migrated_connection();
        conn.execute_batch(
            "INSERT INTO users (id, created_at) VALUES (2, '2026-07-15T00:00:00.000Z'),
                                                       (3, '2026-07-15T00:00:00.000Z');",
        )
        .expect("seed users");
        let now = "2026-07-15T00:00:00.000Z";
        assert!(
            claim(&conn, stored, "10", 2, now, 10).expect("stored claim"),
            "stored: {stored}"
        );
        assert!(
            !claim(&conn, candidate, "30", 3, now, 10).expect("confusable claim executes"),
            "stored: {stored}, candidate: {candidate}"
        );
        for table in ["scopes", "scope_members", "scope_claims"] {
            assert_eq!(count(&conn, table), 1, "stored: {stored}, table: {table}");
        }
    }
}

#[test]
fn membership_management_enforces_the_last_owner_rule() {
    let conn = migrated_connection();
    conn.execute_batch(
        "INSERT INTO users (id, created_at) VALUES (2, '2026-07-15T00:00:00.000Z'),
                                                   (3, '2026-07-15T00:00:00.000Z');
         INSERT INTO identities (provider, provider_account_id, login_snapshot, user_id)
           VALUES ('github', '424242', 'mona', 2),
                  ('github', '583231', 'octocat', 3);",
    )
    .expect("seed users");
    claim(&conn, "fmtlib", "7280970", 2, "2026-07-15T00:00:00.000Z", 3).expect("claim");

    // The role domain is closed in the schema itself:
    // membership disputes are manual SQL, and a typo there must not
    // silently widen access or orphan a scope. (Through the API's
    // INSERT OR IGNORE a bad role is swallowed instead - either way it
    // never lands.)
    let bad_role = conn.execute(
        "INSERT INTO scope_members (scope_name, user_id, role) VALUES ('fmtlib', 3, 'admin')",
        [],
    );
    assert!(bad_role.is_err(), "the role CHECK must refuse 'admin'");
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 3, "admin"],
    )
    .expect("an ignored bad-role insert");
    assert_eq!(member_role(&conn, "fmtlib", 3), None);

    // Only the owner role passes the management gate.
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 3, "member"],
    )
    .expect("add member");
    let owner_gate = |user_id: i64| -> i64 {
        conn.query_row(
            sql::SCOPE_OWNER_MEMBERSHIP,
            rusqlite::params!["fmtlib", user_id],
            |row| row.get(0),
        )
        .expect("owner gate")
    };
    assert_eq!(owner_gate(2), 1);
    assert_eq!(owner_gate(3), 0);

    // Adding an existing member never rewrites their role: an upsert
    // here could demote the last owner.
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 2, "member"],
    )
    .expect("re-add owner");
    assert_eq!(member_role(&conn, "fmtlib", 2), Some("owner".to_owned()));

    // The listing resolves members back to their GitHub identity,
    // deterministically ordered.
    let mut statement = conn.prepare(sql::LIST_SCOPE_MEMBERS).expect("prepare");
    let members: Vec<(String, String, String)> = statement
        .query_map(rusqlite::params!["fmtlib", "github"], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("list members")
        .collect::<Result<_, _>>()
        .expect("member rows");
    assert_eq!(
        members,
        vec![
            ("424242".to_owned(), "mona".to_owned(), "owner".to_owned()),
            (
                "583231".to_owned(),
                "octocat".to_owned(),
                "member".to_owned()
            ),
        ]
    );

    // Removing the last owner is refused inside the statement itself;
    // an ordinary member and a co-owned owner both remove fine.
    let removed = conn
        .execute(sql::REMOVE_SCOPE_MEMBER, rusqlite::params!["fmtlib", 2])
        .expect("remove last owner");
    assert_eq!(removed, 0, "the last owner must survive removal");
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 3, "owner"],
    )
    .expect("promote nobody");
    // User 3 is already a member: the add was ignored, so user 2 is
    // still the only owner and still protected.
    let removed = conn
        .execute(sql::REMOVE_SCOPE_MEMBER, rusqlite::params!["fmtlib", 2])
        .expect("remove still-last owner");
    assert_eq!(removed, 0);
    let removed = conn
        .execute(sql::REMOVE_SCOPE_MEMBER, rusqlite::params!["fmtlib", 3])
        .expect("remove member");
    assert_eq!(removed, 1);

    // With a genuine second owner the first one may leave.
    conn.execute(
        sql::ADD_SCOPE_MEMBER,
        rusqlite::params!["fmtlib", 3, "owner"],
    )
    .expect("add second owner");
    let removed = conn
        .execute(sql::REMOVE_SCOPE_MEMBER, rusqlite::params!["fmtlib", 2])
        .expect("remove co-owner");
    assert_eq!(removed, 1);
    assert_eq!(owner_gate(3), 1);
}

/// The claim confusability read is a plain ordered name listing.
#[test]
fn scope_names_list_in_order() {
    let conn = migrated_connection();
    seed_scope_collision(&conn);
    let mut statement = conn.prepare(sql::LIST_SCOPE_NAMES).expect("prepare scopes");
    let names: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("run scopes")
        .collect::<Result<_, _>>()
        .expect("scope names");
    assert_eq!(names, ["alpha", "beta"]);
}
