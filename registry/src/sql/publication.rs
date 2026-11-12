//! publication: placing verified archives in ARCHIVES.

statements! {
    /// The placement pass's work list: verified revisions whose archive
    /// is not confirmed in ARCHIVES, oldest verdict first. `upload_id`
    /// is NULL once the row has outlived its upload operation's
    /// retention, long after UPLOADS expired the object. `INDEXED BY`
    /// because the registry never runs ANALYZE: left alone, the
    /// planner takes `revisions_verification` and reads every
    /// verified row to sort them.
    UNARCHIVED_REVISIONS =
        "SELECT scope, name, version, revision, checksum, archive_size, upload_id, \
         published_at \
         FROM revisions INDEXED BY revisions_unarchived \
         WHERE verification = 'verified' AND archived_at IS NULL \
         ORDER BY verified_at LIMIT 20";

    /// Records that a revision's archive is in ARCHIVES (`?1` the
    /// time), which is what makes the read plane serve it. No
    /// generation guard: the key and the bytes placed under it follow
    /// from the row's key alone, so the object is this row's archive
    /// whichever verdict made the row verified.
    MARK_REVISION_ARCHIVED =
        "UPDATE revisions SET archived_at = ?1 \
         WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?5 \
         AND verification = 'verified' AND archived_at IS NULL";

    /// Rejects a verified revision whose upload no longer holds the
    /// verified bytes (`?1` the reason, `?2` the time), guarded on the
    /// publish event the pass listed (`?7`, its `published_at`): a row
    /// rejected, revived, and verified again meanwhile is another
    /// generation and is left alone.
    /// [`REFUND_USER_STORED_BYTES`](super::quota::REFUND_USER_STORED_BYTES)
    /// follows in the same batch.
    REJECT_UNARCHIVED_REVISION =
        "UPDATE revisions SET verification = 'rejected', verification_reason = ?1, \
         verified_at = NULL, rejected_at = ?2 \
         WHERE scope = ?3 AND name = ?4 AND version = ?5 AND revision = ?6 \
         AND verification = 'verified' AND archived_at IS NULL AND published_at = ?7";
}
