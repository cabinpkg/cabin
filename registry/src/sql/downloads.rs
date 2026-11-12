//! downloads: the artifact read plane.

statements! {
    /// The artifact route's read-gate and object lookup, addressed by
    /// the immutable unit: the route's filename embeds the revision, so
    /// superseded revisions stay fetchable and the verifier can fetch a
    /// *specific* pending revision. A reservation has no object to
    /// serve yet and is a missing row here. `archived` is whether the
    /// archive is in ARCHIVES, where a verified revision is read from.
    ARTIFACT_BY_REVISION =
        "SELECT verification, upload_id, archived_at IS NOT NULL AS archived FROM revisions \
         WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?4 \
         AND stored_at IS NOT NULL";

    /// The source viewer's lookup: the served revision, which names
    /// the object in ARCHIVES, plus the archive size, which bounds the
    /// ranged read before R2 is consulted (the object holds the bytes
    /// the size was recorded from). Like
    /// [`CURRENT_REVISIONS_BY_PACKAGE`](super::packages::CURRENT_REVISIONS_BY_PACKAGE),
    /// it reads the served-only `current_revisions` view, so pending,
    /// rejected, and corrupt-status rows are missing rows by
    /// construction - sessions have no verify scope, so unlike the
    /// artifact route there is no pending carve-out to branch on.
    SOURCE_VERSION_LOOKUP =
        "SELECT revision, archive_size FROM current_revisions \
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
}
