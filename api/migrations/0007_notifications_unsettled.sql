-- The cron's claim of the next mail to send reads the rows that are
-- neither sent nor given up, oldest first (src/sql/notifications.rs,
-- CLAIM_NOTIFICATION): a scan of this index, never of the settled rows,
-- which stay in the table.
CREATE INDEX notifications_unsettled ON notifications (id)
    WHERE sent_at IS NULL AND failed_at IS NULL;
