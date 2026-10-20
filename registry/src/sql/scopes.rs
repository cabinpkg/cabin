//! scopes: the claim flow and membership management.

statements! {
    /// The claim callback's pre-check: claims are permanent, so an
    /// existing row refuses whoever asks.
    SCOPE_EXISTS = "SELECT COUNT(*) AS n FROM scopes WHERE name = ?1";

    /// Claims a scope, guarded on the claimant's lifetime claim limit:
    /// the count over the append-only `scope_claims` history (`?5` the
    /// claimant, `?6` the class limit), so releasing or transferring a
    /// scope never restores capacity. All three claim-batch statements
    /// repeat this guard verbatim - the history only grows at the
    /// batch's last statement, so within the transaction the three
    /// always agree, and an over-limit claim suppresses every insert
    /// (zero changed rows; the glue answers the uniform denial).
    ///
    /// The second guard, repeated the same way, is the name-fidelity
    /// refusal the primary key cannot give: `?7` is the requested
    /// scope's skeleton (`crate::names::skeleton`, mirrored as the
    /// `REPLACE` chain - the fold's outputs `l`/`o` are never later
    /// inputs, so the sequential replaces equal the per-character
    /// fold), and an existing *other* scope with that skeleton
    /// suppresses the batch. Running inside the transaction, it closes
    /// the race two confusable claims could otherwise win together
    /// through the preflight read. `name != ?1` keeps the guard blind
    /// to the row this statement inserts, so its repeats in the two
    /// siblings still pass, and leaves an exact-name collision to the
    /// primary key: the loser of a claim race fails the statement,
    /// which rolls back its whole batch - [`SEED_CLAIM_OWNER`] and
    /// [`RECORD_SCOPE_CLAIM`] must run in that same batch, so a lost
    /// race can never seed the loser as an owner of the winner's
    /// scope, and a failed claim can never consume claim capacity. The
    /// glue binds `NULL` for an operator-exempted scope
    /// (`CLAIM_SKELETON_EXEMPT_SCOPES`), which matches nothing.
    CLAIM_SCOPE =
        "INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at) \
         SELECT ?1, ?2, ?3, ?4 \
         WHERE (SELECT COUNT(*) FROM scope_claims WHERE claimed_by = ?5) < ?6 \
         AND NOT EXISTS (SELECT 1 FROM scopes WHERE name != ?1 AND \
           REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(name, '-', ''), '_', ''), \
                   '1', 'l'), 'i', 'l'), '0', 'o') = ?7)";

    /// Seeds the claiming user as the new scope's first owner, in the
    /// same batch as [`CLAIM_SCOPE`] and under the same two guards.
    SEED_CLAIM_OWNER =
        "INSERT INTO scope_members (scope_name, user_id, role) \
         SELECT ?1, ?2, 'owner' \
         WHERE (SELECT COUNT(*) FROM scope_claims WHERE claimed_by = ?2) < ?3 \
         AND NOT EXISTS (SELECT 1 FROM scopes WHERE name != ?1 AND \
           REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(name, '-', ''), '_', ''), \
                   '1', 'l'), 'i', 'l'), '0', 'o') = ?4)";

    /// Records the granted claim in the append-only history, last in
    /// the claim batch (the guards must count the history *before*
    /// this grant, like its two siblings'). Rows here are never
    /// updated or deleted: the lifetime limit counts grants, not
    /// current ownership.
    RECORD_SCOPE_CLAIM =
        "INSERT INTO scope_claims (scope_name, claimed_by, claimed_at) \
         SELECT ?1, ?2, ?3 \
         WHERE (SELECT COUNT(*) FROM scope_claims WHERE claimed_by = ?2) < ?4 \
         AND NOT EXISTS (SELECT 1 FROM scopes WHERE name != ?1 AND \
           REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(name, '-', ''), '_', ''), \
                   '1', 'l'), 'i', 'l'), '0', 'o') = ?5)";

    /// Every claimed scope name, for the claim callback's preflight
    /// skeleton confusability refusal (`docs/architecture.md`, "Name
    /// fidelity"; the in-batch guard in [`CLAIM_SCOPE`] is what closes
    /// the race). Scopes are few; the breaker's `d1_rows_read_day`
    /// budget is the tripwire if that stops holding.
    LIST_SCOPE_NAMES = "SELECT name FROM scopes ORDER BY name";

    /// Whether the user holds the `owner` role in the scope: the gate on
    /// every membership-management endpoint. A scope that does not exist
    /// has no owners, so nonexistent and foreign scopes answer
    /// identically, mirroring [`SCOPE_MEMBERSHIP`](super::packages::SCOPE_MEMBERSHIP).
    SCOPE_OWNER_MEMBERSHIP =
        "SELECT COUNT(*) AS n FROM scope_members \
         WHERE scope_name = ?1 AND user_id = ?2 AND role = 'owner'";

    /// The members listing, resolved back to the external identity the
    /// management API speaks (the provider bind is policy's `github`).
    /// Ordered by the stable registry user id for determinism.
    LIST_SCOPE_MEMBERS =
        "SELECT i.provider_account_id, i.login_snapshot, sm.role \
         FROM scope_members sm \
         JOIN identities i ON i.user_id = sm.user_id AND i.provider = ?2 \
         WHERE sm.scope_name = ?1 ORDER BY sm.user_id";

    /// One member's current role, if any (shapes the add/remove
    /// responses).
    SCOPE_MEMBER_ROLE =
        "SELECT role FROM scope_members WHERE scope_name = ?1 AND user_id = ?2";

    /// Adds a member; an existing membership keeps its role (there is no
    /// role-change endpoint, and an upsert here could demote the last
    /// owner).
    ADD_SCOPE_MEMBER =
        "INSERT OR IGNORE INTO scope_members (scope_name, user_id, role) \
         VALUES (?1, ?2, ?3)";

    /// Removes a member unless that would leave the scope ownerless: the
    /// last-owner rule is enforced inside the statement, so concurrent
    /// removals cannot race past it.
    REMOVE_SCOPE_MEMBER =
        "DELETE FROM scope_members WHERE scope_name = ?1 AND user_id = ?2 \
         AND (role != 'owner' OR \
              (SELECT COUNT(*) FROM scope_members \
               WHERE scope_name = ?1 AND role = 'owner') > 1)";
}
