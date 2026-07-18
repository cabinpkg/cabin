-- Per-user quota and budget-breaker state (see docs/architecture.md,
-- "Billing model and the budget breaker"). Additive: existing rows get the
-- defaults; operators either wipe pre-launch per docs/runbook.md or backfill
-- archive_size / published_by / total_stored_bytes with
-- scripts/backfill-0002.sh. Superseded by migration 0006, which recreates
-- these tables with explicit, registry-native attribution and retired the
-- backfill script.

-- Size of the version's archive blob in bytes, and the numeric GitHub id
-- (users.github_id) of the publisher. Both are 0 on rows that predate this
-- migration until backfilled. The checksum index serves the storage
-- self-accounting's first-reference check at publish.
ALTER TABLE versions ADD COLUMN archive_size INTEGER NOT NULL DEFAULT 0;
ALTER TABLE versions ADD COLUMN published_by INTEGER NOT NULL DEFAULT 0;
CREATE INDEX versions_published_by ON versions (published_by);
CREATE INDEX versions_checksum ON versions (checksum);

-- The numeric GitHub id of whoever first published (created) the package;
-- 0 until backfilled. Drives the daily and total package quotas.
ALTER TABLE packages ADD COLUMN created_by INTEGER NOT NULL DEFAULT 0;
CREATE INDEX packages_created_by ON packages (created_by);

-- Quota class name; the class -> quota map lives in code (src/quota.rs).
ALTER TABLE users ADD COLUMN quota_class TEXT NOT NULL DEFAULT 'default';

-- Publish token-bucket state, NULL until the token's first publish:
-- rl_tokens is the remaining fractional token count, rl_updated_at the
-- Unix epoch milliseconds (as text) of the last successful take.
ALTER TABLE tokens ADD COLUMN rl_tokens REAL;
ALTER TABLE tokens ADD COLUMN rl_updated_at TEXT;

-- Breaker state ('normal' | 'warn' | 'writes_blocked'), its human-readable
-- reason, and the exact self-accounted R2 storage in bytes.
INSERT OR IGNORE INTO meta (key, value) VALUES
    ('service_mode', 'normal'),
    ('service_mode_reason', ''),
    ('total_stored_bytes', '0');
