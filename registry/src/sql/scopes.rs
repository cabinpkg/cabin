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
    /// Still an insert into the `name` primary key: the loser of a
    /// claim race fails the statement, which rolls back its whole
    /// batch - [`SEED_CLAIM_OWNER`] and [`RECORD_SCOPE_CLAIM`] must
    /// run in that same batch, so a lost race can never seed the loser
    /// as an owner of the winner's scope, and a failed claim can never
    /// consume claim capacity.
    CLAIM_SCOPE =
        "INSERT INTO scopes (name, proof_provider, proof_account_id, claimed_at) \
         SELECT ?1, ?2, ?3, ?4 \
         WHERE (SELECT COUNT(*) FROM scope_claims WHERE claimed_by = ?5) < ?6";

    /// Seeds the claiming user as the new scope's first owner, in the
    /// same batch as [`CLAIM_SCOPE`] and under the same limit guard.
    SEED_CLAIM_OWNER =
        "INSERT INTO scope_members (scope_name, user_id, role) \
         SELECT ?1, ?2, 'owner' \
         WHERE (SELECT COUNT(*) FROM scope_claims WHERE claimed_by = ?2) < ?3";

    /// Records the granted claim in the append-only history, last in
    /// the claim batch (the guard must count the history *before* this
    /// grant, like its two siblings'). Rows here are never updated or
    /// deleted: the lifetime limit counts grants, not current
    /// ownership.
    RECORD_SCOPE_CLAIM =
        "INSERT INTO scope_claims (scope_name, claimed_by, claimed_at) \
         SELECT ?1, ?2, ?3 \
         WHERE (SELECT COUNT(*) FROM scope_claims WHERE claimed_by = ?2) < ?4";

    /// Every claimed scope name, for the claim callback's skeleton
    /// confusability refusal (`docs/architecture.md`, "Name
    /// fidelity"): the fold runs in Rust (`crate::names::skeleton`),
    /// so the map lives in one place per crate instead of a second
    /// SQL spelling. Scopes are few; the breaker's `d1_rows_read_day`
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
