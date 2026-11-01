//! quota: the publish rate limit and the per-user quota counts.

statements! {
    /// The current token-bucket state straight from the user row.
    USER_BUCKET = "SELECT rl_tokens, rl_updated_at FROM users WHERE id = ?1";

    /// Persists a bucket take iff the row still holds the state the take
    /// was computed from (`IS` keeps the comparison NULL-safe).
    CAS_USER_BUCKET =
        "UPDATE users SET rl_tokens = ?1, rl_updated_at = ?2 \
         WHERE id = ?3 AND rl_tokens IS ?4 AND rl_updated_at IS ?5";

    /// The publisher's stored bytes; rejected rows were refunded.
    /// Superseded revisions keep charging - they stay fetchable.
    USER_STORED_BYTES = "SELECT stored_bytes FROM users WHERE id = ?1";

    /// The creator's total and created-today package counts. Only a new
    /// package spends the daily quota, so a publish into an existing
    /// `(?3, ?4)` skips the day count (zero); otherwise it is an index
    /// range bounded by that quota itself.
    USER_PACKAGE_COUNTS =
        "SELECT package_count, \
         CASE WHEN EXISTS (SELECT 1 FROM packages WHERE scope = ?3 AND name = ?4) THEN 0 \
         ELSE (SELECT COUNT(*) FROM packages WHERE created_by = ?1 \
               AND created_at >= ?2 AND created_at < substr(?2, 1, 10) || 'U') END \
         AS new_today FROM users WHERE id = ?1";

    /// Counts the package
    /// [`INSERT_PACKAGE`](super::packages::INSERT_PACKAGE) just created
    /// toward its creator. It must directly follow that insert in the
    /// batch: the `changes()` coupling of
    /// [`INSERT_TRUSTPUB_TOKEN`](super::trustpub::INSERT_TRUSTPUB_TOKEN),
    /// so an ignored or twin-suppressed insert counts nothing.
    COUNT_USER_PACKAGE =
        "UPDATE users SET package_count = package_count + 1 \
         WHERE id = ?1 AND (SELECT changes()) = 1";

    /// Charges an archive to its publisher, directly after the
    /// [`INSERT_REVISION`](super::packages::INSERT_REVISION) or
    /// [`REVIVE_REJECTED_REVISION`](super::packages::REVIVE_REJECTED_REVISION)
    /// that made the row live (same `changes()` coupling; a revival is
    /// byte-identical, so `?2` is the row's own `archive_size`).
    COUNT_USER_STORED_BYTES =
        "UPDATE users SET stored_bytes = stored_bytes + ?2 \
         WHERE id = ?1 AND (SELECT changes()) = 1";

    /// Refunds a rejected archive to the row's publisher, directly after
    /// the [`MARK_REVISION_REJECTED`](super::packages::MARK_REVISION_REJECTED) that flipped it.
    REFUND_USER_STORED_BYTES =
        "UPDATE users SET stored_bytes = stored_bytes - \
         (SELECT archive_size FROM revisions \
          WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?4) \
         WHERE id = (SELECT published_by FROM revisions \
                     WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?4) \
         AND (SELECT changes()) = 1";

    /// Revisions published into one package on the UTC day `?3`
    /// (`crate::quota::utc_day_prefix`) - the daily per-package version
    /// quota; a respin spends it like a new version, both store a new
    /// archive.
    COUNT_PACKAGE_VERSIONS_ON_DAY =
        "SELECT COUNT(*) AS n FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND published_at >= ?3 \
               AND published_at < substr(?3, 1, 10) || 'U'";

    /// Whether the package row already exists (new-package quotas).
    PACKAGE_EXISTS = "SELECT COUNT(*) AS n FROM packages WHERE scope = ?1 AND name = ?2";

    /// Whether creating `(scope, name)` would collide with an existing
    /// same-scope package under `-`/`_` folding: the deterministic
    /// publish reject (`docs/remote-registry.md`, "Publish").
    /// `REPLACE` in the query, not a normalized column: the
    /// `packages_folded_name` expression index serves it. The
    /// self-exclusion keeps the predicate identical to the in-batch
    /// guard on [`INSERT_PACKAGE`](super::packages::INSERT_PACKAGE).
    TWIN_PACKAGE_EXISTS =
        "SELECT COUNT(*) AS n FROM packages WHERE scope = ?1 AND name != ?2 \
         AND REPLACE(name, '_', '-') = REPLACE(?2, '_', '-')";

    /// The dashboard usage aggregate over everything the user published.
    USER_USAGE =
        "SELECT COALESCE(SUM(CASE WHEN verification != 'rejected' \
         THEN archive_size ELSE 0 END), 0) AS stored_bytes, \
         COALESCE(SUM(CASE WHEN published_at >= ?2 \
                           AND published_at < substr(?2, 1, 10) || 'U' \
                      THEN 1 ELSE 0 END), 0) AS published_today, \
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
