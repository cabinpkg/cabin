//! backup: the verified-artifact replication queue.

statements! {
    /// Enqueues a just-verified version's blob for backup replication,
    /// in the same batch as [`MARK_REVISION_VERIFIED`](super::packages::MARK_REVISION_VERIFIED): the row appears
    /// exactly when the verified transition applied (the guards repeat
    /// the mark's), so the queue is recorded transactionally with the
    /// transition and a crash can never lose the work. Shared checksums
    /// collapse onto one queue row; a key already replicated re-enters
    /// harmlessly (the drain's head sees the copy and settles).
    ENQUEUE_VERIFIED_BACKUP =
        "INSERT INTO backup_pending (key, bytes, enqueued_at) \
         SELECT 'blobs/sha256/' || substr(checksum, 8), archive_size, ?6 FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?7 \
         AND verification = 'verified' AND checksum = ?4 AND published_at = ?5 \
         ON CONFLICT (key) DO NOTHING";

    /// The drain's work list: keyset-paginated (`key > ?1`, key
    /// order), so rows a pass must keep (a missing primary blob under
    /// a still-verified version) are walked past instead of pinning
    /// the page - ten stuck rows must not starve every later healthy
    /// entry. The row's `bytes` is deliberately not read here: the
    /// ledger settles at sizes the drain observes (the head's object,
    /// or the buffered copy), never at the enqueue-time expectation.
    LIST_BACKUP_PENDING =
        "SELECT key FROM backup_pending WHERE key > ?1 ORDER BY key LIMIT 10";

    /// Removes one queue row whose work is done (the copy landed).
    DELETE_BACKUP_PENDING = "DELETE FROM backup_pending WHERE key = ?1";

    /// Retires one queue row as dead - but only while no verified
    /// reference exists, re-checked inside the statement: a
    /// check-then-delete split would let a verdict that lands in
    /// between (enqueueing this very key transactionally) lose its
    /// recorded backup work to a stale reader.
    RETIRE_DEAD_BACKUP_PENDING =
        "DELETE FROM backup_pending WHERE key = ?1 \
         AND NOT EXISTS (SELECT 1 FROM revisions \
                         WHERE checksum = ?2 AND verification = 'verified')";

    /// Live **verified** references to one blob: the drain only copies
    /// blobs the registry still serves as verified content.
    COUNT_LIVE_VERIFIED_BLOB_REFERENCES =
        "SELECT COUNT(*) AS n FROM revisions \
         WHERE checksum = ?1 AND verification = 'verified'";

    /// Queue rows older than an hour (the breaker's backup-health
    /// alert): fresh rows are in-flight work, stale ones mean the
    /// drain is failing or refused.
    COUNT_STALE_BACKUP_PENDING =
        "SELECT COUNT(*) AS n FROM backup_pending \
         WHERE enqueued_at < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-1 hour')";
}
