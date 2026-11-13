//! publication: placing verified archives in ARCHIVES and the documents
//! in INDEX.

statements! {
    /// The placement pass's work list: verified revisions whose archive
    /// is not confirmed in ARCHIVES, oldest verdict first. `upload_id`
    /// is NULL once the row has outlived its upload operation's
    /// retention, long after UPLOADS expired the object. `INDEXED BY`
    /// because the registry never runs ANALYZE: left alone, the
    /// planner takes `revisions_verification` and reads every
    /// verified row to sort them. `governor::Limits` sizes its
    /// infrastructure defaults by the `LIMIT`.
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
    /// whichever verdict made the row verified. The package's document
    /// changes with it, so [`ADVANCE_CATALOG_GENERATION`] and its two
    /// followers come next in the same batch.
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

    /// Counts a change to what a package's document and the catalog
    /// say. It runs directly after the statement that made the change
    /// ([`MARK_REVISION_ARCHIVED`] or
    /// [`SET_VERSION_YANKED`](super::packages::SET_VERSION_YANKED)) and
    /// only when that statement changed its row, as do
    /// [`STAMP_PACKAGE_GENERATION`] and [`RECORD_PUBLICATION_JOB`]
    /// after it, each on the statement before it.
    ADVANCE_CATALOG_GENERATION =
        "UPDATE meta SET value = value + 1 \
         WHERE key = 'catalog_generation' AND (SELECT changes()) = 1";

    /// Gives the changed package the generation its change produced.
    STAMP_PACKAGE_GENERATION =
        "UPDATE packages SET generation = \
         (SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'catalog_generation') \
         WHERE scope = ?1 AND name = ?2 AND (SELECT changes()) = 1";

    /// Records the change as work for the publish pass (`?3` the
    /// time).
    RECORD_PUBLICATION_JOB =
        "INSERT INTO publication_jobs (generation, scope, name, created_at) \
         SELECT generation, scope, name, ?3 FROM packages \
         WHERE scope = ?1 AND name = ?2 AND (SELECT changes()) = 1";

    /// The publish pass's work list: the packages with an open change,
    /// oldest change first. Each costs the pass one D1 call, of the
    /// 1,000 an invocation may make on Workers Paid, and one head and
    /// one put, which `governor::Limits` sizes its infrastructure
    /// defaults by.
    UNPUBLISHED_PACKAGES =
        "SELECT scope, name FROM publication_jobs \
         WHERE completed_at IS NULL GROUP BY scope, name ORDER BY MIN(generation) LIMIT 100";

    /// The generation of a package's document, read in one batch with
    /// the rows the document is rendered from.
    PACKAGE_GENERATION = "SELECT generation FROM packages WHERE scope = ?1 AND name = ?2";

    /// The generation of the catalog, read in one batch with the rows
    /// it is rendered from.
    CATALOG_GENERATION =
        "SELECT CAST(value AS INTEGER) AS generation FROM meta WHERE key = 'catalog_generation'";

    /// Completes a package's changes up to the generation INDEX holds
    /// its document and the catalog at (`?4`; `?1` the time), those
    /// recorded no later than `?5`: the pass's start less the settle
    /// time. A later change, and one that has not settled, keep their
    /// rows.
    COMPLETE_PUBLICATION_JOBS =
        "UPDATE publication_jobs SET completed_at = ?1 \
         WHERE scope = ?2 AND name = ?3 AND generation <= ?4 AND completed_at IS NULL \
         AND created_at <= ?5";

    /// Deletes every change completed longer ago than `?2` (an `SQLite`
    /// time modifier, `crate::publication::JOB_RETENTION`; `?1` the
    /// time). An open change has no completion time and is never
    /// selected, however old.
    PRUNE_COMPLETED_PUBLICATION_JOBS =
        "DELETE FROM publication_jobs INDEXED BY publication_jobs_completed \
         WHERE completed_at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)";

    /// The pipeline's health at `?1`, one row. `stale_pending` counts
    /// the stored uploads that have awaited a verdict for longer than
    /// `?2`, and `oldest_pending` is when the longest-waiting one was
    /// stored. `stalled_archive` is the verdict time of the oldest
    /// verified archive unplaced for longer than `?3`, and
    /// `stalled_job` the recording time of the oldest change open for
    /// longer than `?3`; each is NULL when there is none.
    /// `last_completed_job` is the newest completion, and
    /// `verifier_seen_at` when the verifier last exchanged its identity
    /// for a token. `?2` and `?3` are `SQLite` time modifiers
    /// (`crate::verify::STALE_PENDING`, `crate::publication::STALL`).
    /// Each part reads one partial index, named because the registry
    /// never runs ANALYZE.
    PIPELINE_HEALTH =
        "SELECT \
         (SELECT COUNT(*) FROM revisions INDEXED BY revisions_pending_stored \
          WHERE verification = 'pending' \
          AND stored_at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?2)) AS stale_pending, \
         (SELECT MIN(stored_at) FROM revisions INDEXED BY revisions_pending_stored \
          WHERE verification = 'pending') AS oldest_pending, \
         (SELECT MIN(verified_at) FROM revisions INDEXED BY revisions_unarchived \
          WHERE verification = 'verified' AND archived_at IS NULL \
          AND verified_at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?3)) AS stalled_archive, \
         (SELECT MIN(created_at) FROM publication_jobs INDEXED BY publication_jobs_open \
          WHERE completed_at IS NULL \
          AND created_at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, ?3)) AS stalled_job, \
         (SELECT MAX(completed_at) FROM publication_jobs INDEXED BY publication_jobs_completed \
          WHERE completed_at IS NOT NULL) AS last_completed_job, \
         (SELECT value FROM meta WHERE key = 'verifier_seen_at') AS verifier_seen_at";
}
