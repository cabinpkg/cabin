//! `download_log`: download statistics from the request logs
//! (`src/download_log.rs`).
//!
//! One file's downloads are added in one batch, which D1 runs as one
//! transaction: [`REGISTER_LOG_FILE`], then for each payload of
//! `download_log::Tally::payloads` [`ADD_LOG_DOWNLOADS`],
//! [`ADD_LOG_DAILY_DOWNLOADS`] and [`COUNT_UNKNOWN_LOG_DOWNLOADS`], and
//! last [`FINISH_LOG_FILE`]. The registration writes the file's row, as
//! `adding`, only where the file has none and the statistics still
//! start where the file was read against; the statements after it act
//! only on a file whose row is `adding`; the last makes it `done`. A
//! row is `adding` nowhere but inside the batch that wrote it, so a
//! file that already has a row, or a start that has moved, adds
//! nothing, whichever pass sends the batch, however late, and however
//! often.

statements! {
    /// The keys among the JSON array `?1` that have a row: the files a
    /// pass does not read.
    SETTLED_LOG_FILES =
        "SELECT key FROM processed_log_files WHERE key IN (SELECT value FROM json_each(?1))";

    /// Registers a file (`?1`), where it has no row and `?6` is the
    /// start it was read against. `?2` is now, `?3` the object's
    /// `uploaded`, `?4` its records and `?5` the excluded counts.
    REGISTER_LOG_FILE =
        "INSERT INTO processed_log_files (key, status, at, uploaded, records, excluded) \
         SELECT ?1, 'adding', ?2, ?3, ?4, ?5 \
         WHERE (SELECT value FROM meta WHERE key = 'downloads_cutoff') IS ?6 \
         ON CONFLICT (key) DO NOTHING";

    /// Adds a payload's downloads (`?2`, a JSON array of `[scope, name,
    /// version, day, downloads]`) of the file `?1` to the totals of the
    /// versions that have a row. A version with no row is not made
    /// from a log; a yanked one keeps counting, as it stays
    /// downloadable.
    ADD_LOG_DOWNLOADS =
        "UPDATE versions SET downloads = versions.downloads + added.downloads \
         FROM (SELECT entry.value ->> 0 AS scope, entry.value ->> 1 AS name, \
                      entry.value ->> 2 AS version, SUM(entry.value ->> 4) AS downloads \
               FROM json_each(?2) AS entry GROUP BY 1, 2, 3) AS added \
         WHERE versions.scope = added.scope AND versions.name = added.name \
           AND versions.version = added.version \
           AND EXISTS (SELECT 1 FROM processed_log_files WHERE key = ?1 AND status = 'adding')";

    /// Adds the same payload to the days, for the days that are kept:
    /// the 90 that end with the day of `?3` (now). An older day adds to
    /// its version's total alone. No day is after now's: a file's
    /// requests were all made before the file was written.
    ADD_LOG_DAILY_DOWNLOADS =
        "INSERT INTO version_daily_downloads (scope, name, version, day, downloads) \
         SELECT entry.value ->> 0, entry.value ->> 1, entry.value ->> 2, entry.value ->> 3, \
                entry.value ->> 4 \
         FROM json_each(?2) AS entry \
         WHERE entry.value ->> 3 >= date(?3, '-89 days') \
           AND EXISTS (SELECT 1 FROM versions \
                       WHERE scope = entry.value ->> 0 AND name = entry.value ->> 1 \
                         AND version = entry.value ->> 2) \
           AND EXISTS (SELECT 1 FROM processed_log_files WHERE key = ?1 AND status = 'adding') \
         ON CONFLICT (scope, name, version, day) \
         DO UPDATE SET downloads = downloads + excluded.downloads";

    /// Counts the payload's downloads of versions that have no row on
    /// the file's row. The versions are read in the transaction that
    /// adds, so a version is not taken for missing because another
    /// read is behind.
    COUNT_UNKNOWN_LOG_DOWNLOADS =
        "UPDATE processed_log_files SET unknown_version = unknown_version + \
           (SELECT COALESCE(SUM(entry.value ->> 4), 0) FROM json_each(?2) AS entry \
            WHERE NOT EXISTS (SELECT 1 FROM versions \
                              WHERE scope = entry.value ->> 0 AND name = entry.value ->> 1 \
                                AND version = entry.value ->> 2)) \
         WHERE key = ?1 AND status = 'adding'";

    /// Ends the batch of the file `?1`: its downloads are added.
    FINISH_LOG_FILE =
        "UPDATE processed_log_files SET status = 'done' WHERE key = ?1 AND status = 'adding'";

    /// Records a file (`?1`) whose bytes cannot be counted (`?4` is
    /// why), where it has no row: it is not read again until its row is
    /// deleted, and it added nothing, so reading it then counts it
    /// once. `?2` is now, `?3` the object's `uploaded`.
    REFUSE_LOG_FILE =
        "INSERT INTO processed_log_files (key, status, at, uploaded, error) \
         VALUES (?1, 'error', ?2, ?3, ?4) \
         ON CONFLICT (key) DO NOTHING";

    /// Drops the rows written more than 30 days before `?1` (now),
    /// done and error alike. A file is read only while its object is
    /// younger than the logs' 14 days, so its row outlives every pass
    /// that could read it again.
    DROP_OLD_LOG_FILES =
        "DELETE FROM processed_log_files \
         WHERE at < strftime('%Y-%m-%dT%H:%M:%fZ', ?1, '-30 days')";

    /// Drops the days before the 90 that end with the day of `?1`
    /// (now).
    DROP_OLD_DAILY_DOWNLOADS =
        "DELETE FROM version_daily_downloads WHERE day < date(?1, '-89 days')";
}
