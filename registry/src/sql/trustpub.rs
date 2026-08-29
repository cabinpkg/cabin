//! trusted publishing: the OIDC exchange and revocation.

statements! {
    /// Every config registered for the repository the verified claims
    /// name, by its immutable numeric GitHub ids. Ordered for a
    /// deterministic multi-match refusal (`crate::trustpub::select_config`).
    TRUSTPUB_CONFIGS_BY_REPOSITORY =
        "SELECT scope, workflow_filename, git_ref, environment, quota_class \
         FROM trustpub_configs \
         WHERE repository_owner_id = ?1 AND repository_id = ?2 ORDER BY id";

    /// The backing user the exchange mints for: the matched scope's
    /// oldest owner (`user_id` is the only orderable column; membership
    /// rows carry no timestamp). Publish requires the token's user to
    /// be a scope member and stamps `published_by` from it, so an
    /// unclaimed scope - no owner row - refuses the exchange before
    /// the jti is consumed.
    TRUSTPUB_BACKING_OWNER =
        "SELECT user_id FROM scope_members \
         WHERE scope_name = ?1 AND role = 'owner' ORDER BY user_id LIMIT 1";

    /// Consumes a JWT's jti exactly once (`?2` is the end of the
    /// verifier's acceptance window, Unix seconds), shared by the
    /// exchange and the verdict endpoint over the one ledger. `OR
    /// IGNORE` turns the primary-key conflict into zero changed rows,
    /// which the glue reads as the replay refusal - the loser of a
    /// concurrent replay race sees the same zero without aborting
    /// anything. In the exchange batch it must run directly before
    /// [`INSERT_TRUSTPUB_TOKEN`], which reads `changes()` from this
    /// statement: the coupling keeps a mint that never happened from
    /// burning the jti (a failed batch rolls this consume back too),
    /// and a replayed jti from minting. The verdict batch consumes it
    /// with nothing coupled - nothing is minted there.
    CONSUME_OIDC_JTI =
        "INSERT OR IGNORE INTO oidc_used_jtis (jti, expires_at) VALUES (?1, ?2)";

    /// Lazy replay-guard cleanup, ridden on each successful exchange
    /// and each authenticated verdict (deliberately no cron): a row
    /// whose JWT could no longer verify anyway (`?1` is now, Unix
    /// seconds) protects nothing.
    PRUNE_EXPIRED_OIDC_JTIS = "DELETE FROM oidc_used_jtis WHERE expires_at <= ?1";

    /// Lazy minted-token cleanup beside the jti prune, ridden on each
    /// exchange and each session mint (deliberately no cron): every
    /// token kind is machine-minted and expiring now, and an expired
    /// row is pure residue nobody manages.
    PRUNE_EXPIRED_SHORT_LIVED_TOKENS =
        "DELETE FROM tokens WHERE expires_at <= ?1";

    /// Mints the config arm's short-lived publish token - but only when
    /// the immediately preceding [`CONSUME_OIDC_JTI`] in the same batch
    /// actually inserted its row: `changes()` is connection-scoped and
    /// statement-sequential, the same cross-statement coupling
    /// [`INSERT_USER_FOR_NEW_IDENTITY`](super::auth::INSERT_USER_FOR_NEW_IDENTITY) / [`UPSERT_IDENTITY`](super::auth::UPSERT_IDENTITY) ride
    /// through `last_insert_rowid()`. One transaction, guard inside it:
    /// a replayed jti (zero changes) mints nothing, and any batch
    /// failure rolls the consume back with the mint, so a 500 never
    /// burns a still-valid JWT. The schema's trustpub
    /// CHECK re-enforces the shape written here: expiring within a day
    /// of `created_at`, scope-limited, publish-scoped, and carrying the
    /// config's granted quota class on the row itself.
    INSERT_TRUSTPUB_TOKEN =
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                             expires_at, scope_limit, kind, quota_class) \
         SELECT ?1, ?2, ?3, ?4, 'publish', ?5, ?6, ?7, 'trustpub', ?8 \
         WHERE (SELECT changes()) = 1";

    /// The verifier arm's mint, [`INSERT_TRUSTPUB_TOKEN`]'s sibling with
    /// the same batch coupling: it must directly follow
    /// [`CONSUME_OIDC_JTI`], whose `changes()` it guards on. The
    /// schema's verify shape re-enforces what is written here:
    /// verify-scoped, unconfined, and NULL `quota_class` so the token
    /// inherits the backing user's class - exactly the row the static
    /// verify token carried, minus the unbounded lifetime.
    INSERT_TRUSTPUB_VERIFY_TOKEN =
        "INSERT INTO tokens (id, user_id, name, token_hash, scopes, created_at, \
                             expires_at, kind) \
         SELECT ?1, ?2, ?3, ?4, 'verify', ?5, ?6, 'trustpub' \
         WHERE (SELECT changes()) = 1";

    /// The verifier arm's backing user: the operator identity the
    /// `VERIFIER_BACKING_ACCOUNT_ID` var names, by its immutable
    /// numeric GitHub id. The baseline migration seeds this identity,
    /// so no row means the var and the seed disagree; the refusal
    /// still runs before the jti is consumed, like
    /// [`TRUSTPUB_BACKING_OWNER`]'s unclaimed-scope refusal, so fixing
    /// the mismatch and retrying the same run stays possible.
    VERIFIER_BACKING_USER =
        "SELECT user_id FROM identities \
         WHERE provider = 'github' AND provider_account_id = ?1";

    /// Revocation by the token itself: deletes the authenticated row
    /// iff it is a trustpub one. Zero changed rows - a session token's
    /// id - is the caller's uniform 401, so the endpoint is no
    /// token-kind oracle.
    DELETE_TRUSTPUB_TOKEN = "DELETE FROM tokens WHERE id = ?1 AND kind = 'trustpub'";
}
