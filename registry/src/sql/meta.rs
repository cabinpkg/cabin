//! meta: service state and the storage self-accounting.

statements! {
    /// The pre-launch debug header's generation stamp.
    REGISTRY_GENERATION = "SELECT value FROM meta WHERE key = 'registry_generation'";

    /// One `meta` row by key.
    META_VALUE = "SELECT value FROM meta WHERE key = ?1";

    /// Upserts one `meta` row.
    UPSERT_META =
        "INSERT INTO meta (key, value) VALUES (?1, ?2) \
         ON CONFLICT (key) DO UPDATE SET value = excluded.value";

    /// Counts a published archive's bytes into `total_stored_bytes`
    /// exactly when this batch's [`INSERT_REVISION`](super::packages::INSERT_REVISION) is about to
    /// apply and no live row references the checksum yet.  The
    /// statement runs *before* the insert (mirroring
    /// [`COUNT_STORED_BYTES_ON_REVIVAL`]'s decide-then-flip shape)
    /// and repeats the insert's guards one-for-one - the same-key
    /// conjunct included - so a suppressed insert (a twin, a racing
    /// byte-identical publish, an unflagged respin, an invariance
    /// conflict) can never count bytes, and a blob some other live
    /// row already references is never counted twice.  A post-insert
    /// row-exists test could not tell this batch's row from a racing
    /// byte-identical winner's, which is why the guards read the
    /// pre-insert state instead. The CASTs
    /// here and below keep the TEXT-affinity meta value integer-shaped:
    /// D1 binds numbers as floats, and INTEGER + REAL would otherwise
    /// store "254.0", which the breaker's strict u64 parse rejects.
    COUNT_STORED_BYTES_ON_PUBLISH =
        "INSERT INTO meta (key, value) VALUES ('total_stored_bytes', \
         CASE WHEN (SELECT COUNT(*) FROM revisions \
                    WHERE checksum = ?1 AND verification != 'rejected') = 0 \
              AND EXISTS (SELECT 1 FROM versions \
                          WHERE scope = ?3 AND name = ?4 AND version = ?5) \
              AND NOT EXISTS (SELECT 1 FROM revisions \
                              WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                              AND revision = ?6) \
              AND (?8 OR NOT EXISTS \
                   (SELECT 1 FROM revisions \
                    WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                    AND verification IN ('pending', 'verified') AND checksum <> ?1)) \
              AND NOT EXISTS \
                  (SELECT 1 FROM revisions \
                   WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                   AND verification IN ('pending', 'verified') \
                   AND (json_extract(metadata_json, '$.dependencies') \
                            IS NOT json_extract(?7, '$.dependencies') \
                        OR json_extract(metadata_json, '$.features') \
                            IS NOT json_extract(?7, '$.features') \
                        OR json_extract(metadata_json, '$.standards') \
                            IS NOT json_extract(?7, '$.standards') \
                        OR (json_extract(metadata_json, '$.links') IS NOT NULL \
                            AND json_extract(metadata_json, '$.links') \
                                IS NOT json_extract(?7, '$.links')))) \
              THEN CAST(?2 AS INTEGER) ELSE 0 END) \
         ON CONFLICT (key) DO UPDATE SET \
         value = CAST(value AS INTEGER) + \
         CASE WHEN (SELECT COUNT(*) FROM revisions \
                    WHERE checksum = ?1 AND verification != 'rejected') = 0 \
              AND EXISTS (SELECT 1 FROM versions \
                          WHERE scope = ?3 AND name = ?4 AND version = ?5) \
              AND NOT EXISTS (SELECT 1 FROM revisions \
                              WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                              AND revision = ?6) \
              AND (?8 OR NOT EXISTS \
                   (SELECT 1 FROM revisions \
                    WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                    AND verification IN ('pending', 'verified') AND checksum <> ?1)) \
              AND NOT EXISTS \
                  (SELECT 1 FROM revisions \
                   WHERE scope = ?3 AND name = ?4 AND version = ?5 \
                   AND verification IN ('pending', 'verified') \
                   AND (json_extract(metadata_json, '$.dependencies') \
                            IS NOT json_extract(?7, '$.dependencies') \
                        OR json_extract(metadata_json, '$.features') \
                            IS NOT json_extract(?7, '$.features') \
                        OR json_extract(metadata_json, '$.standards') \
                            IS NOT json_extract(?7, '$.standards') \
                        OR (json_extract(metadata_json, '$.links') IS NOT NULL \
                            AND json_extract(metadata_json, '$.links') \
                                IS NOT json_extract(?7, '$.links')))) \
              THEN CAST(?2 AS INTEGER) ELSE 0 END";

    /// Refunds a rejected archive's bytes exactly when the row - still
    /// pending, still holding the bytes the verdict was read against -
    /// is the checksum's sole live reference (see `src/glue/bearer/verifier.rs`,
    /// `apply_rejection`).
    REFUND_STORED_BYTES_ON_REJECTION =
        "UPDATE meta SET value = MAX(CAST(value AS INTEGER) - \
         CASE WHEN (SELECT COUNT(*) FROM revisions \
                    WHERE checksum = ?1 AND verification != 'rejected') = 1 \
              AND (SELECT verification FROM revisions \
                   WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7) \
                  = 'pending' \
              AND (SELECT checksum FROM revisions \
                   WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7) = ?1 \
              AND (SELECT published_at FROM revisions \
                   WHERE scope = ?2 AND name = ?3 AND version = ?4 AND revision = ?7) = ?6 \
              THEN CAST(?5 AS INTEGER) ELSE 0 END, 0) \
         WHERE key = 'total_stored_bytes'";

    /// Re-counts a revived rejected revision's bytes exactly when the
    /// revival is about to apply and no other live row references the
    /// checksum - a rejection refunded them (see `src/glue/bearer/package.rs`,
    /// `revive_rejected_revision`; revivals are byte-identical, so
    /// the checksum is the row's own).  The conditions mirror
    /// [`REVIVE_REJECTED_REVISION`](super::packages::REVIVE_REJECTED_REVISION)'s guards one-for-one - the
    /// opt-in (`?8`) and resolver-metadata invariance (`?9`)
    /// conjuncts included - so the counter can never gain bytes for
    /// a flip the guards refused.
    COUNT_STORED_BYTES_ON_REVIVAL =
        "UPDATE meta SET value = CAST(value AS INTEGER) + \
         CASE WHEN (SELECT verification FROM revisions \
                    WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?7) \
                   = 'rejected' \
              AND (SELECT checksum FROM revisions \
                   WHERE scope = ?1 AND name = ?2 AND version = ?3 AND revision = ?7) = ?4 \
              AND (SELECT COUNT(*) FROM revisions \
                   WHERE checksum = ?5 AND verification != 'rejected') = 0 \
              AND (?8 OR NOT EXISTS \
                   (SELECT 1 FROM revisions WHERE scope = ?1 AND name = ?2 AND version = ?3 \
                    AND verification IN ('pending', 'verified') AND checksum <> ?4)) \
              AND NOT EXISTS \
                  (SELECT 1 FROM revisions \
                   WHERE scope = ?1 AND name = ?2 AND version = ?3 \
                   AND verification IN ('pending', 'verified') \
                   AND (json_extract(metadata_json, '$.dependencies') \
                            IS NOT json_extract(?9, '$.dependencies') \
                        OR json_extract(metadata_json, '$.features') \
                            IS NOT json_extract(?9, '$.features') \
                        OR json_extract(metadata_json, '$.standards') \
                            IS NOT json_extract(?9, '$.standards') \
                        OR (json_extract(metadata_json, '$.links') IS NOT NULL \
                            AND json_extract(metadata_json, '$.links') \
                                IS NOT json_extract(?9, '$.links')))) \
              THEN CAST(?6 AS INTEGER) ELSE 0 END \
         WHERE key = 'total_stored_bytes'";
}
