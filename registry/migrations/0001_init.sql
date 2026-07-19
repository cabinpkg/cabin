-- Canonical registry state. D1 is the source of truth; R2 only holds
-- immutable, content-addressed archive blobs (blobs/sha256/<checksum-hex>).
--
-- One from-zero baseline on purpose: pre-launch the registry's data is
-- disposable and the operator wipes and re-migrates from zero
-- (scripts/wipe.sh; docs/runbook.md, "Data policy"), so schema changes
-- edit this file in place instead of accreting ALTER TABLE layers.

-- The registry-native identity model (docs/architecture.md, "Two
-- credential planes"): users are registry rows, external accounts live
-- in `identities` keyed by (provider, provider_account_id) -
-- provider-neutral in schema, GitHub-only in policy - and packages are
-- keyed by (scope, name), where a scope is a registry entity claimed by
-- proving control of the same-named GitHub account.
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    created_at TEXT NOT NULL,
    -- Quota class name; the class -> quota map lives in code
    -- (src/quota.rs).
    quota_class TEXT NOT NULL DEFAULT 'default'
);

-- One row per external account that ever signed in. The numeric
-- provider account id (as text) is the identity; `login_snapshot` is
-- the provider login as of the most recent sign-in, display-only
-- (logins can be renamed and reassigned).
CREATE TABLE identities (
    provider TEXT NOT NULL,
    provider_account_id TEXT NOT NULL,
    login_snapshot TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users,
    PRIMARY KEY (provider, provider_account_id)
);

-- A claimed scope: the `<scope>/` prefix of every package name. The
-- proof columns freeze which external account proved control of the
-- same-named provider account at claim time, so a later account reusing
-- the login can never re-claim the string.
CREATE TABLE scopes (
    name TEXT PRIMARY KEY,
    proof_provider TEXT NOT NULL,
    proof_account_id TEXT NOT NULL,
    claimed_at TEXT NOT NULL
);

-- Membership within a scope ('owner' is the admin role). Publish/yank
-- authorization consults only registry state - this table - never a
-- live provider call. The role domain is closed in the schema: the
-- last-owner rule and the owner gate key on the exact 'owner' spelling,
-- and membership disputes are manual SQL (docs/architecture.md,
-- "Scopes") - the constraint keeps a hand-run typo from silently
-- widening access or orphaning a scope.
CREATE TABLE scope_members (
    scope_name TEXT NOT NULL REFERENCES scopes,
    user_id INTEGER NOT NULL REFERENCES users,
    role TEXT NOT NULL CHECK (role IN ('owner', 'member')),
    PRIMARY KEY (scope_name, user_id)
);

CREATE TABLE tokens (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at TEXT,
    -- Publish token-bucket state, NULL until the token's first publish:
    -- rl_tokens is the remaining fractional token count, rl_updated_at
    -- the Unix epoch milliseconds (as text) of the last successful
    -- take.
    rl_tokens REAL,
    rl_updated_at TEXT
);

-- `created_by` / `published_by` hold the registry-native users.id as
-- real foreign keys - attribution is always written explicitly, and a
-- provider account id (or any other stray number) can never enter
-- these tables.
CREATE TABLE packages (
    scope TEXT NOT NULL REFERENCES scopes,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    created_by INTEGER NOT NULL REFERENCES users,
    PRIMARY KEY (scope, name)
);
CREATE INDEX packages_created_by ON packages (created_by);

CREATE TABLE versions (
    scope TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    checksum TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    yanked INTEGER NOT NULL DEFAULT 0,
    published_at TEXT NOT NULL,
    archive_size INTEGER NOT NULL,
    published_by INTEGER NOT NULL REFERENCES users,
    -- The asynchronous verification lifecycle (docs/architecture.md,
    -- "The verification lifecycle"): 'pending' (published, not yet
    -- resolvable), 'verified' (part of the registry, immutable), or
    -- 'rejected' (never became part of the registry; its blob is
    -- reclaimed and the pair may be republished).
    verification TEXT NOT NULL DEFAULT 'pending',
    verification_reason TEXT,
    verified_at TEXT,
    -- Cumulative download counter for the artifact read plane
    -- (docs/architecture.md, "Download counts"): one approximate,
    -- monotonically increasing total per version, incremented
    -- best-effort after a verified download's body is served.
    downloads INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (scope, name, version),
    FOREIGN KEY (scope, name) REFERENCES packages (scope, name)
);
CREATE INDEX versions_published_by ON versions (published_by);
-- The checksum index serves the storage self-accounting's
-- first-reference check at publish.
CREATE INDEX versions_checksum ON versions (checksum);
CREATE INDEX versions_verification ON versions (verification);

-- Blob-replication failure log (see docs/runbook.md, "Disaster
-- recovery"). Publish replicates each archive blob to the BACKUP
-- bucket best-effort; a failed copy lands here keyed by the R2 object
-- key so it can be re-run (scripts/backup-backfill.sh copies every
-- missing blob and clears the table). The breaker cron alerts while
-- rows exist.
CREATE TABLE backup_replication_failures (
    key TEXT PRIMARY KEY,
    failed_at TEXT NOT NULL
);

CREATE TABLE meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- service_mode is the budget-breaker state ('normal' | 'warn' |
-- 'writes_blocked' | 'reads_blocked'; see src/breaker.rs) with its
-- human-readable reason, and total_stored_bytes the exact
-- self-accounted R2 storage in bytes. launched is the data-policy flag
-- (docs/runbook.md, "Data policy"): 'false' while the registry's data
-- is disposable (pre-launch), flipped to 'true' exactly once, by hand,
-- as a launch-checklist item; every destructive maintenance path
-- (scripts/launch-guard.sh) reads it first and refuses while 'true'.
INSERT INTO meta (key, value) VALUES
    ('registry_generation', '1'),
    ('service_mode', 'normal'),
    ('service_mode_reason', ''),
    ('total_stored_bytes', '0'),
    ('launched', 'false');
