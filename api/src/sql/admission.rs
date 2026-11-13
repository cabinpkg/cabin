//! admission: the atomic upload admission, the reservation lifecycle,
//! and the cron sweeps over them.
//!
//! An admission is one D1 batch whose first statement is the **gate**:
//! it holds every condition, and changes exactly one row when they all
//! hold and none otherwise. Every later statement applies only when the
//! statement directly before it changed one row (`changes()`), so the
//! gate's answer runs down the whole batch. A batch is one transaction,
//! but a statement that changes no rows is not an error and rolls
//! nothing back - which is why no later statement may apply on its own
//! conditions.
//!
//! `?1` is the request's clock sample in the stored ISO 8601 shape
//! throughout; its leading ten characters are the UTC usage date.

statements! {
    /// What the admission checks read for one prospective upload: the
    /// user's and the class's allowance rows for the UTC day of `?1`,
    /// the user's pending count (reservations included), whether the
    /// version already has revisions, and when the day's allowances
    /// reopen - the next UTC midnight and the seconds until it, from
    /// the same clock sample the usage date comes from.
    UPLOAD_USAGE =
        "SELECT \
         COALESCE((SELECT bytes FROM user_daily_upload_usage \
                   WHERE usage_date = substr(?1, 1, 10) AND user_id = ?2), 0) AS user_bytes, \
         COALESCE((SELECT uploads FROM user_daily_upload_usage \
                   WHERE usage_date = substr(?1, 1, 10) AND user_id = ?2), 0) AS user_uploads, \
         COALESCE((SELECT bytes FROM shared_daily_upload_usage \
                   WHERE usage_date = substr(?1, 1, 10) AND quota_class = ?3), 0) \
             AS shared_bytes, \
         COALESCE((SELECT uploads FROM shared_daily_upload_usage \
                   WHERE usage_date = substr(?1, 1, 10) AND quota_class = ?3), 0) \
             AS shared_uploads, \
         (SELECT COUNT(*) FROM revisions \
          WHERE published_by = ?2 AND verification = 'pending') AS pending, \
         EXISTS (SELECT 1 FROM revisions \
                 WHERE scope = ?4 AND name = ?5 AND version = ?6) AS version_has_revisions, \
         date(?1, '+1 day') || 'T00:00:00Z' AS reset_at, \
         CAST(strftime('%s', date(?1, '+1 day')) AS INTEGER) \
             - CAST(strftime('%s', ?1) AS INTEGER) AS retry_after_secs";

    /// The revision an upload id is bound to, if any: one id names one
    /// operation, so an id bound elsewhere is refused.
    REVISION_BY_UPLOAD_ID =
        "SELECT scope, name, version, revision FROM revisions WHERE upload_id = ?1";

    /// The gate of a **new** revision's admission: charges the user's
    /// daily allowance (`?3` the archive size) exactly when every
    /// condition holds - the revision key and the upload id (`?10`) are
    /// unused; no live revision of the version holds different bytes
    /// unless `?9` carries the `new-revision` opt-in; the publisher's
    /// storage (`?12`) and the package's daily revisions (`?13`) have
    /// room; a package this publish would create has no `-`/`_` twin
    /// and fits the daily (`?14`) and total (`?15`) package quotas; the
    /// user's (`?16`, `?17`) and the class's (`?11` the class, `?18`,
    /// `?19`) daily bytes and uploads have room for this archive; and
    /// the user's pending count is below `?20`. A limit is exact: an
    /// upload landing on it is admitted, the next is not.
    ADMIT_NEW_UPLOAD =
        "INSERT INTO user_daily_upload_usage (usage_date, user_id, bytes, uploads) \
         SELECT substr(?1, 1, 10), ?2, ?3, 1 \
         WHERE NOT EXISTS \
             (SELECT 1 FROM revisions WHERE scope = ?4 AND name = ?5 AND version = ?6 \
              AND revision = ?7) \
         AND NOT EXISTS (SELECT 1 FROM revisions WHERE upload_id = ?10) \
         AND (?9 OR NOT EXISTS \
              (SELECT 1 FROM revisions WHERE scope = ?4 AND name = ?5 AND version = ?6 \
               AND verification IN ('pending', 'verified') AND checksum <> ?8)) \
         AND (SELECT stored_bytes FROM users WHERE id = ?2) + ?3 <= ?12 \
         AND (SELECT COUNT(*) FROM revisions \
              WHERE scope = ?4 AND name = ?5 AND published_at >= substr(?1, 1, 11) \
                    AND published_at < substr(?1, 1, 10) || 'U') < ?13 \
         AND (EXISTS (SELECT 1 FROM packages WHERE scope = ?4 AND name = ?5) \
              OR (NOT EXISTS \
                      (SELECT 1 FROM packages WHERE scope = ?4 AND name != ?5 \
                       AND REPLACE(name, '_', '-') = REPLACE(?5, '_', '-')) \
                  AND (SELECT COUNT(*) FROM packages WHERE created_by = ?2 \
                       AND created_at >= substr(?1, 1, 11) \
                       AND created_at < substr(?1, 1, 10) || 'U') < ?14 \
                  AND (SELECT package_count FROM users WHERE id = ?2) < ?15)) \
         AND COALESCE((SELECT bytes FROM user_daily_upload_usage \
                       WHERE usage_date = substr(?1, 1, 10) AND user_id = ?2), 0) + ?3 <= ?16 \
         AND COALESCE((SELECT uploads FROM user_daily_upload_usage \
                       WHERE usage_date = substr(?1, 1, 10) AND user_id = ?2), 0) < ?17 \
         AND COALESCE((SELECT bytes FROM shared_daily_upload_usage \
                       WHERE usage_date = substr(?1, 1, 10) AND quota_class = ?11), 0) \
             + ?3 <= ?18 \
         AND COALESCE((SELECT uploads FROM shared_daily_upload_usage \
                       WHERE usage_date = substr(?1, 1, 10) AND quota_class = ?11), 0) < ?19 \
         AND (SELECT COUNT(*) FROM revisions \
              WHERE published_by = ?2 AND verification = 'pending') < ?20 \
         ON CONFLICT (usage_date, user_id) DO UPDATE SET \
         bytes = bytes + excluded.bytes, uploads = uploads + 1";

    /// Charges the user's daily allowance after a gate that is not
    /// [`ADMIT_NEW_UPLOAD`] (a revival's).
    CHARGE_USER_DAILY_UPLOAD =
        "INSERT INTO user_daily_upload_usage (usage_date, user_id, bytes, uploads) \
         SELECT substr(?1, 1, 10), ?2, ?3, 1 WHERE (SELECT changes()) = 1 \
         ON CONFLICT (usage_date, user_id) DO UPDATE SET \
         bytes = bytes + excluded.bytes, uploads = uploads + 1";

    /// Charges the class's shared daily allowance (`?2` the class).
    CHARGE_SHARED_DAILY_UPLOAD =
        "INSERT INTO shared_daily_upload_usage (usage_date, quota_class, bytes, uploads) \
         SELECT substr(?1, 1, 10), ?2, ?3, 1 WHERE (SELECT changes()) = 1 \
         ON CONFLICT (usage_date, quota_class) DO UPDATE SET \
         bytes = bytes + excluded.bytes, uploads = uploads + 1";

    /// Charges a new revision's archive to its publisher, and counts
    /// the package toward them when this admission is about to create
    /// it. Runs before [`ENSURE_PACKAGE`], so the package is absent
    /// here exactly when that statement creates it.
    COUNT_ADMITTED_UPLOAD =
        "UPDATE users SET stored_bytes = stored_bytes + ?2, \
         package_count = package_count \
             + (NOT EXISTS (SELECT 1 FROM packages WHERE scope = ?3 AND name = ?4)) \
         WHERE id = ?1 AND (SELECT changes()) = 1";

    /// Creates the package row on its first admitted revision. An
    /// existing row is rewritten to itself, not ignored: an ignored
    /// insert changes no rows and would stop the batch here.
    ENSURE_PACKAGE =
        "INSERT INTO packages (scope, name, created_at, created_by) \
         SELECT ?1, ?2, ?3, ?4 WHERE (SELECT changes()) = 1 \
         ON CONFLICT (scope, name) DO UPDATE SET created_at = created_at";

    /// Creates the version row on its first admitted revision; an
    /// existing row is rewritten to itself like [`ENSURE_PACKAGE`]'s.
    ENSURE_VERSION =
        "INSERT INTO versions (scope, name, version, yanked, downloads) \
         SELECT ?1, ?2, ?3, 0, 0 WHERE (SELECT changes()) = 1 \
         ON CONFLICT (scope, name, version) DO UPDATE SET yanked = yanked";

    /// Inserts the admitted revision as a reservation: `pending` with
    /// no `stored_at`, bound to its upload id (`?10`) and charged at
    /// its own `published_at` to the class `?11`. The batch's last
    /// statement; its changed row is the admission.
    INSERT_ADMITTED_REVISION =
        "INSERT INTO revisions (scope, name, version, revision, checksum, metadata_json, \
         published_at, archive_size, published_by, verification, upload_id, charged_at, \
         charged_class) \
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10, ?7, ?11 \
         WHERE (SELECT changes()) = 1";

    /// The gate of a **revival**: a new upload operation (`?10`, never
    /// one bound to any row) taking over a rejected revision with the
    /// same bytes - new metadata, publisher, and generation, back to a
    /// reservation. The conditions are [`ADMIT_NEW_UPLOAD`]'s apart
    /// from the package's, plus the generation rule: the new stamp
    /// (`?1`) must exceed the row's, so `(checksum, published_at)`
    /// names one publish event however the sampling clocks relate.
    REVIVE_REJECTED_UPLOAD =
        "UPDATE revisions SET metadata_json = ?21, published_at = ?1, published_by = ?2, \
         verification = 'pending', verification_reason = NULL, verified_at = NULL, \
         rejected_at = NULL, stored_at = NULL, upload_id = ?10, charged_at = ?1, \
         charged_class = ?11 \
         WHERE scope = ?4 AND name = ?5 AND version = ?6 AND revision = ?7 \
         AND verification = 'rejected' AND checksum = ?8 AND published_at < ?1 \
         AND NOT EXISTS (SELECT 1 FROM revisions WHERE upload_id = ?10) \
         AND (?9 OR NOT EXISTS \
              (SELECT 1 FROM revisions WHERE scope = ?4 AND name = ?5 AND version = ?6 \
               AND verification IN ('pending', 'verified') AND checksum <> ?8)) \
         AND (SELECT stored_bytes FROM users WHERE id = ?2) + ?3 <= ?12 \
         AND (SELECT COUNT(*) FROM revisions \
              WHERE scope = ?4 AND name = ?5 AND published_at >= substr(?1, 1, 11) \
                    AND published_at < substr(?1, 1, 10) || 'U') < ?13 \
         AND COALESCE((SELECT bytes FROM user_daily_upload_usage \
                       WHERE usage_date = substr(?1, 1, 10) AND user_id = ?2), 0) + ?3 <= ?16 \
         AND COALESCE((SELECT uploads FROM user_daily_upload_usage \
                       WHERE usage_date = substr(?1, 1, 10) AND user_id = ?2), 0) < ?17 \
         AND COALESCE((SELECT bytes FROM shared_daily_upload_usage \
                       WHERE usage_date = substr(?1, 1, 10) AND quota_class = ?11), 0) \
             + ?3 <= ?18 \
         AND COALESCE((SELECT uploads FROM shared_daily_upload_usage \
                       WHERE usage_date = substr(?1, 1, 10) AND quota_class = ?11), 0) < ?19 \
         AND (SELECT COUNT(*) FROM revisions \
              WHERE published_by = ?2 AND verification = 'pending') < ?20";

    /// The gate of a **resumed** operation: the same upload id (`?10`)
    /// and publisher (`?2`) returning to its own released reservation
    /// within `?14` of its first charge (an `SQLite` time modifier,
    /// `crate::upload::RETRY_WINDOW`). It takes a pending slot and the
    /// storage again under a new generation and is not charged the
    /// daily allowances a second time, so those are not conditions.
    RESUME_RELEASED_UPLOAD =
        "UPDATE revisions SET published_at = ?1, verification = 'pending', \
         verification_reason = NULL, rejected_at = NULL \
         WHERE scope = ?4 AND name = ?5 AND version = ?6 AND revision = ?7 \
         AND verification = 'rejected' AND stored_at IS NULL AND upload_id = ?10 \
         AND published_by = ?2 AND checksum = ?8 AND published_at < ?1 \
         AND charged_at >= strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?14) \
         AND (?9 OR NOT EXISTS \
              (SELECT 1 FROM revisions WHERE scope = ?4 AND name = ?5 AND version = ?6 \
               AND verification IN ('pending', 'verified') AND checksum <> ?8)) \
         AND (SELECT stored_bytes FROM users WHERE id = ?2) + ?3 <= ?12 \
         AND (SELECT COUNT(*) FROM revisions \
              WHERE scope = ?4 AND name = ?5 AND published_at >= substr(?1, 1, 11) \
                    AND published_at < substr(?1, 1, 10) || 'U') < ?13 \
         AND (SELECT COUNT(*) FROM revisions \
              WHERE published_by = ?2 AND verification = 'pending') < ?20";

    /// Turns a reservation into a pending upload the verifier may see,
    /// once its object is known to be in UPLOADS. Guarded on the
    /// generation the caller stored for (`?6` the upload id, `?7` the
    /// `published_at`): a reservation released and taken again in the
    /// meantime is a different generation and is left alone. Whoever
    /// confirms first sets the time; a later confirmation of the same
    /// generation still changes its row, so the request that admitted
    /// an upload never reads a racing retry's confirmation as a loss.
    CONFIRM_UPLOAD_STORED =
        "UPDATE revisions SET stored_at = COALESCE(stored_at, ?1) \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?5 \
         AND upload_id = ?6 AND published_at = ?7 AND verification = 'pending'";

    /// Reservations older than `?2` (an `SQLite` time modifier,
    /// `crate::upload::RESERVATION_TTL`), oldest first: the cron checks
    /// each one's object and confirms or releases it. `governor::Limits`
    /// sizes its infrastructure defaults by the `LIMIT`.
    EXPIRED_RESERVATIONS =
        "SELECT scope, name, version, revision, upload_id, published_at FROM revisions \
         WHERE verification = 'pending' AND stored_at IS NULL \
         AND published_at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2) \
         ORDER BY published_at LIMIT 50";

    /// Releases one expired reservation whose object is not in UPLOADS,
    /// guarded on the listed generation (`?6`). The pending slot is
    /// freed by the row leaving `pending`;
    /// [`REFUND_USER_STORED_BYTES`](super::quota::REFUND_USER_STORED_BYTES)
    /// follows in the same batch. The daily allowances stay charged.
    RELEASE_EXPIRED_RESERVATION =
        "UPDATE revisions SET verification = 'rejected', \
         verification_reason = 'upload_not_stored', rejected_at = ?1 \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?5 \
         AND verification = 'pending' AND stored_at IS NULL AND published_at = ?6";

    /// Refunds the storage of every upload [`TIME_OUT_PENDING_UPLOADS`]
    /// is about to reject. The two statements share one batch and the
    /// same binds, so they select the same rows.
    REFUND_TIMED_OUT_UPLOADS =
        "UPDATE users SET stored_bytes = stored_bytes - \
         (SELECT SUM(archive_size) FROM revisions \
          WHERE published_by = users.id AND verification = 'pending' \
          AND stored_at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)) \
         WHERE id IN (SELECT published_by FROM revisions \
                      WHERE verification = 'pending' \
                      AND stored_at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2))";

    /// Rejects every pending upload stored longer ago than `?2` (an
    /// `SQLite` time modifier, `crate::upload::PENDING_TIMEOUT`): its
    /// object is about to leave UPLOADS, so it can no longer be
    /// verified. A verdict racing this loses its own `pending` guard.
    TIME_OUT_PENDING_UPLOADS =
        "UPDATE revisions SET verification = 'rejected', \
         verification_reason = 'verification_timeout', rejected_at = ?1 \
         WHERE verification = 'pending' \
         AND stored_at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)";

    /// Deletes the users' allowance rows older than yesterday.
    PRUNE_USER_DAILY_UPLOADS =
        "DELETE FROM user_daily_upload_usage WHERE usage_date < date(?1, '-1 day')";

    /// Deletes the classes' allowance rows older than yesterday.
    PRUNE_SHARED_DAILY_UPLOADS =
        "DELETE FROM shared_daily_upload_usage WHERE usage_date < date(?1, '-1 day')";

    /// Forgets the upload operation of every row that turned terminal
    /// longer ago than `?2` (an `SQLite` time modifier,
    /// `crate::upload::UPLOAD_ID_RETENTION`). Pending rows have no
    /// terminal time and are never selected.
    PRUNE_SETTLED_UPLOAD_IDS =
        "UPDATE revisions SET upload_id = NULL, charged_at = NULL, charged_class = NULL \
         WHERE upload_id IS NOT NULL \
         AND COALESCE(verified_at, rejected_at) < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)";
}
