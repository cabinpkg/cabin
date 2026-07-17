-- Cumulative download counter for the artifact read plane
-- (docs/architecture.md, "Download counts"): one approximate,
-- monotonically increasing total per version, incremented best-effort
-- after a verified download's body is served. Additive: existing rows
-- start at zero (no history existed to backfill).
ALTER TABLE versions ADD COLUMN downloads INTEGER NOT NULL DEFAULT 0;
