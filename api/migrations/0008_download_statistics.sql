-- Download statistics from the request logs (src/download_log.rs,
-- src/sql/download_log.rs).

-- One row per request-log file in DOWNLOAD_LOGS that is not read
-- again: 'done' once its downloads are added, 'error' when its bytes
-- cannot be counted however often they are read. 'adding' is a row's
-- status only inside the batch that adds its file's downloads, which
-- ends by making it 'done': the batch's statements find by it that the
-- file is theirs to add, and no other transaction sees it. key is the
-- object's whole key; the log delivery writes a key once and never
-- again, which is what lets a key stand for its bytes. at is when the
-- row was written, and the row is kept 30 days from it. uploaded is
-- the object's time in R2. records, excluded (a JSON object of the
-- records that are not downloads, counted by reason) and
-- unknown_version (the downloads of versions that have no row, which
-- are added to nothing) describe a file that is done.
CREATE TABLE processed_log_files (
    key TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('adding', 'done', 'error')),
    at TEXT NOT NULL,
    uploaded TEXT NOT NULL,
    records INTEGER NOT NULL DEFAULT 0,
    excluded TEXT NOT NULL DEFAULT '{}',
    unknown_version INTEGER NOT NULL DEFAULT 0,
    error TEXT
);
-- The rows are dropped by age (src/sql/download_log.rs,
-- DROP_OLD_LOG_FILES).
CREATE INDEX processed_log_files_at ON processed_log_files (at);

-- A version's downloads on one UTC day (YYYY-MM-DD), for the latest 90
-- days. versions.downloads is the total: it is kept on its own and is
-- not the sum of these rows.
CREATE TABLE version_daily_downloads (
    scope TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    day TEXT NOT NULL,
    downloads INTEGER NOT NULL,
    PRIMARY KEY (scope, name, version, day),
    FOREIGN KEY (scope, name, version) REFERENCES versions (scope, name, version)
);
-- The rows are dropped by day (DROP_OLD_DAILY_DOWNLOADS).
CREATE INDEX version_daily_downloads_day ON version_daily_downloads (day);

-- When this database's download statistics start: a request that
-- started earlier is not counted. It is written with the schema, so a
-- database that a wipe makes anew starts at its own making, and a file
-- read against another start than the database's adds nothing
-- (REGISTER_LOG_FILE).
INSERT INTO meta (key, value)
VALUES ('downloads_cutoff', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
