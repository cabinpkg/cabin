-- The cron deletes a publication job thirty days after it completes
-- and reads the newest completion (src/sql/publication.rs,
-- PRUNE_COMPLETED_PUBLICATION_JOBS and PIPELINE_HEALTH): a range scan
-- and a seek of this index, never a read of the whole table.
CREATE INDEX publication_jobs_completed ON publication_jobs (completed_at)
    WHERE completed_at IS NOT NULL;
