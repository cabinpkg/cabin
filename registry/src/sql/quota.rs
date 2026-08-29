//! quota: the publish rate limit and the per-user quota counts.

statements! {
    /// The current token-bucket state straight from the token row.
    TOKEN_BUCKET = "SELECT rl_tokens, rl_updated_at FROM tokens WHERE id = ?1";

    /// Persists a bucket take iff the row still holds the state the take
    /// was computed from (`IS` keeps the comparison NULL-safe).
    CAS_TOKEN_BUCKET =
        "UPDATE tokens SET rl_tokens = ?1, rl_updated_at = ?2 \
         WHERE id = ?3 AND rl_tokens IS ?4 AND rl_updated_at IS ?5";

    /// The publisher's stored bytes; rejected rows were refunded.
    /// Superseded revisions keep charging - they stay fetchable.
    USER_STORED_BYTES =
        "SELECT COALESCE(SUM(archive_size), 0) AS stored_bytes \
         FROM revisions WHERE published_by = ?1 AND verification != 'rejected'";

    /// The creator's total and created-today package counts.
    USER_PACKAGE_COUNTS =
        "SELECT COUNT(*) AS package_count, \
         COALESCE(SUM(created_at >= ?2), 0) AS new_today \
         FROM packages WHERE created_by = ?1";

    /// Revisions published into one package since a cutoff (the daily
    /// per-package version quota; a respin spends it like a new
    /// version - both store a new archive).
    COUNT_PACKAGE_VERSIONS_SINCE =
        "SELECT COUNT(*) AS n FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND published_at >= ?3";

    /// Whether the package row already exists (new-package quotas).
    PACKAGE_EXISTS = "SELECT COUNT(*) AS n FROM packages WHERE scope = ?1 AND name = ?2";

    /// Whether creating `(scope, name)` would collide with an existing
    /// same-scope package under `-`/`_` folding: the deterministic
    /// publish reject (`docs/architecture.md`, "Name fidelity").
    /// `REPLACE` in the query, not a normalized column - the packages
    /// table is small and this runs once per prospective publish. The
    /// self-exclusion keeps the predicate identical to the in-batch
    /// guards on [`INSERT_PACKAGE`](super::packages::INSERT_PACKAGE) / [`INSERT_VERSION_ROW`](super::packages::INSERT_VERSION_ROW).
    TWIN_PACKAGE_EXISTS =
        "SELECT COUNT(*) AS n FROM packages WHERE scope = ?1 AND name != ?2 \
         AND REPLACE(name, '_', '-') = REPLACE(?2, '_', '-')";

    /// The dashboard usage aggregate over everything the user published.
    USER_USAGE =
        "SELECT COALESCE(SUM(CASE WHEN verification != 'rejected' \
         THEN archive_size ELSE 0 END), 0) AS stored_bytes, \
         COALESCE(SUM(CASE WHEN published_at >= ?2 THEN 1 ELSE 0 END), 0) AS published_today, \
         COALESCE(SUM(verification = 'verified'), 0) AS verified_count, \
         COALESCE(SUM(verification = 'pending'), 0) AS pending_count, \
         COALESCE(SUM(verification = 'rejected'), 0) AS rejected_count \
         FROM revisions WHERE published_by = ?1";

    /// The dashboard's created-package count (quota semantics: created,
    /// not merely published into).
    USER_CREATED_PACKAGE_COUNT = "SELECT COUNT(*) AS n FROM packages WHERE created_by = ?1";

    /// The user's lifetime successful-claim count, for the usage
    /// payload (the enforcement itself lives inside the claim batch's
    /// guards, never on this read).
    USER_SCOPE_CLAIM_COUNT = "SELECT COUNT(*) AS n FROM scope_claims WHERE claimed_by = ?1";
}
