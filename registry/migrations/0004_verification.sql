-- The asynchronous verification lifecycle (docs/architecture.md, "The
-- verification lifecycle"). Every version row carries a verification
-- status: 'pending' (published, not yet resolvable), 'verified' (part of
-- the registry, immutable), or 'rejected' (never became part of the
-- registry; its blob is reclaimed and the pair may be republished).
-- Additive: existing rows were published by the sole operator before the
-- pipeline existed and are backfilled as verified.
ALTER TABLE versions ADD COLUMN verification TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE versions ADD COLUMN verification_reason TEXT;
ALTER TABLE versions ADD COLUMN verified_at TEXT;
CREATE INDEX versions_verification ON versions (verification);

UPDATE versions SET verification = 'verified';
