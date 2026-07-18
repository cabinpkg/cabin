-- The registry-native identity model for the scoped-package-names track
-- (docs/architecture.md, "Two credential planes"): users become registry
-- rows, external accounts live in `identities` keyed by
-- (provider, provider_account_id) - provider-neutral in schema,
-- GitHub-only in policy - and packages are keyed by (scope, name), where
-- a scope is a registry entity claimed by proving control of the
-- same-named GitHub account. Destructive on purpose: the registry is
-- pre-launch, the operator wipes and re-migrates from zero
-- (scripts/wipe.sh; docs/runbook.md, "Data policy"), and there is no
-- bare-name compatibility path. The claim flow and the scope-aware
-- routes land in the steps that follow; until the route step lands,
-- publish cannot satisfy the NOT NULL scope columns and fails at
-- runtime (a mid-track state that is never deployed).

DROP TABLE versions;
DROP TABLE packages;
DROP TABLE tokens;
DROP TABLE users;

CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    created_at TEXT NOT NULL,
    -- Quota class name; the class -> quota map lives in code
    -- (src/quota.rs). Renamed in place from the earlier `plan` column:
    -- pre-launch there is no forward-migration path for deployed
    -- databases, the operator wipes and re-migrates from zero (see the
    -- header above).
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
-- live provider call.
CREATE TABLE scope_members (
    scope_name TEXT NOT NULL REFERENCES scopes,
    user_id INTEGER NOT NULL REFERENCES users,
    role TEXT NOT NULL,
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
    -- Publish token-bucket state, NULL until the token's first publish
    -- (see migration 0002).
    rl_tokens REAL,
    rl_updated_at TEXT
);

-- `created_by` / `published_by` hold the registry-native users.id; the
-- 0002-era `DEFAULT 0` is gone and both are real foreign keys now that
-- the tables are created whole - attribution is always written
-- explicitly, and a provider account id (or any other stray number)
-- can never enter these tables.
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
    verification TEXT NOT NULL DEFAULT 'pending',
    verification_reason TEXT,
    verified_at TEXT,
    PRIMARY KEY (scope, name, version),
    FOREIGN KEY (scope, name) REFERENCES packages (scope, name)
);
CREATE INDEX versions_published_by ON versions (published_by);
CREATE INDEX versions_checksum ON versions (checksum);
CREATE INDEX versions_verification ON versions (verification);
