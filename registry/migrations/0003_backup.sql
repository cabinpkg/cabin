-- Blob-replication failure log (see docs/runbook.md, "Disaster recovery").
-- Publish replicates each archive blob to the BACKUP bucket best-effort;
-- a failed copy lands here keyed by the R2 object key so it can be
-- re-run (scripts/backup-backfill.sh copies every missing blob and
-- clears the table). The breaker cron alerts while rows exist.
CREATE TABLE backup_replication_failures (
    key TEXT PRIMARY KEY,
    failed_at TEXT NOT NULL
);
