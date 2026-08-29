//! downloads: the artifact read plane and blob reclaim.

statements! {
    /// The artifact route's checksum and read-gate lookup, addressed
    /// by the immutable unit: the route's filename embeds the
    /// revision, so superseded revisions stay fetchable and the
    /// verifier can fetch a *specific* pending revision.
    ARTIFACT_BY_REVISION =
        "SELECT checksum, verification FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?4";

    /// The source viewer's lookup: the checksum plus the stored archive
    /// size, which bounds the ranged read before R2 is consulted (the
    /// blob was written from the same bytes the size was recorded
    /// from). The verified filter sits in the query like
    /// [`CURRENT_REVISIONS_BY_PACKAGE`](super::packages::CURRENT_REVISIONS_BY_PACKAGE)'s, so pending, rejected, and
    /// corrupt-status rows are missing rows by construction - sessions
    /// have no verify scope, so unlike the artifact route there is no
    /// pending carve-out to branch on.
    SOURCE_VERSION_LOOKUP =
        "SELECT checksum, archive_size FROM current_revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3";

    /// The public stats totals: verified packages, verified versions,
    /// and served downloads. `scope || '/' || name` is unambiguous -
    /// `/` is in neither grammar - and a registry with no verified
    /// versions answers all zeros.
    REGISTRY_STATS =
        "SELECT COUNT(DISTINCT c.scope || '/' || c.name) AS packages, \
         COUNT(*) AS versions, \
         COALESCE(SUM(v.downloads), 0) AS downloads \
         FROM current_revisions c \
         JOIN versions v ON v.scope = c.scope AND v.name = c.name AND v.version = c.version";

    /// Applies one flush of the batched download telemetry
    /// (`src/telemetry.rs`): the buffered per-version count lands in
    /// one statement per version instead of one write per download.
    /// The `verification` guard keeps the counter honest inside the
    /// statement itself: only verified rows ever count, so the
    /// verifier's pending fetches (readable with the `verify` scope)
    /// and any racing lifecycle change can never increment. Yanked
    /// versions keep counting - they stay downloadable on purpose.
    ADD_VERSION_DOWNLOADS =
        "UPDATE versions SET downloads = downloads + ?4 \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 \
         AND EXISTS (SELECT 1 FROM revisions r \
                     WHERE r.scope = ?1 AND r.name = ?2 AND r.version = ?3 \
                     AND r.verification = 'verified')";

    /// Live (non-rejected) references to one blob, for reclaim.
    COUNT_LIVE_BLOB_REFERENCES =
        "SELECT COUNT(*) AS n FROM revisions \
         WHERE checksum = ?1 AND verification != 'rejected'";

    /// The governor reconciliation's authoritative live set: one size
    /// per distinct live checksum, the same shape the storage
    /// self-accounting counts (`docs/runbook.md`, "Orphaned R2 blobs").
    LIVE_BLOB_SIZES =
        "SELECT checksum, MAX(archive_size) AS size FROM revisions \
         WHERE verification != 'rejected' GROUP BY checksum";
}
